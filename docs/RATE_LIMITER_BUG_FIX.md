# RPC 调用卡住问题 - Bug 修复报告

## 问题描述

**现象**: 在 `nemesisbot daemon cluster auto` 模式下，RPC 调用卡住，无法完成。

**用户反馈**:
- daemon.log 显示 `Calling clusterInstance.Call()` 后就没有后续日志
- rpc.log 只有客户端的 "Calling" 日志，没有服务端响应
- 添加的追踪日志（"Found peer"、"Connected to peer" 等）完全没有输出

---

## 根本原因

### RateLimiter 的严重 Bug

**文件**: `module/cluster/rpc/client.go`
**函数**: `RateLimiter.Acquire()`

#### Bug 代码（修复前）

```go
func (rl *RateLimiter) Acquire(ctx context.Context, peerID string) error {
    // ... refill logic ...

    // Acquire token with retry logic
    for {
        rl.mu.Lock()

        // Refill tokens again
        if time.Since(rl.lastRefill) > rl.refillRate {
            rl.lastRefill = time.Now()
            for peer := range rl.tokens {  // ❌ 只 refill 已存在的 peers
                rl.tokens[peer] = rl.maxTokens
            }
        }

        if rl.tokens[peerID] > 0 {  // ❌ 新 peerID 的值是 0（map 零值）
            // Acquire token
            rl.tokens[peerID]--
            rl.mu.Unlock()
            return nil
        }

        // No token available, wait and retry
        rl.mu.Unlock()
        select {
        case <-ctx.Done():
            return ctx.Err()
        case <-time.After(100 * time.Millisecond):
            continue  // ❌ 死循环：永远不会有 token
        }
    }
}
```

#### 问题分析

1. **新 peerID 初始值为 0**
   - Go 的 map 中，不存在的 key 返回零值
   - `rl.tokens[peerID]` 对于新 peer 返回 `0`

2. **Refill 只更新已存在的 peers**
   - `for peer := range rl.tokens` 只遍历已存在的 key
   - 新 peerID 永远不会被加入 map
   - 新 peerID 永远得不到 tokens

3. **死循环**
   - `rl.tokens[peerID]` 永远是 `0`
   - 条件 `rl.tokens[peerID] > 0` 永远不成立
   - 进入无限等待循环

#### 调用链

```
main.go: clusterInstance.Call(peerID, "hello", payload)
    ↓
cluster.Call(): return c.rpcClient.CallWithContext(...)
    ↓
client.CallWithContext(): c.rateLimiter.Acquire(ctx, peerID)  // ❌ 卡在这里！
    ↓
RateLimiter.Acquire(): 进入死循环，永远不会返回
```

---

## 修复方案

### 修复代码

**文件**: `module/cluster/rpc/client.go:84`

```go
func (rl *RateLimiter) Acquire(ctx context.Context, peerID string) error {
    // ✅ 新增：初始化新 peer 的 tokens
    rl.mu.Lock()
    if _, exists := rl.tokens[peerID]; !exists {
        rl.tokens[peerID] = rl.maxTokens
        rl.requests[peerID] = []time.Time{}
    }
    rl.mu.Unlock()

    // Refill tokens periodically
    // ... 后续代码不变 ...
}
```

### 修复逻辑

1. **检查 peerID 是否存在**
   - `if _, exists := rl.tokens[peerID]; !exists`

2. **初始化新 peer**
   - 设置 tokens 为 `maxTokens`（默认 10）
   - 初始化 requests 为空 slice

3. **确保后续流程正常**
   - `rl.tokens[peerID] > 0` 条件可以成立
   - 能够成功 acquire token
   - 函数正常返回

---

## 测试验证

### 编译验证

```bash
$ go build ./module/...
✅ 编译成功，无错误

$ go build -o nemesisbot.exe ./nemesisbot/
✅ 主程序编译成功
```

### 逻辑验证

**测试代码**:
```go
tokens := make(map[string]int)
maxTokens := 10
peerID := "new-peer"

// Before fix: tokens[peerID] == 0 (zero value)
// After fix: initialize if not exists
if _, exists := tokens[peerID]; !exists {
    tokens[peerID] = maxTokens
}

if tokens[peerID] > 0 {
    tokens[peerID]--  // ✅ Success!
}
```

**测试结果**:
```
✅ Initialized new-peer-test with 10 tokens
✅ Acquired token, remaining: 9
```

---

## 为什么之前的测试"通过"了？

### 我的测试方式

1. **创建独立测试程序** `test_rpc_enhanced.go`
2. **直接使用 RPC Server**，不经过 RateLimiter
3. **服务端和客户端在同一机器**
4. **日志输出到控制台**（不是文件）

### 实际运行环境

1. **使用真实的 Cluster**
2. **经过完整的 RPC 调用链**
3. **跨机器网络调用**
4. **日志写入文件**

### 差异

| 方面 | 我的测试 | 实际运行 |
|------|---------|---------|
| 调用路径 | 直接 RPC Server | Cluster → Client → RateLimiter → RPC |
| RateLimiter | **绕过了** | **必须经过** ❌ |
| 网络环境 | 本地回环 | 跨机器 TCP |
| 日志输出 | 控制台 | 文件 |

**结论**: 我的测试**没有经过 RateLimiter**，所以没有发现这个 Bug！

---

## 预期效果

修复后，RPC 调用应该能够正常完成：

### daemon.log

```
[DEBUG] RPC -> bot-localhost: Starting RPC call...
[DEBUG] RPC -> bot-localhost: Calling clusterInstance.Call()
[DEBUG] RPC -> bot-localhost: Call returned, err=<nil>  ← ✅ 应该出现
[INFO] RPC -> bot-localhost: Response: {...}             ← ✅ 应该出现
```

### rpc.log

```
[INFO] Calling bot-localhost: action=hello
[INFO] Found peer bot-localhost                    ← ✅ 应该出现
[INFO] Peer bot-localhost is online                ← ✅ 应该出现
[INFO] Peer bot-localhost addresses: [...]         ← ✅ 应该出现
[INFO] Attempting to connect to peer bot-localhost ← ✅ 应该出现
[INFO] Connected to peer bot-localhost at ...      ← ✅ 应该出现
[INFO] Sending request action=hello to peer ...    ← ✅ 应该出现
[INFO] Request sent successfully to peer ...       ← ✅ 应该出现
[INFO] Received response from bot-localhost ...    ← ✅ 应该出现

# 服务端日志
[INFO] Accepted connection from ...
[INFO] Received request: action=hello, from=...
[INFO] Response: action=hello, from=..., to=..., payload=...  ← ✅ 应该出现
```

---

## 总结

### 问题

- **Bug**: RateLimiter.Acquire() 对新 peerID 死循环
- **原因**: 新 peerID 永远得不到初始化，tokens 永远是 0
- **影响**: 所有 RPC 调用都卡住，无法完成

### 修复

- **方案**: 在 Acquire() 开始时初始化新 peerID
- **代码**: 5 行新增代码
- **影响**: 无破坏性，只修复 Bug

### 教训

1. **不要假设测试环境能覆盖所有情况**
2. **需要测试完整的调用链**
3. **应该在实际环境中验证**
4. **跨机器测试很重要**

---

**修复完成时间**: 2026-03-04 21:10
**编译状态**: ✅ 成功
**测试状态**: ✅ 逻辑验证通过
**待验证**: 实际环境运行测试
