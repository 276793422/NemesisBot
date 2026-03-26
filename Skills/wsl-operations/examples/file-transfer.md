# 文件传输和操作示例

本文档提供 Windows 和 WSL 之间文件传输和操作的实际使用示例。

## 目录

- [基本路径访问](#基本路径访问)
- [路径转换](#路径转换)
- [文件复制和移动](#文件复制和移动)
- [VS Code 集成](#vs-code-集成)
- [跨平台开发](#跨平台开发)
- [实际场景](#实际场景)

---

## 基本路径访问

### 从 WSL 访问 Windows 文件

```bash
# 访问 Windows 用户目录
wsl bash -lic "ls -la /mnt/c/Users/username/"

# 访问 Windows 系统目录
wsl bash -lic "ls -la /mnt/c/Windows/"

# 访问其他驱动器
wsl bash -lic "ls -la /mnt/d/"
wsl bash -lic "ls -la /mnt/e/"

# 查看所有已挂载的驱动器
wsl bash -lic "ls -la /mnt/"

# 访问 Windows Program Files
wsl bash -lic "cd /mnt/c/Program\ Files/"

# 访问 Windows 临时目录
wsl bash -lic "cd /mnt/c/Users/username/AppData/Local/Temp"
```

### 从 Windows 访问 WSL 文件

```powershell
# 在 Windows 资源管理器中
\\wsl$\Ubuntu\home\username
\\wsl$\Ubuntu\etc
\\wsl$\Ubuntu\var

# 在 Windows 命令提示符中
cd \\wsl$\Ubuntu\home\username

# 在 PowerShell 中
Set-Location \\wsl$\Ubuntu\home\username
```

```bash
# 从 WSL 中在 Windows 资源管理器打开当前目录
wsl bash -lic "explorer.exe ."

# 打开 WSL 主目录
wsl bash -lic "explorer.exe ~"

# 打开特定目录
wsl bash -lic "explorer.exe /mnt/c/Projects"
```

---

## 路径转换

### 使用 wslpath 工具

```bash
# Windows 路径转 WSL 路径
wsl bash -lic "wslpath -u 'C:\Users\username\Documents'"
# 输出: /mnt/c/Users/username/Documents

# WSL 路径转 Windows 路径
wsl bash -lic "wslpath -w '/mnt/c/Users/username/Documents'"
# 输出: C:\Users\username\Documents

# 转换为混合路径（保留正斜杠）
wsl bash -lic "wslpath -m 'C:\Users\username\Documents'"
# 输出: C:/Users/username/Documents

# 转换 WSL 主目录路径
wsl bash -lic "wslpath -w '/home/username'"
# 输出: \\wsl$\Ubuntu\home\username
```

### 路径转换场景

```bash
# 转换路径后使用 VS Code 打开
wsl bash -lic "code \$(wslpath -w /path/to/file)"

# 从 Windows 文件路径在 WSL 中执行命令
win_path="C:\Projects\myproject"
wsl_path=$(wsl bash -lic "wslpath -u '$win_path'")
echo "WSL path: $wsl_path"

# 在 WSL 中操作 Windows 路径
wsl bash -lic "cp /mnt/c/Users/username/file.txt ~/"

# 从 Windows 访问 WSL 文件
wsl bash -lic "explorer.exe \$(wslpath -w ~/myproject)"
```

---

## 文件复制和移动

### 从 Windows 到 WSL

```bash
# 复制 Windows 文件到 WSL 主目录
wsl bash -lic "cp /mnt/c/Users/username/file.txt ~/"

# 复制 Windows 目录到 WSL
wsl bash -lic "cp -r /mnt/c/Users/username/Downloads ~/downloads_backup"

# 复制并保留文件属性
wsl bash -lic "cp -p /mnt/c/Users/username/config.ini ~/"

# 移动 Windows 文件到 WSL
wsl bash -lic "mv /mnt/c/Users/username/file.txt ~/"

# 使用通配符复制多个文件
wsl bash -lic "cp /mnt/c/Users/username/Downloads/*.pdf ~/Documents/"

# 递归复制特定类型的文件
wsl bash -lic "find /mnt/c/Users/username/Source -name '*.py' -exec cp {} ~/PythonProjects/ \;"
```

### 从 WSL 到 Windows

```bash
# 复制 WSL 文件到 Windows 目录
wsl bash -lic "cp ~/file.txt /mnt/c/Users/username/Documents/"

# 复制 WSL 目录到 Windows
wsl bash -lic "cp -r ~/project /mnt/c/Users/username/Projects/"

# 移动 WSL 文件到 Windows
wsl bash -lic "mv ~/archive.zip /mnt/c/Users/username/Downloads/"

# 使用通配符
wsl bash -lic "cp ~/logs/*.log /mnt/c/Users/username/Logs/"

# 复制时保持目录结构
wsl bash -lic "cp -r --parents ~/project/src /mnt/c/Users/username/Backup/"
```

### 使用 rsync 同步

```bash
# 安装 rsync
wsl bash -lic "apt install -y rsync"

# 同步 WSL 目录到 Windows
wsl bash -lic "rsync -avz ~/project/ /mnt/c/Users/username/Projects/project/"

# 同步 Windows 目录到 WSL
wsl bash -lic "rsync -avz /mnt/c/Users/username/Downloads/ ~/downloads/"

# 删除目标中源没有的文件
wsl bash -lic "rsync -avz --delete ~/project/ /mnt/c/Users/username/Projects/project/"

# 显示传输进度
wsl bash -lic "rsync -avz --progress ~/largefile /mnt/c/Users/username/Transfer/"

# 限制传输速度（KB/s）
wsl bash -lic "rsync -avz --bwlimit=1000 ~/largefile /mnt/c/Users/username/Transfer/"

# 仅同步特定文件类型
wsl bash -lic "rsync -avz --include='*.py' --exclude='*' ~/src/ /mnt/c/Users/username/Python/"
```

### 使用 SCP

```bash
# 注意: SCP 主要用于远程传输，但也可用于本地

# 从 Windows 文件系统复制到 WSL（间接方式）
# 首先在 WSL 中生成密钥
wsl bash -lic "ssh-keygen -t rsa -b 4096"

# 复制公钥到 Windows
wsl bash -lic "cp ~/.ssh/id_rsa.pub /mnt/c/Users/username/.ssh/authorized_keys"

# 使用 SCP 复制文件（通过本地主机）
wsl bash -lic "scp /mnt/c/Users/username/file.txt localhost:~/"

# 递归复制目录
wsl bash -lic "scp -r /mnt/c/Users/username/Documents localhost:~/docs_backup/"
```

---

## VS Code 集成

### 从 WSL 打开 VS Code

```bash
# 打开当前目录
wsl bash -lic "code ."

# 打开特定目录
wsl bash -lic "code ~/project"

# 打开特定文件
wsl bash -lic "code ~/project/main.py"

# 打开多个文件
wsl bash -lic "code file1.txt file2.txt file3.txt"

# 在新窗口打开
wsl bash -lic "code -n ~/project"

# 打开并跳转到特定行
wsl bash -lic "code -g ~/project/main.py:42"

# 比较两个文件
wsl bash -lic "code --diff file1.txt file2.txt"
```

### 从 Windows 访问 WSL 文件

```bash
# 转换路径后在 Windows 中打开
wsl bash -lic "code \$(wslpath -w ~/project)"

# 在 VS Code 中集成终端使用 WSL
# 在 VS Code 中:
# 1. 按 Ctrl+Shift+P
# 2. 输入 "Terminal: Create New Terminal"
# 3. 终端默认使用 WSL bash

# 在 VS Code 中设置默认 shell 为 WSL
# 在 settings.json 中添加:
{
    "terminal.integrated.defaultProfile.windows": "WSL"
}
```

### Remote - WSL 扩展

```bash
# 安装 Remote - WSL 扩展后:
# 1. 在 VS Code 中按 Ctrl+Shift+P
# 2. 输入 "WSL: Reopen Folder in WSL"
# 3. 现在 VS Code 直接在 WSL 环境中运行

# 从命令行在 WSL 环境打开 VS Code
wsl bash -lic "code ."

# 现在所有扩展和工具都在 WSL 中运行
# - IntelliSense 正常工作
# - 调试器可以直接调试
# - Git 操作在 WSL 中执行
```

---

## 跨平台开发

### Git 仓库跨平台

```bash
# 在 WSL 中克隆仓库
wsl bash -lic "git clone https://github.com/user/repo.git"

# 在 Windows VS Code 中打开
wsl bash -lic "code \$(wslpath -w repo)"

# 在 Windows Git Bash 中访问 WSL 仓库
# cd \\wsl$\Ubuntu\home\username\repo

# 在 WSL 中提交更改
wsl bash -lic "cd repo && git add . && git commit -m 'Update'"

# 从 Windows 推送（如果配置了 Windows Git）
git push
```

### 项目配置

```bash
# 创建项目目录（在 WSL 中）
wsl bash -lic "mkdir -p ~/projects/myproject && cd ~/projects/myproject"

# 创建符号链接指向 Windows 目录（便于访问）
wsl bash -lic "ln -s /mnt/c/Projects ~/win_projects"

# 访问符号链接
wsl bash -lic "cd ~/win_projects/myproject"

# 在 .bashrc 中添加项目别名
wsl bash -lic "echo 'alias myproj=\"cd ~/projects/myproject\"' >> ~/.bashrc"
wsl bash -lic "source ~/.bashrc"
```

### Python 跨平台开发

```bash
# 在 WSL 中设置 Python 环境
wsl bash -lic "cd ~/projects/myproject"
wsl bash -lic "python3 -m venv venv"
wsl bash -lic "source venv/bin/activate"
wsl bash -lic "pip install -r requirements.txt"

# 在 Windows VS Code 中打开（自动检测虚拟环境）
wsl bash -lic "code ."

# VS Code 会自动:
# - 使用 WSL 中的 Python 解释器
# - 安装扩展到 WSL
# - 在 WSL 中运行和调试代码
```

### Node.js 跨平台开发

```bash
# 在 WSL 中设置 Node.js 项目
wsl bash -lic "cd ~/projects/myproject"
wsl bash -lic "npm init -y"
wsl bash -lic "npm install express"

# 在 VS Code 中打开
wsl bash -lic "code ."

# 运行开发服务器（在 WSL 中）
wsl bash -lic "npm run dev"

# 在 Windows 浏览器访问（WSL 2 支持 localhost）
# http://localhost:3000
```

---

## 实际场景

### 场景 1: 开发 Web 应用

```bash
# 1. 在 WSL 中创建项目
wsl bash -lic "mkdir -p ~/projects/webapp && cd ~/projects/webapp"

# 2. 初始化项目
wsl bash -lic "npm init -y"

# 3. 安装依赖
wsl bash -lic "npm install express"

# 4. 创建应用
wsl bash -lic "cat > server.js << 'EOF'
const express = require('express');
const app = express();
app.get('/', (req, res) => res.send('Hello from WSL!'));
app.listen(3000, () => console.log('Server running on port 3000'));
EOF"

# 5. 在 VS Code 中编辑
wsl bash -lic "code ."

# 6. 启动服务器
wsl bash -lic "node server.js"

# 7. 在 Windows 浏览器访问
# http://localhost:3000
```

### 场景 2: 备份文件

```bash
# 1. 创建备份目录
wsl bash -lic "mkdir -p ~/backups"

# 2. 备份 WSL 配置文件
wsl bash -lic "tar -czf ~/backups/dotfiles-\$(date +%Y%m%d).tar.gz .bashrc .vimrc .gitconfig"

# 3. 备份到 Windows
wsl bash -lic "cp ~/backups/dotfiles-*.tar.gz /mnt/c/Users/username/Backups/"

# 4. 备份项目目录
wsl bash -lic "rsync -avz --delete ~/projects/ /mnt/c/Users/username/OneDrive/Backups/projects/"

# 5. 定期自动备份
wsl bash -lic "echo '0 2 * * * rsync -avz ~/projects/ /mnt/c/Users/username/Backups/' | crontab -"
```

### 场景 3: 跨平台脚本

```bash
# 创建可在 Windows 和 WSL 中运行的脚本
wsl bash -lic "cat > ~/scripts/copy-project.sh << 'EOF'
#!/bin/bash
# 自动复制项目到 Windows 目录

PROJECT_NAME="\${1:-myproject}"
WSL_PATH="~/projects/\$PROJECT_NAME"
WIN_PATH="/mnt/c/Projects/\$PROJECT_NAME"

echo "Copying \$PROJECT_NAME from WSL to Windows..."
rsync -avz --delete \$WSL_PATH/ \$WIN_PATH/

echo "Project copied successfully!"
echo "Windows path: C:\\Projects\\\$PROJECT_NAME"
EOF"

# 添加执行权限
wsl bash -lic "chmod +x ~/scripts/copy-project.sh"

# 使用脚本
wsl bash -lic "~/scripts/copy-project.sh myapp"
```

### 场景 4: 数据处理流水线

```bash
# 1. 从 Windows 获取原始数据
wsl bash -lic "cp /mnt/c/Users/username/Downloads/data.csv ~/projects/data-analysis/"

# 2. 在 WSL 中处理数据
wsl bash -lic "cd ~/projects/data-analysis"
wsl bash -lic "python3 process_data.py"

# 3. 生成报告（输出到 Windows）
wsl bash -lic "python3 generate_report.py > /mnt/c/Users/username/Documents/report.txt"

# 4. 在 Windows 中查看报告
wsl bash -lic "notepad.exe /mnt/c/Users/username/Documents/report.txt"
```

### 场景 5: Docker 跨平台开发

```bash
# 1. 创建 Dockerfile（在 WSL 中）
wsl bash -lic "cd ~/projects/myapp"
wsl bash -lic "cat > Dockerfile << 'EOF'
FROM node:18
WORKDIR /app
COPY package*.json ./
RUN npm install
COPY . .
EXPOSE 3000
CMD ["npm", "start"]
EOF"

# 2. 构建 Docker 镜像
wsl bash -lic "docker build -t myapp ."

# 3. 运行容器
wsl bash -lic "docker run -d -p 3000:3000 --name myapp myapp"

# 4. 从 Windows 浏览器访问
# http://localhost:3000

# 5. 查看容器日志
wsl bash -lic "docker logs -f myapp"
```

### 场景 6: 跨平台搜索文件

```bash
# 在 Windows 和 WSL 中搜索文件

# 1. 搜索 WSL 中的文件
wsl bash -lic "find ~ -name 'config.ini'"

# 2. 搜索 Windows 中的文件
wsl bash -lic "find /mnt/c/Users/username -name 'config.ini'"

# 3. 搜索文件内容
wsl bash -lic "grep -r 'TODO' ~/projects/"
wsl bash -lic "grep -r 'TODO' /mnt/c/Projects/"

# 4. 搜索结果保存到 Windows
wsl bash -lic "grep -r 'function' ~/projects/ > /mnt/c/Users/username/Desktop/search-results.txt"
```

### 场景 7: 媒体文件转换

```bash
# 1. 安装 FFmpeg
wsl bash -lic "apt install -y ffmpeg"

# 2. 从 Windows 获取视频文件
wsl bash -lic "cp /mnt/c/Users/username/Videos/input.mp4 ~/temp/"

# 3. 在 WSL 中转换视频
wsl bash -lic "ffmpeg -i ~/temp/input.mp4 -c:v libx264 -crf 23 ~/temp/output.mp4"

# 4. 输出到 Windows
wsl bash -lic "cp ~/temp/output.mp4 /mnt/c/Users/username/Videos/Converted/"

# 5. 批量转换
wsl bash -lic "for file in /mnt/c/Users/username/Videos/*.avi; do
    filename=\$(basename \"\$file\" .avi)
    ffmpeg -i \"\$file\" \"~/temp/\${filename}.mp4\"
    cp \"~/temp/\${filename}.mp4\" \"/mnt/c/Users/username/Videos/Converted/\"
done"
```

---

## 最佳实践

### 1. 性能优化

```bash
# 推荐: 将工作文件放在 WSL 文件系统中
cd ~/projects
# WSL 文件系统性能更好

# 避免: 频繁访问 /mnt/c/ 中的文件
# 跨文件系统操作会有性能损失

# 如果必须使用 Windows 文件系统:
# 1. 只读取，不频繁写入
# 2. 批量操作而非单个文件操作
# 3. 考虑使用 WSL 1（文件 I/O 更快）
```

### 2. 路径处理

```bash
# 使用引号保护包含空格的路径
wsl bash -lic "ls -la '/mnt/c/Program Files/'"

# 使用 wslpath 进行转换
win_path="C:\Users\username\My Documents"
wsl bash -lic "cd \$(wslpath -u \"$win_path\")"

# 在脚本中使用变量
file_path="/mnt/c/Users/username/file.txt"
wsl bash -lic "cp \"$file_path\" ~/"
```

### 3. 符号链接

```bash
# 创建常用目录的符号链接
wsl bash -lic "ln -s /mnt/c/Projects ~/win_projects"
wsl bash -lic "ln -s /mnt/c/Users/username/Downloads ~/downloads"

# 现在可以快速访问
wsl bash -lic "cd ~/win_projects"
wsl bash -lic "cd ~/downloads"
```

### 4. 权限管理

```bash
# 在 /etc/wsl.conf 中启用元数据
wsl bash -lic "sudo tee -a /etc/wsl.conf << EOF
[automount]
options = \"metadata\"
EOF"

# 重启 WSL
wsl --shutdown

# 现在文件权限会更好地保留
```

---

**文档版本**: 1.0.0
**最后更新**: 2026-03-26
