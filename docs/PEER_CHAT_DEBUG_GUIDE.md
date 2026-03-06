# Peer Chat RPC 调试指南

## 已添加的调试日志

我已在关键位置添加了详细的 INFO 级别日志，帮助你诊断 peer_chat RPC 响应问题。

### 1. peer_chat_handler.go - 请求处理日志

**位置**: `module/cluster/rpc/peer_chat_handler.go`

**新增日志**:
```
[INFO] [PeerChat] Processing {type} request
[INFO] [PeerChat] Request content: {content}
[INFO] [PeerChat] RPC channel is available
[INFO] [PeerChat] Created inbound message: chat_id={id}, correlation_id={id}
[DEBUG] [PeerChat] Inbound message details: channel={ch}, sender={sender}, session={session}
[INFO] [PeerChat] Request sent to MessageBus, waiting for LLM response (correlation_id={id})
[INFO] [PeerChat] Response received! correlation_id={id}, response={response}
[ERROR] [PeerChat] Timeout waiting for response (correlation_id={id})
```

**用途**:
- 追踪 peer_chat handler 是否被调用
- 检查 RPCChannel 是否可用
- 查看请求是否成功发送到 MessageBus
- 确认是否收到响应或超时

### 2. rpc_channel.go - 响应投递日志

**位置**: `module/channels/rpc_channel.go`

**新增日志**:
```
[INFO] RPCChannel.Send called {msg_channel: {ch}, ch_name: {name}, chat_id: {id}, content_len: {len}}
[WARN] Channel mismatch - message not for this channel {msg_channel: {msg}, ch_name: {name}}
[INFO] Channel matched, processing message {content_preview: {preview}}
[WARN] No correlation ID in message {content: {content}}
[INFO] Extracted correlation ID from message {correlation_id: {id}}
[INFO] Found pending request, delivering response {correlation_id: {id}, content_len: {len}, content_preview: {preview}}
[INFO] ✅ Response delivered successfully via Send {correlation_id: {id}}
[WARN] ⚠️ No pending request found for correlation ID {correlation_id: {id}, pending_count: {count}}
[DEBUG] Pending correlation IDs {ids: [...]}
```

**用途**:
- 确认 RPCChannel.Send 是否被调用
- 检查 channel 名称是否匹配
- 验证 correlation ID 是否正确提取
- 确认是否找到对应的 pending request
- 查看响应是否成功投递

### 3. message.go - MessageTool 日志

**位置**: `module/tools/message.go`

**新增日志**:
```
[INFO] MessageTool: RPC channel detected {correlation_id: {id}, content_preview: {preview}}
[INFO] MessageTool: Added correlation ID prefix to RPC message {correlation_id: {id}, final_content_preview: {preview}}
[WARN] MessageTool: ⚠️ No correlation ID in context for RPC channel - response will not be delivered! {content_preview: {preview}}
```

**用途**:
- 确认 MessageTool 是否检测到 RPC channel
- 验证 correlation ID 是否从 context 中获取
- 检查响应消息的格式是否正确

## 如何使用这些日志诊断问题

### 步骤 1: 重启你的两个节点

确保新的日志代码生效：
```bash
# 停止两个节点
# 重新编译和启动
```

### 步骤 2: 触发 peer_chat 请求

从本端发送 peer_chat 请求到对端。

### 步骤 3: 查看对端日志

在对端的日志中查找以下关键信息：

#### 3.1 peer_chat handler 日志

**期望看到**:
```
[INFO] [PeerChat] Processing chat request
[INFO] [PeerChat] Request content: 你好！我是本地节点...
[INFO] [PeerChat] RPC channel is available
[INFO] [PeerChat] Created inbound message: chat_id={id}, correlation_id={id}
[INFO] [PeerChat] Request sent to MessageBus, waiting for LLM response (correlation_id={id})
```

**如果没有看到**:
- `[PeerChat] Processing` → peer_chat handler 没有被调用，检查 RPC server 是否正确注册了 handler
- `RPC channel is available` → RPCChannel 是 nil，检查 RPCChannel 是否正确初始化和设置

#### 3.2 AgentLoop 处理日志

在对端日志中查找：
```
[INFO] Processing message from rpc:...
[INFO] Routed message {agent_id: ..., session_key: ...}
```

**如果没有看到**:
- AgentLoop 可能没有从 MessageBus 接收到消息
- 检查 AgentLoop 是否正在运行

#### 3.3 MessageTool 日志

**期望看到**:
```
[INFO] MessageTool: RPC channel detected {correlation_id: peer-chat-1234567890, ...}
[INFO] MessageTool: Added correlation ID prefix to RPC message {correlation_id: peer-chat-1234567890, ...}
```

**如果看到警告**:
```
[WARN] MessageTool: ⚠️ No correlation ID in context for RPC channel - response will not be delivered!
```

**原因**:
- CorrelationID 没有正确添加到 context
- 检查 AgentLoop.Run 中的 context 设置代码

#### 3.4 RPCChannel.Send 日志

**期望看到**:
```
[INFO] RPCChannel.Send called {msg_channel: rpc, ch_name: rpc, ...}
[INFO] Channel matched, processing message
[INFO] Extracted correlation ID from message {correlation_id: peer-chat-1234567890}
[INFO] Found pending request, delivering response
[INFO] ✅ Response delivered successfully via Send
```

**可能的问题**:

**问题 A**: Channel 不匹配
```
[WARN] Channel mismatch - message not for this channel {msg_channel: rpc, ch_name: something_else}
```
- RPCChannel 的名称不是 "rpc"
- 检查 RPCChannel.Name() 的返回值

**问题 B**: 找不到 correlation ID
```
[WARN] No correlation ID in message {content: response without prefix}
```
- MessageTool 没有添加 correlation ID 前缀
- 或者格式不正确

**问题 C**: 找不到 pending request
```
[WARN] ⚠️ No pending request found for correlation ID {correlation_id: peer-chat-1234567890, pending_count: 0}
[DEBUG] Pending correlation IDs {ids: []}
```
- pendingReqs 中没有这个 correlation ID
- 可能的原因：
  1. correlation ID 不匹配（格式问题）
  2. pending request 已经超时被清理
  3. 请求没有被正确注册

### 步骤 4: 查看本端日志

在本端（调用方）查找：

```
[INFO] Calling {peer_id}: action=peer_chat
[INFO] Found peer {peer_id}
[INFO] Peer {peer_id} is online
[INFO] Connected to peer {peer_id} at {address}
[INFO] Sending request action=peer_chat to peer {peer_id} (id={req_id})
[INFO] Request sent successfully to peer {peer_id}, waiting for response (id={req_id})
```

**如果超时**:
```
[ERROR] Failed to receive response from {peer_id} for {req_id}: timeout waiting for response
```

## 常见问题和解决方案

### 问题 1: 对端日志显示 "No correlation ID in context"

**症状**:
```
[WARN] MessageTool: ⚠️ No correlation ID in context for RPC channel
```

**原因**:
CorrelationID 没有从 InboundMessage 传递到 context。

**检查**:
1. 查看 AgentLoop.Run 中的代码：
```go
ctx = context.WithValue(ctx, "correlation_id", msg.CorrelationID)
```
2. 确认这行代码在 processMessage 之前执行

**解决方案**:
检查 context 是否在某个环节被重置或覆盖。

### 问题 2: 对端日志显示 "No pending request found"

**症状**:
```
[WARN] ⚠️ No pending request found for correlation ID {correlation_id: peer-chat-123, pending_count: 0}
```

**可能原因**:
1. CorrelationID 格式不匹配
2. Pending request 已经超时被清理
3. 请求没有被正确注册

**检查**:
1. 对比 peer_chat handler 创建的 correlation ID 和 MessageTool 添加的 correlation ID
2. 检查是否一致

**解决方案**:
如果格式不一致，需要统一 correlation ID 的生成和使用方式。

### 问题 3: 本端一直超时，对端没有任何日志

**症状**:
- 本端显示 "timeout waiting for response"
- 对端日志中没有任何 peer_chat 相关的日志

**原因**:
请求没有到达对端的 peer_chat handler。

**检查**:
1. 对端的 RPC server 是否正在运行
2. peer_chat handler 是否正确注册
3. 网络连接是否正常

**解决方案**:
1. 检查对端日志中的 RPC server 启动日志
2. 查看 handler 注册日志
3. 使用 ping 测试网络连接

### 问题 4: Channel mismatch

**症状**:
```
[WARN] Channel mismatch - message not for this channel {msg_channel: rpc, ch_name: something_else}
```

**原因**:
RPCChannel 的名称不是 "rpc"。

**检查**:
查看 RPCChannel 的 Name() 方法返回什么。

**解决方案**:
确保 RPCChannel 注册到 ChannelManager 时使用的名称是 "rpc"：
```go
cm.RegisterChannel("rpc", rpcCh)
```

## 手动验证步骤

### 验证 1: RPCChannel 是否注册成功

在对端启动日志中查找：
```
[INFO] agent: RPC channel registered to channel manager
```

如果没有这条日志，说明 RPCChannel 没有被注册到 ChannelManager。

### 验证 2: ChannelManager 中是否有 "rpc" channel

查看启动日志中的 "enabled_channels" 列表，应该包含 "rpc"。

### 验证 3: ping 测试

先使用 ping action 测试两个节点之间的连接：
```json
{
  "action": "ping",
  "peer_id": "对端节点ID"
}
```

如果 ping 成功但 peer_chat 失败，说明网络连接正常，问题在于 peer_chat handler 或响应处理。

## 下一步行动

1. **重启两个节点**，确保新日志代码生效
2. **触发 peer_chat 请求**
3. **收集对端和本端的完整日志**
4. **根据日志定位具体问题**
5. **针对性修复**

如果你能提供完整的日志输出，我可以帮你进一步分析问题所在。
