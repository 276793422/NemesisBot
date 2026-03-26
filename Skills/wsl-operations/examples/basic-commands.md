# 基本命令示例

本文档提供 WSL 基本命令的实际使用示例。

## 目录

- [系统操作](#系统操作)
- [文件管理](#文件管理)
- [文本处理](#文本处理)
- [软件包管理](#软件包管理)
- [开发环境配置](#开发环境配置)
- [日常任务](#日常任务)

---

## 系统操作

### 查看系统信息

```bash
# 查看完整的系统信息
wsl bash -lic "uname -a"
# 输出: Linux hostname 5.10.16.3-microsoft-standard-WSL2 #1 SMP x86_64 GNU/Linux

# 查看系统运行时间
wsl bash -lic "uptime"
# 输出: 10:30:45 up 2 days, 3:15, 0 users, load average: 0.15, 0.05, 0.01

# 查看当前用户
wsl bash -lic "whoami"
# 输出: username

# 查看用户 ID 和所属组
wsl bash -lic "id"
# 输出: uid=1000(username) gid=1000(username) groups=1000(username),27(sudo)
```

### 系统更新

```bash
# 更新软件包列表
wsl bash -lic "apt update"

# 升级所有已安装的软件包
wsl bash -lic "apt upgrade -y"

# 完整系统更新（包括发行版升级）
wsl bash -lic "apt full-upgrade -y"

# 清理不需要的包和缓存
wsl bash -lic "apt autoremove -y && apt clean"
```

### 时间和日期

```bash
# 查看当前日期和时间
wsl bash -lic "date"
# 输出: Wed Mar 26 10:30:45 CST 2026

# 查看日历（当前月份）
wsl bash -lic "cal"

# 查看特定年月的日历
wsl bash -lic "cal 3 2026"

# 查看系统时区
wsl bash -lic "timedatectl"
```

---

## 文件管理

### 目录导航

```bash
# 显示当前工作目录
wsl bash -lic "pwd"
# 输出: /home/username

# 切换到用户主目录
wsl bash -lic "cd ~"

# 切换到上级目录
wsl bash -lic "cd .."

# 切换到上一次所在的目录
wsl bash -lic "cd -"

# 切换到指定目录
wsl bash -lic "cd /var/log"
```

### 查看文件

```bash
# 列出当前目录的所有文件（详细信息）
wsl bash -lic "ls -la"

# 按修改时间排序（最新的在前）
wsl bash -lic "ls -lt"

# 按文件大小排序（最大的在前）
wsl bash -lic "ls -lS"

# 递归列出所有子目录
wsl bash -lic "ls -R"

# 查看文件内容
wsl bash -lic "cat /etc/hostname"

# 分页查看大文件
wsl bash -lic "less /var/log/syslog"

# 查看文件前 20 行
wsl bash -lic "head -n 20 /var/log/syslog"

# 查看文件后 20 行
wsl bash -lic "tail -n 20 /var/log/syslog"

# 实时查看日志文件
wsl bash -lic "tail -f /var/log/syslog"
```

### 创建和删除目录

```bash
# 创建单个目录
wsl bash -lic "mkdir /tmp/test"

# 递归创建多级目录
wsl bash -lic "mkdir -p /tmp/test/nested/dir"

# 删除空目录
wsl bash -lic "rmdir /tmp/test/nested/dir"

# 删除目录及其内容（小心使用！）
wsl bash -lic "rm -rf /tmp/test"
```

### 复制和移动文件

```bash
# 复制文件
wsl bash -lic "cp /etc/hostname /tmp/hostname.copy"

# 递归复制目录
wsl bash -lic "cp -r /var/log /tmp/logs"

# 保留文件属性复制
wsl bash -lic "cp -p /etc/hostname /tmp/hostname.copy"

# 移动/重命名文件
wsl bash -lic "mv /tmp/hostname.copy /tmp/hostname.old"

# 移动目录
wsl bash -lic "mv /tmp/logs /tmp/old_logs"
```

### 文件搜索

```bash
# 在当前目录查找名为 config.yml 的文件
wsl bash -lic "find . -name 'config.yml'"

# 查找所有 .log 文件
wsl bash -lic "find /var/log -name '*.log'"

# 查找大于 100MB 的文件
wsl bash -lic "find /tmp -size +100M"

# 查找 7 天内修改过的文件
wsl bash -lic "find /home/username -mtime -7"

# 在文件内容中搜索关键字
wsl bash -lic "grep 'error' /var/log/syslog"

# 递归搜索目录中所有文件的内容
wsl bash -lic "grep -r 'TODO' ~/projects/"

# 显示包含关键字的行号
wsl bash -lic "grep -n 'function' script.py"
```

---

## 文本处理

### 查看和比较

```bash
# 查看文件类型
wsl bash -lic "file document.pdf"
# 输出: PDF document, version 1.4

# 统计文件的行数、字数、字节数
wsl bash -lic "wc file.txt"
# 输出: 42 168 1024 file.txt (42 行, 168 字, 1024 字节)

# 只统计行数
wsl bash -lic "wc -l file.txt"

# 比较两个文件的不同
wsl bash -lic "diff file1.txt file2.txt"

# 并排比较
wsl bash -lic "diff -y file1.txt file2.txt"
```

### 文本排序和去重

```bash
# 对文件内容排序
wsl bash -lic "sort names.txt"

# 去除重复行
wsl bash -lic "uniq names.txt"

# 排序并去重
wsl bash -lic "sort names.txt | uniq"

# 统计重复行出现的次数
wsl bash -lic "sort names.txt | uniq -c"
```

### 文本转换

```bash
# 转换为小写
wsl bash -lic "tr 'A-Z' 'a-z' < input.txt"

# 转换为大写
wsl bash -lic "tr 'a-z' 'A-Z' < input.txt"

# 删除所有数字
wsl bash -lic "tr -d '0-9' < input.txt"

# 替换字符
wsl bash -lic "tr ' ' '_' < input.txt"
```

---

## 软件包管理

### 安装软件

```bash
# 搜索软件包
wsl bash -lic "apt search python"

# 查看软件包信息
wsl bash -lic "apt show python3"

# 安装单个软件包
wsl bash -lic "apt install -y git"

# 安装多个软件包
wsl bash -lic "apt install -y git vim curl wget"

# 安装特定版本的软件包
wsl bash -lic "apt install -y python3.10"
```

### 管理已安装的软件

```bash
# 列出所有已安装的软件包
wsl bash -lic "apt list --installed"

# 搜索已安装的软件包
wsl bash -lic "apt list --installed | grep python"

# 删除软件包（保留配置）
wsl bash -lic "apt remove python3"

# 删除软件包和配置文件
wsl bash -lic "apt purge python3"

# 删除不需要的依赖
wsl bash -lic "apt autoremove -y"
```

### 软件源管理

```bash
# 查看当前软件源
wsl bash -lic "cat /etc/apt/sources.list"

# 添加 PPA 源
wsl bash -lic "add-apt-repository ppa:deadsnakes/ppa"

# 更新软件包列表
wsl bash -lic "apt update"

# 备份当前软件源配置
wsl bash -lic "sudo cp /etc/apt/sources.list /etc/apt/sources.list.bak"
```

---

## 开发环境配置

### Git 配置

```bash
# 安装 Git
wsl bash -lic "apt install -y git"

# 配置用户名和邮箱
wsl bash -lic "git config --global user.name 'Your Name'"
wsl bash -lic "git config --global user.email 'your.email@example.com'"

# 配置默认分支名
wsl bash -lic "git config --global init.defaultBranch main"

# 配置凭证缓存
wsl bash -lic "git config --global credential.helper cache"

# 查看配置
wsl bash -lic "git config --list"
```

### Python 开发环境

```bash
# 安装 Python 和 pip
wsl bash -lic "apt install -y python3 python3-pip"

# 安装虚拟环境工具
wsl bash -lic "apt install -y python3-venv"

# 创建项目目录
wsl bash -lic "mkdir -p ~/projects/myproject && cd ~/projects/myproject"

# 创建虚拟环境
wsl bash -lic "python3 -m venv venv"

# 激活虚拟环境
wsl bash -lic "source venv/bin/activate"

# 安装常用包
wsl bash -lic "pip install pytest black pylint"

# 退出虚拟环境
wsl bash -lic "deactivate"
```

### Node.js 开发环境

```bash
# 使用 NodeSource 仓库安装最新 Node.js
wsl bash -lic "curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -"
wsl bash -lic "apt install -y nodejs"

# 验证安装
wsl bash -lic "node --version && npm --version"

# 配置 npm 使用国内镜像
wsl bash -lic "npm config set registry https://registry.npmmirror.com"

# 初始化项目
wsl bash -lic "mkdir -p ~/projects/myproject && cd ~/projects/myproject"
wsl bash -lic "npm init -y"

# 安装常用包
wsl bash -lic "npm install --save-dev eslint prettier"
```

### Go 开发环境

```bash
# 下载并安装 Go
wsl bash -lic "wget https://go.dev/dl/go1.21.0.linux-amd64.tar.gz"
wsl bash -lic "sudo tar -C /usr/local -xzf go1.21.0.linux-amd64.tar.gz"

# 配置环境变量
wsl bash -lic "echo 'export PATH=\$PATH:/usr/local/go/bin' >> ~/.bashrc"
wsl bash -lic "echo 'export GOPATH=\$HOME/go' >> ~/.bashrc"
wsl bash -lic "source ~/.bashrc"

# 验证安装
wsl bash -lic "go version"

# 创建项目
wsl bash -lic "mkdir -p ~/projects/myproject && cd ~/projects/myproject"
wsl bash -lic "go mod init myproject"
```

### Docker 安装

```bash
# 更新软件包索引
wsl bash -lic "apt update"

# 安装依赖
wsl bash -lic "apt install -y ca-certificates curl gnupg lsb-release"

# 添加 Docker 官方 GPG 密钥
wsl bash -lic "sudo mkdir -p /etc/apt/keyrings"
wsl bash -lic "curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg"

# 设置 Docker 仓库
wsl bash -lic "echo 'deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu $(lsb_release -cs) stable' | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null"

# 安装 Docker
wsl bash -lic "apt update"
wsl bash -lic "apt install -y docker-ce docker-ce-cli containerd.io"

# 启动 Docker 服务
wsl bash -lic "sudo service docker start"

# 将当前用户添加到 docker 组
wsl bash -lic "sudo usermod -aG docker \$USER"

# 验证安装
wsl bash -lic "docker run hello-world"
```

---

## 日常任务

### 备份文件

```bash
# 创建带时间戳的备份
wsl bash -lic "cp important.txt important-\$(date +%Y%m%d-%H%M%S).txt"

# 备份整个目录
wsl bash -lic "tar -czf backup-\$(date +%Y%m%d).tar.gz ~/projects/"

# 备份到 Windows 目录
wsl bash -lic "tar -czf /mnt/c/Backups/backup-\$(date +%Y%m%d).tar.gz ~/projects/"
```

### 压缩和解压

```bash
# 压缩目录为 tar.gz
wsl bash -lic "tar -czf archive.tar.gz /path/to/directory"

# 解压 tar.gz
wsl bash -lic "tar -xzf archive.tar.gz"

# 解压到指定目录
wsl bash -lic "tar -xzf archive.tar.gz -C /path/to/destination"

# 压缩为 zip
wsl bash -lic "zip -r archive.zip /path/to/directory"

# 解压 zip
wsl bash -lic "unzip archive.zip"

# 列出 zip 内容
wsl bash -lic "unzip -l archive.zip"
```

### 系统清理

```bash
# 清理 APT 缓存
wsl bash -lic "apt clean"

# 删除不需要的包
wsl bash -lic "apt autoremove -y"

# 清理系统日志
wsl bash -lic "sudo journalctl --vacuum-time=7d"

# 清理临时文件
wsl bash -lic "rm -rf /tmp/*"

# 清理用户缓存
wsl bash -lic "rm -rf ~/.cache/*"
```

### 监控系统资源

```bash
# 查看系统负载和运行时间
wsl bash -lic "uptime"

# 查看内存使用情况
wsl bash -lic "free -h"

# 查看磁盘使用情况
wsl bash -lic "df -h"

# 查看目录大小
wsl bash -lic "du -sh /path/to/directory"

# 查找最大的 10 个文件
wsl bash -lic "find /path -type f -exec du -h {} + | sort -rh | head -10"

# 查看进程快照
wsl bash -lic "top -b -n 1"

# 查看 CPU 使用率最高的进程
wsl bash -lic "ps aux --sort=-%cpu | head -20"

# 查看内存使用率最高的进程
wsl bash -lic "ps aux --sort=-%mem | head -20"
```

### 批量重命名

```bash
# 批量添加前缀
wsl bash -lic "rename 's/^/prefix_/' *.txt"

# 批量替换扩展名
wsl bash -lic "rename 's/\.txt$/.md/' *.txt"

# 批量删除文件名中的空格
wsl bash -lic "rename 's/ //g' *"

# 使用循环重命名
wsl bash -lic "for file in *.txt; do mv \"\$file\" \"new_\$file\"; done"
```

### 定时任务

```bash
# 每秒监控进程
wsl bash -lic "watch -n 1 'ps aux | grep nginx'"

# 每分钟检查服务状态
wsl bash -lic "while true; do systemctl status nginx; sleep 60; done"
```

---

## 实用技巧

### 组合命令

```bash
# 更新并清理系统
wsl bash -lic "apt update && apt upgrade -y && apt autoremove -y && apt clean"

# 查找并删除特定文件
wsl bash -lic "find /tmp -name '*.tmp' -delete"

# 监控多个指标
wsl bash -lic "echo '=== CPU ===' && top -b -n 1 | head -10 && echo '=== Memory ===' && free -h"

# 查找大文件并排序
wsl bash -lic "find . -type f -size +10M -exec ls -lh {} + | sort -k5 -h"
```

### 命令别名

```bash
# 创建临时别名
wsl bash -lic "alias ll='ls -la'"
wsl bash -lic "alias update='apt update && apt upgrade -y'"

# 永久添加别名到 .bashrc
wsl bash -lic "echo 'alias ll=\"ls -la\"' >> ~/.bashrc"
wsl bash -lic "source ~/.bashrc"
```

### 环境变量

```bash
# 设置临时环境变量
wsl bash -lic "export NODE_ENV=production && node server.js"

# 永久添加到 .bashrc
wsl bash -lic "echo 'export PATH=\$PATH:/new/path' >> ~/.bashrc"
wsl bash -lic "source ~/.bashrc"

# 查看所有环境变量
wsl bash -lic "env"
```

---

**文档版本**: 1.0.0
**最后更新**: 2026-03-26
