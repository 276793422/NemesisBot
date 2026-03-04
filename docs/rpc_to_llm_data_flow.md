═══════════════════════════════════════════════════════════════
         RPC 到 LLM 数据传递完整流程分析报告
═══════════════════════════════════════════════════════════════

## 📋 问题说明

Bot A 如何通过 RPC 调用 Bot B 的 LLM，以及数据如何在各个组件之间传递。

═══════════════════════════════════════════════════════════════

## 🔄 完整数据流图

```
┌─────────────────────────────────────────────────────────────────────┐
│ Bot A (调用方)                                                       │
│                                                                     │
│  1. ClusterRPCTool.Execute()                                      │
│     - 生成 RPC 请求                                                 │
│     - peer_id: "bot-B"                                             │
│     - action: "llm_forward"                                       │
│     - data: {chat_id, content, ...}                               │
└───────────────────────────┬─────────────────────────────────────────┘
                            │ TCP (RPC JSON)
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│ Bot B (服务方)                                                       │
│                                                                     │
│  2. RPC Server 接收请求                                            │
│     - 解析 JSON 为 RPCMessage                                       │
│     - action = "llm_forward"                                       │
│     - 查找 handler                                                 │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  3. LLMForwardHandler.Handle(payload)                               │
│                                                                     │
│     步骤 3.1: 解析 RPC Payload                                      │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: llm_forward_handler.go:50-88                     │  │
│     │                                                            │  │
│     | 输入 (JSON):                                               |  │
│     | {                                                          |  │
│     |   "chat_id": "user-123",                                   |  │
│     |   "content": "What is AI?",                                |  │
│     |   "sender_id": "bot-A",                                    |  │
│     |   "session_key": "session-abc",                            |  │
│     |   "metadata": {}                                           |  │
│     | }                                                          |  │
│     │                                                            │  │
│     │ 输出 (LLMForwardPayload 结构体):                           │  │
│     │ - ChatID: "user-123"                                       │  │
│     │ - Content: "What is AI?"                                   │  │
│     │ - SenderID: "bot-A"                                        │  │
│     │ - SessionKey: "session-abc"                                │  │
│     │ - Metadata: map[string]string                             │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 3.2: 构造 InboundMessage                                   │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: llm_forward_handler.go:71-88                     │  │
│     │                                                            │  │
│     │ inbound := bus.InboundMessage{                            │  │
│     │     Channel:    "rpc",      // ← 设置为 "rpc"             │  │
│     │     ChatID:     "user-123",                                 │  │
│     │     Content:    "What is AI?",                             │  │
│     │     SenderID:   "bot-A",                                   │  │
│     │     SessionKey: "session-abc",                             │  │
│     │     Metadata:   {},                                        │  │
│     │     // CorrelationID 将在下一步生成                         │  │
│     │ }                                                          │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 3.3: 调用 RPCChannel.Input()                               │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: llm_forward_handler.go:94-98                     │  │
│     │                                                            │  │
│     │ respCh, err := h.rpcChannel.Input(ctx, &inbound)          │  │
│     │                                                            │  │
│     │ 返回值:                                                    │  │
│     │ - respCh: <-chan string (响应通道，等待LLM响应)            │  │
│     │ - err: error (错误信息)                                    │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  4. RPCChannel.Input(ctx, inbound)                                  │
│                                                                     │
│     步骤 4.1: 生成 CorrelationID                                    │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:189-191                           │  │
│     │                                                            │  │
│     │ if inbound.CorrelationID == "" {                          │  │
│     │     inbound.CorrelationID = generateCorrelationID()       │  │
│     │     // 生成格式: "rpc-1709553600123456789"                │  │
│     │ }                                                          │  │
│     │                                                            │  │
│     │ inbound.Channel = ch.Name()  // 设置为 "rpc"               │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 4.2: 创建待处理请求并注册                                  │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:194-210                          │  │
│     │                                                            │  │
│     │ // 创建响应通道                                           │  │
│     │ respCh := make(chan string, 1)                           │  │
│     │                                                            │  │
│     │ // 注册到 pendingReqs map                                 │  │
│     │ ch.pendingReqs[inbound.CorrelationID] = &pendingRequest{  │  │
│     │     correlationID: "rpc-1709553600123456789",              │  │
│     │     responseCh:    respCh,                                │  │
│     │     createdAt:     time.Now(),                            │  │
│     │     timeout:       60s,                                   │  │
│     │ }                                                          │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 4.3: 发送到 MessageBus                                      │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:213                              │  │
│     │                                                            │  │
│     │ ch.base.bus.PublishInbound(*inbound)                      │  │
│     │                                                            │  │
│     │ 此时 InboundMessage 包含:                                  │  │
│     │ - Channel: "rpc"                                          │  │
│     │ - ChatID: "user-123"                                       │  │
│     │ - Content: "What is AI?"                                   │  │
│     │ - CorrelationID: "rpc-1709553600123456789"  ← 关键！       │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 4.4: 返回响应通道                                          │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:215                              │  │
│     │                                                            │  │
│     │ return respCh, nil  // 返回响应通道                       │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  5. MessageBus → AgentLoop.processMessage()                         │
│                                                                     │
│     步骤 5.1: 接收 InboundMessage                                    │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: loop.go:506-510                                  │  │
│     │                                                            │  │
│     │ // 如果消息包含 CorrelationID，添加到 context               │  │
│     │ if msg.CorrelationID != "" {                              │  │
│     │     ctx = context.WithValue(ctx, "correlation_id",         │  │
│     │         msg.CorrelationID)                                │  │
│     │ }                                                          │  │
│     │                                                            │  │
│     │ // 现在 ctx 包含 correlation_id                            │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 5.2: 调用 runAgentLoop                                     │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: loop.go:512                                      │  │
│     │                                                            │  │
│     │ result, err := al.runAgentLoop(ctx, agent, {              │  │
│     │     SessionKey:  "session-abc",                            │  │
│     │     Channel:     "rpc",                                   │  │
│     │     ChatID:      "user-123",                               │  │
│     │     UserMessage: "What is AI?",                           │  │
│     │     ...                                                    │  │
│     │ })                                                         │  │
│     │                                                            │  │
│     │ // ctx (包含 correlation_id) 被传递给 LLM 和 Tools        │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  6. LLM 处理 (Bot B 的 LLM 配置和工具)                              │
│                                                                     │
│     步骤 6.1: LLM 生成回复                                          │
│     - 使用 Bot B 的 LLM 配置                                       │
│     - 使用 Bot B 的工具集                                          │
│     - 生成回复内容                                                 │
│                                                                     │
│     步骤 6.2: MessageTool 发送回复                                   │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: message.go:92-99                                │  │
│     │                                                            │  │
│     │ // 检测 channel == "rpc"                                  │  │
│     │ if channel == "rpc" {                                     │  │
│     │     // 从 context 读取 correlation_id                      │  │
│     │     if correlationID := getCorrelationIDFromContext(ctx); │  │
│     │         correlationID != "" {                             │  │
│     │         // 在内容前添加 CorrelationID 前缀！              │  │
│     │         finalContent = fmt.Sprintf("[rpc:%s] %s",        │  │
│     │             correlationID, content)                        │  │
│     │     }                                                      │  │
│     │ }                                                          │  │
│     │                                                            │  │
│     │ // 结果:                                                 │  │
│     │ // "[rpc:rpc-1709553600123456789] AI is..."              │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  7. MessageBus.PublishOutbound()                                    │
│                                                                     │
│     步骤 7.1: MessageTool 调用 sendCallback                          │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: message.go:101                                  │  │
│     │                                                            │  │
│     │ t.sendCallback(channel, chatID, finalContent)             │  │
│     │                                                            │  │
│     │ // 触发:                                                 │  │
│     │ // msgBus.PublishOutbound(OutboundMessage{                │  │
│     │ //   Channel: "rpc",                                      │  │
│     │ //   ChatID: "user-123",                                   │  │
│     │ //   Content: "[rpc:rpc-1709553600123456789] AI is...",    │  │
│     │ // })                                                      │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  8. RPCChannel.outboundListener()                                   │
│                                                                     │
│     步骤 8.1: 监听 OutboundChannel                                   │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:234-243                          │  │
│     │                                                            │  │
│     │ case msg, ok := <-ch.base.bus.OutboundChannel():          │  │
│     │     // 只处理来自 "rpc" channel 的消息                     │  │
│     │     if msg.Channel != ch.Name() {                         │  │
│     │         continue  // 跳过其他 channel                     │  │
│     │     }                                                      │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 8.2: 提取 CorrelationID                                    │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:245-253                          │  │
│     │                                                            │  │
│     │ // Content 格式: "[rpc:rpc-xxx] actual content"           │  │
│     │ correlationID := extractCorrelationID(msg.Content)        │  │
│     │                                                            │  │
│     │ // 提取出 "rpc-1709553600123456789"                       │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 8.3: 匹配待处理请求                                        │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: rpc_channel.go:256-272                          │  │
│     │                                                            │  │
│     │ // 查找 pendingReqs[correlationID]                        │  │
│     │ req, exists := ch.pendingReqs[correlationID]              │  │
│     │                                                            │  │
│     │ if exists {                                               │  │
│     │     // 移除 CorrelationID 前缀                            │  │
│     │     actualContent := removeCorrelationID(msg.Content)      │  │
│     │     // "AI is..."                                         │  │
│     │                                                            │  │
│     │     // 发送到响应通道                                      │  │
│     │     req.responseCh <- actualContent                        │  │
│     │ }                                                          │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  9. LLMForwardHandler.Handle() 收到响应                             │
│                                                                     │
│     步骤 9.1: 等待响应通道                                           │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: llm_forward_handler.go:107-119                   │  │
│     │                                                            │  │
│     │ case response, ok := <-respCh:                            │  │
│     │     if !ok {                                              │  │
│     │         // 超时或通道关闭                                  │  │
│     │         return errorResponse("LLM processing timeout")     │  │
│     │     }                                                      │  │
│     │                                                            │  │
│     │     // 收到响应！                                          │  │
│     │     return successResponse(response)                       │  │
│     └───────────────────────────────────────────────────────────┘  │
│                                                                     │
│     步骤 9.2: 构造 RPC 响应                                         │
│     ┌───────────────────────────────────────────────────────────┐  │
│     │ 代码位置: llm_forward_handler.go:148-152                   │  │
│     │                                                            │  │
│     │ return map[string]interface{}{                           │  │
│     │     "success": true,                                      │  │
│     │     "content": "AI is...",  // ← LLM 的实际响应          │  │
│     │ }                                                          │  │
│     └───────────────────────────────────────────────────────────┘  │
└───────────────────────────┬─────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────────┐
│  10. RPC Server 返回响应                                           │
│                                                                      │
│      发送 JSON 响应给 Bot A:                                         │
│      {                                                               │
│        "version": "1.0",                                             │
│        "id": "msg-xxx",  // 与请求相同                               │
│        "type": "response",                                          │
│        "payload": {                                                 │
│          "success": true,                                           │
│          "content": "AI is..."                                      │
│        }                                                             │
│      }                                                               │
└─────────────────────────────────────────────────────────────────────┘

═══════════════════════════════════════════════════════════════

## 🔑 关键机制说明

### 1. CorrelationID 生成与传递

| 阶段 | 位置 | 说明 |
|------|------|------|
| 生成 | RPCChannel.Input() | 生成唯一ID: "rpc-{unix_nano}" |
| 传递到context | AgentLoop.processMessage() | context.WithValue(ctx, "correlation_id", id) |
| 传递给LLM | runAgentLoop() | ctx作为参数传递 |
| 添加到响应 | MessageTool.Execute() | 检测到"rpc" channel，添加前缀 |
| 提取匹配 | RPCChannel.outboundListener() | 从Content提取ID，匹配pending请求 |

### 2. 数据格式转换

| 阶段 | 数据类型 | 示例 |
|------|---------|------|
| RPC Payload | map[string]interface{} | {chat_id, content, ...} |
| InboundMessage | 结构体 | {Channel, ChatID, Content, CorrelationID} |
| Context Value | interface{} | "rpc-1709553600123456789" |
| LLM Response | string | "AI is..." |
| Outbound Content | string (带前缀) | "[rpc:rpc-xxx] AI is..." |
| RPC Response | map[string]interface{} | {success: true, content: "AI is..."} |

### 3. 异步等待机制

```
LLMForwardHandler.Handle()
    |
    |-- 1. rpcChannel.Input() → 注册pending请求
    |       |
    |       |-- 返回 respCh
    |
    |-- 2. <-respCh (阻塞等待)
    |       |
    |       |-- RPCChannel.outboundListener() 监听
    |       |-- 收到 OutboundMessage
    |       |-- 提取 CorrelationID
    |       |-- 匹配 pendingReqs[id]
    |       |-- responseCh <- content
    |
    └── 3. 收到响应，返回
```

═══════════════════════════════════════════════════════════════

## 📊 数据结构对照表

### RPC Payload → InboundMessage

| RPC Payload 字段 | InboundMessage 字段 | 值示例 |
|-----------------|---------------------|--------|
| data.chat_id | ChatID | "user-123" |
| data.content | Content | "What is AI?" |
| data.sender_id | SenderID | "bot-A" |
| data.session_key | SessionKey | "session-abc" |
| data.metadata | Metadata | map[string]string |
| - | Channel | "rpc" (自动设置) |
| - | CorrelationID | "rpc-1709553600123456789" (自动生成) |

### LLM Response → RPC Response

| LLM Response | Outbound Content | RPC Response Content |
|--------------|-----------------|---------------------|
| "AI is..." | "[rpc:rpc-xxx] AI is..." | "AI is..." (去掉前缀) |

═══════════════════════════════════════════════════════════════

## 🎯 核心设计要点

1. **Channel 模式**: RPCChannel 实现标准的 Channel 接口
2. **CorrelationID**: 用于匹配请求和响应的唯一标识
3. **Context 传递**: 通过 context 传递 CorrelationID，不修改 MessageBus 签名
4. **响应前缀**: 使用 `[rpc:correlation_id]` 前缀在响应中传递 ID
5. **异步匹配**: pendingReqs map 实现异步响应匹配
6. **超时处理**: 60秒超时，自动清理过期请求

═══════════════════════════════════════════════════════════════
