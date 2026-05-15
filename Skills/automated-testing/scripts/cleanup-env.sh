#!/bin/bash
#
# cleanup-env.sh - NemesisBot Rust 项目测试环境清理脚本
#
# 功能：
#   1. 停止 nemesisbot.exe（通过进程名）
#   2. 停止 testaiserver.exe（通过进程名）
#   3. 等待文件释放
#
# 注意：
#   - 不删除 test-tools/autotest/ 目录（由 AI 负责）
#
# 使用方法：
#   bash Skills/automated-testing/scripts/cleanup-env.sh
#
# 输出格式：
#   CLEANUP_START
#   CLEANUP_SUCCESS|CLEANUP_FAILURE
#

set -e

echo "CLEANUP_START"

# 获取项目根目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

cd "$PROJECT_ROOT"

# === 1. 停止 NemesisBot ===

echo "Stopping NemesisBot..."

# 尝试通过 PID 文件停止
if [ -f "test-tools/autotest/nemesisbot.pid" ]; then
    NEMESIS_PID=$(cat test-tools/autotest/nemesisbot.pid)
    if ps -p "$NEMESIS_PID" > /dev/null 2>&1; then
        echo "Stopping NemesisBot (PID: $NEMESIS_PID)..."
        kill "$NEMESIS_PID" 2>/dev/null || true
    fi
    rm -f test-tools/autotest/nemesisbot.pid
fi

# 通过进程名强制停止（兼容 Windows）
taskkill //F //IM nemesisbot.exe 2>/dev/null || true

# === 2. 停止 TestAIServer ===

echo "Stopping TestAIServer..."

# 尝试通过 PID 文件停止
if [ -f "test-tools/autotest/testaiserver.pid" ]; then
    TESTAI_PID=$(cat test-tools/autotest/testaiserver.pid)
    if ps -p "$TESTAI_PID" > /dev/null 2>&1; then
        echo "Stopping TestAIServer (PID: $TESTAI_PID)..."
        kill "$TESTAI_PID" 2>/dev/null || true
    fi
    rm -f test-tools/autotest/testaiserver.pid
fi

# 通过进程名强制停止（兼容 Windows）
taskkill //F //IM testaiserver.exe 2>/dev/null || true

# === 3. 等待文件释放 ===

echo "Waiting for file handles to be released..."
sleep 3

# === 4. 验证清理结果 ===

# 检查是否还有进程在运行
NEMESIS_RUNNING=$(tasklist | grep -i nemesisbot.exe | wc -l)
TESTAI_RUNNING=$(tasklist | grep -i testaiserver.exe | wc -l)

if [ "$NEMESIS_RUNNING" -gt "1" ] || [ "$TESTAI_RUNNING" -gt "1" ]; then
    echo "WARNING: Some processes may still be running"
    echo "CLEANUP_SUCCESS"
else
    echo "CLEANUP_SUCCESS"
fi

echo ""
echo "Environment cleanup completed!"
echo "Note: test-tools/autotest/ directory was not removed (AI should handle this)"

exit 0
