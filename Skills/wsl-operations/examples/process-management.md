# 进程管理示例

本文档提供 WSL 中进程管理的实际使用示例。

## 目录

- [查看进程](#查看进程)
- [管理进程](#管理进程)
- [服务管理](#服务管理)
- [资源监控](#资源监控)
- [故障排除](#故障排除)
- [高级场景](#高级场景)

---

## 查看进程

### 基本进程查看

```bash
# 查看所有进程
wsl bash -lic "ps aux"
# 输出列: USER, PID, %CPU, %MEM, VSZ, RSS, TTY, STAT, START, TIME, COMMAND

# 查看前 20 个进程
wsl bash -lic "ps aux | head -20"

# 以树形结构查看进程
wsl bash -lic "pstree -p"
# 输出: systemd(1)───bash(1234)───sleep(5678)

# 查看完整进程列表（包括其他用户的）
wsl bash -lic "ps -ef"

# 查看当前用户的进程
wsl bash -lic "ps -u \$USER"

# 查看特定用户的进程
wsl bash -lic "ps -u root"
```

### 进程排序

```bash
# 按 CPU 使用率排序（降序）
wsl bash -lic "ps aux --sort=-%cpu | head -20"

# 按内存使用率排序（降序）
wsl bash -lic "ps aux --sort=-%mem | head -20"

# 按 PID 排序
wsl bash -lic "ps aux --sort=pid"

# 按进程运行时间排序
wsl bash -lic "ps aux --sort=-etime | head -20"
```

### 查找特定进程

```bash
# 按名称查找进程
wsl bash -lic "ps aux | grep nginx"
# 输出示例:
# root      1234  0.0  0.1  50000  2000 ?        Ss   10:00   0:00 nginx: master process

# 使用 pgrep 查找进程 PID
wsl bash -lic "pgrep nginx"
# 输出: 1234 5678

# 查看进程的完整命令行
wsl bash -lic "ps aux | grep python | grep -v grep"

# 查看进程详细信息
wsl bash -lic "ps -p 1234 -f"

# 查找占用端口的进程
wsl bash -lic "lsof -i :3000"
# 输出: COMMAND   PID USER   FD   TYPE  DEVICE SIZE/OFF NODE NAME
#       node    1234 user   20u  IPv4 1234567      0t0  TCP *:3000 (LISTEN)

# 查看特定用户的所有进程
wsl bash -lic "ps -U username -u username u"
```

### 实时监控

```bash
# 使用 top 实时监控
wsl bash -lic "top"

# 使用 htop（需要安装）
wsl bash -lic "apt install -y htop && htop"

# 在 top 中排序:
# - P: 按 CPU 排序
# - M: 按内存排序
# - T: 按时间排序
# - k: 杀死进程
# - q: 退出

# 使用 watch 持续查看
wsl bash -lic "watch -n 1 'ps aux | grep nginx'"

# 查看系统负载
wsl bash -lic "watch -n 1 'uptime'"
```

---

## 管理进程

### 结束进程

```bash
# 优雅地结束进程（SIGTERM）
wsl bash -lic "kill 1234"

# 强制结束进程（SIGKILL）
wsl bash -lic "kill -9 1234"

# 按名称结束进程
wsl bash -lic "pkill nginx"

# 结束所有同名进程
wsl bash -lic "killall nginx"

# 结束特定用户的所有进程
wsl bash -lic "pkill -u username"

# 发送特定信号
wsl bash -lic "kill -HUP 1234"        # 重新加载配置
wsl bash -lic "kill -TERM 1234"      # 终止进程（默认）
wsl bash -lic "kill -INT 1234"       # 中断进程
```

### 进程控制

```bash
# 暂停进程（SIGSTOP）
wsl bash -lic "kill -STOP 1234"

# 恢复进程（SIGCONT）
wsl bash -lic "kill -CONT 1234"

# 查看进程状态
wsl bash -lic "ps -o pid,stat,command -p 1234"

# 查看进程的线程
wsl bash -lic "ps -eLf | grep 1234"

# 查看进程打开的文件描述符
wsl bash -lic "ls -la /proc/1234/fd"
```

### 后台进程

```bash
# 在后台运行命令
wsl bash -lic "long-running-command &"

# 后台运行并忽略 SIGHUP 信号
wsl bash -lic "nohup long-running-command > output.log 2>&1 &"

# 查看后台任务
wsl bash -lic "jobs"
# 输出: [1]+  Running                 long-running-command &

# 将后台任务恢复到前台
wsl bash -lic "fg %1"

# 将后台任务恢复到后台继续运行
wsl bash -lic "bg %1"

# 查看最后一个后台任务的 PID
wsl bash -lic "echo \$!"

# 等待后台任务完成
wsl bash -lic "wait %1"
```

### 使用 screen 管理会话

```bash
# 安装 screen
wsl bash -lic "apt install -y screen"

# 创建新的 screen 会话
wsl bash -lic "screen -S mysession"

# 在 screen 中:
# - Ctrl+A D: 分离会话（detach）
# - Ctrl+A C: 创建新窗口
# - Ctrl+A N: 切换到下一个窗口
# - Ctrl+A P: 切换到上一个窗口
# - Ctrl+A K: 杀死当前窗口
# - Ctrl+A ?: 显示帮助

# 列出所有 screen 会话
wsl bash -lic "screen -ls"

# 重新连接到会话
wsl bash -lic "screen -r mysession"

# 强制连接到会话（如果有多人连接）
wsl bash -lic "screen -x mysession"

# 杀死会话
wsl bash -lic "screen -X -S mysession quit"
```

### 使用 tmux 管理会话

```bash
# 安装 tmux
wsl bash -lic "apt install -y tmux"

# 创建新的 tmux 会话
wsl bash -lic "tmux new -s mysession"

# 在 tmux 中:
# - Ctrl+B D: 分离会话
# - Ctrl+B C: 创建新窗口
# - Ctrl+B N: 切换到下一个窗口
# - Ctrl+B P: 切换到上一个窗口
# - Ctrl+B 0-9: 切换到指定窗口
# - Ctrl+B |: 垂直分割窗口
# - Ctrl+B %: 水平分割窗口
# - Ctrl+B ?: 显示帮助

# 列出所有 tmux 会话
wsl bash -lic "tmux ls"

# 重新连接到会话
wsl bash -lic "tmux attach -t mysession"

# 杀死会话
wsl bash -lic "tmux kill-session -t mysession"
```

---

## 服务管理

### 使用 systemctl

```bash
# 查看服务状态
wsl bash -lic "systemctl status nginx"
# 输出示例:
# ● nginx.service - A high performance web server
#    Loaded: loaded (/lib/systemd/system/nginx.service; enabled)
#    Active: active (running) since Wed 2026-03-26 10:00:00 CST

# 启动服务
wsl bash -lic "sudo systemctl start nginx"

# 停止服务
wsl bash -lic "sudo systemctl stop nginx"

# 重启服务
wsl bash -lic "sudo systemctl restart nginx"

# 重新加载服务配置（不中断连接）
wsl bash -lic "sudo systemctl reload nginx"

# 开机启用服务
wsl bash -lic "sudo systemctl enable nginx"

# 禁用服务（不开机启动）
wsl bash -lic "sudo systemctl disable nginx"

# 查看所有服务
wsl bash -lic "systemctl list-units --type=service"

# 查看所有失败的服务
wsl bash -lic "systemctl --failed"

# 查看服务是否开机启动
wsl bash -lic "systemctl is-enabled nginx"

# 查看服务是否活动
wsl bash -lic "systemctl is-active nginx"
```

### 使用 service 命令

```bash
# 查看服务状态
wsl bash -lic "sudo service nginx status"

# 启动服务
wsl bash -lic "sudo service nginx start"

# 停止服务
wsl bash -lic "sudo service nginx stop"

# 重启服务
wsl bash -lic "sudo service nginx restart"

# 查看所有服务状态
wsl bash -lic "sudo service --status-all"
```

### 查看服务日志

```bash
# 查看服务日志
wsl bash -lic "journalctl -u nginx"

# 实时查看日志
wsl bash -lic "journalctl -u nginx -f"

# 查看最近 50 行日志
wsl bash -lic "journalctl -u nginx -n 50"

# 查看指定时间范围的日志
wsl bash -lic "journalctl -u nginx --since '1 hour ago'"

# 查看传统应用日志
wsl bash -lic "tail -f /var/log/nginx/access.log"
```

### Docker 容器管理

```bash
# 列出运行中的容器
wsl bash -lic "docker ps"

# 列出所有容器（包括停止的）
wsl bash -lic "docker ps -a"

# 查看容器日志
wsl bash -lic "docker logs -f container-name"

# 查看容器资源使用
wsl bash -lic "docker stats"

# 停止容器
wsl bash -lic "docker stop container-name"

# 启动容器
wsl bash -lic "docker start container-name"

# 重启容器
wsl bash -lic "docker restart container-name"

# 进入运行中的容器
wsl bash -lic "docker exec -it container-name bash"

# 删除容器
wsl bash -lic "docker rm container-name"

# 删除所有停止的容器
wsl bash -lic "docker container prune"
```

---

## 资源监控

### CPU 监控

```bash
# 查看当前 CPU 使用率
wsl bash -lic "top -b -n 1 | head -20"

# 查看特定时间间隔的 CPU 使用
wsl bash -lic "sar -u 1 5"
# 需要安装: apt install -y sysstat

# 查看 CPU 核心数
wsl bash -lic "nproc"

# 查看每个 CPU 核心的使用率
wsl bash -lic "mpstat -P ALL"
```

### 内存监控

```bash
# 查看内存使用情况
wsl bash -lic "free -h"
# 输出: total, used, free, available

# 持续监控内存
wsl bash -lic "watch -n 1 free -h"

# 查看详细的内存信息
wsl bash -lic "cat /proc/meminfo"

# 查看特定进程的内存使用
wsl bash -lic "ps -p 1234 -o pid,vsz,rss,cmd"

# 按内存使用排序显示进程
wsl bash -lic "ps aux --sort=-%mem | head -20"
```

### 磁盘监控

```bash
# 查看磁盘使用
wsl bash -lic "df -h"

# 查看特定文件系统的使用情况
wsl bash -lic "df -h /home"

# 查看目录大小
wsl bash -lic "du -sh /path/to/directory"

# 查找最大的文件和目录
wsl bash -lic "du -h /path | sort -rh | head -20"

# 查看磁盘 I/O 统计
wsl bash -lic "iostat -x 1 5"
```

### 网络监控

```bash
# 查看网络连接
wsl bash -lic "netstat -tulpn"

# 查看特定端口
wsl bash -lic "netstat -tulpn | grep :3000"

# 使用 ss（现代替代工具）
wsl bash -lic "ss -tulpn"

# 查看网络接口流量
wsl bash -lic "iftop"
# 需要安装: apt install -y iftop

# 测试网络速度
wsl bash -lic "curl -s https://raw.githubusercontent.com/sivel/speedtest-cli/master/speedtest.py | python3"
```

---

## 故障排除

### 查找僵尸进程

```bash
# 查找僵尸进程
wsl bash -lic "ps aux | grep Z"

# 查找僵尸进程的父进程
wsl bash -lic "ps aux | awk '{print \$8}' | grep -c Z"

# 杀死僵尸进程的父进程
wsl bash -lic "ps -eo pid,ppid,state,comm | grep Z | awk '{print \$2}' | xargs -r kill -9"
```

### 查找占用 CPU 过高的进程

```bash
# 查找 CPU 使用率超过 50% 的进程
wsl bash -lic "ps aux --sort=-%cpu | awk 'NR>1 && \$3>50 {print}'"

# 使用 top 找到进程后查看详细信息
wsl bash -lic "top -b -n 1 | head -20"

# 使用 strace 跟踪进程
wsl bash -lic "strace -p 1234"
```

### 查找内存泄漏

```bash
# 查看进程内存使用变化
wsl bash -lic "watch -n 5 'ps -p 1234 -o pid,vsz,rss,cmd'"

# 使用 valgrind 检测内存泄漏
wsl bash -lic "valgrind --leak-check=full ./program"

# 使用 pmap 查看进程内存映射
wsl bash -lic "pmap 1234"
```

### 查找占用文件的进程

```bash
# 查找占用特定文件的进程
wsl bash -lic "lsof /path/to/file"

# 查找占用特定目录的进程
wsl bash -lic "lsof +D /path/to/directory"

# 查找占用特定端口的进程
wsl bash -lic "lsof -i :8080"

# 查找删除但还被占用的文件
wsl bash -lic "lsof +L1"
```

---

## 高级场景

### 批量管理进程

```bash
# 批量结束多个进程
wsl bash -lic "ps aux | grep 'old-process' | awk '{print \$2}' | xargs kill"

# 批量结束特定用户的进程
wsl bash -lic "pkill -u username"

# 批量修改进程优先级
wsl bash -lic "pgrep process-name | xargs -r renice +5"

# 按模式匹配结束进程
wsl bash -lic "pkill -f 'pattern'"
```

### 进程优先级

```bash
# 以低优先级启动进程
wsl bash -lic "nice -n 19 command"

# 以高优先级启动进程（需要 root）
wsl bash -lic "sudo nice -n -5 command"

# 修改运行中进程的优先级
wsl bash -lic "renice +5 -p 1234"

# 查看进程优先级
wsl bash -lic "ps -o pid,ni,command -p 1234"
```

### CPU 亲和性

```bash
# 将进程绑定到特定 CPU 核心
wsl bash -lic "taskset -p 0x1 1234"        # 绑定到 CPU 0
wsl bash -lic "taskset -p 0x3 1234"        # 绑定到 CPU 0 和 1

# 在特定 CPU 上启动进程
wsl bash -lic "taskset -c 0 command"

# 查看进程的 CPU 亲和性
wsl bash -lic "taskset -p 1234"
```

### 实时监控脚本

```bash
# 监控特定进程的资源使用
wsl bash -lic "while true; do clear; ps -p 1234 -o pid,%cpu,%mem,etime,cmd; sleep 2; done"

# 监控端口占用
wsl bash -lic "watch -n 1 'netstat -tulpn | grep :3000'"

# 监控系统资源并记录到文件
wsl bash -lic "while true; do date >> monitor.log; ps aux --sort=-%cpu | head -10 >> monitor.log; free -h >> monitor.log; sleep 60; done"
```

### 自动化脚本

```bash
# 自动重启崩溃的服务
#!/bin/bash
while true; do
    if ! systemctl is-active --quiet nginx; then
        systemctl restart nginx
        echo "Nginx restarted at \$(date)" >> /var/log/nginx-restart.log
    fi
    sleep 60
done

# 清理僵尸进程
wsl bash -lic "ps aux | awk '{print \$8}' | grep -c Z | xargs -I {} echo 'Found {} zombie processes'"

# 自动清理超过 24 小时的临时文件
wsl bash -lic "find /tmp -type f -mtime +1 -delete"
```

---

**文档版本**: 1.0.0
**最后更新**: 2026-03-26
