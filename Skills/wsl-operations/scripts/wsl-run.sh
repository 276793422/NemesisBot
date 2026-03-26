#!/usr/bin/env bash
# WSL 命令执行辅助脚本
# 用途: 简化 WSL 命令执行，提供错误处理和日志记录
# 使用方法: ./wsl-run.sh "<command>"

set -e  # 遇到错误立即退出

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 显示帮助信息
show_help() {
    echo "WSL 命令执行辅助脚本"
    echo ""
    echo "用法:"
    echo "  $0 \"<command>\"              - 在 WSL 中执行命令"
    echo "  $0 -v \"<command>\"           - 显示详细输出"
    echo "  $0 -l \"<command>\"           - 记录日志到文件"
    echo "  $0 -d <distro> \"<command>\"  - 指定发行版"
    echo "  $0 -h                        - 显示此帮助信息"
    echo ""
    echo "示例:"
    echo "  $0 \"ls -la /home\""
    echo "  $0 -v \"apt update && apt upgrade -y\""
    echo "  $0 -l \"python3 script.py\""
    echo "  $0 -d Ubuntu-22.04 \"node server.js\""
    echo ""
}

# 记录日志
log_message() {
    local log_file="$HOME/wsl-run.log"
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo "[$timestamp] $1" >> "$log_file"
}

# 执行 WSL 命令
execute_command() {
    local command="$1"
    local distro="$2"
    local verbose="$3"
    local log="$4"

    # 构建 WSL 命令
    local wsl_cmd="wsl"
    if [ -n "$distro" ]; then
        wsl_cmd="$wsl_cmd -d $distro"
    fi
    wsl_cmd="$wsl_cmd bash -lic \"$command\""

    # 显示信息
    echo -e "${BLUE}=== WSL 命令执行 ===${NC}"
    echo -e "${YELLOW}执行时间:${NC} $(date '+%Y-%m-%d %H:%M:%S')"
    if [ -n "$distro" ]; then
        echo -e "${YELLOW}发行版:${NC} $distro"
    fi
    echo -e "${YELLOW}命令:${NC} $command"
    echo -e "${BLUE}=====================${NC}"
    echo ""

    # 记录日志
    if [ "$log" = "true" ]; then
        log_message "执行命令: $command"
    fi

    # 执行命令
    local start_time=$(date +%s)
    if [ "$verbose" = "true" ]; then
        eval "$wsl_cmd"
    else
        eval "$wsl_cmd" 2>&1
    fi
    local exit_code=$?
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    # 显示结果
    echo ""
    if [ $exit_code -eq 0 ]; then
        echo -e "${GREEN}✓ 命令执行成功${NC}"
        echo -e "${YELLOW}耗时:${NC} ${duration}秒"
        if [ "$log" = "true" ]; then
            log_message "命令执行成功 (耗时: ${duration}秒)"
        fi
    else
        echo -e "${RED}✗ 命令执行失败 (退出码: $exit_code)${NC}"
        if [ "$log" = "true" ]; then
            log_message "命令执行失败 (退出码: $exit_code)"
        fi
    fi

    return $exit_code
}

# 解析参数
VERBOSE=false
LOG=false
DISTRO=""

while getopts ":hvltd:" opt; do
    case $opt in
        h)
            show_help
            exit 0
            ;;
        v)
            VERBOSE=true
            ;;
        l)
            LOG=true
            ;;
        d)
            DISTRO="$OPTARG"
            ;;
        \?)
            echo -e "${RED}无效选项: -$OPTARG${NC}" >&2
            show_help
            exit 1
            ;;
        :)
            echo -e "${RED}选项 -$OPTARG 需要参数${NC}" >&2
            exit 1
            ;;
    esac
done

shift $((OPTIND-1))

# 检查是否提供了命令
if [ -z "$1" ]; then
    echo -e "${RED}错误: 未提供命令${NC}" >&2
    echo ""
    show_help
    exit 1
fi

# 执行命令
execute_command "$1" "$DISTRO" "$VERBOSE" "$LOG"
exit $?
