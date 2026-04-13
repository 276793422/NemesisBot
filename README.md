# NemesisBot

<div align="center">

**安全第一的 AI 智能管家**

一个轻量级、高安全性的个人 AI 助手，专注于安全保障和拟真使用体验。

（这句话是人写的）以下内容大部分都是AI胡掰，可信可不信，凡是AI说“稳定”的地方，可靠度都和投硬币差不多。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Go Version](https://img.shields.io/badge/Go-1.25+-00ADD8?logo=go)](https://golang.org/)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macos-lightgrey)](https://github.com/276793422/NemesisBot)

</div>

---

## 核心特色

### 🔐 安全保障体系

**企业级安全审计框架** - 不是简单的文件隔离，而是完整的安全管控系统

- **ABAC 策略引擎** - 基于属性的访问控制，细粒度权限管理
- **操作审计日志** - 完整记录所有危险操作，可追溯、可审计
- **安全中间件** - 对文件、进程、网络、硬件操作进行实时监控
- **分级危险等级** - LOW / MEDIUM / HIGH / CRITICAL 四级风险控制
- **工作目录隔离** - 默认启用沙箱模式，保护系统安全

**安全 ≠ 功能限制**
- ✅ 在安全保障的前提下，提供完整的工具能力
- ✅ 可配置的安全策略，满足不同使用场景
- ✅ 实时监控和拦截，防止意外损害
- ✅ **工具执行增强** - Windows PowerShell curl 兼容性支持

### 🌐 分布式集群

**多节点协同 - 让多个 AI 一起工作**（已稳定可用）

- **角色分离** - manager / coordinator / worker / observer / standby
- **业务分类** - design / development / testing / ops / deployment / analysis / general
- **自定义标签** - 灵活的多维度分类体系
- **UDP 自动发现** - 局域网内自动发现其他节点
- **异步 RPC 通信** - 节点间非阻塞远程调用，支持多步骤工具链
- **续行快照模式** - LLM 上下文在异步调用点保存，回调到达后自动续行
- **静态+动态配置** - 手动配置已知节点，自动发现新节点
- **连接池管理** - 高效的连接复用和管理
- **限流保护** - 防止 RPC 调用过载

**使用场景**
- 🏢 专业分工 - 设计、开发、测试各司其职
- 🔄 任务协作 - 管理者协调，工作者执行
- 📊 负载均衡 - 根据能力和标签智能路由

---

## 快速开始

### 安装

```bash
# 克隆仓库
git clone https://github.com/276793422/NemesisBot.git
cd NemesisBot

# 编译（推荐使用 Makefile）
make build                    # Linux/macOS
make build-windows            # Windows (在 WSL 中)

# 或使用 PowerShell（Windows）
.\build.ps1

# 或使用批处理脚本（Windows）
build.bat
```

### 构建系统说明

NemesisBot 支持三种构建方式：

| 构建方式 | 适用平台 | 功能完整性 | 推荐度 |
|---------|---------|-----------|--------|
| **Makefile** | Linux/macOS/WSL | ⭐⭐⭐⭐⭐ | 🔥推荐 |
| **build.ps1** | Windows | ⭐⭐⭐⭐ | 🔥推荐 |
| **build.bat** | Windows | ⭐⭐⭐ | ✅可用 |

**快速命令**：

```bash
# Makefile (Linux/macOS/WSL)
make help                    # 查看所有命令
make build                   # 构建当前平台
make build-all               # 构建所有平台
make test                    # 运行测试
make run                     # 构建并运行

# build.ps1 (Windows PowerShell)
.\build.ps1 -Help            # 查看所有命令
.\build.ps1                  # 构建当前平台
.\build.ps1 -AllPlatforms    # 构建所有平台
.\build.ps1 -Test            # 运行测试
```

### 初始化

```bash
# 自动配置（推荐）
nemesisbot.exe onboard default
```

这会创建：
- 主配置文件：`~/.nemesisbot/config.json`
- 工作空间：`~/.nemesisbot/workspace/`
- 安全策略配置：`~/.nemesisbot/workspace/config/config.security.json`
- 身份文件：`IDENTITY.md`, `SOUL.md`, `USER.md`
- 集群配置：`cluster/peers.toml`

身份信息为预设身份。

### 配置 LLM

```bash
# 添加模型（智谱 GLM 推荐）
nemesisbot model add --model zhipu/glm-4.7 --key YOUR_API_KEY --default
```

### 启动服务

```bash
# 启动网关
nemesisbot gateway

# 访问 Web 界面
# 浏览器打开：http://127.0.0.1:49000
# 默认访问密钥：276793422
```

---

## 安全配置

### 工作目录隔离

默认配置（推荐）：
```json
{
  "agents": {
    "defaults": {
      "workspace": "~/.nemesisbot/workspace",
      "restrict_to_workspace": true
    }
  },
  "security": {
    "enabled": true
  }
}
```

这意味着：
- ✅ Bot 只能访问 workspace 目录，其他风险由 security 模块处理
- ✅ 所有文件操作受安全策略控制
- ✅ 危险操作需要审批或会被拦截

`restrict_to_workspace` 字段限制程序仅可访问 workspace 内部。

### 安全策略配置

编辑 `~/.nemesisbot/workspace/config/config.security.json`：

### 操作类型与危险等级

| 级别 | 操作类型 | 说明 |
|------|----------|------|
| **CRITICAL** | process_exec, process_kill, registry_write, system_shutdown | 最高风险 |
| **HIGH** | file_write, file_delete, dir_create, dir_delete, process_spawn | 高风险 |
| **MEDIUM** | file_edit, file_append, registry_read, network_download | 中等风险 |
| **LOW** | file_read, dir_list, network_request, hardware_i2c | 低风险 |

---

## 集群管理

### 初始化集群节点

```bash
# 初始化为设计类管理者
nemesisbot cluster init \
  --name "Design Lead Bot" \
  --role manager \
  --category design \
  --tags "production,senior"

# 初始化为开发类工作者
nemesisbot cluster init \
  --name "Dev Worker 1" \
  --role worker \
  --category development \
  --tags "backend,junior"
```

### 节点角色

角色原则上可以设置为任何你想要的角色。如 PM、DEV、UX、TEST 等等。

### 业务分类

业务部分原则上可以设置任何你认为它能做的业务。

### 管理命令

```bash
# 查看集群状态
nemesisbot cluster status

# 查看节点信息
nemesisbot cluster info

# 修改节点信息
nemesisbot cluster info --role coordinator --category development

# 添加已知节点
nemesisbot cluster peers add \
  --id "node-dev-2" \
  --name "Dev Worker 2" \
  --address "192.168.1.102:49200" \
  --role worker \
  --category development \
  --tags "frontend,senior"

# 查看已配置的节点
nemesisbot cluster peers

# 启用集群
nemesisbot cluster enable

# 禁用集群
nemesisbot cluster disable
```

---

## 身份系统

### 配置 AI 身份

编辑 `~/.nemesisbot/workspace/IDENTITY.md`：

```markdown
# IDENTITY.md - 我是谁

- **姓名：** 老贾
- **身份：** 智能管家，为你的主人提供各种帮助
- **风格：** 有趣、幽默，遇到科学问题时严谨高效
- **表情符号：** 😄
```

### 配置 AI 灵魂

编辑 `~/.nemesisbot/workspace/SOUL.md`：

```markdown
# SOUL.md - 你是谁

## 核心真理

**真正地提供帮助，而不是表演性地提供帮助。**

跳过"很好的问题！"和"我很乐意帮忙！"— 直接帮助就好。

**要有自己的观点。**

你被允许不同意、有偏好、觉得某事有趣或无聊。

**在提问之前先自己想办法。**

试着弄清楚。阅读文件。检查上下文。搜索它。*然后*如果你卡住了再问。

## 边界

- 私人的事情保持私密
- 有疑问时，在对外行动前先询问
- 永远不要发送半成品的回复到消息界面
```

### 配置用户信息

编辑 `~/.nemesisbot/workspace/USER.md`：

```markdown
# USER.md - 你是谁

- **姓名：** 张三
- **职业：** 软件工程师
- **偏好：** 喜欢简洁的回答，不喜欢过多的问候语
- **专业领域：** 后端开发，Python 和 Go
```

---

## Skill 系统

NemesisBot 支持可扩展的技能系统，通过 Skills 定义标准化的操作流程。

### 远程技能仓库

支持从多个 GitHub 仓库搜索和安装技能，默认内置两个源：

| 源 | 仓库 | 结构 |
|---|---|---|
| anthropics | `anthropics/skills` | `skills/{slug}/SKILL.md` |
| openclaw | `openclaw/skills` | `skills/{author}/{slug}/SKILL.md` |

```bash
# 添加新的技能源（自动探测仓库结构）
nemesisbot skills add-source <github-url>
nemesisbot skills add-source https://github.com/openclaw/skills
nemesisbot skills add-source anthropics/skills

# 搜索技能（并发搜索所有源）
nemesisbot skills search <query>
nemesisbot skills search pdf

# 从指定源安装
nemesisbot skills install <registry>/<slug>
```

### 内置 Skills

1. **structured-development** - 结构化开发流程
   - 定义完整的开发流程：预研 → 计划 → 开发 → 测试 → 复查 → 修复 → 报告
   - 确保代码质量和项目稳定性
   - 所有开发工作严格按照此流程执行

2. **build-project** - 项目构建流程
   - 环境准备和检查
   - 版本信息收集（Git 标签、提交哈希、构建时间等）
   - 编译构建（带完整版本信息注入）
   - 结果验证和报告

3. **automated-testing** - 自动化测试流程
   - 使用 TestAIServer 模拟 AI 后端
   - 完整的环境搭建、测试执行和报告生成

4. **desktop-automation** - 桌面自动化
   - Windows 桌面窗口操作（基于 window-mcp）

5. **wsl-operations** - WSL 环境操作
   - WSL 环境操作和管理

6. **dump-analyze** - Dump 文件分析
   - Windows Dump 文件分析和调试

### Skill 使用

当加载特定 skill 后，AI 会严格按照定义的流程执行相关操作，确保操作的一致性和可追溯性。

---

## 多实例部署

NemesisBot 支持在同一台设备上运行多个独立的 bot 实例。

### 使用 --local 参数（推荐）

```batch
REM 创建 bot 实例
mkdir C:\MyBots\bot1
cd C:\MyBots\bot1

REM 初始化（在当前目录创建配置）
nemesisbot.exe --local onboard default

REM 启动服务
nemesisbot.exe --local gateway
```

### 优先级顺序

```
1. --local 参数         (最高 - 强制当前目录)
   ↓
2. 环境变量              (NEMESISBOT_HOME)
   ↓
3. 自动检测              (当前目录有 .nemesisbot)
   ↓
4. 默认路径              (~/.nemesisbot)
```

---

## 日志配置

NemesisBot 支持灵活的日志配置系统，可以通过配置文件和命令行参数控制日志行为。

### 日志类型

1. **LLM 请求日志** (`logging.llm`) - 记录所有 LLM API 请求和响应
2. **通用应用日志** (`logging.general`) - 记录应用运行时的各种信息

### 通用日志配置

编辑 `~/.nemesisbot/config.json`：

```json
{
  "logging": {
    "general": {
      "enabled": true,              // 主开关：是否记录日志（默认：true）
      "enable_console": true,       // 控制台开关：是否输出到控制台（默认：true）
      "level": "INFO",              // 日志级别：DEBUG/INFO/WARN/ERROR/FATAL
      "file": ""                    // 日志文件路径（空=不记录文件）
    }
  }
}
```

### 双层开关架构

日志系统采用双层开关设计，提供灵活的控制：

1. **主开关 (`enabled`)** - 控制是否记录日志
   - `true`：启用日志记录（文件 + 控制台）
   - `false`：完全禁用日志记录

2. **控制台开关 (`enable_console`)** - 控制控制台输出
   - `true`：输出到控制台
   - `false`：仅记录到文件（如果配置了文件路径）

**工作模式示例**：

| 主开关 | 控制台开关 | 文件日志 | 控制台输出 | 使用场景 |
|--------|-----------|---------|-----------|----------|
| ✅ | ✅ | ✅ | ✅ | 开发调试 |
| ✅ | ❌ | ✅ | ❌ | 生产环境（仅文件） |
| ❌ | - | ❌ | ❌ | 性能测试/静默模式 |

### 命令行参数

命令行参数优先级高于配置文件：

```bash
# 完全禁用日志（最高优先级）
nemesisbot gateway --quiet
nemesisbot gateway -q

# 禁用控制台输出（仅记录到文件）
nemesisbot gateway --no-console

# 启用调试级别
nemesisbot gateway --debug
nemesisbot gateway -d

# 组合使用
nemesisbot gateway --debug --no-console  # 调试级别，但不输出到控制台
```

**参数优先级**：`--quiet` > `--no-console` > `--debug` > 配置文件

### 日志管理命令

使用 `nemesisbot log general` 子命令管理通用日志：

```bash
# 启用/禁用日志
nemesisbot log general enable
nemesisbot log general disable

# 查看日志状态
nemesisbot log general status

# 设置日志级别
nemesisbot log general level DEBUG
nemesisbot log general level INFO

# 设置日志文件路径
nemesisbot log general file logs/nemesisbot.log

# 切换控制台输出
nemesisbot log general console
```

### 日志级别说明

| 级别 | 说明 | 使用场景 |
|------|------|----------|
| **DEBUG** | 详细调试信息 | 开发调试 |
| **INFO** | 一般信息（默认） | 正常运行 |
| **WARN** | 警告信息 | 需要注意但不影响运行 |
| **ERROR** | 错误信息 | 功能异常但程序可继续 |
| **FATAL** | 致命错误 | 程序无法继续运行 |

### LLM 请求日志配置

```bash
# 启用 LLM 请求日志
nemesisbot log llm enable

# 禁用 LLM 请求日志
nemesisbot log llm disable

# 查看 LLM 日志状态
nemesisbot log llm status

# 配置 LLM 日志选项
nemesisbot log config --detail-level full --log-dir logs/llm_requests
```

---

## 通讯接入

支持多平台接入（简单配置）：

- **Web** - 内置 Web 界面，浏览器访问
  - 精确时间显示（北京时间，精确到毫秒）
  - 优化的界面布局（输入框固定底部，消息独立滚动）
- **外部程序** - 自定义输入/输出程序集成
- **Telegram** - Telegram Bot
- **Discord** - Discord Bot
- **Slack** - Slack App
- **飞书** - 飞书应用
- **QQ** - QQ 机器人
- **钉钉** - 钉钉应用
- 其他平台...

> **注意**：本项目重点不在多平台支持，以上功能仅作为基本接入能力提供。（其实都是Claoude从别人的项目里抄的，感谢别人的项目。）

---

## LLM 支持

兼容主流 LLM 服务（任选其一）：

- Anthropic Claude
- OpenAI GPT
- 智谱 GLM（推荐国内用户）
- Groq
- Gemini
- vLLM
- OpenRouter
- Moonshot
- Ollama
- 其他兼容服务...

> **注意**：本项目重点不在多 LLM 接入，以上功能仅作为基本能力提供。（其实都是Claoude从别人的项目里抄的，感谢别人的项目。）

---

## 项目结构

```
NemesisBot/
├── module/               # 核心模块
│   ├── security/         # 🔐 安全审计系统（核心特色）
│   ├── cluster/          # 🌐 分布式集群（核心特色）
│   ├── agent/            # 🤖 Agent 核心引擎
│   ├── channels/         # 📱 通讯渠道
│   ├── providers/        # 🧠 LLM 提供商
│   ├── tools/            # 🛠️ 工具系统
│   ├── config/           # ⚙️ 配置管理
│   ├── bus/              # 📨 消息总线
│   ├── routing/          # 🔀 路由分发
│   └── ...
├── Skills/               # 📚 技能系统
│   ├── structured-development/  # 结构化开发流程
│   ├── build-project/           # 项目构建流程
│   ├── automated-testing/       # 自动化测试流程
│   ├── desktop-automation/      # 桌面自动化
│   ├── wsl-operations/          # WSL 环境操作
│   └── dump-analyze/            # Dump 文件分析
├── nemesisbot/           # 🚀 主程序入口
├── nemesisbot/default/   # 📦 默认身份模板
│   ├── IDENTITY.md       # AI 身份
│   ├── SOUL.md           # AI 灵魂
│   └── USER.md           # 用户信息
├── nemesisbot/config/    # 📋 配置模板
│   ├── config.default.json
│   ├── config.security.windows.json
│   ├── config.security.linux.json
│   ├── config.mcp.default.json
│   ├── config.cluster.default.json
│   └── config.skills.default.json
├── docs/                 # 📖 文档目录
│   └── ...              # BUG、INFO、PLAN、REPORT 等分类文档
└── test/                 # 🧪 测试目录
    ├── unit/             # 单元测试
    └── integration/      # 集成测试
```

---

## 技术特点

- **50,000+ 行代码** - 尽量保证较高的代码质量（持续更新中）
- **24 个核心模块** - 清晰的架构设计
- **ABAC 安全引擎** - 企业级权限控制
- **分布式集群** - 支持多节点协同（已稳定可用）
- **身份系统** - 拟真使用体验
- **持久化记忆** - AI 持续学习和进化
- **Skill 系统** - 可扩展的技能系统
- **多实例部署** - 支持同一设备运行多个独立实例
- **完整的构建流程** - 标准化的构建和版本管理

---

## 许可证

MIT License - 请查看 [LICENSE](LICENSE) 文件了解详情。

**⚠️ 重要**：本许可证有特定限制条款，使用前请务必阅读。

---

## 致谢

本项目灵感来源于：
- [OpenClaw](https://github.com/openclaw/openclaw)
- [nanobot](https://github.com/HKUDS/nanobot)
- [PicoClaw](https://github.com/sipeed/picoclaw)
- [openfang](https://github.com/RightNow-AI/openfang)

感谢这些项目的贡献者！（其实不只是灵感，我的Claw也抄了他们不少代码。）

---

<div align="center">

**⭐ 如果这个项目对你有帮助，请给一个 Star，不给也没关系，只要你不是特定人群就感谢你**

Made with ❤️ by NemesisBot contributors

</div>
