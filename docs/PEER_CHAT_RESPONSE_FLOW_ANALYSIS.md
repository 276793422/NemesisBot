# Peer Chat RPC 响应流程分析与问题诊断

## 问题描述

**症状**：
- 本端发送 peer_chat 请求到对端
- 对端能够接收到消息
- 但本端接收不到对端的响应，一直超时（30秒）

**日志**：
```
2026/03/05 22:16:07 [INFO] agent: Tool call: cluster_rpc({"action":"peer_chat","data":{"content":"你好！我是本地节点...","type":"chat"},"peer_id":"bot-CloudServer-..."})
2026/03/05 22:16:07 [INFO] tool: Tool execution started
2026/03/05 22:16:37 [ERROR] tool: Tool execution failed {error=RPC call failed: failed to receive response: timeout waiting for response}
```

## 完整消息流程追踪

### 1. 本端发送请求

```
cluster_rpc Tool.Execute()
  → Cluster.CallWithContext(ctx, peerID, "peer_chat", payload)
    → RPCClient.CallWithContext(ctx, peerID, "peer_chat", payload)
      → 连接到对端
      → 发送请求
      → 等待响应（30秒超时）← 超时发生在这里
```

### 2. 对端接收请求

```
RPC Server (对端)
  → acceptLoop() 接收连接
  → handleConnection() 处理连接
  → handleRequest() 处理请求
    → 找到 peer_chat handler
    → 调用 handler(req.Payload)
```

### 3. 对端 peer_chat handler 处理

```go
// module/cluster/rpc/peer_chat_handler.go
func (h *PeerChatHandler) Handle(payload map[string]interface{}) (map[string]interface{}, error) {
    // 1. 解析 payload
    // 2. 验证 content 字段
    // 3. 设置默认 type 为 "request"

    // 4. 调用 handleLLMRequest
    return h.handleLLMRequest(&req)
}

func (h *PeerChatHandler) handleLLMRequest(req *PeerChatPayload) (map[string]interface{}, error) {
    // 1. 检查 rpcChannel 是否可用
    if h.rpcChannel == nil {
        return errorResponse("error", "rpc channel not available")
    }

    // 2. 提取 chat_id, session_key, sender_id
    chatID := "default"
    sessionKey := "default-session"
    senderID := "remote-peer"
    if req.Context != nil {
        // 从 context 提取
    }

    // 3. 创建 InboundMessage
    inbound := &bus.InboundMessage{
        Channel:       "rpc",
        ChatID:        chatID,
        Content:       req.Content,
        SenderID:      senderID,
        SessionKey:    sessionKey,
        CorrelationID: fmt.Sprintf("peer-chat-%d", time.Now().UnixNano()), // ← 关键
    }

    // 4. 发送到 RPCChannel
    ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
    defer cancel()

    respCh, err := h.rpcChannel.Input(ctx, inbound)
    if err != nil {
        return errorResponse("error", "failed to process")
    }

    // 5. 等待响应 ← 可能卡在这里
    select {
    case response := <-respCh:
        return successResponse(response, nil)
    case <-ctx.Done():
        return errorResponse("error", "timeout") // ← 可能返回这个
    }
}
```

### 4. RPCChannel.Input 流程

```go
// module/channels/rpc_channel.go
func (ch *RPCChannel) Input(ctx context.Context, inbound *bus.InboundMessage) (<-chan string, error) {
    // 1. 生成 CorrelationID（如果没有）
    if inbound.CorrelationID == "" {
        inbound.CorrelationID = generateCorrelationID()
    }
    inbound.Channel = ch.Name() // "rpc"

    // 2. 创建 pending request ← 关键步骤
    respCh := make(chan string, 1)

    ch.mu.Lock()
    ch.pendingReqs[inbound.CorrelationID] = &pendingRequest{
        correlationID: inbound.CorrelationID,
        responseCh:    respCh,
        createdAt:     time.Now(),
        timeout:       ch.getRequestTimeout(inbound.Metadata),
    }
    ch.mu.Unlock()

    // 3. 发送到 MessageBus ← 关键步骤
    ch.base.bus.PublishInbound(*inbound)

    return respCh, nil
}
```

### 5. AgentLoop 处理

```go
// module/agent/loop.go
func (al *AgentLoop) Run(ctx context.Context) error {
    for al.running.Load() {
        select {
        case <-ctx.Done():
            return nil
        default:
            // 1. 从 MessageBus 接收消息
            msg, ok := al.bus.ConsumeInbound(ctx)
            if !ok {
                continue
            }

            // 2. 添加 CorrelationID 到 context ← 关键步骤
            ctx = context.WithValue(ctx, "correlation_id", msg.CorrelationID)

            // 3. 处理消息
            response, err := al.processMessage(ctx, msg)

            // 4. 如果有响应且 MessageTool 没有发送，则发送
            if response != "" && !alreadySent {
                al.bus.PublishOutbound(bus.OutboundMessage{
                    Channel: msg.Channel,
                    ChatID:  msg.ChatID,
                    Content: response,
                })
            }
        }
    }
}
```

### 6. MessageTool 发送响应

```go
// module/tools/message.go
func (t *MessageTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    content := args["content"].(string)
    channel := "rpc" // ← 从参数或默认值获取
    chatID := "some-chat-id"

    // 对于 RPC channel，添加 CorrelationID 前缀
    finalContent := content
    if channel == "rpc" {
        if correlationID := getCorrelationIDFromContext(ctx); correlationID != "" {
            finalContent = fmt.Sprintf("[rpc:%s] %s", correlationID, content)
            // ↑ 关键：格式必须是 "[rpc:correlation_id] actual response"
        }
    }

    // 调用 sendCallback
    if err := t.sendCallback(channel, chatID, finalContent); err != nil {
        return ErrorResult
    }

    t.sentInRound = true // ← 标记已发送
    return SilentResult
}
```

sendCallback 的实现在 AgentLoop 中：
```go
messageTool.SetSendCallback(func(channel, chatID, content string) error {
    msgBus.PublishOutbound(bus.OutboundMessage{
        Channel: channel,
        ChatID:  chatID,
        Content: content,
    })
    return nil
})
```

### 7. ChannelManager 分发响应

```go
// module/channels/manager.go
func (m *Manager) dispatchOutbound(ctx context.Context) {
    for {
        select {
        case <-ctx.Done():
            return
        case msg, ok := <-m.bus.OutboundChannel():
            if !ok {
                return
            }

            // 1. 跳过内部 channel
            if constants.IsInternalChannel(msg.Channel) {
                continue // "rpc" 不在内部 channel 列表中，不会被跳过
            }

            // 2. 找到对应的 channel
            m.mu.RLock()
            channel, exists := m.channels[msg.Channel]
            m.mu.RUnlock()

            if !exists {
                logger.ErrorCF("channels", "Unknown channel", ...)
                continue
            }

            // 3. 调用 channel.Send
            if err := channel.Send(ctx, msg); err != nil {
                logger.ErrorCF("channels", "Error sending", ...)
            }
        }
    }
}
```

### 8. RPCChannel.Send 处理响应

```go
// module/channels/rpc_channel.go
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
    // 1. 只处理来自本 channel 的消息
    if msg.Channel != ch.Name() { // ch.Name() = "rpc"
        return nil
    }

    // 2. 提取 CorrelationID ← 关键步骤
    // 格式: "[rpc:correlation_id] actual response"
    correlationID := extractCorrelationID(msg.Content)
    if correlationID == "" {
        logger.DebugCF("rpc", "No correlation ID in message", ...)
        return nil
    }

    // 3. 找到 pending request ← 关键步骤
    ch.mu.RLock()
    req, exists := ch.pendingReqs[correlationID]
    ch.mu.RUnlock()

    if exists {
        actualContent := removeCorrelationID(msg.Content)
        select {
        case req.responseCh <- actualContent: // ← 发送响应
            logger.DebugCF("rpc", "Delivered response via Send", ...)
        case <-time.After(time.Second):
            logger.WarnCF("rpc", "Failed to deliver response (timeout)", ...)
        }
    } else {
        logger.DebugCF("rpc", "No pending request for correlation ID", ...)
    }

    return nil
}
```

## 潜在问题点分析

### 问题 1: CorrelationID 不匹配

**可能原因**：
- peer_chat handler 生成的 CorrelationID 格式：`peer-chat-1234567890`
- MessageTool 添加的前缀格式：`[rpc:peer-chat-1234567890] response`
- RPCChannel.Send 提取的 CorrelationID 应该：`peer-chat-1234567890`

**检查方法**：
添加日志查看 CorrelationID 的生成和传递

### 问题 2: RPCChannel 未正确注册到 ChannelManager

**可能原因**：
- RPCChannel 可能没有在 ChannelManager 中注册
- 或者注册的名称不是 "rpc"

**检查方法**：
```bash
# 查看日志中的 "enabled_channels" 列表
# 应该包含 "rpc"
```

### 问题 3: MessageTool 没有被调用

**可能原因**：
- LLM 可能没有调用 message 工具
- 或者 LLM 调用了其他工具但没有返回响应

**检查方法**：
添加日志查看 MessageTool.Execute 是否被调用

### 问题 4: CorrelationID 没有传递到 context

**可能原因**：
- AgentLoop.Run 中虽然添加了 CorrelationID 到 context
- 但可能在某些情况下 context 被重置或覆盖

**检查方法**：
在 MessageTool.Execute 中添加日志查看 correlationID

### 问题 5: RPCChannel.Send 的条件检查失败

**可能原因**：
```go
if msg.Channel != ch.Name() {
    return nil // ← 如果这里不匹配，响应会被忽略
}
```

**检查方法**：
在 RPCChannel.Start 中添加日志确认 ch.Name() 返回 "rpc"

### 问题 6: 响应发送时机问题

**可能原因**：
- AgentLoop.Run 在 processMessage 返回后会检查 alreadySent
- 如果 MessageTool 已经发送了响应，AgentLoop 会再次发送
- 但这不应该导致问题，因为第二次发送会被 RPCChannel.Send 处理

## 诊断步骤

### 步骤 1: 添加调试日志

在 `module/cluster/rpc/peer_chat_handler.go` 的 `handleLLMRequest` 方法中添加：

```go
func (h *PeerChatHandler) handleLLMRequest(req *PeerChatPayload) (map[string]interface{}, error) {
    h.cluster.LogRPCInfo("[PeerChat] Processing %s request", req.Type)
    h.cluster.LogRPCInfo("[PeerChat] Request content: %s", req.Content)

    // 检查 rpcChannel
    if h.rpcChannel == nil {
        h.cluster.LogRPCError("[PeerChat] RPC channel is nil", nil)
        return h.errorResponse("error", "rpc channel not available"), nil
    }
    h.cluster.LogRPCInfo("[PeerChat] RPC channel is available", nil)

    // ... 创建 inbound ...

    h.cluster.LogRPCInfo("[PeerChat] Created CorrelationID: %s", inbound.CorrelationID)

    respCh, err := h.rpcChannel.Input(ctx, inbound)
    if err != nil {
        h.cluster.LogRPCError("[PeerChat] Failed to send to RPC channel: %v", err)
        return h.errorResponse("error", "failed to process"), nil
    }

    h.cluster.LogRPCInfo("[PeerChat] Waiting for response...", nil)

    select {
    case response := <-respCh:
        h.cluster.LogRPCInfo("[PeerChat] Response received: %s", response)
        return h.successResponse(response, nil), nil
    case <-ctx.Done():
        h.cluster.LogRPCError("[PeerChat] Timeout waiting for response", nil)
        return h.errorResponse("error", "timeout"), nil
    }
}
```

### 步骤 2: 检查 RPCChannel 注册

在 `module/channels/rpc_channel.go` 的 `Start` 方法开始处添加：

```go
func (ch *RPCChannel) Start(ctx context.Context) error {
    logger.InfoCF("rpc", "RPCChannel starting",
        map[string]interface{}{
            "name": ch.Name(),
            "running": ch.running,
        })

    // ... 现有代码 ...
}
```

### 步骤 3: 检查 Send 方法调用

在 `module/channels/rpc_channel.go` 的 `Send` 方法中添加：

```go
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
    logger.InfoCF("rpc", "RPCChannel.Send called",
        map[string]interface{}{
            "msg_channel": msg.Channel,
            "ch_name": ch.Name(),
            "content_preview": utils.Truncate(msg.Content, 100),
        })

    if msg.Channel != ch.Name() {
        logger.WarnCF("rpc", "Channel mismatch",
            map[string]interface{}{
                "msg_channel": msg.Channel,
                "ch_name": ch.Name(),
            })
        return nil
    }

    correlationID := extractCorrelationID(msg.Content)
    logger.InfoCF("rpc", "Extracted correlation ID",
        map[string]interface{}{
            "correlation_id": correlationID,
        })

    if correlationID == "" {
        logger.WarnCF("rpc", "No correlation ID found",
            map[string]interface{}{
                "content": msg.Content,
            })
        return nil
    }

    // ... 现有代码 ...
}
```

### 步骤 4: 检查 MessageTool

在 `module/tools/message.go` 的 `Execute` 方法中添加：

```go
func (t *MessageTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    // ... 现有代码 ...

    // For RPC channel
    finalContent := content
    if channel == "rpc" {
        correlationID := getCorrelationIDFromContext(ctx)
        logger.InfoCF("agent", "MessageTool: RPC channel detected",
            map[string]interface{}{
                "correlation_id": correlationID,
                "content_preview": utils.Truncate(content, 100),
            })

        if correlationID != "" {
            finalContent = fmt.Sprintf("[rpc:%s] %s", correlationID, content)
            logger.InfoCF("agent", "MessageTool: Added correlation prefix",
                map[string]interface{}{
                    "final_content_preview": utils.Truncate(finalContent, 100),
                })
        } else {
            logger.WarnCF("agent", "MessageTool: No correlation ID in context for RPC channel", nil)
        }
    }

    // ... 现有代码 ...
}
```

## 可能的解决方案

### 方案 1: 确保 RPCChannel 正确注册

检查 Cluster 初始化时是否正确创建和注册了 RPCChannel：

```go
// module/cluster/cluster.go
func (c *Cluster) setupChannels() error {
    // ... 创建 RPCChannel ...

    // 确保注册到 ChannelManager
    if err := c.channelManager.RegisterChannel(c.rpcChannel); err != nil {
        return fmt.Errorf("failed to register RPC channel: %w", err)
    }

    return nil
}
```

### 方案 2: 确保 CorrelationID 正确传递

检查整个流程中 CorrelationID 的传递：

1. peer_chat handler 创建时：`CorrelationID = "peer-chat-1234567890"`
2. AgentLoop 接收后：添加到 context
3. MessageTool 使用时：从 context 获取并添加前缀
4. RPCChannel.Send：提取并发送到 respCh

### 方案 3: 检查 LLM 是否正确调用 MessageTool

添加日志检查 LLM 是否调用了 message 工具：

```go
// module/tools/message.go
func (t *MessageTool) Execute(ctx context.Context, args map[string]interface{}) *ToolResult {
    logger.InfoCF("agent", "MessageTool.Execute called",
        map[string]interface{}{
            "channel": t.defaultChannel,
            "chat_id": t.defaultChatID,
            "args": args,
        })
    // ... 现有代码 ...
}
```

## 建议的调试顺序

1. ✅ **检查对端日志**：
   - 查看 peer_chat handler 是否被调用
   - 查看 CorrelationID 是否正确生成
   - 查看 "RPC channel is available" 日志
   - 查看 "Waiting for response..." 日志
   - 查看 "Response received" 或 "Timeout" 日志

2. ✅ **检查 RPCChannel 是否注册**：
   - 查看启动日志中的 "enabled_channels"
   - 应该包含 "rpc"

3. ✅ **检查 MessageTool 是否被调用**：
   - 查看是否有 "MessageTool.Execute called" 日志
   - 查看是否有 "RPC channel detected" 日志
   - 查看 correlation_id 是否正确

4. ✅ **检查 RPCChannel.Send 是否被调用**：
   - 查看是否有 "RPCChannel.Send called" 日志
   - 查看 channel 匹配检查
   - 查看 CorrelationID 提取

5. ✅ **检查响应投递**：
   - 查看是否有 "Delivered response via Send" 日志
   - 或 "No pending request for correlation ID" 日志

## 下一步行动

根据日志输出，定位具体是哪个环节出了问题，然后针对性修复。
