# CLAUDE.md

本文件为 Claude Code (claude.ai/code) 在此代码库中工作时提供指导。

---

## ⚠️ 关键警告

**Windows 平台后台进程管理**：
- ❌ **严格禁止**使用 `start /B`、`cmd /c start`、`start` 等命令
- ❌ **严格禁止**使用任何会弹窗或创建新窗口的命令
- ✅ 使用 PowerShell `Start-Process -WindowStyle Hidden`
- ✅ 使用 Bash 后台符号 `&` 配合 `run_in_background: true` 参数
- ✅ 优先使用项目辅助脚本（setup-env/cleanup-env）

**原因**：`start /B` 等命令会弹窗报错，如果没有人工干预会导致流程永久卡住。

---

## 构建和测试命令

### 构建项目

```bash
# 标准构建（Windows，使用 cmd.exe）
build.bat

# 指定输出文件名构建
build.bat mybot.exe

# 使用 PowerShell 支持构建（使用 PowerShell 执行命令）
build.bat powershell

# 同时使用两个选项
build.bat mybot.exe powershell
```

构建脚本会自动：
- 从 git tag 提取版本号，如果没有则使用 `0.0.0.1`
- 提取 git commit hash
- 通过 ldflags 注入版本信息：`main.version`、`main.gitCommit`、`main.buildTime`、`main.goVersion`

### 运行测试

```bash
# 运行某个模块的所有测试
go test ./module/...

# 运行特定包的测试
go test ./module/channels/...
go test ./module/cluster/rpc/...

# 使用竞态检测器运行
go test -race ./module/...

# 运行单个测试
go test -run TestFunctionName ./module/path/to/test

# 运行集成测试
go test ./test/integration/...

# 详细输出模式
go test -v ./module/...
```

### 运行应用程序

```bash
# 启动网关（Web UI）
nemesisbot.exe gateway

# 使用本地模式（配置在 ./.nemesisbot 而不是 ~/.nemesisbot）
nemesisbot.exe --local gateway

# 集群管理
nemesisbot.exe cluster status
nemesisbot.exe cluster init --name "机器人名称" --role worker --category development
nemesisbot.exe cluster enable

# 模型管理
nemesisbot.exe model add --model zhipu/glm-4.7 --key YOUR_KEY --default

# 配置管理
nemesisbot.exe onboard default --local   # 使用默认配置，在当前目录初始化
nemesisbot.exe onboard default           # 使用默认配置根据默认流程初始化
nemesisbot.exe log config                # 配置日志详细级别
```

---

### 测试工具

1. `test\TestAIServer` AI 服务器模拟器。<br>目录中存在一个 AI 服务器的模拟工具，若需要测试AI相关功能，可以使用此工具模拟服务器。工具编译、运行后，可以使用 **`nemesisbot model add --model test/testai-1.1 --base http://127.0.0.1:8080/v1 --key test-key --default`** 命令来给nemesisbot 设置大模型，然后尝试模拟或执行内部流程。
2. `test\mcp` 测试用 MCP 服务器。 

---

## 架构概览

NemesisBot 是一个具有安全控制的分布式 AI 代理系统。架构围绕消息总线展开，将从各种通道来的入站消息路由到 Agent 引擎，然后将出站响应通过通道路由回去。

### 核心消息流

```
入站路径:
Channel (rpc/web/discord/feishu 等)
  → ChannelManager.Register()
  → bus.PublishInbound(InboundMessage)
  → AgentLoop 通过订阅接收
  → Agent 执行（LLM + 工具）
  → bus.PublishOutbound(OutboundMessage)

出站路径:
bus.PublishOutbound()
  → ChannelManager.dispatchOutbound() 协程
  → 按名称找到匹配的通道
  → channel.Send(ctx, OutboundMessage)
  → 通道投递到外部服务
```

**核心类型**（module/bus/types.go）：
- `InboundMessage`：Channel、SenderID、ChatID、Content、Media、SessionKey、CorrelationID
- `OutboundMessage`：Channel、ChatID、Content
- `CorrelationID`：用于 RPC 请求-响应匹配

### 模块架构

**消息总线**（module/bus/）：
- 消息路由的中心发布/订阅系统
- 通道订阅 InboundMessage，发布 OutboundMessage
- 线程安全，支持多个并发订阅者

**通道管理器**（module/channels/manager.go）：
- 所有通道的生命周期管理（Start/Stop）
- 将出站消息路由到适当的通道
- 处理消息过滤和投递
- 关键：`dispatchOutbound()` 在专用协程中运行，监听 `bus.OutboundChannel()`

**Agent 引擎**（module/agent/）：
- `loop.go`：核心执行循环（AgentLoop.Run）
  - 从 bus 接收 InboundMessage
  - 使用对话历史构建上下文
  - 调用 LLM 并传入工具定义
  - 执行工具（可能多次迭代）
  - 将最终响应发布到 OutboundMessage
- `instance.go`：Agent 实例管理
- `memory.go`：对话记忆和上下文
- `context.go`：请求上下文处理

**通道**（module/channels/）：
- 每个通道实现 `Channel` 接口
- `base.go`：BaseChannel 提供通用功能
- `rpc_channel.go`：用于 RPC/集群通信的特殊通道
  - 有 `Input(ctx, InboundMessage) (<-chan string, error)` 供 RPC 处理器使用
  - 通过 CorrelationID 前缀匹配响应：`[rpc:correlation_id] content`
  - 对 peer_chat 至关重要：响应必须有 correlation ID 前缀

**集群/RPC**（module/cluster/）：
- `cluster.go`：主集群编排
- `continuation_store.go`：续行快照持久化存储（Phase 2）
  - 快照存储在 `{workspace}/cluster/rpc_cache/{taskID}.json`
  - 包含 LLM 消息上下文（json.RawMessage）、toolCallID、channel、chatID
  - 支持内存+磁盘双写，启动时可从磁盘恢复
- `task_manager.go`：异步任务状态管理
  - `onTaskComplete` 回调：任务完成时通知 Cluster
  - Phase 2 移除了阻塞的 `WaitForTask`，改为回调驱动
- `task.go`：任务模型
  - `OriginalChannel`/`OriginalChatID`：发起方通道信息（用于续行通知路由）
- `rpc/client.go`：调用远程节点的 RPC 客户端
  - `CallWithContext()`：发送请求，等待响应
  - 超时：60 分钟（line 195）- 最外层超时保护
- `rpc/server.go`：处理传入请求的 RPC 服务器
  - `handleRequest()`：路由到已注册的处理器
  - `sendMessage()`：发送 TCP 响应
- `rpc/peer_chat_handler.go`：处理 peer_chat action（B 端）
  - 立即返回 ACK，异步处理 LLM
  - LLM 完成后回调 A 端的 `peer_chat_callback`
  - 超时：59 分钟（line 132）
- `transport/`：TCP 连接池和帧处理
  - `conn.go`：带有读写协程的 TCPConn
  - `frame.go`：长度前缀的二进制帧
  - `pool.go`：支持重用的连接池

**安全**（module/security/）：
- `middleware.go`：拦截危险操作（文件、进程、注册表、网络）
- `auditor.go`：ABAC 策略引擎（基于属性的访问控制）
- 四个风险级别：LOW / MEDIUM / HIGH / CRITICAL
- 可通过配置禁用（`security.enabled = false`）

### 关键配置位置

**超时配置**（module/agent/loop.go:1595-1596）：
- 目前配置为长超时层级（由外到内）：
```go
// RPC Client (client.go:195)        - 60 分钟（最外层）
// PeerChat Handler (peer_chat_handler.go:132) - 59 分钟
// RPCChannel (loop.go:1595)         - 58 分钟（最内层）
cfg := &channels.RPCChannelConfig{
    MessageBus:      msgBus,
    RequestTimeout:  58 * time.Minute,  // Line 1595
    CleanupInterval: 30 * time.Second,  // Line 1596
}
```

**重要**：超时层级为 RPC Client (60min) > PeerChat (59min) > RPCChannel (58min)，确保内层先超时，外层能正确处理。

**通道启动**（module/agent/loop.go:1606）：
- RPC 通道绝不能在 `setupClusterRPCChannel()` 中启动
- ChannelManager.StartAll() 是唯一的启动点
- 防止 "RPC channel already running" 错误

### 已知问题

**出站通道竞争**（2026-03-05 已修复）：
- 之前：RPCChannel 和 dispatchOutbound 争抢消息
- 修复：正确的通道注册和生命周期管理

---

## 关键模式和约定

### 通道 Correlation ID 模式

对于 RPC/集群通信，响应必须包含 correlation ID 前缀：

```go
// 正确格式
content := fmt.Sprintf("[rpc:%s] 实际响应内容", correlationID)

// RPCChannel.Send() 提取 correlationID 并路由到待处理的请求
// 如果缺少前缀，响应会丢失
```

**关键**：AgentLoop.Run（line 315-326）在 LLM 直接返回文本时为 RPC 通道添加前缀：
```go
if msg.Channel == "rpc" && msg.CorrelationID != "" {
    finalContent = fmt.Sprintf("[rpc:%s] %s", msg.CorrelationID, response)
}
```

### 工具执行流程

当 LLM 调用工具时（module/tools/message.go）：
1. 工具检查 channel == "rpc"
2. 从 context 提取 correlationID
3. 添加前缀：`[rpc:correlation_id] content`
4. 设置 `sentInRound = true` 标志
5. AgentLoop 在发布前检查 `alreadySent` 以避免重复

### 续行快照模式（Phase 2）

当 LLM 调用 `cluster_rpc` 工具时的非阻塞流程：

```
A 端发起（非阻塞）:
1. LLM 调用 cluster_rpc → 工具返回 AsyncResult(taskID)
2. AgentLoop 保存续行快照:
   - 内存: continuations[taskID] = {messages, toolCallID, channel, chatID}
   - 磁盘: {workspace}/cluster/rpc_cache/{taskID}.json
3. LLM 生成 "已发送请求" → 发送给用户 → 当前轮次结束

B 端处理:
4. B 立即返回 ACK → A 解除 TCP 连接
5. B 异步处理 LLM → 完成后回调 A 的 peer_chat_callback

A 端接收回调（续行）:
6. CallbackHandler → TaskManager.CompleteCallback → onTaskComplete(taskID)
7. Cluster.handleTaskComplete → bus.PublishInbound("system", "cluster_continuation:{taskID}")
8. AgentLoop.processMessage 拦截 cluster_continuation 前缀
9. handleClusterContinuation(taskID):
   - 加载续行快照（先查内存，再查磁盘）
   - 追加真实工具结果到 messages
   - 续行 LLM 调用（支持多步骤工具链继续执行）
   - 发送最终响应给用户
```

**关键注意事项**：
- `cluster_rpc` 工具实现 `ContextualTool` 接口，通过 `SetContext(channel, chatID)` 注入上下文
- 快照保存时机：在追加 tool_result 之前（此时 messages 包含 assistant 的 tool_call 但不包含 tool_result）
- 嵌套异步：续行中再次触发 cluster_rpc 时，自动保存新快照
- `Cluster.SetMessageBus()` 必须在 `setupClusterRPCChannel` 中、`SetRPCChannel` 之前调用

### 工作空间和配置

**路径优先级**：
1. `--local` 标志（强制使用 ./.nemesisbot）
2. 环境变量 `NEMESISBOT_HOME`
3. 自动检测（如果当前目录存在 .nemesisbot）
4. 默认：`~/.nemesisbot`

**关键文件**：
- `IDENTITY.md`：AI 人设/身份
- `SOUL.md`：AI 核心行为原则
- `USER.md`：用户偏好
- `config.json`：主配置
- `cluster/peers.toml`：已知的集群对等节点

### 安全区域

**操作风险级别**：
- **CRITICAL**：process_exec、process_kill、registry_write、system_shutdown
- **HIGH**：file_write、file_delete、dir_create、dir_delete、process_spawn
- **MEDIUM**：file_edit、file_append、registry_read、network_download
- **LOW**：file_read、dir_list、network_request、hardware_i2c

**工作空间限制**：
- `restrict_to_workspace: true` 限制文件访问仅在工作区内
- 安全中间件仍可拦截工作区外的操作
- 设置为 false 以获得完整系统访问（不推荐）

---

## 测试指南

### 测试目录

位置：`test`

- 目录下放置所有测试所需文件或项目
- 未来若需增加，须在此目录下新增对应内容

### 单元测试

位置：`test/unit/` 目录中按照项目目录层级放置的对应 `*_test.go`

- 若需增加对应单元测试，则也要按照当前规则，按照项目目录层级放置

### 集成测试

位置：`test/integration/`

- `channels/`：通道集成测试
- `rpc/`：RPC 流程测试
- `web/`：WebSocket 集成测试

### 集群测试

位置：`test/cluster/`

- `cluster-test/main.go`：多节点测试
- `rpc/server_test.go`：RPC 服务器测试
- `transport/`：连接池和帧测试

### 运行特定测试类别

```bash
# 所有通道测试
go test ./module/channels/...

# 集群 RPC 测试
go test ./module/cluster/rpc/...

# 传输层测试
go test ./test/cluster/transport/...
```

### 其他测试

若有特定的测试工具需求，可在当前目录下创建对应测试工具项目。

---

## 重要说明

### Windows PowerShell 兼容性

项目对 Windows PowerShell 的 `curl` 别名有特殊处理（会重定向到 `Invoke-WebRequest`）：
- 工具会自动将 `curl` 替换为 `curl.exe`
- 使用 `build.bat powershell` 构建以启用此功能
- 这对 Windows 上的外部工具执行至关重要

### Windows 后台进程管理

**⚠️ 严格禁止的命令**：
- ❌ `start /B` - 会弹窗报错，无人干预时永久卡住
- ❌ `cmd /c start` - 会创建新窗口，导致流程阻塞
- ❌ `start` - Windows 批处理命令，不适合后台运行

**✅ 推荐方法**：

**方法 1：使用项目辅助脚本（推荐）**
```powershell
# PowerShell
.\Skills\automated-testing\scripts\setup-env.ps1
.\Skills\automated-testing\scripts\cleanup-env.ps1
```
```bash
# Bash
bash Skills/automated-testing/scripts/setup-env.sh
bash Skills/automated-testing/scripts/cleanup-env.sh
```

**方法 2：PowerShell Start-Process**
```powershell
Start-Process -FilePath "./nemesisbot.exe" -ArgumentList "gateway" -WindowStyle Hidden
```

**方法 3：Bash 后台运行**
```bash
# 使用 Claude Code Bash 工具的 run_in_background 参数
./nemesisbot.exe gateway > nemesisbot.log 2>&1 &
```

**进程管理**：
```bash
# Windows 停止进程
taskkill //F //IM nemesisbot.exe

# 查找进程 PID
tasklist | grep -i nemesisbot.exe | head -1 | awk '{print $2}'
```

**重要说明**：
- 辅助脚本已封装所有后台进程管理逻辑
- 优先使用脚本而非手动操作
- 脚本会自动处理进程启停、PID 保存、健康检查

### 多实例部署

使用 `--local` 标志运行多个独立的 bot 实例：
```batch
mkdir C:\Bots\bot1
cd C:\Bots\bot1
nemesisbot.exe --local gateway
```

每个实例获得自己的 `.nemesisbot/` 目录，而不是使用 `~/.nemesisbot`。

### Skill 系统

**远程 Registry（多源技能搜索和安装）**：
- 配置文件：`workspace/config/config.skills.json`
- 默认内置源：`anthropics/skills`（两层结构）、`openclaw/skills`（三层结构）
- CLI 管理命令：
  ```bash
  nemesisbot skills add-source <github-url>  # 自动探测仓库结构并添加为新源
  nemesisbot skills search <query>           # 并发搜索所有源，合并结果
  nemesisbot skills install <registry>/<slug> # 从指定源安装
  ```
- `add-source` 自动探测三种仓库结构：`skills/{slug}/SKILL.md`、`skills/{author}/{slug}/SKILL.md`、根目录 `{slug}/SKILL.md`
- 搜索流程：并发查询所有 registry → 合并 → 按 score 降序排序 → 截断到 limit

**本地 Skills 目录**：`Skills/` 目录中的技能定义了标准化工作流程：

**开发流程**：
- `structured-development/`：带有阶段的开发流程（plan → develop → test → review）
- `build-project/`：带有版本注入的构建流程

**测试流程**：
- `automated-testing/`：完整的自动化测试流程
  - 使用 TestAIServer 模拟 AI 后端
  - 支持 WebSocket 通信测试
  - 提供 setup-env/cleanup-env 辅助脚本
  - 详见：`Skills/automated-testing/SKILL.md`
  - 快速开始：`Skills/automated-testing/examples/quick-test.md`

**桌面自动化**：
- `desktop-automation/`：Windows 桌面窗口操作（基于 window-mcp）

**运维工具**：
- `wsl-operations/`：WSL 环境操作和管理
- `dump-analyze/`：Dump 文件分析和调试

当加载技能时，AI 严格遵循定义的流程。

---

## 文件组织参考

**入口点**：`nemesisbot/main.go` - 命令路由
**CLI 命令**：`nemesisbot/command/` - 命令实现
**配置模板**：`nemesisbot/config/*.json` - 默认配置

**核心模块**：
- `module/agent/loop.go` - 主执行循环（**理解 agent 流程的起点**）
- `module/bus/` - 消息总线
- `module/channels/manager.go` - 通道生命周期和路由
- `module/cluster/` - 集群编排和续行快照
  - `cluster.go` - 主编排、bus 注入、handleTaskComplete
  - `continuation_store.go` - 续行快照持久化存储
  - `task_manager.go` - 异步任务状态 + onTaskComplete 回调
- `module/cluster/rpc/` - 集群通信的 RPC 客户端/服务器
- `module/security/` - 安全中间件和 ABAC

**测试结构**：
- `test/unit/` - 单元测试
- `test/integration/` - 集成测试
- `test/cluster/` - 集群和 RPC 测试

**文档**：
- `docs/BUG/` - 已知问题和调查，已知问题的分析，文件创建到这里，每个文件记录一个 BUG 或一个文件记录多个 BUG
- `docs/INFO/` - 技术信息，项目技术信息，文件创建到这里
- `docs/PLAN/` - 规划文档，新的开发规划，文件创建到这里，每个文件记录一个开发计划，便于完成计划后归档
- `docs/REPORT/` - 分析报告，开发过程中各种报告，文件创建到这里

### 文档操作说明

- 文件目录内的所有文件格式均为 markdown 格式。
- 文档内的文件名字均以日期开头，文件名格式为：YYYY-MM-DD_[正常文件名].md。
- `docs/BUG/` 目录只存放现有存在的 BUG 。
- 若 BUG 修复完成，则删除 BUG 信息，并添加文件到 `docs/REPORT/` 目录，标记 BUG 修复并记录报告。
- `docs/PLAN/` 目录只存放现在还存在的开发计划，包括进行中、暂停的。
- 若开发计划已经完成，则归档到其他目录中，如 `docs/INFO/` 或 `docs/REPORT/` 中，同时删除 `docs/PLAN/` 目录中的原始文件。

---

## 安全配置注意事项

**工作区隔离是默认且推荐的配置**：
- Bot 只能访问 workspace 目录
- 所有文件操作受安全策略控制
- 危险操作需要审批或会被拦截

**禁用安全模块**（不推荐）：
```json
{
  "security": {
    "enabled": false
  }
}
```
这会移除所有安全检查，Bot 可以访问整个系统。
