# WSL 故障排除指南

本文档提供 WSL 使用中的常见问题和解决方案。

## 目录

- [安装和配置问题](#安装和配置问题)
- [性能问题](#性能问题)
- [网络连接问题](#网络连接问题)
- [文件系统问题](#文件系统问题)
- [内存和资源问题](#内存和资源问题)
- [权限问题](#权限问题)
- [应用程序兼容性](#应用程序兼容性)
- [WSL 2 特定问题](#wsl-2-特定问题)
- [调试技巧](#调试技巧)

---

## 安装和配置问题

### WSL 未安装

**症状**:
```
'wsl' is not recognized as an internal or external command
```

**解决方案**:

1. **以管理员身份打开 PowerShell** 并执行:
```powershell
dism.exe /online /enable-feature /featurename:Microsoft-Windows-Subsystem-Linux /all /norestart
dism.exe /online /enable-feature /featurename:VirtualMachinePlatform /all /norestart
```

2. **重启计算机**

3. **下载并安装 WSL2 Linux 内核更新包**:
   - 访问: https://aka.ms/wsl2kernel
   - 下载并运行安装程序

4. **设置 WSL 2 为默认版本**:
```powershell
wsl --set-default-version 2
```

5. **安装 Linux 发行版**:
```powershell
# 从 Microsoft Store 安装，或使用命令行
wsl --install -d Ubuntu-22.04
```

### 发行版安装失败

**症状**:
```
Error: 0x8007019e
Error: 0x80370102
```

**解决方案**:

1. **启用 WSL 功能**:
```powershell
dism.exe /online /enable-feature /featurename:Microsoft-Windows-Subsystem-Linux /all /norestart
```

2. **启用虚拟机平台**:
```powershell
dism.exe /online /enable-feature /featurename:VirtualMachinePlatform /all /norestart
```

3. **确保 BIOS 中启用了虚拟化**:
   - Intel: VT-x
   - AMD: AMD-V

4. **检查 Hyper-V 是否被禁用**（某些冲突情况）:
```powershell
# 查看状态
bcdedit /enum | find "hypervisorlaunchtype"

# 如果被禁用，启用它
bcdedit /set hypervisorlaunchtype auto
```

### WSL 版本不匹配

**症状**: 发行版运行在 WSL 1 但需要 WSL 2 的特性。

**解决方案**:

```bash
# 查看当前 WSL 版本
wsl --list --verbose

# 转换为 WSL 2
wsl --set-version Ubuntu-22.04 2

# 设置 WSL 2 为默认
wsl --set-default-version 2
```

---

## 性能问题

### 文件 I/O 性能慢

**症状**: 在 `/mnt/c/` 上操作文件时速度很慢。

**原因**: 跨文件系统操作（WSL 2 访问 Windows 文件系统）有性能开销。

**解决方案**:

1. **将工作文件放在 WSL 文件系统中**:
```bash
# 推荐: 在 WSL 主目录中工作
cd ~
mkdir projects
cd projects
git clone https://github.com/user/repo.git
```

2. **如果必须在 Windows 文件系统中工作**:
```bash
# 使用 WSL 1（文件 I/O 更快）
wsl --set-version Ubuntu-22.04 1

# 或使用 9P 协议缓存（WSL 2）
# 在 /etc/wsl.conf 中配置
```

3. **配置 WSL 以提高性能**:
```bash
# 编辑 /etc/wsl.conf
sudo nano /etc/wsl.conf

# 添加以下内容
[automount]
enabled = true
options = "metadata"
# 缓存元数据以提高性能

[interop]
enabled = false
# 禁用 Windows 互操作以提高性能
```

### CPU 占用过高

**症状**: WSL 进程占用大量 CPU。

**解决方案**:

1. **限制 WSL 使用的 CPU 核心数**:
```bash
# 在 Windows 用户目录创建 .wslconfig
# C:\Users\username\.wslconfig

[wsl2]
memory=4GB
processors=2
swap=2GB
```

2. **重启 WSL**:
```bash
wsl --shutdown
```

3. **检查占用 CPU 的进程**:
```bash
wsl bash -lic "top -b -n 1"
wsl bash -lic "ps aux --sort=-%cpu | head -20"
```

### 启动速度慢

**症状**: WSL 发行版启动需要很长时间。

**解决方案**:

1. **减少启动服务**:
```bash
# 查看启动服务
wsl bash -lic "systemctl list-unit-files --type=service | grep enabled"

# 禁用不需要的服务
wsl bash -lic "sudo systemctl disable service-name"
```

2. **优化 .bashrc 和 .bash_profile**:
```bash
# 注释掉耗时的命令
wsl bash -lic "nano ~/.bashrc"

# 将耗时命令移到后台或延迟执行
```

3. **使用较轻量的发行版**:
```bash
# 考虑使用 Alpine 而不是 Ubuntu
wsl --install -d Alpine
```

---

## 网络连接问题

### 无法访问外网

**症状**: `ping` 或 `curl` 无法连接到外网。

**解决方案**:

1. **检查 WSL 网络模式**:
```bash
# 查看 Windows 防火墙规则
# 确保 WSL 允许出站连接
```

2. **使用镜像模式（WSL 2 0.58.0+）**:
```bash
# 在 .wslconfig 中配置
[wsl2]
networkingMode=mirrored
```

3. **检查 DNS 配置**:
```bash
wsl bash -lic "cat /etc/resolv.conf"

# 手动设置 DNS
wsl bash -lic "sudo rm /etc/resolv.conf"
wsl bash -lic "sudo bash -c 'echo \"nameserver 8.8.8.8\" > /etc/resolv.conf'"
wsl bash -lic "sudo bash -c 'echo \"nameserver 8.8.4.4\" >> /etc/resolv.conf'"
```

4. **重启网络服务**:
```bash
wsl bash -lic "sudo service network-manager restart"
```

### localhost 端口转发问题

**症状**: Windows 浏览器无法访问 `localhost:3000`（WSL 中运行的应用）。

**解决方案**:

1. **在 WSL 2 中使用 localhost 即可访问**:
```bash
# WSL 2 支持 localhost 转发，应该直接可以工作
wsl bash -lic "python3 -m http.server 3000"
# 在 Windows 浏览器访问: http://localhost:3000
```

2. **如果不行，使用 WSL IP 地址**:
```bash
# 获取 WSL IP 地址
wsl bash -lic "hostname -I"
# 在 Windows 浏览器访问: http://172.x.x.x:3000
```

3. **检查 Windows 防火墙**:
```powershell
# 添加防火墙规则允许入站连接
New-NetFirewallRule -DisplayName "WSL" -Direction Inbound -InterfaceAlias "vEthernet (WSL)" -Action Allow
```

### VPN 导致的网络问题

**症状**: 连接 VPN 后 WSL 无法访问网络。

**解决方案**:

1. **使用镜像模式网络**（推荐）:
```ini
# .wslconfig
[wsl2]
networkingMode=mirrored
```

2. **配置 VPN 适配器**:
```powershell
# 在 PowerShell 中查看网络适配器
Get-NetAdapter

# 确保允许 WSL 适配器通过 VPN
```

3. **使用 NAT 模式**:
```ini
# .wslconfig
[wsl2]
networkingMode=nat
```

---

## 文件系统问题

### 权限错误

**症状**: `Permission denied` 错误。

**解决方案**:

1. **检查文件权限**:
```bash
wsl bash -lic "ls -la /path/to/file"
```

2. **修改权限**:
```bash
wsl bash -lic "chmod +x script.sh"
wsl bash -lic "chmod 755 /path/to/file"
```

3. **使用 sudo**:
```bash
wsl bash -lic "sudo command"
```

4. **修复所有者**:
```bash
wsl bash -lic "sudo chown -R username:username /path/to/dir"
```

### 挂载 Windows 驱动器失败

**症状**: `/mnt/c/` 等目录不存在或为空。

**解决方案**:

1. **手动挂载驱动器**:
```bash
wsl bash -lic "sudo mkdir /mnt/c"
wsl bash -lic "sudo mount -t drvfs 'C:' /mnt/c"
```

2. **检查 automount 配置**:
```bash
wsl bash -lic "cat /etc/wsl.conf"

# 添加或修改
[automount]
enabled = true
root = /mnt/
options = "metadata"
```

3. **重启 WSL**:
```bash
wsl --shutdown
```

### 磁盘空间不足

**症状**: `No space left on device` 错误。

**解决方案**:

1. **检查磁盘使用**:
```bash
wsl bash -lic "df -h"
```

2. **清理不需要的包**:
```bash
wsl bash -lic "sudo apt autoremove -y"
wsl bash -lic "sudo apt clean"
```

3. **扩展 WSL 2 虚拟磁盘**:
```powershell
# 在 Windows 中关闭 WSL
wsl --shutdown

# 查找 VHDX 文件位置
# C:\Users\username\AppData\Local\Packages\...

# 使用 PowerShell 调整大小
Resize-VHD -Path "C:\path\to\ext4.vhdx" -SizeBytes 80GB
```

4. **查找大文件**:
```bash
wsl bash -lic "find ~ -type f -size +100M -exec ls -lh {} +"
```

---

## 内存和资源问题

### 内存不足

**症状**: WSL 进程被杀或系统变慢。

**解决方案**:

1. **限制 WSL 内存使用**:
```ini
# .wslconfig
[wsl2]
memory=4GB
```

2. **增加交换空间**:
```ini
# .wslconfig
[wsl2]
swap=4GB
```

3. **检查内存使用**:
```bash
wsl bash -lic "free -h"
wsl bash -lic "ps aux --sort=-%mem | head -20"
```

4. **优化内存使用**:
```bash
# 减少并发进程
wsl bash -lic "export GOMAXPROCS=2"

# 清理缓存
wsl bash -lic "sync && echo 3 | sudo tee /proc/sys/vm/drop_caches"
```

### WSL 无法启动

**症状**: WSL 发行版启动失败或立即退出。

**解决方案**:

1. **查看日志**:
```powershell
# 在 Windows 事件查看器中查看 WSL 日志
eventvwr.msc
# 应用程序和服务 -> Microsoft -> Windows -> WSL
```

2. **重置发行版**:
```powershell
# 导出数据
wsl --export Ubuntu-22.04 D:\backup\ubuntu.tar

# 注销发行版
wsl --unregister Ubuntu-22.04

# 重新导入
wsl --import Ubuntu-22.04 D:\WSL\Ubuntu D:\backup\ubuntu.tar
```

3. **修复文件系统**:
```bash
# 启动发行版时运行 fsck
wsl bash -lic "sudo fsck /dev/sdXX"
```

---

## 权限问题

### sudo 权限问题

**症状**: `username is not in the sudoers file` 错误。

**解决方案**:

1. **将用户添加到 sudo 组**:
```bash
# 以 root 身份登录
wsl -u root

# 添加用户到 sudo 组
usermod -aG sudo username
```

2. **编辑 sudoers 文件**:
```bash
wsl bash -lic "sudo visudo"

# 添加行
username ALL=(ALL:ALL) ALL
```

### 文件所有权问题

**症状**: 无法编辑 `/mnt/c/` 中的文件。

**原因**: Windows 文件系统权限与 Linux 不完全兼容。

**解决方案**:

1. **在 /etc/wsl.conf 中启用元数据**:
```ini
[automount]
options = "metadata"
```

2. **使用 Windows 工具编辑 Windows 文件**:
```bash
# 使用 VS Code
wsl bash -lic "code /mnt/c/path/to/file.txt"

# 使用 Windows 记事本
wsl bash -lic "notepad.exe /mnt/c/path/to/file.txt"
```

3. **复制文件到 WSL 文件系统编辑**:
```bash
wsl bash -lic "cp /mnt/c/file.txt ~/file.txt"
wsl bash -lic "nano ~/file.txt"
wsl bash -lic "cp ~/file.txt /mnt/c/file.txt"
```

---

## 应用程序兼容性

### Docker 不工作

**症状**: Docker 命令报错或无法启动容器。

**解决方案**:

1. **安装 Docker Desktop for Windows**:
   - 确保 "Use WSL 2 based engine" 已启用
   - 在 Settings -> Resources -> WSL Integration 中启用发行版

2. **检查 Docker 状态**:
```bash
wsl bash -lic "docker info"
```

3. **重新安装 Docker**:
```bash
# 移除旧版本
wsl bash -lic "sudo apt remove docker docker-engine docker.io containerd runc"

# 安装新版本
wsl bash -lic "curl -fsSL https://get.docker.com -o get-docker.sh"
wsl bash -lic "sudo sh get-docker.sh"
```

### systemctl 不工作

**症状**: `System has not been booted with systemd as init system` 错误。

**解决方案**:

1. **启用 systemd（推荐）**:
```bash
# 编辑 /etc/wsl.conf
wsl bash -lic "sudo nano /etc/wsl.conf"

# 添加
[boot]
systemd=true

# 重启 WSL
wsl --shutdown
```

2. **使用 service 命令作为替代**:
```bash
# 代替 systemctl start nginx
wsl bash -lic "sudo service nginx start"

# 代替 systemctl status nginx
wsl bash -lic "sudo service nginx status"
```

### 图形应用不显示

**症状**: X11 或 Wayland 应用无法显示窗口。

**解决方案**:

1. **使用 WSLg（Windows 11）**:
   - WSLg 在 Windows 11 中默认支持图形应用
   - 确保 Windows 是最新版本

2. **在 Windows 10 中使用 X Server**:
```bash
# 安装 VcXsrv 或 X410

# 设置 DISPLAY 变量
export DISPLAY=$(cat /etc/resolv.conf | grep nameserver | awk '{print $2}'):0

# 允许连接
export LIBGL_ALWAYS_INDIRECT=1
```

3. **使用 Wayland**:
```bash
export WAYLAND_DISPLAY=wayland-0
```

---

## WSL 2 特定问题

### WSL 2 虚拟磁盘损坏

**症状**: WSL 2 无法启动或频繁崩溃。

**解决方案**:

1. **检查和修复虚拟磁盘**:
```powershell
# 关闭 WSL
wsl --shutdown

# 查找 VHDX 文件
dir "$env:LOCALAPPDATA\Packages\**\LocalState\" -Recurse -Filter ext4.vhdx

# 使用 PowerShell 修复
Optimize-VHD -Path "path\to\ext4.vhdx" -Mode Full
```

2. **导出并重新导入**:
```powershell
# 导出
wsl --export Ubuntu-22.04 D:\backup\ubuntu.tar

# 注销损坏的发行版
wsl --unregister Ubuntu-22.04

# 导入
wsl --import Ubuntu-22.04 D:\WSL\Ubuntu-22.04 D:\backup\ubuntu.tar
```

### WSL 2 IP 地址变化

**症状**: 每次重启后 WSL 2 IP 地址改变。

**解决方案**:

1. **使用 localhost**（WSL 2.0.58+ 支持）:
```bash
# Windows 可以直接访问 localhost
# WSL 中的服务
wsl bash -lic "python3 -m http.server 3000"
# Windows 浏览器访问: http://localhost:3000
```

2. **使用端口转发**:
```powershell
# 在 PowerShell 中创建脚本
$port = "3000"
$wslIp = wsl hostname -I
$wslIp = $wslIp.Trim()
netsh interface portproxy add v4tov4 listenport=$port listenaddress=0.0.0.0 connectport=$port connectaddress=$wslIp
```

3. **使用 .wslconfig 镜像模式**:
```ini
[wsl2]
networkingMode=mirrored
```

---

## 调试技巧

### 启用详细日志

```bash
# 在 Windows 中启用 WSL 日志
# 在注册表中添加
[HKEY_CURRENT_USER\SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss]
"LogLevel"=dword:0000000f

# 查看 Windows 事件日志
eventvwr.msc
```

### 使用 strace 调试

```bash
# 跟踪命令执行
wsl bash -lic "strace command"

# 跟踪运行中的进程
wsl bash -lic "strace -p 1234"

# 保存到文件
wsl bash -lic "strace -o trace.log command"
```

### 检查 WSL 版本

```bash
# 查看 WSL 版本
wsl --version

# 查看发行版版本
wsl --list --verbose

# 查看内核版本
wsl bash -lic "uname -a"
```

### 性能分析

```bash
# CPU 性能
wsl bash -lic "time command"

# 内存使用
wsl bash -lic "valgrind --leak-check=full command"

# I/O 性能
wsl bash -lic "iostat -x 1 5"
```

---

## 获取帮助

### 官方资源

- **WSL 官方文档**: https://docs.microsoft.com/windows/wsl
- **WSL GitHub Issues**: https://github.com/microsoft/WSL/issues
- **WSL Release Notes**: https://github.com/microsoft/WSL/releases

### 社区资源

- **Reddit r/WSL**: https://reddit.com/r/WSL
- **Stack Overflow [wsl] tag**: https://stackoverflow.com/questions/tagged/wsl

### 报告问题

在报告问题时，提供以下信息：

```bash
# WSL 版本
wsl --version

# 发行版信息
wsl --list --verbose

# 内核版本
wsl bash -lic "uname -a"

# 复现步骤
# 预期行为
# 实际行为
```

---

**文档版本**: 1.0.0
**最后更新**: 2026-03-26
**适用平台**: Windows 10/11 with WSL 1/2
