#!/bin/bash
#
# cleanup-env.sh - Memory UAT 环境清理脚本
#
# 使用方法：
#   bash Skills/memory-uat/scripts/cleanup-env.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
WORKDIR="$PROJECT_ROOT/test-tools/memory-uat-workdir"

echo "========================================="
echo " Memory UAT 环境清理"
echo "========================================="

# === 1. 停止 TestAIServer ===

if [ -f "$WORKDIR/testaiserver.pid" ]; then
    PID=$(cat "$WORKDIR/testaiserver.pid")
    if kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null || true
        echo "TestAIServer (PID: $PID) stopped"
    else
        echo "TestAIServer (PID: $PID) already stopped"
    fi
    rm -f "$WORKDIR/testaiserver.pid"
fi

# === 2. 杀死占用端口的进程 ===

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

# === 3. 删除工作目录 ===

if [ -d "$WORKDIR" ]; then
    rm -rf "$WORKDIR"
    echo "Work directory removed: $WORKDIR"
else
    echo "Work directory not found (already cleaned)"
fi

echo ""
echo "========================================="
echo " 环境清理完成"
echo "========================================="
