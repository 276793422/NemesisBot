#!/usr/bin/env bash
# ===========================================================================
#  exe-sign-tool 一键脚本（Linux / macOS / git-bash）
#  功能：给指定可执行文件加签名，并自动验证检测。
#  用法：quick-sign.sh <executable-path>
#  例：  quick-sign.sh /path/to/app
#        quick-sign.sh target/debug/nemesisbot
# ===========================================================================
set -uo pipefail

if [ "$#" -lt 1 ]; then
  echo "Usage: $0 <executable-path>"
  echo "  给指定可执行文件加签名，然后验证检测。"
  echo "  首次运行自动生成密钥（keygen），之后复用 keys/ 目录。"
  echo
  echo "  例: $0 /path/to/app"
  exit 1
fi

# 解析绝对路径
TARGET="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KEYS="$SCRIPT_DIR/keys"

if [ ! -f "$TARGET" ]; then
  echo "ERROR: file not found: $TARGET" >&2
  exit 1
fi

# 切到项目根（cargo run 需在 workspace 根执行）
cd "$SCRIPT_DIR/../.."

run() { cargo run -q -p exe-sign-tool -- "$@"; }

# [1/3] 密钥：首次生成，之后复用
if [ ! -f "$KEYS/exe_sign.key" ]; then
  echo "[1/3] keygen -> $KEYS"
  run keygen --out "$KEYS" || { echo "KEYGEN FAILED"; exit 1; }
else
  echo "[1/3] reuse existing keys: $KEYS"
fi

# [2/3] 加签名（原地追加 4KB envelope；--key-dir 自动找 key+sym）
echo "[2/3] sign: $TARGET"
run sign "$TARGET" --key-dir "$KEYS" \
  || { echo "SIGN FAILED"; exit 1; }

# [3/3] 验证检测（--key-dir 自动找 pub+sym）
echo "[3/3] verify:"
run verify "$TARGET" --key-dir "$KEYS" \
  || { echo "VERIFY FAILED -> 文件可能已被篡改或密钥不匹配"; exit 1; }

echo ""
echo "DONE: $TARGET  signed and verified OK."
exit 0
