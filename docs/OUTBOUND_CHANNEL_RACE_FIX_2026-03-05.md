# Outbound Channel 竞争问题修复报告

**日期**: 2026-03-05
**问题类型**: Channel 消费者竞争
**严重程度**: 高 - 导致消息丢失

---

## 问题描述

用户报告：LLM 已经返回了内容，日志显示 "Outbound response published"，但返回的内容无法发送到 web 界面。

**用户日志**:
```
[INFO] agent: Processing message from web:web:bf4a4258cd0b45de: 在么
[INFO] agent: Routed message {session_key=agent:main:main, matched_by=default}
[INFO] agent: LLM response without tool calls (direct answer) {content_chars=90}
[INFO] agent: Response: 在的，老铁！😄...
[INFO] agent: Outbound response published {channel=web, chat_id=web:bf4a4258cd0b45de}
```

消息在 agent 层成功发布，但 web 界面未收到。

---

## 根本原因分析

### 竞争问题

**两个组件同时在读取同一个 Go channel (`MessageBus.outbound`)**:

1. `channels/manager.go:297` - `dispatchOutbound` 从 `m.bus.OutboundChannel()` 读取
2. `channels/rpc_channel.go:234` - `outboundListener` 也从 `ch.base.bus.OutboundChannel()` 读取

**Go channel 特性**: 当一个 channel 被多个消费者读取时，每条消息只会被**随机一个**消费者获取（竞争关系），而不是广播给所有消费者。

### 问题流程

```
1. Agent 发布 "web" channel 的消息到 bus.outbound
2. RPC channel 的 outboundListener 可能先抢到这条消息
3. RPC channel 检查 msg.Channel != "rpc"，执行 continue
4. 消息被丢弃！
5. dispatchOutbound 永远收不到这条消息
6. Web 界面收不到响应
```

### 相关代码位置

**竞争点 1 - channels/manager.go:290-354**:
```go
func (m *Manager) dispatchOutbound(ctx context.Context) {
    for {
        case msg, ok := <-m.bus.OutboundChannel():  // ← 消费者 1
            // ... 处理消息
    }
}
```

**竞争点 2 - channels/rpc_channel.go:218-280**:
```go
func (ch *RPCChannel) outboundListener(ctx context.Context) {
    for {
        case msg, ok := <-ch.base.bus.OutboundChannel():  // ← 消费者 2（同一个 channel！）
            if msg.Channel != ch.Name() {
                continue  // ← 不是 rpc 的消息被丢弃！
            }
            // ... 处理 rpc 消息
    }
}
```

---

## 修复方案

### 修改 1: `module/channels/rpc_channel.go`

**修改 `Send()` 方法** - 让它真正处理消息：

```go
// 修改前 (空实现)
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
    // RPC channel doesn't actively send messages
    // Responses are delivered through the pending request mechanism
    return nil
}

// 修改后 (真正处理消息)
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
    // Only process messages from this channel
    if msg.Channel != ch.Name() {
        return nil
    }

    // Extract correlation ID from content
    correlationID := extractCorrelationID(msg.Content)
    if correlationID == "" {
        return nil
    }

    // Find pending request and deliver response
    ch.mu.RLock()
    req, exists := ch.pendingReqs[correlationID]
    ch.mu.RUnlock()

    if exists {
        actualContent := removeCorrelationID(msg.Content)
        select {
        case req.responseCh <- actualContent:
            logger.DebugCF("rpc", "Delivered response via Send", ...)
        case <-time.After(time.Second):
            logger.WarnCF("rpc", "Failed to deliver response", ...)
        }
    }
    return nil
}
```

**修改 `outboundListener()` 方法** - 不再从 OutboundChannel 读取：

```go
// 修改后
func (ch *RPCChannel) outboundListener(ctx context.Context) {
    defer ch.wg.Done()
    logger.InfoC("rpc", "Outbound listener started (deprecated - using Send() method)")

    // Simply wait for stop signal - 不再读取 OutboundChannel
    select {
    case <-ch.stopCh:
    case <-ctx.Done():
    }
}
```

### 修改 2: `module/cluster/cluster.go`

**添加 `GetRPCChannel()` 方法**：

```go
// GetRPCChannel returns the RPC channel (may be nil if not configured)
func (c *Cluster) GetRPCChannel() *channels.RPCChannel {
    c.mu.RLock()
    defer c.mu.RUnlock()
    return c.rpcChannel
}
```

### 修改 3: `module/agent/loop.go`

**修改 `SetChannelManager()` 方法** - 注册 RPC channel：

```go
func (al *AgentLoop) SetChannelManager(cm *channels.Manager) {
    al.channelManager = cm

    // Register RPC channel to channel manager if cluster has one
    if al.cluster != nil {
        if rpcCh := al.cluster.GetRPCChannel(); rpcCh != nil {
            cm.RegisterChannel("rpc", rpcCh)
            logger.InfoC("agent", "RPC channel registered to channel manager")
        }
    }
}
```

---

## 修复后的消息流

```
Agent 发布消息到 bus.outbound
    ↓
dispatchOutbound 读取消息（唯一消费者）
    ↓
根据 msg.Channel 查找对应的 channel
    ↓
调用 channel.Send(ctx, msg)
    ├─→ web channel → 发送到 WebSocket
    ├─→ telegram channel → 发送到 Telegram
    ├─→ discord channel → 发送到 Discord
    └─→ rpc channel → 传递给等待的 RPC 调用者
```

---

## 历史问题回顾

这是本周第二次出现类似的竞争问题：

1. **第一次**: `DEBUG_REPORT_2026-02-28_CHANNEL_CONSUMER_COMPETITION.md`
   - web/server.go 和 channels/manager.go 竞争读取 OutboundChannel
   - 已通过禁用 web/server.go 中的 dispatchOutbound 修复

2. **第二次**: 本次修复
   - rpc_channel.go 和 channels/manager.go 竞争读取 OutboundChannel
   - 通过让 RPC channel 不再直接读取 OutboundChannel 修复

---

## 教训总结

1. **Go channel 是单消费者设计**：多个 goroutine 读取同一个 channel 会导致消息随机分配，而不是广播
2. **消息总线模式应该只有一个分发器**：所有消息应该由一个中心分发器处理
3. **Channel 实现应该通过 Send() 方法接收消息**：而不是自己从总线读取
