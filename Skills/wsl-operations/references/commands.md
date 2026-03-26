# WSL 命令完整参考

本文档提供 WSL 命令的完整参考，涵盖所有常用操作和选项。

## 目录

- [命令执行基础](#命令执行基础)
- [系统命令](#系统命令)
- [文件操作](#文件操作)
- [软件包管理](#软件包管理)
- [开发工具](#开发工具)
- [系统监控](#系统监控)
- [进程管理](#进程管理)
- [服务管理](#服务管理)
- [网络操作](#网络操作)
- [用户和权限](#用户和权限)
- [压缩解压](#压缩解压)
- [WSL 管理](#wsl-管理)
- [高级操作](#高级操作)

---

## 命令执行基础

### 基本语法

```bash
# 标准格式（推荐）
wsl bash -lic "<command>"

# 各参数说明
bash    - 使用 bash shell
-l      - login shell，加载完整环境（~/.bash_profile, ~/.bashrc, /etc/profile）
-i      - interactive shell，交互式模式（可选）
-c      - 执行后面的命令
```

### 指定发行版

```bash
# 指定特定发行版
wsl -d Ubuntu-22.04 bash -lic "ls -la"

# 查看可用发行版
wsl --list

# 设置默认发行版
wsl --set-default Ubuntu-22.04
```

### 指定用户

```bash
# 以 root 用户执行
wsl -u root bash -lic "apt update"

# 以特定用户执行
wsl -u username bash -lic "whoami"
```

### 命令链

```bash
# 使用 && 链接命令
wsl bash -lic "cd /home && ls -la"

# 使用 ; 分隔命令
wsl bash -lic "pwd; ls -la; whoami"

# 使用 || 处理错误
wsl bash -lic "cd /nonexistent || echo 'Directory not found'"

# 使用管道
wsl bash -lic "ps aux | grep nginx"
```

---

## 系统命令

### 系统信息

```bash
# 系统详细信息
wsl bash -lic "uname -a"
# 输出示例: Linux hostname 5.10.16.3-microsoft-standard-WSL2 #1 SMP x86_64 GNU/Linux

# 系统版本
wsl bash -lic "uname -r"

# 硬件架构
wsl bash -lic "uname -m"

# 主机名
wsl bash -lic "hostname"

# 当前用户
wsl bash -lic "whoami"

# 用户 ID 和组信息
wsl bash -lic "id"

# 用户所属组
wsl bash -lic "groups"

# 系统运行时间
wsl bash -lic "uptime"

# 系统日期和时间
wsl bash -lic "date"

# 日历
wsl bash -lic "cal"
```

### 环境变量

```bash
# 查看所有环境变量
wsl bash -lic "env"

# 查看 PATH
wsl bash -lic "echo \$PATH"

# 查看 HOME
wsl bash -lic "echo \$HOME"

# 查看 USER
wsl bash -lic "echo \$USER"

# 设置临时环境变量
wsl bash -lic "export NODE_ENV=production && node app.js"

# 永久添加到 .bashrc
wsl bash -lic "echo 'export PATH=\$PATH:/new/path' >> ~/.bashrc"

# 重新加载 .bashrc
wsl bash -lic "source ~/.bashrc"
```

---

## 文件操作

### 基本文件操作

```bash
# 列出文件
wsl bash -lic "ls -la"

# 列出包括隐藏文件
wsl bash -lic "ls -A"

# 按时间排序
wsl bash -lic "ls -lt"

# 按大小排序
wsl bash -lic "ls -lS"

# 递归列出
wsl bash -lic "ls -R"

# 查看文件内容
wsl bash -lic "cat file.txt"

# 查看文件前几行
wsl bash -lic "head -n 20 file.txt"

# 查看文件后几行
wsl bash -lic "tail -n 20 file.txt"

# 实时查看日志
wsl bash -lic "tail -f logfile.log"

# 分页查看大文件
wsl bash -lic "less largefile.log"

# 查看文件类型
wsl bash -lic "file filename"

# 统计行数、字数、字符数
wsl bash -lic "wc -lwc file.txt"
```

### 目录操作

```bash
# 切换目录
wsl bash -lic "cd /path/to/dir"

# 切换到上级目录
wsl bash -lic "cd .."

# 切换到用户主目录
wsl bash -lic "cd ~"

# 切换到上一次所在目录
wsl bash -lic "cd -"

# 显示当前目录
wsl bash -lic "pwd"

# 创建目录
wsl bash -lic "mkdir /path/to/dir"

# 递归创建目录
wsl bash -lic "mkdir -p /path/to/nested/dir"

# 删除空目录
wsl bash -lic "rmdir /path/to/empty/dir"

# 删除目录及其内容
wsl bash -lic "rm -rf /path/to/dir"

# 复制目录
wsl bash -lic "cp -r /source/dir /dest/dir"

# 移动/重命名目录
wsl bash -lic "mv /old/path /new/path"
```

### 文件搜索

```bash
# 按名称查找文件
wsl bash -lic "find /path -name 'filename.txt'"

# 按模式查找
wsl bash -lic "find /path -name '*.log'"

# 按类型查找（f=文件, d=目录）
wsl bash -lic "find /path -type f"

# 按大小查找
wsl bash -lic "find /path -size +100M"

# 按修改时间查找
wsl bash -lic "find /path -mtime -7"  # 7天内修改的文件

# 查找后执行命令
wsl bash -lic "find /path -name '*.tmp' -delete"

# 在文件内容中搜索
wsl bash -lic "grep 'pattern' file.txt"

# 递归搜索
wsl bash -lic "grep -r 'pattern' /path/to/dir"

# 不区分大小写
wsl bash -lic "grep -i 'pattern' file.txt"

# 显示行号
wsl bash -lic "grep -n 'pattern' file.txt"

# 反向匹配
wsl bash -lic "grep -v 'pattern' file.txt"

# 使用正则表达式
wsl bash -lic "grep -E 'pattern1|pattern2' file.txt"
```

---

## 软件包管理

### Debian/Ubuntu (apt)

```bash
# 更新软件包列表
wsl bash -lic "apt update"

# 升级所有软件包
wsl bash -lic "apt upgrade -y"

# 完整升级（包括依赖关系变化）
wsl bash -lic "apt full-upgrade -y"

# 安装软件包
wsl bash -lic "apt install -y package-name"

# 安装多个软件包
wsl bash -lic "apt install -y git vim curl"

# 删除软件包（保留配置）
wsl bash -lic "apt remove package-name"

# 删除软件包和配置
wsl bash -lic "apt purge package-name"

# 删除不需要的依赖
wsl bash -lic "apt autoremove -y"

# 清理下载的存档文件
wsl bash -lic "apt clean"

# 搜索软件包
wsl bash -lic "apt search keyword"

# 查看软件包详情
wsl bash -lic "apt show package-name"

# 列出已安装的软件包
wsl bash -lic "apt list --installed"

# 查看软件包的依赖关系
wsl bash -lic "apt depends package-name"

# 添加软件源
wsl bash -lic "add-apt-repository ppa:repository"

# 更新所有软件包（包括发行版升级）
wsl bash -lic "apt dist-upgrade"
```

### Alpine Linux (apk)

```bash
# 更新软件包索引
wsl bash -lic "apk update"

# 升级所有软件包
wsl bash -lic "apk upgrade"

# 安装软件包
wsl bash -lic "apk add package-name"

# 删除软件包
wsl bash -lic "apk del package-name"

# 搜索软件包
wsl bash -lic "apk search keyword"

# 查看软件包信息
wsl bash -lic "apk info package-name"

# 列出已安装的软件包
wsl bash -lic "apk info"

# 删除不需要的软件包
wsl bash -lic "apk cache clean"
```

---

## 开发工具

### Git 操作

```bash
# 克隆仓库
wsl bash -lic "git clone https://github.com/user/repo.git"

# 克隆到指定目录
wsl bash -lic "git clone https://github.com/user/repo.git /path/to/dir"

# 查看状态
wsl bash -lic "git status"

# 拉取最新代码
wsl bash -lic "git pull"

# 拉取并合并
wsl bash -lic "git pull --rebase"

# 查看日志
wsl bash -lic "git log"

# 简洁日志
wsl bash -lic "git log --oneline -10"

# 查看分支
wsl bash -lic "git branch"

# 创建并切换分支
wsl bash -lic "git checkout -b feature-branch"

# 切换分支
wsl bash -lic "git checkout main"

# 添加所有更改
wsl bash -lic "git add ."

# 提交更改
wsl bash -lic "git commit -m 'Commit message'"

# 推送到远程
wsl bash -lic "git push"

# 推送到特定分支
wsl bash -lic "git push origin feature-branch"

# 查看远程仓库
wsl bash -lic "git remote -v"

# 查看差异
wsl bash -lic "git diff"

# 暂存更改
wsl bash -lic "git stash"

# 恢复暂存的更改
wsl bash -lic "git stash pop"
```

### Python 开发

```bash
# 检查 Python 版本
wsl bash -lic "python3 --version"

# 安装包
wsl bash -lic "pip3 install package-name"

# 安装 requirements.txt
wsl bash -lic "pip3 install -r requirements.txt"

# 列出已安装的包
wsl bash -lic "pip3 list"

# 查看包信息
wsl bash -lic "pip3 show package-name"

# 导出已安装的包
wsl bash -lic "pip3 freeze > requirements.txt"

# 创建虚拟环境
wsl bash -lic "python3 -m venv venv"

# 激活虚拟环境
wsl bash -lic "source venv/bin/activate"

# 退出虚拟环境
wsl bash -lic "deactivate"

# 运行 Python 脚本
wsl bash -lic "python3 script.py"

# 运行单元测试
wsl bash -lic "python3 -m pytest"

# 代码检查
wsl bash -lic "pylint script.py"

# 格式化代码
wsl bash -lic "black script.py"
```

### Node.js 开发

```bash
# 检查 Node.js 版本
wsl bash -lic "node --version"

# 检查 npm 版本
wsl bash -lic "npm --version"

# 初始化项目
wsl bash -lic "npm init -y"

# 安装依赖
wsl bash -lic "npm install"

# 安装特定包
wsl bash -lic "npm install package-name"

# 安装开发依赖
wsl bash -lic "npm install --save-dev package-name"

# 全局安装
wsl bash -lic "npm install -g package-name"

# 运行脚本
wsl bash -lic "npm run script-name"

# 运行测试
wsl bash -lic "npm test"

# 构建项目
wsl bash -lic "npm run build"

# 更新依赖
wsl bash -lic "npm update"

# 审计安全漏洞
wsl bash -lic "npm audit"

# 修复安全漏洞
wsl bash -lic "npm audit fix"

# 使用 yarn 替代
wsl bash -lic "yarn install"
wsl bash -lic "yarn add package-name"
wsl bash -lic "yarn build"
```

### Go 开发

```bash
# 检查 Go 版本
wsl bash -lic "go version"

# 初始化模块
wsl bash -lic "go mod init module-name"

# 下载依赖
wsl bash -lic "go mod download"

# 整理依赖
wsl bash -lic "go mod tidy"

# 构建项目
wsl bash -lic "go build"

# 运行项目
wsl bash -lic "go run main.go"

# 运行测试
wsl bash -lic "go test ./..."

# 运行测试并显示覆盖率
wsl bash -lic "go test -cover ./..."

# 格式化代码
wsl bash -lic "go fmt ./..."

# 交叉编译
wsl bash -lic "GOOS=windows GOARCH=amd64 go build"

# 安装可执行文件
wsl bash -lic "go install"
```

---

## 系统监控

### CPU 和进程

```bash
# 查看进程快照
wsl bash -lic "top -b -n 1"

# 交互式进程查看器
wsl bash -lic "htop"

# 查看特定用户的进程
wsl bash -lic "top -u username"

# 查看所有进程
wsl bash -lic "ps aux"

# 查看前 20 个进程
wsl bash -lic "ps aux | head -20"

# 按内存使用排序
wsl bash -lic "ps aux --sort=-%mem | head -20"

# 按 CPU 使用排序
wsl bash -lic "ps aux --sort=-%cpu | head -20"

# 查找特定进程
wsl bash -lic "ps aux | grep nginx"

# 使用 pgrep 查找进程
wsl bash -lic "pgrep -f nginx"

# 查看进程树
wsl bash -lic "pstree -p"

# 查看父进程和子进程关系
wsl bash -lic "ps axjf"
```

### 内存监控

```bash
# 内存使用情况（人类可读）
wsl bash -lic "free -h"

# 持续监控内存（每秒刷新）
wsl bash -lic "watch -n 1 free -h"

# 详细内存信息
wsl bash -lic "cat /proc/meminfo"

# 虚拟内存统计
wsl bash -lic "vmstat 1 5"

# 内存映射
wsl bash -lic "pmap 1234"  # PID 为 1234 的进程
```

### 磁盘监控

```bash
# 磁盘使用情况
wsl bash -lic "df -h"

# 查看特定文件系统
wsl bash -lic "df -h /mnt/c"

# inode 使用情况
wsl bash -lic "df -i"

# 目录大小
wsl bash -lic "du -sh /path/to/dir"

# 查找最大的目录
wsl bash -lic "du -sh /path/* | sort -hr | head -10"

# 查找最大的文件
wsl bash -lic "find /path -type f -exec du -h {} + | sort -rh | head -10"

# 磁盘 I/O 统计
wsl bash -lic "iostat -x 1 5"
```

### 网络监控

```bash
# 查看网络接口
wsl bash -lic "ip addr"

# 查看路由表
wsl bash -lic "ip route"

# 查看网络连接
wsl bash -lic "netstat -tulpn"

# 现代网络统计工具
wsl bash -lic "ss -tulpn"

# 查看特定端口
wsl bash -lic "netstat -tulpn | grep :8080"

# 网络延迟测试
wsl bash -lic "ping -c 4 google.com"

# 追踪路由
wsl bash -lic "traceroute google.com"

# DNS 查询
wsl bash -lic "nslookup google.com"

# 带宽测试
wsl bash -lic "curl -s https://raw.githubusercontent.com/sivel/speedtest-cli/master/speedtest.py | python3"

# 下载速度测试
wsl bash -lic "curl -o /dev/null http://speedtest.tele2.net/100MB.zip"
```

---

## 进程管理

### 查看进程

```bash
# 查看所有进程
wsl bash -lic "ps aux"

# 查看进程树
wsl bash -lic "pstree -p"

# 查看完整进程列表
wsl bash -lic "ps auxf"

# 查看特定用户的进程
wsl bash -lic "ps -u username"

# 按名称查找进程
wsl bash -lic "pgrep -af process-name"

# 查看占用端口的进程
wsl bash -lic "lsof -i :8080"

# 查看进程打开的文件
wsl bash -lic "lsof -p 1234"

# 查看进程详细信息
wsl bash -lic "ps aux | grep 1234"
```

### 控制进程

```bash
# 优雅地结束进程（SIGTERM）
wsl bash -lic "kill 1234"

# 强制结束进程（SIGKILL）
wsl bash -lic "kill -9 1234"

# 按名称结束进程
wsl bash -lic "pkill -f process-name"

# 结束所有同名进程
wsl bash -lic "killall process-name"

# 暂停进程（SIGSTOP）
wsl bash -lic "kill -STOP 1234"

# 恢复进程（SIGCONT）
wsl bash -lic "kill -CONT 1234"

# 发送特定信号
wsl bash -lic "kill -HUP 1234"  # 重新加载配置

# 等待进程结束
wsl bash -lic "wait 1234"
```

### 后台进程

```bash
# 后台运行命令
wsl bash -lic "command &"

# 后台运行并忽略 SIGHUP
wsl bash -lic "nohup command > output.log 2>&1 &"

# 查看后台任务
wsl bash -lic "jobs"

# 恢复后台任务到前台
wsl bash -lic "fg %1"

# 恢复后台任务到后台继续运行
wsl bash -lic "bg %1"

# 忽略 SIGHUP 信号
wsl bash -lic "disown -h %1"

# 使用 screen
wsl bash -lic "screen -S session-name"
wsl bash -lic "screen -r session-name"

# 使用 tmux
wsl bash -lic "tmux new -s session-name"
wsl bash -lic "tmux attach -t session-name"
```

---

## 服务管理

### systemd (systemctl)

```bash
# 查看服务状态
wsl bash -lic "systemctl status service-name"

# 启动服务
wsl bash -lic "systemctl start service-name"

# 停止服务
wsl bash -lic "systemctl stop service-name"

# 重启服务
wsl bash -lic "systemctl restart service-name"

# 重新加载服务配置
wsl bash -lic "systemctl reload service-name"

# 开机启用服务
wsl bash -lic "systemctl enable service-name"

# 禁用服务
wsl bash -lic "systemctl disable service-name"

# 查看所有服务
wsl bash -lic "systemctl list-units --type=service"

# 查看失败的服助
wsl bash -lic "systemctl --failed"

# 查看服务日志
wsl bash -lic "journalctl -u service-name"

# 实时查看服务日志
wsl bash -lic "journalctl -u service-name -f"

# 查看最近 50 行日志
wsl bash -lic "journalctl -u service-name -n 50"
```

### 传统 init.d

```bash
# 查看服务状态
wsl bash -lic "service service-name status"

# 启动服务
wsl bash -lic "service service-name start"

# 停止服务
wsl bash -lic "service service-name stop"

# 重启服务
wsl bash -lic "service service-name restart"

# 查看所有服务
wsl bash -lic "service --status-all"
```

### Docker 容器

```bash
# 列出运行中的容器
wsl bash -lic "docker ps"

# 列出所有容器
wsl bash -lic "docker ps -a"

# 列出镜像
wsl bash -lic "docker images"

# 运行容器
wsl bash -lic "docker run -d -p 8080:80 image-name"

# 停止容器
wsl bash -lic "docker stop container-id"

# 启动容器
wsl bash -lic "docker start container-id"

# 删除容器
wsl bash -lic "docker rm container-id"

# 删除镜像
wsl bash -lic "docker rmi image-id"

# 进入容器
wsl bash -lic "docker exec -it container-id bash"

# 查看容器日志
wsl bash -lic "docker logs -f container-id"

# 使用 docker-compose
wsl bash -lic "docker-compose up -d"
wsl bash -lic "docker-compose down"
wsl bash -lic "docker-compose logs -f"
```

---

## 网络操作

### SSH 连接

```bash
# SSH 登录
wsl bash -lic "ssh user@192.168.1.100"

# 指定端口登录
wsl bash -lic "ssh -p 2222 user@host"

# 使用密钥登录
wsl bash -lic "ssh -i ~/.ssh/key.pem user@host"

# 生成 SSH 密钥对
wsl bash -lic "ssh-keygen -t rsa -b 4096"

# 复制公钥到服务器
wsl bash -lic "ssh-copy-id user@host"

# SSH 隧道（本地端口转发）
wsl bash -lic "ssh -L 8080:localhost:80 user@host"

# SSH 隧道（远程端口转发）
wsl bash -lic "ssh -R 8080:localhost:80 user@host"

# SOCKS 代理
wsl bash -lic "ssh -D 1080 user@host"
```

### 文件传输

```bash
# SCP 上传文件
wsl bash -lic "scp local-file.txt user@host:/remote/path/"

# SCP 下载文件
wsl bash -lic "scp user@host:/remote/file.txt /local/path/"

# SCP 递归复制目录
wsl bash -lic "scp -r /local/dir user@host:/remote/path/"

# rsync 同步
wsl bash -lic "rsync -avz /local/dir/ user@host:/remote/dir/"

# rsync 删除目标中源没有的文件
wsl bash -lic "rsync -avz --delete /local/dir/ user@host:/remote/dir/"

# 限制传输速度
wsl bash -lic "rsync -avz --bwlimit=1000 /local/dir/ user@host:/remote/dir/"
```

### HTTP/HTTPS

```bash
# 下载文件
wsl bash -lic "wget https://example.com/file.zip"

# 断点续传
wsl bash -lic "wget -c https://example.com/file.zip"

# 使用 curl 下载
wsl bash -lic "curl -O https://example.com/file.zip"

# 查看响应头
wsl bash -lic "curl -I https://www.google.com"

# POST 请求
wsl bash -lic "curl -X POST -d 'data' https://api.example.com/endpoint"

# JSON POST
wsl bash -lic "curl -X POST -H 'Content-Type: application/json' -d '{\"key\":\"value\"}' https://api.example.com/endpoint"

# 上传文件
wsl bash -lic "curl -F 'file=@filename' https://upload.example.com"

# 带认证的请求
wsl bash -lic "curl -u username:password https://api.example.com"
```

---

## 用户和权限

### 用户管理

```bash
# 查看当前用户
wsl bash -lic "whoami"

# 查看用户 ID
wsl bash -lic "id"

# 查看所有用户
wsl bash -lic "cat /etc/passwd"

# 添加用户
wsl bash -lic "sudo adduser username"

# 删除用户
wsl bash -lic "sudo deluser username"

# 修改用户密码
wsl bash -lic "passwd"

# 将用户添加到 sudo 组
wsl bash -lic "sudo usermod -aG sudo username"

# 查看用户组
wsl bash -lic "groups"

# 查看所有组
wsl bash -lic "cat /etc/group"

# 切换到 root 用户
wsl bash -lic "sudo -i"

# 以其他用户身份执行命令
wsl bash -lic "sudo -u username command"
```

### 权限管理

```bash
# 查看文件权限
wsl bash -lic "ls -l file.txt"

# 修改文件权限（数字方式）
wsl bash -lic "chmod 755 file.txt"

# 添加执行权限
wsl bash -lic "chmod +x script.sh"

# 递归修改权限
wsl bash -lic "chmod -R 755 /path/to/dir"

# 修改所有者
wsl bash -lic "sudo chown user:group file.txt"

# 递归修改所有者
wsl bash -lic "sudo chown -R user:group /path/to/dir"

# 修改组
wsl bash -lic "sudo chgrp group file.txt"

# 设置 setuid
wsl bash -lic "chmod u+s file.txt"

# 设置 setgid
wsl bash -lic "chmod g+s directory"

# 设置 sticky bit
wsl bash -lic "chmod +t directory"

# 查看默认权限
wsl bash -lic "umask"

# 设置默认权限
wsl bash -lic "umask 022"
```

---

## 压缩解压

### tar 操作

```bash
# 创建 tar.gz 压缩文件
wsl bash -lic "tar -czf archive.tar.gz /path/to/dir"

# 解压 tar.gz
wsl bash -lic "tar -xzf archive.tar.gz"

# 解压到指定目录
wsl bash -lic "tar -xzf archive.tar.gz -C /path/to/dest"

# 查看 tar.gz 内容
wsl bash -lic "tar -tzf archive.tar.gz"

# 创建 tar.bz2 压缩文件
wsl bash -lic "tar -cjf archive.tar.bz2 /path/to/dir"

# 解压 tar.bz2
wsl bash -lic "tar -xjf archive.tar.bz2"

# 创建 tar.xz 压缩文件
wsl bash -lic "tar -cJf archive.tar.xz /path/to/dir"

# 解压 tar.xz
wsl bash -lic "tar -xJf archive.tar.xz"

# 排除文件
wsl bash -lic "tar -czf archive.tar.gz --exclude='*.log' /path/to/dir"

# 显示进度
wsl bash -lic "tar -czvf archive.tar.gz /path/to/dir"
```

### zip 操作

```bash
# 创建 zip 压缩文件
wsl bash -lic "zip -r archive.zip /path/to/dir"

# 解压 zip
wsl bash -lic "unzip archive.zip"

# 解压到指定目录
wsl bash -lic "unzip archive.zip -d /path/to/dest"

# 查看 zip 内容
wsl bash -lic "unzip -l archive.zip"

# 不覆盖解压
wsl bash -lic "unzip -n archive.zip"

# 覆盖解压
wsl bash -lic "unzip -o archive.zip"

# 设置压缩级别
wsl bash -lic "zip -r -9 archive.zip /path/to/dir"
```

### 其他压缩格式

```bash
# gzip 压缩单个文件
wsl bash -lic "gzip file.txt"

# gzip 解压
wsl bash -lic "gzip -d file.txt.gz"

# bzip2 压缩
wsl bash -lic "bzip2 file.txt"

# bzip2 解压
wsl bash -lic "bzip2 -d file.txt.bz2"

# xz 压缩
wsl bash -lic "xz file.txt"

# xz 解压
wsl bash -lic "xz -d file.txt.xz"

# 7z 压缩（需要安装 p7zip）
wsl bash -lic "7z a archive.7z /path/to/dir"

# 7z 解压
wsl bash -lic "7z x archive.7z"
```

---

## WSL 管理

### 查看状态

```bash
# 列出发行版
wsl --list

# 详细列出发行版
wsl --list --verbose

# 查看 WSL 状态
wsl --status

# 查看 WSL 版本
wsl --version

# 查看在线发行版
wsl --list --online
```

### 发行版管理

```bash
# 设置默认发行版
wsl --set-default Ubuntu-22.04

# 设置默认 WSL 版本
wsl --set-default-version 2

# 终止特定发行版
wsl --terminate Ubuntu-22.04

# 注销发行版（删除）
wsl --unregister Ubuntu-22.04

# 启动特定发行版
wsl -d Ubuntu-22.04

# 以特定用户启动
wsl -d Ubuntu-22.04 -u root
```

### 系统操作

```bash
# 关闭所有 WSL 实例
wsl --shutdown

# 更新 WSL
wsl --update

# 查看更新日志
wsl --update --help

# 导出发行版
wsl --export Ubuntu-22.04 D:\backup\ubuntu.tar

# 导入发行版
wsl --import Ubuntu-22.04 D:\WSL\Ubuntu D:\backup\ubuntu.tar
```

---

## 高级操作

### 性能分析

```bash
# 测量命令执行时间
wsl bash -lic "time command"

# 详细时间统计
wsl bash -lic "time -v command"

# CPU 性能分析
wsl bash -lic "perf stat command"

# strace 跟踪系统调用
wsl bash -lic "strace command"

# 跟踪运行中的进程
wsl bash -lic "strace -p 1234"

# ltrace 跟踪库调用
wsl bash -lic "ltrace command"

# Valgrind 内存检查
wsl bash -lic "valgrind --leak-check=full ./program"
```

### 系统调试

```bash
# 查看内核日志
wsl bash -lic "dmesg"

# 查看最近的内核消息
wsl bash -lic "dmesg | tail"

# 查看系统日志
wsl bash -lic "journalctl"

# 查看最近的日志
wsl bash -lic "journalctl -e"

# 查看特定时间的日志
wsl bash -lic "journalctl --since '1 hour ago'"

# 查看系统引导日志
wsl bash -lic "journalctl -b"

# 查看应用日志
wsl bash -lic "tail -f /var/log/app.log"

# 查看认证日志
wsl bash -lic "tail -f /var/log/auth.log"
```

### 批量操作

```bash
# 批量重命名
wsl bash -lic "rename 's/old/new/' *.txt"

# 批量查找和替换
wsl bash -lic "find . -type f -name '*.py' -exec sed -i 's/old/new/g' {} +"

# 批量修改权限
wsl bash -lic "find . -type f -name '*.sh' -exec chmod +x {} +"

# 批量删除
wsl bash -lic "find . -type f -name '*.tmp' -delete"

# 批量编译
wsl bash -lic "for dir in */; do cd \"$dir\" && ./build.sh && cd ..; done"

# 并行执行
wsl bash -lic "parallel command ::: arg1 arg2 arg3"

# 监控文件变化
wsl bash -lic "inotifywait -m /path/to/dir"

# 定时执行
wsl bash -lic "watch -n 5 'ps aux | grep nginx'"
```

---

## 快捷参考

### 常用组合

```bash
# 快速更新系统
wsl bash -lic "apt update && apt upgrade -y"

# 快速安装开发工具
wsl bash -lic "apt install -y git vim curl wget build-essential"

# 快速查看系统资源
wsl bash -lic "echo '=== CPU ===' && top -b -n 1 | head -20 && echo '=== Memory ===' && free -h && echo '=== Disk ===' && df -h"

# 快速清理系统
wsl bash -lic "apt autoremove -y && apt clean && rm -rf /tmp/*"

# 快速查找大文件
wsl bash -lic "find . -type f -size +100M -exec ls -lh {} +"
```

---

**文档版本**: 1.0.0
**最后更新**: 2026-03-26
**适用平台**: Windows 10/11 with WSL
