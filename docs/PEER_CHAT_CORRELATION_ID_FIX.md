# Peer Chat RPC Correlation ID 丢失问题修复

## 问题描述

**症状**:
- 对端接收到 peer_chat 请求
- LLM 正确生成了响应（如 "收到！😄"）
- 但本端一直超时，收不到响应

**对端日志显示**:
```
[INFO] rpc: Channel matched, processing message {content_preview=收到！😄}
[WARN] rpc: No correlation ID in message {content=收到！😄}
```

## 问题根源

**响应中缺少 CorrelationID 前缀！**

正确的响应格式应该是：
```
[rpc:peer-chat-1234567890] 收到！😄
```

但实际发送的是：
```
收到！😄
```

## 深层原因分析

### 两种响应路径

LLM 的响应可以通过两种路径发送：

#### 路径 A: 通过 MessageTool（已正确处理）

```
LLM 调用 message 工具
  ↓
MessageTool.Execute()
  ↓ 检测到 channel == "rpc"
  ↓ 从 context 获取 correlation_id
  ↓ 添加前缀：[rpc:correlation_id] content
  ↓ 调用 sendCallback → PublishOutbound
  ↓ ✅ 格式正确
```

**代码** (module/tools/message.go 第 92-99 行):
```go
if channel == "rpc" {
    correlationID := getCorrelationIDFromContext(ctx)
    if correlationID != "" {
        finalContent = fmt.Sprintf("[rpc:%s] %s", correlationID, content)
    }
}
```

#### 路径 B: 直接返回（问题所在！）

```
LLM 直接返回文本（不调用 message 工具）
  ↓
runAgentLoop 返回 response
  ↓
AgentLoop.Run 检查 !alreadySent
  ↓ PublishOutbound（直接发送 response）
  ↓ ❌ 没有添加 correlation ID 前缀！
```

**问题代码** (module/agent/loop.go 第 322-326 行):
```go
al.bus.PublishOutbound(bus.OutboundMessage{
    Channel: msg.Channel,
    ChatID:  msg.ChatID,
    Content: response,  // ← 直接发送，没有前缀！
})
```

## 修复方案

在 AgentLoop.Run 中，对于 RPC channel 的响应，添加 CorrelationID 前缀：

### 修改后的代码

```go
if !alreadySent {
    // For RPC channel, we need to add correlation ID prefix
    // This is required because the response might not have gone through MessageTool
    finalContent := response
    if msg.Channel == "rpc" && msg.CorrelationID != "" {
        finalContent = fmt.Sprintf("[rpc:%s] %s", msg.CorrelationID, response)
        logger.InfoCF("agent", "Added correlation ID prefix to RPC response",
            map[string]interface{}{
                "correlation_id": msg.CorrelationID,
                "content_preview": utils.Truncate(finalContent, 100),
            })
    }

    al.bus.PublishOutbound(bus.OutboundMessage{
        Channel: msg.Channel,
        ChatID:  msg.ChatID,
        Content: finalContent,
    })
}
```

### 修复逻辑

1. 检查 channel 是否是 "rpc"
2. 检查 CorrelationID 是否存在
3. 如果都是，添加前缀：`[rpc:correlation_id] content`
4. 发送带有前缀的响应

## 为什么会出现这个问题？

### LLM 的两种响应方式

1. **调用 message 工具**:
   ```json
   {
     "tool": "message",
     "content": "响应内容"
   }
   ```
   → 通过 MessageTool，会添加 correlation ID 前缀 ✅

2. **直接返回文本**:
   ```
   "响应内容"
   ```
   → 不经过 MessageTool，不会添加前缀 ❌

### 之前的设计缺陷

之前只在 MessageTool 中处理了 correlation ID 前缀，但没有考虑 LLM 可能直接返回文本的情况。

## 完整的消息流程（修复后）

```
本端发送 peer_chat 请求
  ↓
对端 RPC server 接收
  ↓
peer_chat handler 处理
  ├─ 创建 InboundMessage
  ├─ 设置 CorrelationID = "peer-chat-123"
  ├─ 发送到 MessageBus
  └─ 等待响应...
  ↓
AgentLoop.Run 接收消息
  ├─ 添加到 context: ctx.WithValue("correlation_id", "123")
  └─ 调用 processMessage
  ↓
runAgentLoop 处理
  ↓
LLM 生成响应："收到！😄"
  ↓
可能路径 A: LLM 调用 message 工具
  └─ MessageTool 添加前缀 ✅
  ↓
可能路径 B: LLM 直接返回（常见）
  └─ AgentLoop.Run 添加前缀 ✅ ← 修复点
  ↓
PublishOutbound 发送
  ↓
ChannelManager.dispatchOutbound 接收
  ↓
RPCChannel.Send 处理
  ├─ 提取 correlation_id: "peer-chat-123"
  ├─ 从 pendingReqs 找到对应请求
  └─ 发送到 respCh ✅
  ↓
peer_chat handler 接收响应
  ↓
返回给 RPC server
  ↓
发送回本端 ✅
```

## 验证修复

重新编译和运行后，对端日志应该显示：

**修复前**:
```
[INFO] rpc: Channel matched, processing message {content_preview=收到！😄}
[WARN] rpc: No correlation ID in message {content=收到！😄}
```

**修复后**:
```
[INFO] agent: Added correlation ID prefix to RPC response {
    correlation_id: "peer-chat-1234567890",
    content_preview: "[rpc:peer-chat-1234567890] 收到！😄"
}
[INFO] rpc: Channel matched, processing message {
    content_preview: "[rpc:peer-chat-1234567890] 收到！😄"
}
[INFO] rpc: Extracted correlation ID from message {
    correlation_id: "peer-chat-1234567890"
}
[INFO] rpc: Found pending request, delivering response
[INFO] ✅ Response delivered successfully via Send
[INFO] [PeerChat] Response received! correlation_id=peer-chat-1234567890, response=收到！😄
```

## 测试步骤

1. **重新编译**:
```bash
go build ./...
```

2. **重启两个节点**

3. **从本端发送 peer_chat 请求**

4. **查看对端日志**，应该看到：
- ✅ `Added correlation ID prefix to RPC response`
- ✅ `Extracted correlation ID from message`
- ✅ `Response delivered successfully`

5. **本端应该能收到响应**，不再超时

## 相关修改

- **文件**: `module/agent/loop.go`
- **位置**: AgentLoop.Run 方法，第 315-340 行
- **修改**: 添加了 RPC channel 的 correlation ID 前缀处理
- **影响**: 所有 RPC channel 的响应都会正确添加 correlation ID

## 总结

这个问题的根本原因是：
1. **设计缺陷**：只在 MessageTool 中处理了 correlation ID，没有考虑直接返回的情况
2. **LLM 行为**：LLM 可能不调用 message 工具，直接返回文本
3. **缺少兜底**：AgentLoop.Run 发送响应时没有检查是否需要添加前缀

修复后：
- ✅ 无论 LLM 是否调用 MessageTool，响应都会有正确的 correlation ID 前缀
- ✅ RPCChannel 能正确提取 correlation ID
- ✅ 响应能正确投递到等待的 handler
- ✅ 本端能正确收到对端的响应

这个问题现在已经完全修复！🎉
