#!/bin/bash
# setup-test.sh — Download all-MiniLM-L6-v2 ONNX model + tokenizer for testing
#
# Usage:
#   bash test-tools/plugin-onnx-test/scripts/setup-test.sh
#
# Downloads:
#   - model.onnx       (~90 MB)  from HuggingFace sentence-transformers
#   - tokenizer.json    (~470 KB) from HuggingFace sentence-transformers
#
# Output:
#   test-tools/plugin-onnx-test/test-data/model.onnx
#   test-tools/plugin-onnx-test/test-data/tokenizer.json

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DATA_DIR="$SCRIPT_DIR/../test-data"

MODEL_NAME="sentence-transformers/all-MiniLM-L6-v2"
BASE_URL="https://hf-mirror.com/${MODEL_NAME}/resolve/main"

echo "[setup-test] Downloading test model: ${MODEL_NAME}"
echo "[setup-test] Target: ${TEST_DATA_DIR}"
echo ""

mkdir -p "$TEST_DATA_DIR"

# Download model.onnx
if [ -f "$TEST_DATA_DIR/model.onnx" ]; then
    echo "[setup-test] model.onnx already exists, skipping download"
else
    echo "[setup-test] Downloading model.onnx (~90 MB)..."
    curl -L -o "$TEST_DATA_DIR/model.onnx" "${BASE_URL}/onnx/model.onnx"
    echo "[setup-test] OK model.onnx downloaded"
fi

# Download tokenizer.json
if [ -f "$TEST_DATA_DIR/tokenizer.json" ]; then
    echo "[setup-test] tokenizer.json already exists, skipping download"
else
    echo "[setup-test] Downloading tokenizer.json (~470 KB)..."
    curl -L -o "$TEST_DATA_DIR/tokenizer.json" "${BASE_URL}/tokenizer.json"
    echo "[setup-test] OK tokenizer.json downloaded"
fi

# Verify files
echo ""
echo "[setup-test] Verification:"
MODEL_SIZE=$(wc -c < "$TEST_DATA_DIR/model.onnx")
TOK_SIZE=$(wc -c < "$TEST_DATA_DIR/tokenizer.json")
echo "  model.onnx:      $(( MODEL_SIZE / 1048576 )) MB"
echo "  tokenizer.json:  $(( TOK_SIZE / 1024 )) KB"

if [ "$MODEL_SIZE" -lt 1000000 ]; then
    echo "[setup-test] ERROR: model.onnx is too small (< 1 MB), download may have failed"
    exit 1
fi

if [ "$TOK_SIZE" -lt 1000 ]; then
    echo "[setup-test] ERROR: tokenizer.json is too small (< 1 KB), download may have failed"
    exit 1
fi

echo ""
echo "[setup-test] Ready. Run tests with:"
echo "  cd plugins/plugin-onnx"
echo "  PLUGIN_ONNX_TEST_MODEL_DIR=\"$(cygpath -w "$TEST_DATA_DIR" 2>/dev/null || echo "$TEST_DATA_DIR")\" cargo test -- --ignored"
