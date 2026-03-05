# RPC 调用完整流程分析

## 调用链路追踪

```
main.go (daemon)
    ↓ line 705
clusterInstance.Call(peerID, "hello", payload)
    ↓ line 442
cluster.go: CallWithContext(context.Background(), ...)
    ↓ line 458
cluster.logger.RPCInfo("Calling %s: action=%s", ...)  ← 用户看到了这个日志
    ↓ line 459
c.rpcClient.CallWithContext(ctx, peerID, action, payload)
    ↓ line 212
client.rateLimiter.Acquire(ctx, peerID)  ← 卡在这里（之前）
    ↓ line 223
c.cluster.LogRPCInfo("Found peer %s", peerID)  ← 用户没有看到
    ↓ line 235
c.cluster.LogRPCInfo("Peer %s is online", peerID)  ← 用户没有看到
    ↓ line 279
c.cluster.LogRPCInfo("Connected to peer ...")  ← 用户没有看到
    ↓ line 286
c.cluster.LogRPCInfo("Sending request ...")  ← 用户没有看到
    ↓ line 296
c.receiveResponseWithContext(ctx, conn, req.ID)  ← 等待响应
    ↓ line 303
c.cluster.LogRPCInfo("Received response ...")  ← 用户没有看到
    ↓
返回到 main.go:709
log("DEBUG", "RPC -> %s: Call returned, err=%v", ...)  ← 用户没有看到
```

## 问题定位

**用户的日志中只有**：
```
[INFO] Calling bot-localhost: action=hello
```

**没有的日志**：
- daemon.log: `Call returned, err=...`
- rpc.log: `Found peer`, `Peer is online`, `Connected`, `Sending request`, `Received response`

**结论**: 代码卡在 `client.go:212` 的 `rateLimiter.Acquire()`

---

## 修复验证

### RateLimiter.Acquire() 修复

**文件**: `module/cluster/rpc/client.go:84-91`

**修复代码**:
```go
func (rl *RateLimiter) Acquire(ctx context.Context, peerID string) error {
    // ✅ 修复：初始化新 peer 的 tokens
    rl.mu.Lock()
    if _, exists := rl.tokens[peerID]; !exists {
        rl.tokens[peerID] = rl.maxTokens      // 10
        rl.requests[peerID] = []time.Time{}
    }
    rl.mu.Unlock()

    // ... 后续代码
}
```

**修复前的问题**:
- `rl.tokens[peerID]` 对于新 peer 返回 0（map 零值）
- Refill 只更新已存在的 peers
- `rl.tokens[peerID]` 永远是 0
- 死循环：`if rl.tokens[peerID] > 0` 永远不成立

**修复后**:
- 新 peer 被初始化为 maxTokens（10）
- `rl.tokens[peerID] > 0` 可以成立
- Acquire 成功返回

---

## 完整验证步骤

让我验证所有关键点：

### 1. 编译验证
```bash
$ go build ./module/...
✅ 编译成功
```

### 2. 代码验证

#### 检查 RateLimiter.Acquire() 是否有修复
```bash
$ grep -A 5 "Initialize peer tokens" module/cluster/rpc/client.go
```

预期输出：
```
    // Initialize peer tokens if not exists
    rl.mu.Lock()
    if _, exists := rl.tokens[peerID]; !exists {
        rl.tokens[peerID] = rl.maxTokens
```

#### 检查所有日志点
```go
// client.go:223 - Found peer
c.cluster.LogRPCInfo("Found peer %s", peerID)

// client.go:235 - Peer is online
c.cluster.LogRPCInfo("Peer %s is online", peerID)

// client.go:270 - Attempting to connect
c.cluster.LogRPCInfo("Attempting to connect to peer %s at %v", peerID, fullAddresses)

// client.go:279 - Connected
c.cluster.LogRPCInfo("Connected to peer %s at %s", peerID, selectedAddress)

// client.go:286 - Sending request
c.cluster.LogRPCInfo("Sending request action=%s to peer %s (id=%s)", req.Action, peerID, req.ID)

// client.go:293 - Request sent
c.cluster.LogRPCInfo("Request sent successfully to peer %s, waiting for response (id=%s)", peerID, req.ID)

// client.go:303 - Received response
c.cluster.LogRPCInfo("Received response from %s: type=%s, id=%s", peerID, response.Type, response.ID)
```

#### 检查服务端响应日志
```go
// server.go:228 - Default response
s.cluster.LogRPCInfo("Response: action=%s, from=%s, to=%s, id=%s, payload=%+v",
    req.Action, req.From, req.To, req.ID, defaultPayload)

// server.go:242 - Error response
s.cluster.LogRPCInfo("Response: action=%s, from=%s, to=%s, id=%s, error=%s",
    req.Action, req.From, req.To, req.ID, err.Error())

// server.go:253 - Success response
s.cluster.LogRPCInfo("Response: action=%s, from=%s, to=%s, id=%s, payload=%+v",
    req.Action, req.From, req.To, req.ID, result)
```

---

## 预期的完整日志输出

### daemon.log
```
[DEBUG] RPC -> bot-localhost: Starting RPC call...
[DEBUG] RPC -> bot-localhost: Calling clusterInstance.Call()
[DEBUG] RPC -> bot-localhost: Call returned, err=<nil>  ← 应该出现
[INFO] RPC -> bot-localhost: Response: {...}  ← 应该出现
```

### rpc.log (客户端)
```
[INFO] Calling bot-localhost: action=hello
[INFO] Found peer bot-localhost  ← 应该出现
[INFO] Peer bot-localhost is online  ← 应该出现
[INFO] Peer bot-localhost addresses: [192.168.137.42]  ← 应该出现
[INFO] Peer bot-localhost RPCPort: 21949  ← 应该出现
[INFO] Attempting to connect to peer bot-localhost at [192.168.137.42:21949]  ← 应该出现
[INFO] Connected to peer bot-localhost at 192.168.137.42:21949  ← 应该出现
[INFO] Sending request action=hello to peer bot-localhost (id=msg-xxx)  ← 应该出现
[INFO] Request sent successfully to peer bot-localhost, waiting for response (id=msg-xxx)  ← 应该出现
[INFO] Received response from bot-localhost: type=response, id=msg-xxx  ← 应该出现
```

### rpc.log (服务端 - bot-localhost 节点的日志)
```
[INFO] Accepted connection from 192.168.137.1:xxxxx  ← 应该出现
[INFO] Received request: action=hello, from=bot-Yaoguai, id=msg-xxx  ← 应该出现
[INFO] Response: action=hello, from=bot-Yaoguai, to=bot-localhost, id=msg-xxx, payload=map[...]  ← 应该出现
```

---

## 可能的问题点

### 1. hello handler 没有注册
**现象**: 服务端返回默认响应
**日志**: `[INFO] No handler for action 'hello'`

**解决**: 确认 hello handler 已注册

### 2. 对端节点没有启动
**现象**: 连接失败
**日志**: `[ERROR] Failed to connect to peer xxx: connection refused`

**解决**: 确保两个节点都在运行

### 3. 网络不通
**现象**: 连接超时
**日志**: `[ERROR] Failed to connect to peer xxx: timeout`

**解决**: 检查防火墙和网络连接

### 4. 响应超时
**现象**: 请求发送成功，但没有响应
**日志**: `[ERROR] Failed to receive response: timeout`

**解决**: 检查服务端处理逻辑

---

## 当前状态

### 已修复
- ✅ RateLimiter.Acquire() 初始化问题
- ✅ 客户端追踪日志
- ✅ 服务端响应日志

### 待验证
- ⏳ 实际运行测试
- ⏳ 跨机器通信测试
- ⏳ hello handler 注册确认

---

## 下一步

1. **重新编译**: `build.bat`
2. **启动两个节点**: 在不同机器上
3. **查看日志**: 确认完整流程
4. **报告结果**: 所有日志是否都出现

---

**分析完成时间**: 2026-03-04 21:20
**修复状态**: ✅ RateLimiter bug 已修复
**编译状态**: ✅ 成功
**待测试**: 实际环境验证
