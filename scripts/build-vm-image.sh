#!/usr/bin/env bash
# ============================================================================
# P5 最小镜像管线（第 2 步交付物，实施计划 §3「最小镜像管线」）
#
# 产出（<out>/ 目录两件套 + manifest.json）：
#   base.qcow2          成品：裸 Debian genericcloud 云镜像 + 内嵌 cloud-init
#                       nocloud seed（/var/lib/cloud/seed/nocloud/{user-data,
#                       meta-data}，首启装配 guest agent：b64 注入二进制 +
#                       launcher 读 SMBIOS DMI serial 作 token + systemd 常驻
#                       + 持久 networkd DHCP 单元（冷启网络不依赖 cloud-init
#                       per-instance 语义）+ 9p 共享子树 best-effort 挂载）
#   base.upstream.qcow2 上游原件留存（来源校验锚；重跑可复现）
#   manifest.json       来源/校验和/日期/端口清单（G2 留痕面）
#
# 为什么 seed 烧进 base 而不是 cdrom ISO（2026-10-20 定谳）：Debian bookworm
# genericcloud 的 cloud-init 22.4.2 经 systemd generator 在 boot 早期跑
# ds-identify；q35 AHCI cdrom 在检测窗内缺席（blkid 无物可扫）→ 静默把整套
# cloud-init 单元摘出 boot 事务（serial 无任何痕迹）→ 无网络/无 guest agent。
# seed 目录是 ds-identify 的第 ③ 路检测（纯文件检查，零介质零时序依赖），
# 实测 12.8s 完成首启装配（cdrom 形态 240s 超时的根修）。
#
# 诚实边界：
# - Linux 宿主脚本（qemu-img + loop 挂载烧录，需 root——WSL 用 `wsl -u root`
#   或 sudo）；Windows 宿主走 WSL 或远程 Linux 构建回传（实施计划 §6 风险
#   表既定）；
# - rootfs 按 p1 分区挂载（Debian genericcloud 布局，实测钉死；上游换布局
#   = 挂载失败诚实拒绝）；
# - aarch64 = 接口预留档，选择即诚实拒绝（GuestArch 同一口径）；
# - 坏校验和 = 构建拒绝 + 产物改名留痕（base.upstream.qcow2.corrupt）+
#   manifest 不写；
# - 磁盘扩容不需要本脚本管：overlay 大于 base 时，genericcloud 的 cloud-init
#   growpart/resizefs 首启自动扩 rootfs 到 overlay 尺寸。
#
# 用法：
#   scripts/build-vm-image.sh --guest-bin target/x86_64-unknown-linux-musl/release/nemesis-guest
#   scripts/build-vm-image.sh --guest-bin ... --base-url <URL> --sha256 <HEX>   # 来源可换（推荐钉死校验和；不钉则从 source 同目录拉上游 SUMS 对账）
#   scripts/build-vm-image.sh --guest-bin ... --skip-download                    # base.upstream.qcow2 已在位（复验校验和后直接烧录）
#   scripts/build-vm-image.sh --guest-bin ... --variant desktop                  # WM+Chromium 变体（E 线人工验证面载体）
#   scripts/build-vm-image.sh --guest-bin ... --apt-mirror <URL> --apt-suite <S> # 受限网络：guest sources 覆写为镜像源（suite 如 bookworm，两参必同给）
# ============================================================================
set -euo pipefail

cd "$(dirname "$0")/.."

# ---------------------------------------------------------------------------
# 预设（镜像来源配置化的脚本层；WSAPI/CLI 包装在第 4 步加 preset 列表）
# ---------------------------------------------------------------------------
DEFAULT_BASE_URL="https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"
GUEST_AGENT_PORT=48500

OUT_DIR="target/vm-image"
GUEST_BIN=""
BASE_URL="$DEFAULT_BASE_URL"
SHA256_OVERRIDE=""
ARCH="x86_64"
SKIP_DOWNLOAD=0
VARIANT=""
APT_MIRROR=""
APT_SUITE=""

usage() { grep '^#' "$0" | head -40; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --out)          OUT_DIR="$2"; shift 2 ;;
        --guest-bin)    GUEST_BIN="$2"; shift 2 ;;
        --base-url)     BASE_URL="$2"; shift 2 ;;
        --sha256)       SHA256_OVERRIDE="$2"; shift 2 ;;
        --arch)         ARCH="$2"; shift 2 ;;
        --variant)      VARIANT="$2"; shift 2 ;;
        --apt-mirror)   APT_MIRROR="$2"; shift 2 ;;
        --apt-suite)    APT_SUITE="$2"; shift 2 ;;
        --skip-download) SKIP_DOWNLOAD=1; shift ;;
        -h|--help)      usage ;;
        *) echo "!! 未知参数: $1"; usage ;;
    esac
done

fail() { echo "!! $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 前置闸：arch / guest 二进制 / 下载工具 / 烧录工具 / root（全部诚实报缺）
# ---------------------------------------------------------------------------
[ "$ARCH" = "x86_64" ] || fail "arch '$ARCH' 为接口预留档，当前不产出（合法值：x86_64）"
[ -n "$GUEST_BIN" ] || fail "缺 --guest-bin（scripts/build-guest.sh 的 musl 产物）"
[ -f "$GUEST_BIN" ] || fail "guest 二进制不存在: $GUEST_BIN"
# file(1) 措辞随版本分叉：旧版 "statically linked"；新版对 musl rustc 产物
# （默认 static-pie）报 "static-pie linked"——两者都是无 interpreter 的静态
# 形态，与 build-guest.sh 的 ld-musl 缺席断言同口径（有 interpreter 才非静态）。
file "$GUEST_BIN" 2>/dev/null | grep -Eqi "statically linked|static-pie linked" \
    || fail "guest 二进制非静态链接（musl 目标要求 statically linked / static-pie linked）：$(file "$GUEST_BIN" 2>/dev/null || echo 'file 命令不可用')"

# 烧录步硬依赖：qemu-img（raw/qcow2 convert）+ root（losetup/mount）。
command -v qemu-img >/dev/null 2>&1 \
    || fail "qemu-img 缺席（Debian/Ubuntu: apt install qemu-utils）"
[ "$(id -u)" = 0 ] || fail "烧录 seed 需要 root（losetup/mount）——WSL 用 wsl -u root 运行，Linux 用 sudo"

# 下载工具：curl 或 wget（二选一在场即可）
fetch() { # fetch <url> <out-file>
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fail "curl / wget 都不可用，无法下载"
    fi
}
probe_url() { # HEAD 探测（构建前先确认 URL 可达，坏 URL 不进长下载）
    if command -v curl >/dev/null 2>&1; then
        # -L：源站 302 到 CDN 是常态（cloud.debian.org 实测），不跟随会误判
        curl -fsSLI -m 15 -o /dev/null "$1"
    else
        wget --spider -T 15 -q "$1"
    fi
}

mkdir -p "$OUT_DIR"
UPSTREAM_FILE="$OUT_DIR/base.upstream.qcow2"

# ---------------------------------------------------------------------------
# [1] base 下载 + 校验（显式 --sha256 > 上游 SUMS sidecar；坏 = 拒绝留痕）
# ---------------------------------------------------------------------------
echo "== [1/4] base 镜像下载 + 校验 =="
BASE_NAME="${BASE_URL##*/}"
if [ "$SKIP_DOWNLOAD" = 1 ]; then
    [ -f "$UPSTREAM_FILE" ] || fail "--skip-download 但 $UPSTREAM_FILE 不存在"
else
    echo "   HEAD 探测: $BASE_URL"
    probe_url "$BASE_URL" || fail "URL 不可达（构建前 HEAD 探测失败）: $BASE_URL"
    echo "   下载 → $UPSTREAM_FILE"
    fetch "$BASE_URL" "$UPSTREAM_FILE"
fi
[ -f "$UPSTREAM_FILE" ] || fail "base 镜像不在位: $UPSTREAM_FILE"

EXPECTED_SHA="$SHA256_OVERRIDE"
ALGO="sha256"
if [ -z "$EXPECTED_SHA" ]; then
    # 上游 sidecar 对任意配置 source 生效（config 层契约「sha256 空 = 从
    # {source} 同目录的 SUMS sidecar 查表」——B7 真机走查根修：旧闸只对
    # 脚本默认 URL 开放 sidecar，config 钉 bookworm + 空 sha256 必撞死）。
    # SHA256SUMS 优先；缺席（bookworm/trixie latest 实测只有 SHA512SUMS）
    # 回落 SHA512SUMS——算法跟随上游现实，信任锚都是上游随镜像同发的清单。
    SUMS="$OUT_DIR/SUMS.upstream"
    if fetch "${BASE_URL%/*}/SHA256SUMS" "$SUMS" 2>/dev/null && [ -s "$SUMS" ]; then
        ALGO="sha256"
    elif fetch "${BASE_URL%/*}/SHA512SUMS" "$SUMS" 2>/dev/null && [ -s "$SUMS" ]; then
        ALGO="sha512"
    else
        fail "上游校验清单（SHA256SUMS/SHA512SUMS）都拉不到（校验无据 = 拒绝构建）"
    fi
    echo "   上游 ${ALGO}SUMS 对账"
    EXPECTED_SHA="$(awk -v n="$BASE_NAME" '$2=="*"n || $2==n {print $1}' "$SUMS")"
    [ -n "$EXPECTED_SHA" ] || fail "上游 SUMS 清单里找不到 $BASE_NAME（来源可能已轮换，换钉住版本）"
fi
if [ "$ALGO" = sha256 ]; then
    ACTUAL_SHA="$(sha256sum "$UPSTREAM_FILE" | awk '{print $1}')"
else
    ACTUAL_SHA="$(sha512sum "$UPSTREAM_FILE" | awk '{print $1}')"
fi
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
    mv "$UPSTREAM_FILE" "$UPSTREAM_FILE.corrupt"   # 留痕：坏产物保留待查
    fail "校验和不匹配：期望 $EXPECTED_SHA 实得 $ACTUAL_SHA（坏产物已改名 $UPSTREAM_FILE.corrupt，manifest 不写）"
fi
echo "   $ALGO 对账通过: ${ACTUAL_SHA:0:16}..."

# ---------------------------------------------------------------------------
# [2] cloud-init seed 内容（guest 二进制 b64 + launcher + systemd unit）
# ---------------------------------------------------------------------------
echo "== [2/4] cloud-init seed 内容生成 =="
GUEST_B64="$(base64 -w0 "$GUEST_BIN")"
GUEST_SHA="$(sha256sum "$GUEST_BIN" | awk '{print $1}')"
SEED_DIR="$OUT_DIR/seed.d"
rm -rf "$SEED_DIR"
mkdir -p "$SEED_DIR"

# variant 扩展（P5 第 3 步 E 线）：desktop = WM + Chromium（浏览器登录保持
# 人工验证面的载体——自动化断言不含 GUI 登录，诚实边界）。cloud-config
# packages 段首启 apt 安装（数百 MB，一次性进 base 之后的 overlay/镜像）。
# B8b：默认 variant 预装基础工具链（build-essential/git/curl，约几百 MB）
# ——开箱即写代码，apt install 工具链的分钟级首启等待不再必要。
case "$VARIANT" in
    "")          VARIANT_USERDATA='
packages:
  - build-essential
  - git
  - curl
' ;;
    desktop)     VARIANT_USERDATA='
packages:
  - openbox
  - xinit
  - xterm
  - chromium
' ;;
    *) fail "未知 variant: $VARIANT（当前支持: desktop）" ;;
esac

# 受限网络环境镜像源（2026-10-20 真机走查实证：企业出口对 deb.debian.org
# 限速 ~22KB/s，国内镜像 ~10MB/s——450 倍差，首启装配等待窗装不完包）。
# 覆写 cloud images 的 deb822 sources（/etc/apt/sources.list.d/debian.sources）
# 为镜像 URIs——单文件替换、无遗留双源告警；Signed-By 沿用镜像内同款
# Debian 归档钥匙环路径（URIs 换、信任锚不换）。security 镜像按惯例同域
# 加 -security 后缀（如 mirrors.ustc.edu.cn/debian-security）。
APT_USERDATA=""
if [ -n "$APT_MIRROR" ]; then
    [ -n "$APT_SUITE" ] || fail "--apt-mirror 需要配套 --apt-suite（如 bookworm）"
    APT_USERDATA="
  - path: /etc/apt/sources.list.d/debian.sources
    permissions: \"0644\"
    content: |
      Types: deb
      URIs: $APT_MIRROR
      Suites: $APT_SUITE $APT_SUITE-updates
      Components: main contrib non-free non-free-firmware
      Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg

      Types: deb
      URIs: $APT_MIRROR-security
      Suites: $APT_SUITE-security
      Components: main contrib non-free non-free-firmware
      Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg"
fi

# launcher：token 不进镜像——每次启动从 SMBIOS DMI serial 现读（宿主 QEMU
# -smbios type=1,serial=<per-instance token> 注入），镜像零改动、零密钥落盘。
cat > "$SEED_DIR/user-data" <<EOF
#cloud-config
hostname: nemesis-guest
manage_etc_hosts: true

write_files:
  - path: /usr/local/bin/nemesis-guest-agent
    encoding: b64
    content: $GUEST_B64
    permissions: "0755"
  - path: /usr/local/bin/nemesis-guest-launch.sh
    permissions: "0755"
    content: |
      #!/bin/sh
      # per-instance token：从 SMBIOS DMI serial 现读（缺失 = 空串，鉴权闸关）
      serial=""
      if [ -r /sys/class/dmi/id/product_serial ]; then
          serial="\$(cat /sys/class/dmi/id/product_serial 2>/dev/null || true)"
      fi
      export NEMESISBOT_GUEST_TOKEN="\$serial"
      export NEMESISBOT_GUEST_WORKSPACE=/workspace
      # B8a：systemd 系统服务环境无 HOME——不补则 ~ 不展开（字面量），
      # cd ~ 直接失败；指向执行根，agent 与人同惠。
      # （注：heredoc 未引用——注释里不得写反引号或美元括号替换语法，会被真执行。）
      export HOME=/workspace
      exec /usr/local/bin/nemesis-guest-agent --listen 0.0.0.0:$GUEST_AGENT_PORT
  - path: /etc/systemd/system/nemesis-guest-agent.service
    permissions: "0644"
    content: |
      [Unit]
      Description=NemesisBot Guest Agent (VM executor)
      After=network-online.target
      Wants=network-online.target

      [Service]
      ExecStart=/usr/local/bin/nemesis-guest-launch.sh
      Restart=always
      RestartSec=2

      [Install]
      WantedBy=multi-user.target
  # 持久网络配置（冷重启根修，2026-10-18 远程取证定谳）：cloud-init 的网络
  # 配置是 per-instance 语义——只有首启真正 apply，后续冷启依赖它渲染的
  # /etc/netplan/50-cloud-init.yaml；而 genericcloud 底图上该文件渲染为
  # 0 字节空文件 → systemd-networkd 无配置 → 冷启 NIC unmanaged → 无 IP
  # → slirp hostfwd 不可达 → guest agent 永远不就绪。补一份不依赖
  # cloud-init 语义的持久 networkd 单元：每次冷启 networkd 直接管理 virtio
  # 网卡并 DHCP（slirp 侧 10.0.2.15 秒回）。按 Driver 匹配而非 NIC 名
  # （q35 x86_64 是 enp0s2，arm64 预留档命名不同；virtio_net 驱动名一致）。
  - path: /etc/systemd/network/10-nemesis-virtio.network
    permissions: "0644"
    content: |
      [Match]
      Driver=virtio_net

      [Network]
      DHCP=yes
${APT_USERDATA}
${VARIANT_USERDATA}
runcmd:
  - systemctl daemon-reload
  - systemctl enable --now nemesis-guest-agent.service
  # 9p 共享子树 best-effort（Windows 宿主无 virtfs = 挂载缺席，/workspace 落 VM 磁盘）
  - mkdir -p /workspace
  - sh -c 'mount -t 9p -o trans=virtio,version=9p2000.L workspace /workspace 2>>/var/log/nemesis-guest-provision.log || echo "9p workspace tag absent; /workspace on VM disk" >>/var/log/nemesis-guest-provision.log'
EOF

# INSTANCE_ID 内容寻址（user-data sha 前 8 位）：seed 内容（guest 二进制 b64 /
# variant / apt-mirror / 脚本修订）任何变化自动换 id。cloud-init NoCloud 按
# instance-id 缓存 per-instance 状态——同 id 旧实例不会重读新 user-data；
# 内容寻址保证「seed 变了 → 实例重建语义」，同内容同日重跑幂等。
USERDATA_SHA="$(sha256sum "$SEED_DIR/user-data" | awk '{print $1}')"
INSTANCE_ID="nemesis-guest-base-$(date +%Y%m%d)-${USERDATA_SHA:0:8}"

cat > "$SEED_DIR/meta-data" <<EOF
instance-id: $INSTANCE_ID
local-hostname: nemesis-guest
EOF

# ---------------------------------------------------------------------------
# [3] seed 烧录进 base（raw 中转 + loop 挂 p1 + 写 NoCloud seed 目录）
# ---------------------------------------------------------------------------
echo "== [3/4] seed 烧录进 base（/var/lib/cloud/seed/nocloud/）=="
WORK_RAW="$OUT_DIR/.work.raw"
SEED_DEST="/var/lib/cloud/seed/nocloud"
LOOP=""
MNT=""
cleanup_burn() {
    # 有状态副作用收尸：mount/loop 不清会泄漏到宿主（下次构建 losetup 撞占）。
    if [ -n "$MNT" ] && mountpoint -q "$MNT" 2>/dev/null; then
        umount "$MNT" || true
    fi
    if [ -n "$LOOP" ]; then
        losetup -d "$LOOP" || true
    fi
    [ -n "$MNT" ] && rmdir "$MNT" 2>/dev/null || true
    rm -f "$WORK_RAW"
}
trap cleanup_burn EXIT

echo "   convert raw 中转"
qemu-img convert -O raw "$UPSTREAM_FILE" "$WORK_RAW"
LOOP="$(losetup -fP --show "$WORK_RAW")" || fail "losetup 失败（loop 设备不可用）"
MNT="$(mktemp -d)"
mount "${LOOP}p1" "$MNT" || fail "rootfs 挂载失败（${LOOP}p1 不在？Debian genericcloud 布局假设 p1，上游换布局需诚实适配）"

install -Dm644 "$SEED_DIR/user-data"  "$MNT$SEED_DEST/user-data"
install -Dm644 "$SEED_DIR/meta-data"  "$MNT$SEED_DEST/meta-data"
# 回读校验：写进去的字节 = seed 字节（cmp 静默吞写坏比想象常见，诚实对账）。
cmp -s "$SEED_DIR/user-data" "$MNT$SEED_DEST/user-data" || fail "user-data 烧录回读不一致"
cmp -s "$SEED_DIR/meta-data" "$MNT$SEED_DEST/meta-data" || fail "meta-data 烧录回读不一致"
echo "   seed 已烧录（instance-id=$INSTANCE_ID）"

umount "$MNT"
MNT=""
losetup -d "$LOOP"
LOOP=""
trap - EXIT

echo "   convert 回 qcow2 成品"
qemu-img convert -O qcow2 "$WORK_RAW" "$OUT_DIR/base.qcow2"
rm -f "$WORK_RAW"

# ---------------------------------------------------------------------------
# [4] manifest（G2 留痕面：来源/校验和/日期/端口——镜像描述的文件层）
# ---------------------------------------------------------------------------
echo "== [4/4] manifest =="
UPSTREAM_SIZE="$(stat -c%s "$UPSTREAM_FILE" 2>/dev/null || stat -f%z "$UPSTREAM_FILE")"
BASE_SIZE="$(stat -c%s "$OUT_DIR/base.qcow2" 2>/dev/null || stat -f%z "$OUT_DIR/base.qcow2")"
FINAL_SHA="$(sha256sum "$OUT_DIR/base.qcow2" | awk '{print $1}')"
cat > "$OUT_DIR/manifest.json" <<EOF
{
  "arch": "$ARCH",
  "variant": "${VARIANT:-base}",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "generator": "scripts/build-vm-image.sh",
  "guest_agent_port": $GUEST_AGENT_PORT,
  "workspace_mount_tag": "workspace",
  "base": { "digest_algo": "sha256", "digest": "$FINAL_SHA", "size_bytes": $BASE_SIZE, "file": "base.qcow2", "seed_burned_in": "$SEED_DEST" },
  "upstream": { "url": "$BASE_URL", "digest_algo": "$ALGO", "digest": "$ACTUAL_SHA", "size_bytes": $UPSTREAM_SIZE, "file": "base.upstream.qcow2" },
  "seed": { "instance_id": "$INSTANCE_ID", "apt_mirror": "${APT_MIRROR:-}", "files": ["user-data", "meta-data"] },
  "guest_bin": { "sha256": "$GUEST_SHA", "injected": "first-boot via cloud-init (b64 in user-data)" }
}
EOF

echo ""
echo "build-vm-image: PASS"
echo "  $OUT_DIR/base.qcow2  ($BASE_SIZE bytes, seed burned in at $SEED_DEST)"
echo "  $OUT_DIR/base.upstream.qcow2  (上游原件留存)"
echo "  $OUT_DIR/manifest.json"
echo "冒烟：cargo run -p nemesis-sandbox --features vm --example vm_smoke -- \\"
echo "  --base $OUT_DIR/base.qcow2"
