#!/usr/bin/env bash
# WSL 进程查看辅助脚本
# 用途: 格式化显示 WSL 进程信息，支持过滤和排序
# 使用方法: ./wsl-ps.sh [options] [pattern]

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# 显示帮助信息
show_help() {
    echo "WSL 进程查看辅助脚本"
    echo ""
    echo "用法:"
    echo "  $0 [options] [pattern]"
    echo ""
    echo "选项:"
    echo "  -c <number>    - 显示前 N 个进程 (默认: 20)"
    echo "  -s <field>     - 按字段排序 (pid, cpu, mem, time)"
    echo "  -u <user>      - 只显示特定用户的进程"
    echo "  -k             - 杀死匹配的进程"
    echo "  -d <distro>    - 指定 WSL 发行版"
    echo "  -t             - 显示进程树"
    echo "  -h             - 显示此帮助信息"
    echo ""
    echo "排序字段:"
    echo "  pid            - 按 PID 排序"
    echo "  cpu            - 按 CPU 使用率排序 (默认)"
    echo "  mem            - 按内存使用率排序"
    echo "  time           - 按运行时间排序"
    echo ""
    echo "示例:"
    echo "  $0                          - 显示 CPU 使用最高的 20 个进程"
    echo "  $0 -c 10                    - 显示前 10 个进程"
    echo "  $0 -s mem                  - 按内存使用排序"
    echo "  $0 nginx                    - 查找 nginx 相关进程"
    echo "  $0 -u username              - 显示特定用户的进程"
    echo "  $0 -k python                - 杀死所有 python 进程"
    echo "  $0 -t                       - 显示进程树"
    echo ""
}

# 构建排序参数
build_sort() {
    local sort_field="$1"
    case "$sort_field" in
        pid)
            echo "--sort=pid"
            ;;
        cpu)
            echo "--sort=-%cpu"
            ;;
        mem)
            echo "--sort=-%mem"
            ;;
        time)
            echo "--sort=-etime"
            ;;
        *)
            echo "--sort=-%cpu"
            ;;
    esac
}

# 显示进程信息
show_processes() {
    local pattern="$1"
    local count="$2"
    local sort="$3"
    local user="$4"
    local distro="$5"

    # 构建 WSL 命令
    local wsl_cmd="wsl"
    if [ -n "$distro" ]; then
        wsl_cmd="$wsl_cmd -d $distro"
    fi

    # 构建 ps 命令
    local ps_cmd="ps aux"
    if [ -n "$sort" ]; then
        ps_cmd="$ps_cmd $sort"
    fi
    if [ -n "$user" ]; then
        ps_cmd="$ps_cmd -u $user"
    fi

    # 执行命令
    if [ -n "$pattern" ]; then
        echo -e "${CYAN}=== 搜索包含 '${pattern}' 的进程 ===${NC}"
        echo ""
        $wsl_cmd bash -lic "$ps_cmd | grep -E '$pattern|USER|PID'"
    else
        echo -e "${CYAN}=== 进程列表 ===${NC}"
        echo ""
        $wsl_cmd bash -lic "$ps_cmd | head -n $((count + 1))"
    fi
}

# 显示进程树
show_tree() {
    local distro="$1"

    echo -e "${CYAN}=== 进程树 ===${NC}"
    echo ""

    local wsl_cmd="wsl"
    if [ -n "$distro" ]; then
        wsl_cmd="$wsl_cmd -d $distro"
    fi

    $wsl_cmd bash -lic "pstree -p"
}

# 杀死进程
kill_processes() {
    local pattern="$1"
    local distro="$2"

    echo -e "${YELLOW}警告: 即将杀死包含 '${pattern}' 的进程${NC}"
    echo -n "确认继续? [y/N] "
    read -r confirm

    if [ "$confirm" != "y" ] && [ "$confirm" != "Y" ]; then
        echo -e "${BLUE}操作已取消${NC}"
        return 0
    fi

    local wsl_cmd="wsl"
    if [ -n "$distro" ]; then
        wsl_cmd="$wsl_cmd -d $distro"
    fi

    echo ""
    echo -e "${CYAN}正在查找进程...${NC}"

    # 获取 PID 列表
    local pids=$($wsl_cmd bash -lic "pgrep -f '$pattern'" || true)

    if [ -z "$pids" ]; then
        echo -e "${YELLOW}未找到匹配的进程${NC}"
        return 1
    fi

    echo -e "${GREEN}找到以下进程:${NC}"
    $wsl_cmd bash -lic "ps -p $pids -o pid,user,comm"

    echo ""
    echo -e "${CYAN}正在终止进程...${NC}"
    $wsl_cmd bash -lic "pkill -f '$pattern'"

    if [ $? -eq 0 ]; then
        echo -e "${GREEN}✓ 进程已终止${NC}"
    else
        echo -e "${RED}✗ 终止进程失败${NC}"
        return 1
    fi
}

# 显示进程详情
show_details() {
    local pid="$1"
    local distro="$2"

    local wsl_cmd="wsl"
    if [ -n "$distro" ]; then
        wsl_cmd="$wsl_cmd -d $distro"
    fi

    echo -e "${CYAN}=== 进程详情 (PID: $pid) ===${NC}"
    echo ""

    # 基本信息
    echo -e "${YELLOW}基本信息:${NC}"
    $wsl_cmd bash -lic "ps -p $pid -o pid,user,%cpu,%mem,vsz,rss,etime,comm"

    echo ""

    # 完整命令行
    echo -e "${YELLOW}完整命令行:${NC}"
    $wsl_cmd bash -lic "cat /proc/$pid/cmdline | tr '\\0' ' '"
    echo ""

    # 打开的文件
    echo -e "${YELLOW}打开的文件:${NC}"
    $wsl_cmd bash -lic "lsof -p $pid 2>/dev/null | head -20 || echo '无法获取文件信息'"
}

# 解析参数
COUNT=20
SORT=""
USER=""
DISTRO=""
KILL=false
TREE=false

while getopts ":c:s:u:d:kth" opt; do
    case $opt in
        c)
            COUNT="$OPTARG"
            ;;
        s)
            SORT=$(build_sort "$OPTARG")
            ;;
        u)
            USER="$OPTARG"
            ;;
        d)
            DISTRO="$OPTARG"
            ;;
        k)
            KILL=true
            ;;
        t)
            TREE=true
            ;;
        h)
            show_help
            exit 0
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

# 检查是否安装了 WSL
if ! command -v wsl &> /dev/null; then
    echo -e "${RED}错误: 未找到 WSL 命令${NC}" >&2
    exit 1
fi

# 执行操作
if [ "$TREE" = true ]; then
    show_tree "$DISTRO"
elif [ "$KILL" = true ]; then
    if [ -z "$1" ]; then
        echo -e "${RED}错误: -k 选项需要提供搜索模式${NC}" >&2
        exit 1
    fi
    kill_processes "$1" "$DISTRO"
else
    # 检查参数是否是数字（PID）
    if [[ "$1" =~ ^[0-9]+$ ]]; then
        show_details "$1" "$DISTRO"
    else
        show_processes "$1" "$COUNT" "$SORT" "$USER" "$DISTRO"
    fi
fi

exit 0
