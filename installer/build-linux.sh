#!/usr/bin/env bash
# ============================================
# NemesisBot 安装包构建脚本（Linux）
# ============================================
# 用法: installer/build-linux.sh [选项]
#   （无参数）     构建 bot + 安装器 + 打包 → dist/nemesisbot-setup-<ver>-x64.run
#   --skip-bot     复用既有 bin/bin_linux（不重编主程序）
#   --clean        传给主仓构建脚本（清理后构建）
#   --sign <keys>  用 keys.json 签名安装包（绿级信任链；缺省 = 不签）
#   --anchor <hex> 编译期固化根锚（NEMESIS_BUILD_ROOT_ANCHOR）
#   --out <file>   输出路径（缺省 dist/nemesisbot-setup-<ver>-x64.run）
#   --help         显示帮助
#
# 全程在 installer/ 独立 workspace 内运作，产物落 installer/dist/，
# 对外部目录零污染（隔离契约见 installer/README.md）。

set -euo pipefail

cd "$(dirname "$0")"
ROOT="$(cd .. && pwd)"

SKIP_BOT=0
CLEAN=0
SIGN=""
ANCHOR=""
OUT=""

show_help() {
  cat <<'EOF'
NemesisBot 安装包构建（Linux）

用法: installer/build-linux.sh [选项]
  --skip-bot     复用既有 bin/bin_linux
  --clean        清理后构建主程序
  --sign <keys>  keys.json 签名（绿级；缺省不签 = 黄色提示）
  --anchor <hex> 编译期固化根锚
  --out <file>   输出路径
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --skip-bot) SKIP_BOT=1 ;;
    --clean) CLEAN=1 ;;
    --sign) SIGN="${2:?--sign 需要路径}"; shift ;;
    --anchor) ANCHOR="${2:?--anchor 需要 hex}"; shift ;;
    --out) OUT="${2:?--out 需要路径}"; shift ;;
    --help|-h) show_help; exit 0 ;;
    *) echo "[ERROR] 未知参数: $1（--help 查看用法）"; exit 1 ;;
  esac
  shift
done

# ① 主程序构建（主仓脚本；产物 bin/bin_linux/）。
if [ "$SKIP_BOT" -eq 0 ]; then
  echo "[1/5] 构建主程序（scripts/build-linux.sh）..."
  # shellcheck disable=SC2086
  bash "$ROOT/scripts/build-linux.sh" ${CLEAN:+--clean}
else
  echo "[1/5] 跳过主程序构建（复用既有 bin/bin_linux）"
  if [ ! -f "$ROOT/bin/bin_linux/nemesisbot" ]; then
    echo "[ERROR] bin/bin_linux/nemesisbot 缺席——去掉 --skip-bot 先构建"
    exit 1
  fi
fi

# ② 图标进载荷（share/icon.png；.desktop 入口引用）。
mkdir -p "$ROOT/bin/bin_linux/share"
cp -f "$ROOT/web/public/favicon.png" "$ROOT/bin/bin_linux/share/icon.png"
echo "[2/5] 图标已同步 share/icon.png"

# ③ 安装器构建（本 workspace release；骨架 = 未签名原始二进制）。
# 定 Target 构建——workspace 三成员里 setup-windows 是 win32 依赖
# （winreg/windows-sys/egui win32），Linux 主机上全量 `cargo build` 必炸
# E0432（盲区实录 2026-11-10：远程盒首跑即红）。Windows 主机全量无碍。
# 注意 pack 是 setup-core 包的 [[bin]]（`-p pack` 不存在——包名解析失败实录）。
echo "[3/5] 构建安装器（cargo build --release -p setup-linux -p setup-core）..."
cd "$(dirname "$0")"
if [ -n "$ANCHOR" ]; then
  export NEMESIS_BUILD_ROOT_ANCHOR="$ANCHOR"
fi
cargo build --release -p setup-linux -p setup-core
if [ ! -f target/release/nbsetup-linux ]; then
  echo "[ERROR] nbsetup-linux 未产出"
  exit 1
fi

# ④ 版本号（与主仓构建脚本同源：git tag，缺省 0.0.0.1）。
VERSION="$(git describe --tags --abbrev=0 2>/dev/null || echo 0.0.0.1)"

# ⑤ 打包 NBSETUP1（payload = bin_linux，骨架 = nbsetup-linux）。
if [ -z "$OUT" ]; then
  OUT="dist/nemesisbot-setup-${VERSION}-x64.run"
fi
mkdir -p dist
echo "[4/5] 打包 NBSETUP1（版本 $VERSION）..."
PACK_ARGS=(pack
  --payload "$ROOT/bin/bin_linux"
  --skeleton target/release/nbsetup-linux
  --out "$OUT"
  --version "$VERSION"
  --platform linux)
if [ -n "$SIGN" ]; then
  PACK_ARGS+=(--sign "$SIGN")
fi
./target/release/pack "${PACK_ARGS[@]}"

# ⑤b 整包签名（ELF/raw v4 footer；安装器启动外壳自检的绿级来源——两层
# 信任链的第二层，第一层 = pack --sign 的 manifest blob 签名）。
if [ -n "$SIGN" ]; then
  echo "签名整包（外壳自检绿级）..."
  ./target/release/pack sign "$OUT" --keys "$SIGN"
fi

echo "[5/5] 检视成品..."
./target/release/pack inspect "$OUT"
echo
echo "产物: $OUT"
