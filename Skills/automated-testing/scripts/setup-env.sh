#!/bin/bash
#
# setup-env.sh - NemesisBot 测试环境准备脚本
#
# 功能：
#   1. 编译 testaiserver.exe → test/autotest/
#   2. 编译 nemesisbot.exe → test/autotest/
#   3. 编译 websocket_chat_client.exe → test/autotest/
#   4. 启动 testaiserver（后台）
#   5. 保存 testaiserver PID
#
# 使用方法：
#   bash Skills/automated-testing/scripts/setup-env.sh
#
# 输出格式：
#   SETUP_START
#   SETUP_SUCCESS|SETUP_FAILURE
#   TESTAI_PID=<pid>
#   TESTAI_PORT=8080
#

set -e

# 颜色输出（可选）
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "SETUP_START"

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# 进入项目根目录
cd "$PROJECT_ROOT"

# === 1. 检查环境 ===

# 检查 Go
if ! command -v go &> /dev/null; then
    echo "ERROR: Go not installed"
    echo "SETUP_FAILURE"
    exit 1
fi

# 检查必要目录
if [ ! -d "test/TestAIServer" ]; then
    echo "ERROR: test/TestAIServer not found"
    echo "SETUP_FAILURE"
    exit 1
fi

if [ ! -f "test/websocket_chat_client.go" ]; then
    echo "ERROR: test/websocket_chat_client.go not found"
    echo "SETUP_FAILURE"
    exit 1
fi

# === 2. 创建测试目录 ===

echo "Creating test/autotest directory..."
mkdir -p test/autotest

# === 3. 编译组件 ===

echo "Compiling test components..."

# 编译 TestAIServer（必须在自身模块目录中编译）
echo "[1/3] Compiling TestAIServer..."
cd test/TestAIServer
if ! go build -o ../autotest/testaiserver.exe .; then
    echo "ERROR: Failed to compile TestAIServer"
    cd "$PROJECT_ROOT"
    echo "SETUP_FAILURE"
    exit 1
fi
cd "$PROJECT_ROOT"

# 编译 NemesisBot
echo "[2/3] Compiling NemesisBot..."
# IMPORTANT: 必须使用 production build tag 才能编译 Wails UI
if ! go build -tags production -o test/autotest/nemesisbot.exe ./nemesisbot; then
    echo "ERROR: Failed to compile NemesisBot"
    echo "SETUP_FAILURE"
    exit 1
fi

# 编译 WebSocket 客户端
echo "[3/3] Compiling WebSocket client..."
if ! go build -o test/autotest/websocket_chat_client.exe test/websocket_chat_client.go; then
    echo "ERROR: Failed to compile WebSocket client"
    echo "SETUP_FAILURE"
    exit 1
fi

# 验证编译产物
if [ ! -f "test/autotest/testaiserver.exe" ] || \
   [ ! -f "test/autotest/nemesisbot.exe" ] || \
   [ ! -f "test/autotest/websocket_chat_client.exe" ]; then
    echo "ERROR: Compilation artifacts missing"
    echo "SETUP_FAILURE"
    exit 1
fi

echo "Compilation successful"

# === 4. 启动 TestAIServer ===

echo "Starting TestAIServer..."

cd test/autotest

# 停止可能存在的旧进程
taskkill //F //IM testaiserver.exe 2>/dev/null || true
sleep 1

# 启动 TestAIServer
./testaiserver.exe &
TESTAI_PID=$!

# 保存 PID
echo "$TESTAI_PID" > testaiserver.pid

# 等待 TestAIServer 就绪
echo "Waiting for TestAIServer to be ready..."
READY=0
for i in {1..30}; do
    if curl -s http://127.0.0.1:8080/v1/models > /dev/null 2>&1; then
        READY=1
        break
    fi
    sleep 1
done

if [ $READY -eq 0 ]; then
    echo "ERROR: TestAIServer failed to start"
    echo "SETUP_FAILURE"
    exit 1
fi

# === 5. 输出结果 ===

echo "SETUP_SUCCESS"
echo "TESTAI_PID=$TESTAI_PID"
echo "TESTAI_PORT=8080"
echo "WORK_DIR=$PWD"

echo ""
echo "Environment setup completed successfully!"
echo "TestAIServer is running with PID: $TESTAI_PID"
echo "TestAIServer endpoint: http://127.0.0.1:8080/v1"

exit 0
