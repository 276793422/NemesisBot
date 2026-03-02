# 多实例部署指南

本文档详细说明如何在同一台设备上部署和管理多个独立的 NemesisBot 实例。

## 目录

- [使用场景](#使用场景)
- [部署方式](#部署方式)
- [完整示例](#完整示例)
- [常见问题](#常见问题)

---

## 使用场景

### 场景 1：开发/生产环境隔离

```
D:/Projects/
├── nemesisbot-dev/     # 开发环境
│   └── .nemesisbot/
└── nemesisbot-prod/     # 生产环境
    └── .nemesisbot/
```

### 场景 2：多客户部署

```
C:/Bots/
├── client-a/           # 客户 A 的 bot
│   └── .nemesisbot/
├── client-b/           # 客户 B 的 bot
│   └── .nemesisbot/
└── client-c/           # 客户 C 的 bot
    └── .nemesisbot/
```

### 场景 3：功能隔离

```
C:/Bots/
├── discord-bot/        # 专门服务 Discord
├── telegram-bot/       # 专门服务 Telegram
└── web-assistant/      # Web 界面助手
```

---

## 部署方式

### 方式 A：--local 参数（推荐新手）

**优点**：明确、直观、不易出错

#### 步骤 1：创建实例目录

```batch
mkdir C:\MyBots\bot1
cd C:\MyBots\bot1
```

#### 步骤 2：初始化配置

```batch
nemesisbot.exe --local onboard default
```

这会在当前目录创建 `.nemesisbot/` 目录，包含：
- `config.json` - 主配置
- `config.mcp.json` - MCP 配置
- `config.security.json` - 安全配置

#### 步骤 3：配置 API Key

```batch
nemesisbot.exe --local model add --model zhipu/glm-4.7-flash --key YOUR_API_KEY --default
```

#### 步骤 4：启动服务

```batch
REM 前台运行
nemesisbot.exe --local gateway

REM 后台运行（Windows）
start /B nemesisbot.exe --local gateway > bot1.log 2>&1
```

#### 步骤 5：验证配置位置

```batch
REM 查看配置文件
dir .nemesisbot

REM 查看工作空间
dir .nemesisbot\workspace
```

---

### 方式 B：自动检测模式（推荐长期使用）

**优点**：初始化后无需额外参数，更简洁

#### 步骤 1：创建实例目录和标记

```batch
mkdir C:\MyBots\bot1
cd C:\MyBots\bot1

REM 创建 .nemesisbot 目录（触发自动检测）
mkdir .nemesisbot
```

#### 步骤 2：初始化配置

```batch
REM 无需 --local，自动检测到 .nemesisbot
nemesisbot.exe onboard default
```

#### 步骤 3：配置和启动

```batch
REM 配置 API Key
nemesisbot.exe model add --model zhipu/glm-4.7-flash --key YOUR_API_KEY --default

REM 启动服务（无需 --local）
nemesisbot.exe gateway
```

#### 步骤 4：创建启动脚本

创建 `start.bat`：

```batch
@echo off
REM Bot 1 启动脚本
cd /d C:\MyBots\bot1
nemesisbot.exe gateway
```

---

### 方式 C：环境变量（适合脚本自动化）

#### 步骤 1：创建实例目录

```batch
mkdir C:\MyBots\bot1
```

#### 步骤 2：创建初始化脚本

创建 `init.bat`：

```batch
@echo off
cd /d C:\MyBots\bot1
set NEMESISBOT_HOME=%CD%\.nemesisbot

nemesisbot.exe onboard default
nemesisbot.exe model add --model zhipu/glm-4.7-flash --key YOUR_API_KEY --default

echo.
echo 初始化完成！
echo 配置位置: %NEMESISBOT_HOME%
pause
```

#### 步骤 3：创建启动脚本

创建 `start.bat`：

```batch
@echo off
cd /d C:\MyBots\bot1
set NEMESISBOT_HOME=%CD%\.nemesisbot

echo Starting Bot 1...
echo Config: %NEMESISBOT_HOME%
nemesisbot.exe gateway
```

---

## 完整示例

### 示例 1：快速测试两个 bot 实例

```batch
REM === Bot 1 ===
mkdir C:\TestBots\bot1
cd C:\TestBots\bot1

REM 使用 --local 快速初始化
..\..\nemesisbot.exe --local onboard default
..\..\nemesisbot.exe --local model add --model zhipu/glm-4.7-flash --key KEY1 --default

REM === Bot 2 ===
cd C:\TestBots
mkdir bot2
cd bot2

..\..\nemesisbot.exe --local onboard default
..\..\nemesisbot.exe --local model add --model zhipu/glm-4.7-flash --key KEY2 --default

REM === 启动 Bot 1 ===
cd C:\TestBots\bot1
start "Bot1" ..\..\nemesisbot.exe --local gateway

REM === 启动 Bot 2 ===
cd C:\TestBots\bot2
start "Bot2" ..\..\nemesisbot.exe --local gateway
```

### 示例 2：生产环境多实例部署

#### 目录结构

```
C:/Services/NemesisBot/
├── bin/
│   └── nemesisbot.exe
├── instances/
│   ├── main-bot/
│   │   ├── .nemesisbot/
│   │   ├── start.bat
│   │   └── stop.bat
│   ├── test-bot/
│   │   ├── .nemesisbot/
│   │   ├── start.bat
│   │   └── stop.bat
│   └── dev-bot/
│       ├── .nemesisbot/
│       ├── start.bat
│       └── stop.bat
└── README.txt
```

#### 主控脚本

创建 `C:/Services/NemesisBot/manage.bat`：

```batch
@echo off
REM NemesisBot 多实例管理脚本

if "%1"=="" goto usage
if "%1"=="start" goto start
if "%1"=="stop" goto stop
if "%1"=="status" goto status
goto usage

:usage
echo 用法:
echo   manage.bat start [instance]  启动指定实例
echo   manage.bat stop [instance]   停止指定实例
echo   manage.bat status            查看所有实例状态
goto end

:start
if "%2"=="" (
    echo 错误: 请指定实例名称
    goto usage
)
set INSTANCE_DIR=instances\%2
if not exist "%INSTANCE_DIR%\.nemesisbot" (
    echo 错误: 实例 %2 不存在或未初始化
    goto end
)
cd /d "%~dp0%INSTANCE_DIR%"
start "%2%" ..\..\bin\nemesisbot.exe gateway
echo 实例 %2 已启动
goto end

:stop
taskkill /FI "WINDOWTITLE eq %2*" /IM nemesisbot.exe 2>nul
if errorlevel 1 (
    echo 实例 %2 未运行或已停止
) else (
    echo 实例 %2 已停止
)
goto end

:status
echo 实例状态:
for /d %%D in (instances\*) do (
    echo   %%~nxD:
    tasklist /FI "WINDOWTITLE eq %%~nxD* IMAGENAME eq nemesisbot.exe" 2>nul | find "nemesisbot.exe" >nul
    if errorlevel 1 (
        echo     状态: 未运行
    ) else (
        echo     状态: 运行中
    )
)
goto end

:end
```

#### 使用方法

```batch
REM 初始化所有实例
cd C:\Services\NemesisBot\instances\main-bot
..\..\bin\nemesisbot.exe --local onboard default

cd ..\test-bot
..\..\bin\nemesisbot.exe --local onboard default

REM 启动主实例
manage.bat start main-bot

REM 查看状态
manage.bat status

REM 停止实例
manage.bat stop main-bot
```

---

## 常见问题

### Q1: 如何确认 bot 使用的是哪个配置？

**A**: 查看日志输出：

```batch
REM 使用 --local 时会显示
nemesisbot.exe --local gateway
# 输出: 📍 Local mode enabled: using ./.nemesisbot

REM 查看实际使用的配置路径
nemesisbot.exe --local status
```

### Q2: 多个实例会冲突吗？

**A**: 不会，如果配置正确的话。需要确保：

1. **不同的配置目录** - 每个实例有自己的 `.nemesisbot`
2. **不同的端口** - 修改 `config.json` 中的端口配置

```json
{
  "channels": {
    "web": {
      "port": 49000  // Bot 1 使用 49000
    },
    "websocket": {
      "port": 49001
    }
  }
}
```

Bot 2 改为 49002/49003，以此类推。

### Q3: --local 和环境变量哪个优先级高？

**A**: `--local` 最高。优先级：

```
--local > NEMESISBOT_HOME > 自动检测 > ~/.nemesisbot
```

### Q4: 如何备份和迁移 bot 实例？

**A**: 直接复制整个实例目录：

```batch
REM 备份
xcopy C:\MyBots\bot1 D:\Backup\bot1\ /E /I /H /R /Y

REM 迁移到新机器
xcopy D:\Backup\bot1\ E:\NewBots\bot1\ /E /I /H /R /Y
cd E:\NewBots\bot1
nemesisbot.exe gateway
```

### Q5: 如何批量管理多个实例？

**A**: 使用脚本或任务计划程序：

```batch
REM 创建启动所有实例的脚本
for /d %%D in (C:\MyBots\*) do (
    cd "%%D"
    start "%%~nxD" ..\nemesisbot.exe --local gateway
)
```

或者使用 Windows 任务计划程序设置开机自启。

---

## 最佳实践

### 1. 目录命名规范

```
C:/Bots/
├── prod-main/          # 生产环境主实例
├── prod-test/          # 生产环境测试实例
├── dev-experimental/   # 开发环境
└── temp-testing/       # 临时测试
```

### 2. 配置管理

每个实例应该有：

- 独立的配置文件
- 独立的 workspace
- 独立的日志目录
- 独立的端口配置

### 3. 监控和日志

```batch
REM 启动时输出日志到文件
nemesisbot.exe --local gateway > bot.log 2>&1

REM 查看日志
type bot.log
```

### 4. 资源限制

每个实例会占用一定的内存和 CPU，建议：
- 监控资源使用
- 限制同时运行的实例数量
- 使用配置文件限制并发（`concurrent_request_mode`）

---

## 相关文档

- [README.md](../README.md) - 主项目文档
- [安全说明](../README.md#-安全说明) - 安全机制说明
- [环境变量配置](../README.md#-环境变量) - 环境变量列表
