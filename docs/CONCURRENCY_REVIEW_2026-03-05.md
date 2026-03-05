# NemesisBot 并发问题全面审视报告

**日期**: 2026-03-05
**审视范围**: 所有 Go channel 相关的并发竞争问题

---

## 已发现并修复的问题

### 问题 1: RPC Channel 与 Manager 竞争 OutboundChannel

**状态**: ✅ 已修复

**位置**:
- `module/channels/rpc_channel.go:234` - `outboundListener`
- `module/channels/manager.go:297` - `dispatchOutbound`

**问题描述**:
两个 goroutine 同时从 `MessageBus.outbound` channel 读取，导致消息随机丢失。

**修复方案**:
1. RPC channel 不再从 `OutboundChannel()` 读取
2. RPC channel 通过 `Send()` 方法接收消息
3. RPC channel 被注册到 channel manager 中

---

## 历史问题（已在之前修复）

### 问题 2: Web Server 与 Manager 竞争 OutboundChannel

**状态**: ✅ 已在之前修复

**位置**:
- `module/web/server.go:318` - `dispatchOutbound`
- `module/channels/manager.go:297` - `dispatchOutbound`

**修复方案**:
- `web/server.go` 中的 `dispatchOutbound` 已被禁用（第 62 行被注释）

---

## 当前消息流架构

### Outbound 消息流（LLM 响应 → 用户）

```
AgentLoop.PublishOutbound()
    ↓
MessageBus.outbound (channel)
    ↓
Manager.dispatchOutbound() ← 唯一消费者
    ↓
根据 msg.Channel 查找 channel
    ↓
channel.Send(ctx, msg)
    ├─→ web channel → WebChannel.Send() → WebSocket
    ├─→ telegram channel → TelegramChannel.Send() → Telegram API
    ├─→ discord channel → DiscordChannel.Send() → Discord API
    ├─→ rpc channel → RPCChannel.Send() → pendingRequest.responseCh
    └─→ 其他 channels...
```

### Inbound 消息流（用户消息 → LLM）

```
各 Channel 接收消息
    ↓
MessageBus.PublishInbound()
    ↓
MessageBus.inbound (channel)
    ↓
AgentLoop.ConsumeInbound() ← 唯一消费者
    ↓
AgentLoop 处理消息
```

---

## 代码检查清单

### ✅ OutboundChannel 消费者

| 组件 | 状态 | 说明 |
|------|------|------|
| `channels/manager.go` | ✅ 活跃 | 唯一消费者，负责分发 |
| `channels/rpc_channel.go` | ✅ 已禁用 | 不再读取 OutboundChannel |
| `web/server.go` | ✅ 已禁用 | dispatchOutbound 被注释掉 |

### ✅ InboundChannel 消费者

| 组件 | 状态 | 说明 |
|------|------|------|
| `agent/loop.go` | ✅ 正确 | 唯一消费者 |

### ✅ Channel Send() 方法实现

| Channel | 实现状态 | 说明 |
|---------|----------|------|
| web | ✅ | 正确实现 |
| telegram | ✅ | 正确实现 |
| discord | ✅ | 正确实现 |
| slack | ✅ | 正确实现 |
| feishu | ✅ | 正确实现 |
| dingtalk | ✅ | 正确实现 |
| line | ✅ | 正确实现 |
| onebot | ✅ | 正确实现 |
| qq | ✅ | 正确实现 |
| whatsapp | ✅ | 正确实现 |
| maixcam | ✅ | 正确实现 |
| external | ✅ | 正确实现 |
| websocket | ✅ | 正确实现 |
| rpc | ✅ | 已修复，现在正确实现 |

---

## 潜在风险点（需持续关注）

### 1. 新增 Channel 时的注意事项

**风险**: 如果新增的 Channel 直接从 `OutboundChannel()` 读取，会再次引入竞争问题。

**建议**: 所有 Channel 应该：
- 实现 `Send(ctx context.Context, msg bus.OutboundMessage) error` 方法
- 注册到 `channels.Manager` 中
- **不要**自己从 `OutboundChannel()` 读取

### 2. 内部 Channel 的处理

**当前内部 Channel**:
- `cli` - 命令行
- `system` - 系统消息
- `subagent` - 子代理

**处理方式**: 在 `dispatchOutbound` 中被跳过（不发送给用户）

### 3. MessageBus 的设计限制

**当前设计**: Go channel 是单消费者模式

**如果需要广播**: 需要修改 `MessageBus` 实现：
```go
// 方案 1: 每个消费者一个 channel
type MessageBus struct {
    outboundSubscribers []chan OutboundMessage
    mu                  sync.RWMutex
}

// 方案 2: 使用 pub/sub 库
```

---

## 测试建议

### 1. 并发压力测试

```bash
# 启动多个 channel，发送大量消息，验证无丢失
# 预期：所有消息都应该被正确送达
```

### 2. RPC + Web 同时使用测试

```bash
# 同时使用 RPC channel 和 Web channel
# 验证两种消息都能正确送达
```

### 3. 消息顺序测试

```bash
# 验证同一 session 的消息顺序正确
```

---

## 修改文件列表

| 文件 | 修改类型 | 说明 |
|------|----------|------|
| `module/channels/rpc_channel.go` | 修改 | Send() 方法实现，outboundListener 禁用 |
| `module/cluster/cluster.go` | 新增 | GetRPCChannel() 方法 |
| `module/agent/loop.go` | 修改 | SetChannelManager() 注册 RPC channel |

---

## 结论

**当前状态**: 所有的 channel 竞争问题已修复

**架构健康度**: ✅ 良好
- Outbound channel 只有 `dispatchOutbound` 一个消费者
- Inbound channel 只有 `AgentLoop` 一个消费者
- 所有 channel 都通过 `Send()` 方法接收消息

**持续维护建议**:
1. 新增 Channel 时，务必实现 `Send()` 方法并注册到 Manager
2. 不要在任何 Channel 中直接读取 `OutboundChannel()`
3. 定期审查 `grep -r "OutboundChannel()" module/` 确保没有新增的消费者
