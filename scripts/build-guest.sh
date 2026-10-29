#!/usr/bin/env bash
# ============================================================================
# P5 第 1 步 guest 交叉编译管线（build-guest）——musl 静态 ELF 构建 + 头断言
#
# 产出 nemesis-guest 的双架构 musl 静态二进制（VM 内执行体，第 2 步镜像
# 注入用），并做**产物级断言**（不是「构建没报错就算成」）：
#   1. ELF magic（7f 45 4c 46）+ e_machine 架构位（x86-64=3E / aarch64=B7，
#      偏移 18 LE）——防「构建出来了但架构不对」；
#   2. 静态性：产物不含 "ld-musl" 字符串（动态链接才有 interpreter 路径）
#      ——VM 基础镜像不保证有 musl loader，静态是硬前提；
#   3. file(1) 交叉印证（可用时）+ sha256 清单；
#   4. 宿主构建 stdio 冒烟（本机可跑）+ x86_64-musl 在 Linux 宿主的功能
#      冒烟；aarch64 产物只做头断言——「运行不承诺」双注明（本地无
#      aarch64 运行环境；远程 aarch64 设备冒烟是后续项）。
#   5. musl 依赖图纯度（BUG 2026-10-30）：依赖图不得含 C 代码 crate
#      （ring / aws-lc-sys / libsqlite3-sys）——guest 保持纯 Rust，
#      nemesis-utils 的 "http" feature 在 guest 树保持关闭。
#
# 用法（仓库根目录）：
#   bash scripts/build-guest.sh                     # 双 musl 目标 + 宿主冒烟
#   bash scripts/build-guest.sh "x86_64-unknown-linux-musl"   # 只建一个
#
# 产物根跟随 CARGO_TARGET_DIR（未设 = target/）：build-windows.bat /
# build-linux.sh 都是设了 target 重定向后再调本脚本，内部路径必须与 cargo
# 实际落点同源，否则宿主冒烟与交叉产物 cp 全部扑空。
#
# 退出码：0 = 全绿；非 0 = 有红灯（构建失败直接中止）。
# ============================================================================
set -u
cd "$(dirname "$0")/.." || exit 1

TARGETS="${1:-x86_64-unknown-linux-musl aarch64-unknown-linux-musl}"
OUT_ROOT=bin/bin_guest
PKG=nemesis-guest
HOST_BIN_EXT=""
[ "$(uname -s)" = "Linux" ] || HOST_BIN_EXT=".exe"

# 产物根单一真相（2026-10-22 实录：build-windows.bat 全局设
# CARGO_TARGET_DIR=target\target_windows 后调本脚本，硬编码 target/ 让宿主
# 冒烟与交叉产物 cp 全部扑空 → bin_guest 落空 → plugin-vm 在
# NMB_VM_REQUIRE_RESOURCES=1 下 fail-closed 拒绝出包）。CI 未设该 env，
# 回落 target/ 行为不变。反斜杠统一归一成 /（git-bash 工具链对混合分隔符
# 容忍度不一）。
ART_ROOT="${CARGO_TARGET_DIR:-target}"
ART_ROOT="${ART_ROOT//\\//}"

FAIL=0
step() { printf '\n===== %s =====\n' "$*"; }

# stdio 冒烟：一条 v2 exec 回声请求 → 响应必须含 v2 形态 + call_id 回显 +
# 命令回声（宿主 cmd /C 与 guest sh -c 对 echo 同效）。
# 工作区显式给临时目录：guest 缺省 /workspace 是 VM 内语义，宿主直跑没有。
smoke_stdio() {
    local bin="$1"
    local req='{"v":2,"call_id":"smoke-1","tool":"exec","args":"{\"command\":\"echo guest-build-smoke\"}","context":{}}'
    local ws resp
    ws=$(mktemp -d) || return 1
    resp=$(printf '%s\n' "$req" | NEMESISBOT_GUEST_WORKSPACE="$ws" "$bin" 2>/dev/null)
    rm -rf "$ws" 2>/dev/null || true
    case "$resp" in
        *'"v":2'*'smoke-1'*'guest-build-smoke'*) return 0 ;;
        *) echo "!! smoke 响应形态不对: ${resp:-<empty>}"; return 1 ;;
    esac
}

# ELF 头断言：magic + e_machine（偏移 18，LE 两字节）。
assert_elf_arch() {
    local bin="$1" want_em="$2" tag="$3"
    local hex magic em
    hex=$(od -A n -t x1 -N 20 "$bin" 2>/dev/null | tr -d ' \n')
    if [ -z "$hex" ]; then
        echo "!! [$tag] ELF 头读取失败"; FAIL=1; return
    fi
    magic=${hex:0:8}
    em=${hex:36:4}
    [ "$magic" = "7f454c46" ] || { echo "!! [$tag] ELF magic 不对: $magic"; FAIL=1; }
    [ "$em" = "$want_em" ] || { echo "!! [$tag] e_machine 不对: $em（期望 $want_em）"; FAIL=1; }
}

step "musl 目标安装检查（缺则 rustup target add）"
for t in $TARGETS; do
    if rustup target list --installed | grep -qw "$t"; then
        echo "ok: $t 已安装"
    else
        echo "安装 $t ..."
        rustup target add "$t" || { echo "!! rustup target add $t 失败"; exit 1; }
    fi
done

step "musl 依赖图纯度绊线（C 依赖检查，BUG 2026-10-30）"
# guest 必须保持纯 Rust（.cargo/config.toml 链接器注记的前提）：任何 C 代码
# crate（ring / aws-lc-sys / libsqlite3-sys）进 musl 依赖图都会把交叉构建变成
# C 工具链构建——无 musl-gcc 的机器死在 ring 编译段，有 musl-tools 的机器更糟：
# 静默编过 = 纯 Rust 不变量被破坏且无任何报错。`cargo tree -i X` 在 X 在图中时
# 退出 0（纯元数据查询，零编译），所以这里命令成功 = 绊线触发。
for p in ring aws-lc-sys libsqlite3-sys; do
    if cargo tree -p "$PKG" --target x86_64-unknown-linux-musl -i "$p" >/dev/null 2>&1; then
        echo "!! guest musl 依赖图含 C 代码依赖: $p"
        echo "   guest 必须保持纯 Rust——勿启用 nemesis-utils 的 http feature，"
        echo "   或向 guest 依赖树添加任何 C 支撑的 crate"
        exit 1
    fi
done
echo "ok: musl 依赖图无 C 代码依赖（ring/aws-lc-sys/libsqlite3-sys）"

step "宿主构建 + stdio 冒烟（本机形态，跨平台行为基准）"
cargo build --release -p "$PKG" || { echo "!! 宿主构建失败"; exit 1; }
HOST_BIN="${ART_ROOT}/release/${PKG}${HOST_BIN_EXT}"
if smoke_stdio "$HOST_BIN"; then
    echo "ok: 宿主冒烟通过（$HOST_BIN）"
else
    echo "!! 宿主冒烟失败"; FAIL=1
fi

for t in $TARGETS; do
    arch=${t%%-*}
    step "交叉构建 $t"
    cargo build --release -p "$PKG" --target "$t" \
        || { echo "!! 交叉构建失败: $t"; exit 1; }
    src="${ART_ROOT}/$t/release/${PKG}"
    outdir="$OUT_ROOT/$arch"
    mkdir -p "$outdir"
    cp "$src" "$outdir/$PKG"
    bin="$outdir/$PKG"

    assert_elf_arch "$bin" \
        "$([ "$arch" = x86_64 ] && echo 3e00 || echo b700)" "$arch"

    if grep -aq 'ld-musl' "$bin"; then
        echo "!! [$arch] 含 ld-musl——不是静态链接（VM 基础镜像无 musl loader）"
        FAIL=1
    else
        echo "ok: [$arch] 静态（无 ld-musl interpreter）"
    fi

    if command -v file >/dev/null 2>&1; then
        file "$bin" || true
    else
        echo "(file(1) 不可用，跳过交叉印证——ELF 头断言已覆盖)"
    fi
    echo "sha256[$arch] = $(sha256sum "$bin" | awk '{print $1}')"

    if [ "$arch" = x86_64 ] && [ "$(uname -s)" = Linux ] && [ "$(uname -m)" = x86_64 ]; then
        if smoke_stdio "$bin"; then
            echo "ok: [$arch] Linux 宿主功能冒烟通过（与宿主构建行为等价探针同一条）"
        else
            echo "!! [$arch] Linux 宿主功能冒烟失败"; FAIL=1
        fi
    else
        echo "注: [$arch] 运行冒烟不在本机能力范围（宿主 $(uname -s)/$(uname -m)）——"
        echo "    只承诺 ELF 头/静态断言；运行行为不承诺，等价验证归远程 Linux 设备。"
    fi
done

printf '\n'
if [ "$FAIL" -eq 0 ]; then
    echo "build-guest: ALL GREEN（产物在 $OUT_ROOT/<arch>/）"
else
    echo "build-guest: FAIL（见上方 !! 行）"
fi
exit "$FAIL"
