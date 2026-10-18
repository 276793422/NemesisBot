#!/usr/bin/env bash
# ============================================================================
# P5 最小镜像管线（第 2 步交付物，实施计划 §3「最小镜像管线」）
#
# 产出（<out>/ 目录三件套 + manifest.json）：
#   base.qcow2      裸 Debian genericcloud 云镜像（下载 + 上游 SHA256SUMS 校验）
#   seed.iso        cloud-init nocloud seed（首启装配 guest agent：b64 注入
#                   二进制 + launcher 读 SMBIOS DMI serial 作 token + systemd
#                   常驻 + 持久 networkd DHCP 单元（冷启网络不依赖 cloud-init
#                   per-instance 语义）+ 9p 共享子树 best-effort 挂载）
#   manifest.json   来源/校验和/日期/端口清单（G2 留痕面）
#
# 诚实边界：
# - Linux 宿主脚本（ISO 工具 genisoimage/xorriso/mkisofs 系）；Windows 宿主
#   走 WSL 或远程 Linux 构建回传（实施计划 §6 风险表既定）；
# - aarch64 = 接口预留档，选择即诚实拒绝（GuestArch 同一口径）；
# - 坏校验和 = 构建拒绝 + 产物改名留痕（base.qcow2.corrupt）+ manifest 不写；
# - 磁盘扩容不需要本脚本管：overlay 大于 base 时，genericcloud 的 cloud-init
#   growpart/resizefs 首启自动扩 rootfs 到 overlay 尺寸。
#
# 用法：
#   scripts/build-vm-image.sh --guest-bin target/x86_64-unknown-linux-musl/release/nemesis-guest
#   scripts/build-vm-image.sh --guest-bin ... --base-url <URL> --sha256 <HEX>   # 来源可换（换源必须给钉死的校验和）
#   scripts/build-vm-image.sh --guest-bin ... --skip-download                    # base.qcow2 已在位（复验校验和后直接做 seed）
#   scripts/build-vm-image.sh --guest-bin ... --variant desktop                  # WM+Chromium 变体（E 线人工验证面载体）
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

usage() { grep '^#' "$0" | head -40; exit 2; }

while [ $# -gt 0 ]; do
    case "$1" in
        --out)          OUT_DIR="$2"; shift 2 ;;
        --guest-bin)    GUEST_BIN="$2"; shift 2 ;;
        --base-url)     BASE_URL="$2"; shift 2 ;;
        --sha256)       SHA256_OVERRIDE="$2"; shift 2 ;;
        --arch)         ARCH="$2"; shift 2 ;;
        --variant)      VARIANT="$2"; shift 2 ;;
        --skip-download) SKIP_DOWNLOAD=1; shift ;;
        -h|--help)      usage ;;
        *) echo "!! 未知参数: $1"; usage ;;
    esac
done

fail() { echo "!! $*" >&2; exit 1; }

# ---------------------------------------------------------------------------
# 前置闸：arch / guest 二进制 / 下载工具 / ISO 工具（全部诚实报缺）
# ---------------------------------------------------------------------------
[ "$ARCH" = "x86_64" ] || fail "arch '$ARCH' 为接口预留档，当前不产出（合法值：x86_64）"
[ -n "$GUEST_BIN" ] || fail "缺 --guest-bin（scripts/build-guest.sh 的 musl 产物）"
[ -f "$GUEST_BIN" ] || fail "guest 二进制不存在: $GUEST_BIN"
# file(1) 措辞随版本分叉：旧版 "statically linked"；新版对 musl rustc 产物
# （默认 static-pie）报 "static-pie linked"——两者都是无 interpreter 的静态
# 形态，与 build-guest.sh 的 ld-musl 缺席断言同口径（有 interpreter 才非静态）。
file "$GUEST_BIN" 2>/dev/null | grep -Eqi "statically linked|static-pie linked" \
    || fail "guest 二进制非静态链接（musl 目标要求 statically linked / static-pie linked）：$(file "$GUEST_BIN" 2>/dev/null || echo 'file 命令不可用')"

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

# ISO 工具：genisoimage / xorriso / mkisofs 三选一（make_iso 统一入口）
MKISO=""
if command -v genisoimage >/dev/null 2>&1; then MKISO="genisoimage";
elif command -v xorriso >/dev/null 2>&1; then MKISO="xorriso";
elif command -v mkisofs >/dev/null 2>&1; then MKISO="mkisofs";
else fail "ISO 工具缺席（需 genisoimage / xorriso / mkisofs 任一；Debian/Ubuntu: apt install genisoimage）"
fi
make_iso() { # make_iso <out.iso> <dir-with-user-data+meta-data>
    case "$MKISO" in
        genisoimage|mkisofs) "$MKISO" -quiet -output "$1" -volid cidata -joliet -rock "$2" ;;
        xorriso)             "$MKISO" -as mkisofs -quiet -output "$1" -volid cidata -joliet -rock "$2" ;;
    esac
}

mkdir -p "$OUT_DIR"
BASE_FILE="$OUT_DIR/base.qcow2"

# ---------------------------------------------------------------------------
# [1] base 下载 + 校验（显式 --sha256 > 上游 SHA256SUMS sidecar；坏 = 拒绝留痕）
# ---------------------------------------------------------------------------
echo "== [1/4] base 镜像下载 + 校验 =="
BASE_NAME="${BASE_URL##*/}"
if [ "$SKIP_DOWNLOAD" = 1 ]; then
    [ -f "$BASE_FILE" ] || fail "--skip-download 但 $BASE_FILE 不存在"
else
    echo "   HEAD 探测: $BASE_URL"
    probe_url "$BASE_URL" || fail "URL 不可达（构建前 HEAD 探测失败）: $BASE_URL"
    echo "   下载 → $BASE_FILE"
    fetch "$BASE_URL" "$BASE_FILE"
fi
[ -f "$BASE_FILE" ] || fail "base 镜像不在位: $BASE_FILE"

EXPECTED_SHA="$SHA256_OVERRIDE"
ALGO="sha256"
if [ -z "$EXPECTED_SHA" ]; then
    [ "$BASE_URL" = "$DEFAULT_BASE_URL" ] || fail "换源必须显式给 --sha256（钉死信任锚，不猜上游校验和）"
    # 上游 sidecar：SHA256SUMS 优先；缺席（trixie/latest 实测只有 SHA512SUMS）
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
    ACTUAL_SHA="$(sha256sum "$BASE_FILE" | awk '{print $1}')"
else
    ACTUAL_SHA="$(sha512sum "$BASE_FILE" | awk '{print $1}')"
fi
if [ "$ACTUAL_SHA" != "$EXPECTED_SHA" ]; then
    mv "$BASE_FILE" "$BASE_FILE.corrupt"   # 留痕：坏产物保留待查
    fail "校验和不匹配：期望 $EXPECTED_SHA 实得 $ACTUAL_SHA（坏产物已改名 $BASE_FILE.corrupt，manifest 不写）"
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

INSTANCE_ID="nemesis-guest-base-$(date +%Y%m%d)-${GUEST_SHA:0:8}"

# variant 扩展（P5 第 3 步 E 线）：desktop = WM + Chromium（浏览器登录保持
# 人工验证面的载体——自动化断言不含 GUI 登录，诚实边界）。cloud-config
# packages 段首启 apt 安装（数百 MB，一次性进 base 之后的 overlay/镜像）。
case "$VARIANT" in
    "")          VARIANT_USERDATA="" ;;
    desktop)     VARIANT_USERDATA='
packages:
  - openbox
  - xinit
  - xterm
  - chromium
' ;;
    *) fail "未知 variant: $VARIANT（当前支持: desktop）" ;;
esac

cat > "$SEED_DIR/meta-data" <<EOF
instance-id: $INSTANCE_ID
local-hostname: nemesis-guest
EOF

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
${VARIANT_USERDATA}
runcmd:
  - systemctl daemon-reload
  - systemctl enable --now nemesis-guest-agent.service
  # 9p 共享子树 best-effort（Windows 宿主无 virtfs = 挂载缺席，/workspace 落 VM 磁盘）
  - mkdir -p /workspace
  - sh -c 'mount -t 9p -o trans=virtio,version=9p2000.L workspace /workspace 2>>/var/log/nemesis-guest-provision.log || echo "9p workspace tag absent; /workspace on VM disk" >>/var/log/nemesis-guest-provision.log'
EOF

# ---------------------------------------------------------------------------
# [3] seed ISO
# ---------------------------------------------------------------------------
echo "== [3/4] seed ISO 打包（$MKISO）=="
SEED_ISO="$OUT_DIR/seed.iso"
rm -f "$SEED_ISO"
make_iso "$SEED_ISO" "$SEED_DIR"
[ -f "$SEED_ISO" ] || fail "seed ISO 未产出（$MKISO 静默失败）"

# ---------------------------------------------------------------------------
# [4] manifest（G2 留痕面：来源/校验和/日期/端口——镜像描述的文件层）
# ---------------------------------------------------------------------------
echo "== [4/4] manifest =="
BASE_SIZE="$(stat -c%s "$BASE_FILE" 2>/dev/null || stat -f%z "$BASE_FILE")"
SEED_SHA="$(sha256sum "$SEED_ISO" | awk '{print $1}')"
cat > "$OUT_DIR/manifest.json" <<EOF
{
  "arch": "$ARCH",
  "variant": "${VARIANT:-base}",
  "built_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "generator": "scripts/build-vm-image.sh",
  "guest_agent_port": $GUEST_AGENT_PORT,
  "workspace_mount_tag": "workspace",
  "base": { "url": "$BASE_URL", "digest_algo": "$ALGO", "digest": "$ACTUAL_SHA", "size_bytes": $BASE_SIZE, "file": "base.qcow2" },
  "seed": { "sha256": "$SEED_SHA", "file": "seed.iso", "instance_id": "$INSTANCE_ID" },
  "guest_bin": { "sha256": "$GUEST_SHA", "injected": "first-boot via cloud-init (b64 in user-data)" }
}
EOF

echo ""
echo "build-vm-image: PASS"
echo "  $BASE_FILE  ($BASE_SIZE bytes)"
echo "  $SEED_ISO"
echo "  $OUT_DIR/manifest.json"
echo "冒烟：cargo run -p nemesis-sandbox --features vm --example vm_smoke -- \\"
echo "  --base $OUT_DIR/base.qcow2 --seed $SEED_ISO"
