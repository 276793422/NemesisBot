# --local 参数快速参考

## 一行命令快速开始

```batch
mkdir mybot && cd mybot && nemesisbot.exe --local onboard default && nemesisbot.exe --local gateway
```

## 常用命令示例

### 初始化实例

```batch
# 使用 --local 参数
nemesisbot.exe --local onboard default

# 或者先创建目录触发自动检测
mkdir .nemesisbot
nemesisbot.exe onboard default
```

### 配置模型

```batch
# 显式指定当前目录
nemesisbot.exe --local model add --model zhipu/glm-4.7-flash --key YOUR_KEY

# 自动检测模式
nemesisbot.exe model add --model zhipu/glm-4.7-flash --key YOUR_KEY
```

### 启动服务

```batch
# 前台运行
nemesisbot.exe --local gateway

# 后台运行
start /B nemesisbot.exe --local gateway > bot.log 2>&1
```

### 查看状态

```batch
# 查看 bot 状态
nemesisbot.exe --local status

# 查看已配置模型
nemesisbot.exe --local model list

# 查看通道状态
nemesisbot.exe --local channel list
```

## 多实例管理

### 创建多个 bot

```batch
REM Bot 1
mkdir bot1
cd bot1
..\nemesisbot.exe --local onboard default
start ..\nemesisbot.exe --local gateway

REM Bot 2
cd ..\bot2
..\nemesisbot.exe --local onboard default
start ..\nemesisbot.exe --local gateway
```

### 批量启动脚本

```batch
@echo off
REM 启动所有 bot 实例
for /d %%D in (C:\Bots\*) do (
    cd "%%D"
    if exist ".nemesisbot" (
        echo Starting %%~nxD...
        start "%%~nxD" ..\nemesisbot.exe gateway
    )
)
```

## 优先级速查

| 优先级 | 方式 | 命令 |
|--------|------|------|
| 🥇 最高 | `--local` | `nemesisbot --local <cmd>` |
| 🥈 其次 | 环境变量 | `set NEMESISBOT_HOME=./.nemesisbot` |
| 🥉 自动 | 自动检测 | `mkdir .nemesisbot && nemesisbot <cmd>` |
| 🏅 默认 | 用户目录 | `nemesisbot <cmd>` |

## 故障排查

### 问题：配置没有在当前目录创建

**解决**：确保使用了 `--local` 参数或创建了 `.nemesisbot` 目录

```batch
# 检查是否使用 --local
nemesisbot.exe --local version

# 检查是否有 .nemesisbot 目录
dir .nemesisbot
```

### 问题：多个实例端口冲突

**解决**：修改配置文件中的端口

```json
{
  "channels": {
    "web": {"port": 49000},
    "websocket": {"port": 49001}
  }
}
```

### 问题：如何查看使用的是哪个配置？

**解决**：使用 `--local` 会显示提示信息

```batch
nemesisbot.exe --local version
# 输出: 📍 Local mode enabled: using ./.nemesisbot
```

## 快速测试

```batch
REM 5分钟快速测试

REM 1. 创建测试实例
mkdir test-quick
cd test-quick

REM 2. 初始化
..\nemesisbot.exe --local onboard default

REM 3. 验证配置位置
dir .nemesisbot

REM 4. 清理
cd ..
rmdir /s /q test-quick
```
