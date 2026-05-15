#!/bin/bash
#
# setup-env.sh - NemesisBot Rust 项目测试环境准备脚本
#
# 功能：
#   1. 编译 testaiserver.exe → test-tools/autotest/（Go 项目）
#   2. 编译 nemesisbot.exe → test-tools/autotest/（Rust 项目）
#   3. 编译 websocket_chat_client.exe → test-tools/autotest/（Rust 项目）
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

# 检查 Rust
if ! command -v cargo &> /dev/null; then
    echo "ERROR: Cargo not installed (Rust toolchain required)"
    echo "SETUP_FAILURE"
    exit 1
fi

# 检查 Go（TestAIServer 仍为 Go 项目）
if ! command -v go &> /dev/null; then
    echo "ERROR: Go not installed (required for TestAIServer)"
    echo "SETUP_FAILURE"
    exit 1
fi

# 检查必要目录
if [ ! -d "test-tools/TestAIServer" ]; then
    echo "ERROR: test-tools/TestAIServer not found"
    echo "SETUP_FAILURE"
    exit 1
fi

if [ ! -d "test-tools/websocket-client" ]; then
    echo "ERROR: test-tools/websocket-client not found"
    echo "SETUP_FAILURE"
    exit 1
fi

# === 2. 创建测试目录 ===

echo "Creating test-tools/autotest directory..."
mkdir -p test-tools/autotest

# === 3. 编译组件 ===

echo "Compiling test components..."

# 编译 TestAIServer（Go 项目，必须在自身模块目录中编译）
echo "[1/3] Compiling TestAIServer..."
cd test-tools/TestAIServer
if ! go build -o ../autotest/testaiserver.exe .; then
    echo "ERROR: Failed to compile TestAIServer"
    cd "$PROJECT_ROOT"
    echo "SETUP_FAILURE"
    exit 1
fi
cd "$PROJECT_ROOT"

# 编译 NemesisBot（Rust 项目，从根目录编译）
echo "[2/3] Compiling NemesisBot..."
if ! cargo build --release -p nemesisbot; then
    echo "ERROR: Failed to compile NemesisBot"
    echo "SETUP_FAILURE"
    exit 1
fi
cp target/release/nemesisbot.exe test-tools/autotest/

# 编译 WebSocket 客户端（Rust 项目）
echo "[3/3] Compiling WebSocket client..."
cd test-tools/websocket-client
if ! cargo build --release; then
    echo "ERROR: Failed to compile WebSocket client"
    cd "$PROJECT_ROOT"
    echo "SETUP_FAILURE"
    exit 1
fi
# 复制编译产物（二进制名取决于 Cargo.toml 配置）
# 尝试常见的二进制名
for bin_name in websocket_chat_client websocket-client ws_client; do
    if [ -f "target/release/${bin_name}.exe" ]; then
        cp "target/release/${bin_name}.exe" ../autotest/websocket_chat_client.exe
        break
    fi
done
cd "$PROJECT_ROOT"

# 验证编译产物
if [ ! -f "test-tools/autotest/testaiserver.exe" ] || \
   [ ! -f "test-tools/autotest/nemesisbot.exe" ]; then
    echo "ERROR: Compilation artifacts missing"
    echo "SETUP_FAILURE"
    exit 1
fi

echo "Compilation successful"

# === 4. 启动 TestAIServer ===

echo "Starting TestAIServer..."

cd test-tools/autotest

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
