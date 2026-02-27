# NemesisBot

<div align="center">

**轻量级个人 AI 助手**

一个超轻量、易部署的个人 AI Agent，支持多平台接入和强大的工具扩展能力。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Go Version](https://img.shields.io/badge/Go-1.25+-00ADD8?logo=go)](https://golang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macos-lightgrey)](https://github.com/276793422/NemesisBot)
[![Tests](https://img.shields.io/badge/tests-15%20passing-success)](https://github.com/276793422/NemesisBot)
[![Code](https://img.shields.io/badge/code-33K%20lines-blue)](https://github.com/276793422/NemesisBot)
[![Files](https://img.shields.io/badge/files-134%20modules-informational)](https://github.com/276793422/NemesisBot)

</div>

一定要看到最后，或者从后往前看吧。后面的都是最重要的。

---

## ✨ 特性

- **🪶 超轻量级** - 极简架构，资源占用极低，可在各种设备上运行
- **🔒 安全沙箱** - 工作目录隔离，文件操作过滤，保护系统安全
- **🤖 多 LLM 支持** - 兼容 Anthropic Claude、OpenAI、智谱 GLM、Groq、Gemini、vLLM、OpenRouter 等 10+ 主流模型
- **📱 多平台接入** - 支持 14 个通讯平台：Telegram、Discord、Slack、Line、QQ、飞书、钉钉、WhatsApp、OneBot、MaixCam、外部程序、Web 等
- **🔌 MCP 协议** - 支持 Model Context Protocol，可无缝扩展工具能力
- **⚡ 灵活配置** - JSON 配置文件，支持环境变量，易于定制
- **🔌 插件系统** - 动态加载安全模块和其他扩展
- **🔄 故障转移** - FallbackChain 自动切换 LLM，保证服务可用性
- **⏰ 定时任务** - 内置 Cron 支持，可设置定时提醒和任务
- **🛠️ 技能扩展** - 支持从 GitHub 安装社区技能
- **🌐 Web 界面** - 内置 Web 服务器，支持浏览器访问

---

## 🚀 快速开始

### 环境要求

- Go 1.25 或更高版本
- 操作系统：Windows

### 安装

#### 从源码编译

```bash
# 克隆仓库
git clone https://github.com/276793422/NemesisBot.git
cd NemesisBot

# 编译（Windows）
go build -ldflags "-X main.version=0.0.0.1 -X main.gitCommit=0.0.0.1 -X main.buildTime=2026/2/19 -X main.goVersion=1.25.7 -s -w" -o nemesisbot.exe ./nemesisbot/
```

#### 使用预编译版本（可选）

前往 [Releases](https://github.com/276793422/NemesisBot/releases) 页面下载适合你系统的二进制文件。（可以不用去，目前没有）

---

## ⚙️ 配置

### 0. 帮助

首次运行，可直接运行命令

```bash
nemesisbot.exe
```

然后可以看到有帮助内容

```bash
🤖 nemesisbot - Personal AI Assistant v0.0.0.1

Usage: nemesisbot <command>

Commands:
  onboard     Initialize nemesisbot configuration and workspace
  agent       Interact with the agent directly
  auth        Manage authentication (login, logout, status)
  gateway     Start nemesisbot gateway
  status      Show nemesisbot status
  channel     Manage communication channels (list, enable, disable, status)
  model       Manage LLM models (list, add, remove)
  cron        Manage scheduled tasks
  mcp         Manage MCP servers (list, add, remove, test)
  log         Manage LLM request logging
  migrate     Migrate from OpenClaw to NemesisBot
  skills      Manage skills (install, list, remove)
  version     Show version information
```

剩下的可以根据命令行提示一步一步设置，或者跟着下面流程。

### 1. 初始化配置文件

首次运行时，NemesisBot 会自动创建默认配置文件：

```bash
nemesisbot.exe onboard
```

配置文件位于：`~/.nemesisbot/config.json`

### 2. 配置 LLM 提供商

#### 自动配置

通过如下命令可以自动设置相关模型

```bash
nemesisbot model add --model zhipu/glm-4.7 --key xxx --base https://open.bigmodel.cn/api/paas/v4 --default
```

#### 手动配置

编辑 `config.json`，在 `model_list` 中配置你的 LLM：

##### 使用智谱 GLM（推荐国内用户）

```json
{
  "model_name": "glm-4.7",
  "model": "zhipu/glm-4.7",
  "api_key": "your-zhipu-api-key",
  "api_base": "https://open.bigmodel.cn/api/paas/v4"
}
```

### 3. 设置默认模型

在 `agents.defaults` 中设置默认使用的模型：

```json
{
  "agents": {
    "defaults": {
      "llm": "glm-4.7",  // 使用 model_name 字段
      "workspace": "~/.nemesisbot/workspace",
      "restrict_to_workspace": true
    }
  }
}
```

### 4. 配置通讯渠道（可选）

NemesisBot 支持 14 个通讯平台，以下是常用平台配置示例：

#### Telegram Bot

```json
{
  "channels": {
    "telegram": {
      "enabled": true,
      "token": "YOUR_TELEGRAM_BOT_TOKEN",
      "allow_from": ["YOUR_USER_ID"]
    }
  }
}
```

#### Discord Bot

```json
{
  "channels": {
    "discord": {
      "enabled": true,
      "token": "YOUR_DISCORD_BOT_TOKEN",
      "allow_from": []
    }
  }
}
```

#### Slack

```json
{
  "channels": {
    "slack": {
      "enabled": true,
      "bot_token": "xoxb-YOUR-BOT-TOKEN",
      "app_token": "xapp-YOUR-APP-TOKEN",
      "allow_from": []
    }
  }
}
```

#### 飞书

```json
{
  "channels": {
    "feishu": {
      "enabled": true,
      "app_id": "YOUR_APP_ID",
      "app_secret": "YOUR_APP_SECRET",
      "encrypt_key": "YOUR_ENCRYPT_KEY",
      "verification_token": "YOUR_VERIFICATION_TOKEN",
      "allow_from": []
    }
  }
}
```

#### QQ 机器人

```json
{
  "channels": {
    "qq": {
      "enabled": true,
      "app_id": "YOUR_QQ_APP_ID",
      "app_secret": "YOUR_QQ_APP_SECRET",
      "allow_from": []
    }
  }
}
```

#### OneBot (CQHTTP / NapCat / LLONE)

```json
{
  "channels": {
    "onebot": {
      "enabled": true,
      "ws_url": "ws://127.0.0.1:3001",
      "access_token": "",
      "reconnect_interval": 5,
      "group_trigger_prefix": [],
      "allow_from": []
    }
  }
}
```

#### 外部程序通道（External）

外部程序通道允许您连接自定义的输入/输出程序，实现与外部应用的双向通信：

```json
{
  "channels": {
    "external": {
      "enabled": true,
      "input_exe": "C:\\Tools\\input.exe",
      "output_exe": "C:\\Tools\\output.exe",
      "chat_id": "external:main",
      "allow_from": [],
      "sync_to_web": true,
      "web_session_id": ""
    }
  }
}
```

**参数说明**：
- `input_exe`: 输入程序的完整路径，程序从 stdout 读取用户输入
- `output_exe`: 输出程序的完整路径，程序从 stdin 接收 AI 响应
- `chat_id`: 会话标识符，格式为 `external:xxx`
- `sync_to_web`: 是否同步消息到 Web 界面
- `web_session_id`: 指定 Web 会话 ID（空值=广播到所有会话）

**工作流程**：
```
输入程序 (stdout) → NemesisBot → LLM → NemesisBot → 输出程序
                                          ↓
                                    Web 界面（同步显示）
```

**使用场景**：
- 集成第三方桌面应用
- 自定义输入/输出处理
- 特殊格式转换
- 硬件设备通信

**其他支持的平台**：钉钉、Line、WhatsApp、MaixCam 等

---

## 📖 使用方法

### 命令行模式

直接运行程序，在命令行中与 AI 对话：

```bash
nemesisbot.exe agent
```

不建议使用命令行模式。可以使用 web 通道。

### Web 界面

通过 gateway 方式启动后

```
nemesisbot.exe gateway
```

启用 Web 渠道后，通过浏览器访问：

```
http://localhost:8080
```

#### Web 认证配置（推荐）

为了安全性，建议设置访问密钥。有三种方式配置：

##### 方式一：交互式命令（推荐手动使用）

```bash
# 交互式设置 token（安全）
nemesisbot channel web auth
```

**优点**：
- ✅ Token 不在命令行参数中暴露
- ✅ 避免在进程列表和终端历史中泄露
- ✅ 适合日常手动配置

##### 方式二：命令行直接设置（推荐脚本/自动化）

```bash
# 快速设置 token
nemesisbot channel web auth set my-secret-token
nemesisbot channel web auth set 276793422

# 查看当前 token
nemesisbot channel web auth get
```

**优点**：
- ✅ 方便快捷，一次输入即可
- ✅ 适合脚本和自动化场景
- ⚠️ Token 可能在进程列表和 shell 历史中可见

##### 方式三：编辑配置文件

编辑 `config.json`，在 `channels.web` 中配置：

```json
{
  "channels": {
    "web": {
      "enabled": true,
      "host": "0.0.0.0",
      "port": 8080,
      "path": "/ws",
      "auth_token": "your-secret-key-here",  // 设置密钥后启用认证
      "session_timeout": 3600
    }
  }
}
```

##### 方式四：使用环境变量

```bash
export NEMESISBOT_CHANNELS_WEB_AUTH_TOKEN="your-secret-key-here"
```

**注意**：环境变量方式可能在进程列表中暴露 token。

#### 其他管理命令

```bash
# 查看当前认证状态
nemesisbot channel web status

# 查看详细配置
nemesisbot channel web config

# 清除认证 token
nemesisbot channel web clear
```

#### 使用说明

设置密钥后：
- 首次访问需要输入密钥登录
- 选中"记住我"选项后，密钥保存在浏览器本地，下次访问自动登录
- 点击"退出"按钮可以清除密钥并注销

#### 安全建议

- 使用强密钥（至少 16 位随机字符）
- 生产环境建议使用 HTTPS
- 定期更换密钥以提高安全性
- 确保配置文件权限安全（Unix/Mac 建议 0600）
- 手动配置优先使用交互式命令
- 脚本/自动化可以使用命令行模式，但注意清理历史记录

### 聊天平台

配置好对应平台的 Bot 后，直接在 Telegram/飞书 等平台与 AI 对话。

### 常用命令

```bash
# 查看版本
nemesisbot.exe

# 查看帮助
nemesisbot.exe --help

# 指定配置文件
nemesisbot.exe --config /path/to/config.json

# 设置模型
nemesisbot.exe agent set llm claude-sonnet-4

# 安装技能
nemesisbot.exe skills install <github-repo>

# 列出所有命令
nemesisbot.exe --help
```

---

## 🔐 安全说明

### 工作目录隔离

默认情况下，NemesisBot 启用了工作目录限制（`restrict_to_workspace: true`），这意味着：

- ✅ Bot 只能访问 `workspace` 目录及其子目录
- ✅ 保护系统文件和敏感目录不被访问
- ✅ 防止误操作导致的数据丢失

### 修改工作目录

如需访问其他目录，可以在配置中修改：

```json
{
  "agents": {
    "defaults": {
      "workspace": "C:/MyProjects",
      "restrict_to_workspace": true
    }
  }
}
```

⚠️ **警告**：将 `restrict_to_workspace` 设为 `false` 会解除所有限制，请谨慎使用！

✅ **提示**：虽然解除了限制，但是项目提供第二套安全机制，设置 "security": { "enabled": false } 为true。则会开启更强大的安全机制。

---

## 📁 项目结构

```
NemesisBot/
├── nemesisbot/            # 主程序入口 (CLI 命令)
│   └── main.go           # 命令行接口实现
├── module/               # 核心模块 (24 个模块)
│   ├── agent/            # Agent 核心引擎 (loop, context, memory)
│   ├── channels/         # 通讯渠道 (13 个平台实现)
│   ├── providers/        # LLM 提供商 (factory, fallback, cooldown)
│   ├── tools/            # 内置工具 (文件, shell, web, 硬件)
│   ├── security/         # 安全审计 (auditor, middleware, plugin)
│   ├── mcp/              # MCP 协议支持
│   ├── bus/              # 消息总线
│   ├── plugin/           # 插件系统
│   ├── config/           # 配置管理
│   ├── session/          # 会话管理
│   ├── routing/          # 消息路由
│   ├── auth/             # OAuth 认证 (PKCE)
│   ├── skills/           # 技能系统
│   ├── cron/             # 定时任务
│   ├── devices/          # 设备检测 (USB)
│   ├── heartbeat/        # 心跳机制
│   ├── health/           # 健康检查
│   ├── state/            # 状态管理
│   ├── voice/            # 语音转写
│   ├── web/              # Web 服务器 (WebSocket)
│   ├── logger/           # 日志系统
│   ├── migrate/          # 配置迁移
│   ├── utils/            # 工具函数
│   └── constants/        # 常量定义
├── test/                 # 测试文件
│   ├── unit/             # 单元测试 (config, routing, security, tools, utils)
│   └── mcp/              # MCP 协议测试
├── config/               # 配置文件示例
│   ├── config.default.json
│   └── config.mcp.default.json
├── default/              # 默认身份文件
│   ├── IDENTITY.md       # AI 身份定义
│   ├── SOUL.md           # 角色性格
│   └── USER.md           # 用户信息
├── docs/                 # 项目文档 (49 个文档)
│   └── *.md              # 其他技术文档
├── workspace/            # 默认工作目录
│   ├── AGENT.md          # Agent 文档
│   ├── BOOTSTRAP.md      # 初始化引导
│   ├── IDENTITY.md       # 身份信息
│   ├── SOUL.md           # 角色定义
│   ├── USER.md           # 用户信息
│   ├── TOOLS.md          # 工具参考
│   ├── BOOT.md           # 启动流程
│   ├── HEARTBEAT.md      # 心跳机制
│   ├── memory/           # 持久化内存文件
│   ├── scripts/          # 安装脚本
│   │   ├── install-clawhub-skill.bat
│   │   └── install-clawhub-skill.sh
│   └── skills/           # 技能安装目录
│       ├── github/       # GitHub 技能
│       ├── weather/      # 天气技能
│       ├── summarize/    # 摘要技能
│       └── test-skill/   # 测试技能
├── build.bat             # Windows 构建脚本
├── go.mod / go.sum       # Go 依赖管理
├── README.md             # 项目文档
└── LICENSE               # MIT 许可证
```

**技术栈**:
- **语言**: Go 1.25+
- **依赖**: 22 个直接依赖，33 个间接依赖
- **协议**: MIT License (特定限制条款)

---

## 📊 项目状态

### 代码质量

| 指标 | 数值 | 说明 |
|------|------|------|
| **代码规模** | 137 文件 / 34,488 行 | 模块化架构，代码精简 |
| **测试覆盖** | 23 测试用例 / 7 模块 | 基础功能测试，持续改进中 |
| **测试状态** | ✅ 全部通过 | config, routing, security, tools, utils |
| **导出接口** | 190 个函数 / 14 个接口 | 良好的抽象设计 |
| **资源管理** | 25 处 defer Close() | 确保资源正确释放 |
| **并发安全** | 31 处锁保护 | RWMutex/Mutex 保证线程安全 |
| **核心模块** | 24 个模块 | agent, channels, providers, tools, security 等 |

### 支持平台

| 类型 | 数量 | 说明 |
|------|------|------|
| **通讯渠道** | 14 个平台 | Telegram, Discord, Slack, Line, QQ, 飞书, 钉钉, WhatsApp, OneBot, MaixCam, External, Web 等 |
| **LLM 提供商** | 10+ 服务 | Anthropic, OpenAI, 智谱, Groq, Gemini, vLLM, OpenRouter, Moonshot, Ollama, NVIDIA 等 |
| **内置工具** | 20+ 工具 | 文件操作, Shell 执行, Web 搜索, Cron 定时, 硬件交互 (I2C/SPI), MCP 协议等 |

### 架构特点

```
module/
├── agent/          # Agent 核心引擎 - 迭代循环, 内存管理, 会话路由
├── channels/       # 多平台适配 - 统一消息接口, 13 个平台实现
├── providers/      # LLM 抽象层 - 工厂模式, Fallback 链, 冷却机制
├── tools/          # 工具系统 - 可插拔注册, 安全沙箱
├── security/       # 安全审计 - ABAC 策略引擎, 操作日志
├── mcp/            # MCP 协议 - Model Context Protocol 支持
├── bus/            # 消息总线 - 异步通信, 解耦设计
├── heartbeat/      # 心跳机制 - 系统存活监控
├── health/         # 健康检查 - 服务状态报告
├── state/          # 状态管理 - 持久化状态
├── plugin/         # 插件系统 - 动态扩展能力
└── ...             # 其他模块 (config, session, routing, skills, cron, web, logger, migrate, utils, constants)
```

**设计优势**:
- ✅ 依赖倒置 - 清晰的抽象层
- ✅ 插件化 - 安全模块可作为插件动态加载
- ✅ 故障转移 - FallbackChain 自动切换 LLM
- ✅ 工作目录隔离 - 保护系统安全
- ✅ 跨平台 - Windows/Linux/macOS 全支持

---

## 🛠️ 高级功能

### MCP 服务器集成

配置 MCP 服务器以扩展工具能力：

编辑 `config.mcp.json`：

```json
{
  "mcp": {
    "enabled": true,
    "servers": [
      {
        "name": "filesystem",
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "C:/allowed/path"],
        "env": {}
      }
    ]
  }
}
```

### 技能系统

安装社区贡献的技能：

```bash
# 从 GitHub 安装
nemesisbot.exe skills install username/repository

# 从 ClawHub 安装
nemesisbot.exe skills install-clawhub author skill-name

# 列出已安装技能
nemesisbot.exe skills list
```

### 定时任务

使用 Cron 功能设置定时任务：

```
/cron add "0 9 * * *" "发送每日提醒"
```

---

## 📝 更新日志

### v0.0.0.2 (2026-02-28)

**新增功能**:
- ✅ **外部程序通道** - 支持连接自定义输入/输出程序
  - 通过 stdin/stdout 与外部程序双向通信
  - 支持同步消息到 Web 界面
  - 完整的会话历史持久化
  - 独立的配置和权限管理
- ✅ **测试覆盖** - 添加外部通道的单元测试和集成测试
  - 8 个单元测试用例全部通过
  - 8 个集成测试框架就绪

**代码质量**:
- ✅ 新增 3 个文件（external.go, 单元测试, 集成测试）
- ✅ 约 1,300 行新代码
- ✅ 100% 测试通过率

**文档更新**:
- ✅ 更新配置文件说明
- ✅ 添加外部通道使用示例
- ✅ 更新支持平台数量（13 → 14）

### v0.0.0.1 (2026-02-23)

**核心功能**:
- ✅ 多 LLM 提供商支持（Anthropic, OpenAI, 智谱, Groq, Gemini 等）
- ✅ 14 个通讯平台接入（Telegram, Discord, 飞书, QQ, 钉钉，外部程序等）
- ✅ 外部程序通道，支持自定义输入/输出程序集成
- ✅ MCP 协议支持，可扩展工具能力
- ✅ 安全沙箱机制，工作目录隔离
- ✅ 插件系统，支持动态扩展
- ✅ Web 界面和 WebSocket 支持
- ✅ 定时任务（Cron）功能
- ✅ 技能系统，可安装社区插件
- ✅ OAuth 2.0 认证系统（PKCE）
- ✅ 语音转写支持
- ✅ USB 设备监控
- ✅ 心跳机制和健康检查
- ✅ 状态管理系统
- ✅ 配置迁移工具（OpenClaw → NemesisBot）

**代码质量**:
- ✅ 134 个源文件，33,007 行代码
- ✅ 15 个单元测试全部通过
- ✅ 190 个导出函数，14 个接口
- ✅ 良好的并发安全性和资源管理
- ✅ 24 个核心模块
- ✅ 49 个技术文档

**架构改进**:
- ✅ 模块化设计，清晰的职责分离
- ✅ FallbackChain 自动故障转移
- ✅ 消息总线解耦通道与 Agent
- ✅ 安全审计系统（ABAC 策略引擎）
- ✅ 身份系统（IDENTITY.md, SOUL.md, USER.md）

---

## 🗺️ 发展路线

### 已完成 ✅
- [x] 多 LLM 提供商支持
- [x] 14 个通讯平台接入（包括外部程序通道）
- [x] 外部程序通道（自定义输入/输出程序）
- [x] MCP 协议支持
- [x] 安全沙箱机制
- [x] 插件系统
- [x] Web 界面
- [x] 定时任务
- [x] 技能系统
- [x] 心跳机制
- [x] 健康检查
- [x] 状态管理
- [x] 配置迁移工具 (从 OpenClaw)
- [x] 单元测试框架

### 计划中 🚧
- [ ] 容器化支持 (Dockerfile, Kubernetes)（没计划）
- [ ] API 文档自动生成
- [ ] 监控指标 (Prometheus)
- [ ] 集成测试套件（没计划）
- [ ] 压力测试（没计划）
- [ ] 安全加固 (命令白名单)
- [ ] 配置热重载
- [ ] 分布式追踪 (OpenTelemetry)

### 长期目标 🎯
- [ ] 微服务架构
- [ ] 水平扩展支持
- [ ] 服务网格集成（没计划）
- [ ] 多租户支持（没计划）

---

## 🤝 参与贡献

欢迎贡献代码、报告问题或提出建议！

不贡献代码也没关系，我也不是自己写的，都是 AI 自己开发。

### 如何贡献

1. Fork 本仓库
2. 创建特性分支 (`git checkout -b feature/AmazingFeature`)
3. 提交更改 (`git commit -m 'Add some AmazingFeature'`)
4. 推送到分支 (`git push origin feature/AmazingFeature`)
5. 开启 Pull Request

### 开发规范

- 遵循 Go 语言代码规范
- 添加单元测试覆盖新功能
- 更新相关文档
- 确保所有测试通过

---

## 🐛 问题反馈

如果您遇到问题或有功能建议，请：

1. 查看已有 [Issues](https://github.com/276793422/NemesisBot/issues)
2. 搜索是否已有类似问题
3. 创建新 Issue，包含：
   - 详细的问题描述
   - 复现步骤
   - 错误日志
   - 环境信息（OS, Go 版本等）

---

## 📄 许可证

本项目采用 MIT 许可证 - 查看 [LICENSE](LICENSE) 文件了解详情。

一定要先看 LICENSE ，因为我的授权有例外。

---

## 🙏 致谢

这里才是最重要的。

本项目灵感来源于 [OpenClaw](https://github.com/openclaw/openclaw)

后于12日准备开发此项目时，用AI仿照 [nanobot](https://github.com/HKUDS/nanobot) 项目制作了一个 Golang 框架。

于16日发现了 [PicoClaw](https://github.com/sipeed/picoclaw) 项目后，让AI去学习此项目的代码，结果AI把大部分内容都直接抄来了，包括用不上的。

行吧，感谢如上项目，感谢各位老铁在百忙之中使用AI写出代码，供我的AI学习。

如果如上两位 ID 把我也加例外了，记得告诉我，我就乖乖地让AI把从你们那边抄来的代码，都去掉。可能费点事，但是我觉得我能做到。

---

<div align="center">

**⭐ 如果这个项目对你有帮助，请给一个 Star，不给也没关系，只要你不是特定人群，就感谢你！**

Made with ❤️ by NemesisBot contributors

</div>
