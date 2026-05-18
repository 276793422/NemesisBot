#!/bin/bash
#
# setup-env.sh - Memory UAT 环境准备脚本
#
# 功能：
#   1. 验证二进制文件存在（nemesisbot.exe, testaiserver.exe）
#   2. 可选：编译 plugin-onnx DLL
#   3. 创建工作目录
#   4. 启动 TestAIServer
#
# 使用方法：
#   bash Skills/memory-uat/scripts/setup-env.sh

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
WORKDIR="$PROJECT_ROOT/test-tools/memory-uat-workdir"

echo "========================================="
echo " Memory UAT 环境准备"
echo "========================================="

cd "$PROJECT_ROOT"

# === 1. 检查二进制文件 ===

NEMESISBOT_BIN=""
if [ -f "target/release/nemesisbot.exe" ]; then
    NEMESISBOT_BIN="target/release/nemesisbot.exe"
    echo -e "${GREEN}[OK]${NC} nemesisbot.exe (release)"
elif [ -f "target/debug/nemesisbot.exe" ]; then
    NEMESISBOT_BIN="target/debug/nemesisbot.exe"
    echo -e "${YELLOW}[WARN]${NC} nemesisbot.exe (debug)"
else
    echo -e "${RED}[MISSING]${NC} nemesisbot.exe not found. Run: cargo build --release -p nemesisbot"
    exit 1
fi

AI_BIN=""
if [ -f "test-tools/TestAIServer/testaiserver.exe" ]; then
    AI_BIN="test-tools/TestAIServer/testaiserver.exe"
    echo -e "${GREEN}[OK]${NC} testaiserver.exe"
else
    echo -e "${YELLOW}[BUILD]${NC} Building testaiserver.exe..."
    cd test-tools/TestAIServer && go build -o testaiserver.exe && cd "$PROJECT_ROOT"
    AI_BIN="test-tools/TestAIServer/testaiserver.exe"
    echo -e "${GREEN}[OK]${NC} testaiserver.exe (built)"
fi

# === 2. 检查 plugin DLL ===

PLUGIN_DLL=""
if [ -f "target/release/plugins/plugin_onnx.dll" ]; then
    PLUGIN_DLL="target/release/plugins/plugin_onnx.dll"
    echo -e "${GREEN}[OK]${NC} plugin_onnx.dll (release)"
elif [ -f "target/debug/plugins/plugin_onnx.dll" ]; then
    PLUGIN_DLL="target/debug/plugins/plugin_onnx.dll"
    echo -e "${YELLOW}[WARN]${NC} plugin_onnx.dll (debug)"
else
    echo -e "${YELLOW}[SKIP]${NC} plugin_onnx.dll not found. Plugin tests will be skipped."
fi

# === 3. 检查 ONNX 模型 ===

MODEL_DIR="test-tools/plugin-onnx-test/test-data"
if [ -f "$MODEL_DIR/model.onnx" ] && [ -f "$MODEL_DIR/tokenizer.json" ]; then
    echo -e "${GREEN}[OK]${NC} ONNX model files"
else
    echo -e "${YELLOW}[SKIP]${NC} ONNX model files not found. Run: bash test-tools/plugin-onnx-test/scripts/setup-test.sh"
fi

# === 4. 创建工作目录 ===

echo ""
echo "Creating work directory: $WORKDIR"
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR/plugins"

cp "$NEMESISBOT_BIN" "$WORKDIR/"
cp "$AI_BIN" "$WORKDIR/"

if [ -n "$PLUGIN_DLL" ]; then
    cp "$PLUGIN_DLL" "$WORKDIR/plugins/"
fi

echo -e "${GREEN}[OK]${NC} Work directory created"

# === 5. 杀死占用端口的进程 ===

echo ""
echo "Cleaning up ports..."
for port in 8080 49000 18790; do
    pids=$(netstat -ano 2>/dev/null | grep ":${port} " | grep "LISTENING" | awk '{print $5}' | sort -u)
    for pid in $pids; do
        if [ -n "$pid" ]; then
            taskkill //F //PID "$pid" 2>/dev/null || true
            echo "  Killed PID $pid (port $port)"
        fi
    done
done

# === 6. 启动 TestAIServer ===

echo ""
echo "Starting TestAIServer..."
cd "$WORKDIR"
./testaiserver.exe &
AI_PID=$!
cd "$PROJECT_ROOT"

# 等待 AI server 启动
sleep 2

# 验证 AI server
if curl -s http://127.0.0.1:8080/v1/models > /dev/null 2>&1; then
    echo -e "${GREEN}[OK]${NC} TestAIServer started (PID: $AI_PID, port: 8080)"
else
    echo -e "${RED}[FAIL]${NC} TestAIServer failed to start"
    exit 1
fi

echo ""
echo "========================================="
echo " 环境准备完成"
echo "========================================="
echo " 工作目录: $WORKDIR"
echo " AI Server PID: $AI_PID"
echo " Plugin DLL: ${PLUGIN_DLL:-NOT AVAILABLE}"
echo ""
echo " 下一步：按照 SKILL.md 的 Stage 3-6 执行 UAT 测试"
echo "========================================="

# 保存 PID
echo "$AI_PID" > "$WORKDIR/testaiserver.pid"
