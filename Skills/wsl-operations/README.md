# WSL Operations Skill

## 概述

WSL Operations Skill 为 Windows 用户提供通过 Windows Subsystem for Linux (WSL) 执行 Linux 操作的完整指南。这个 skill 专为需要在 Windows 上进行 Linux 开发、测试和系统管理的用户设计。

## 主要功能

1. **命令执行** - 在 WSL 中执行 Linux 命令和脚本
2. **系统监控** - 查看 CPU、内存、磁盘和网络状态
3. **进程管理** - 查看、管理和终止进程与服务
4. **文件操作** - Windows 和 WSL 之间的文件传输和路径转换
5. **WSL 管理** - 控制 WSL 实例的状态和生命周期

## 适用场景

- 在 Windows 上编译和测试 Linux 项目
- 管理 WSL 中的服务和进程
- 监控 WSL 系统资源使用
- 在 Windows 和 Linux 之间传输文件
- 执行 Linux 系统维护任务

## 快速开始

### 基本用法

```bash
# 执行命令
wsl bash -lic "ls -la /home"

# 查看系统状态
wsl --list --verbose

# 管理进程
wsl bash -lic "ps aux | grep nginx"
```

### 脚本执行

```bash
# 执行构建脚本
wsl bash -lic "./build.sh"

# 执行部署脚本
wsl bash -lic "./deploy.sh --env production"
```

## 文件结构

```
wsl-operations/
├── SKILL.md                    # 主要技能定义
├── README.md                   # 本文件
├── references/
│   ├── commands.md            # WSL 命令完整参考
│   ├── path-conversion.md     # 路径转换详细说明
│   └── troubleshooting.md     # 常见问题解决
├── examples/
│   ├── basic-commands.md      # 基本命令示例
│   ├── process-management.md  # 进程管理示例
│   └── file-transfer.md      # 文件操作示例
└── scripts/
    ├── wsl-run.sh            # WSL 命令执行辅助脚本
    └── wsl-ps.sh             # WSL 进程查看辅助脚本
```

## 使用指南

### 1. 参考 SKILL.md

查看 `SKILL.md` 获取：
- 何时使用此技能
- 快速开始指南
- 核心功能说明

### 2. 查阅参考文档

- `references/commands.md` - 完整的 WSL 命令参考
- `references/path-conversion.md` - 路径转换详细说明
- `references/troubleshooting.md` - 故障排除指南

### 3. 查看示例

- `examples/basic-commands.md` - 基本命令使用示例
- `examples/process-management.md` - 进程管理实际场景
- `examples/file-transfer.md` - 文件传输操作示例

### 4. 使用辅助脚本

- `scripts/wsl-run.sh` - 简化命令执行
- `scripts/wsl-ps.sh` - 格式化进程信息

## 技术要求

- Windows 10/11 with WSL installed
- WSL 2 recommended
- Bash shell in WSL distribution
- Linux distribution: Ubuntu, Debian, Alpine, etc.

## 常见用例

### 开发场景

```bash
# 编译项目
wsl bash -lic "cd /path/to/project && make && make install"

# 运行测试
wsl bash -lic "npm test"
wsl bash -lic "python -m pytest"

# 代码检查
wsl bash -lic "eslint ."
wsl bash -lic "pylint *.py"
```

### 系统管理

```bash
# 系统更新
wsl bash -lic "apt update && apt upgrade -y"

# 服务管理
wsl bash -lic "systemctl restart docker"
wsl bash -lic "service nginx restart"

# 日志查看
wsl bash -lic "journalctl -u nginx -f"
```

## 贡献和反馈

如有问题或建议，请通过以下方式反馈：
- 提交 issue 到项目仓库
- 更新文档和示例
- 分享使用经验和技巧

## 版本历史

- v1.0.0 - 初始版本
  - 基础命令执行
  - 系统监控功能
  - 进程管理功能
  - 文件操作指南
  - WSL 管理功能
