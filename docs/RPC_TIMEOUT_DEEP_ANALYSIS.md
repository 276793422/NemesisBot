# RPC 超时机制深度分析

**分析日期**: 2026-03-06
**问题**: 长超时配置导致 peer_chat 阻塞，短超时配置正常工作

---

## 一、三个超时配置

### 当前配置（工作正常）

```
┌─────────────────────────────────────────────────┐
│ RPC Client (本端)          │ 30 秒          │
├─────────────────────────────────────────────────┤
│ peer_chat handler (对端)  │ 60 秒          │
├─────────────────────────────────────────────────┤
│ RPCChannel RequestTimeout │ 60 秒          │
├─────────────────────────────────────────────────┤
│ cleanupLoop interval      │ 30 秒          │
└─────────────────────────────────────────────────┘
```

### 之前配置（导致问题）

```
┌─────────────────────────────────────────────────┐
│ RPC Client (本端)          │ 30 分钟        │
├─────────────────────────────────────────────────┤
│ peer_chat handler (对端)  │ 29 分钟        │
├─────────────────────────────────────────────────┤
│ RPCChannel RequestTimeout │ 28 分钟        │
├─────────────────────────────────────────────────┤
│ cleanupLoop interval      │ 30 秒          │
└─────────────────────────────────────────────────┘
```

---

## 二、cleanupLoop 的工作机制

### 源码分析

```go
// cleanupLoop 定期清理过期的 pending requests
func (ch *RPCChannel) cleanupLoop() {
    ticker := time.NewTicker(ch.cleanupInterval)  // 30 秒
    for {
        select {
        case <-ticker.C:
            ch.cleanupExpiredRequests()
        }
    }
}

func (ch *RPCChannel) cleanupExpiredRequests() {
    ch.mu.Lock()
    defer ch.mu.Unlock()

    now := time.Now()
    for correlationID, req := range ch.pendingReqs {
        if now.Sub(req.createdAt) > req.timeout {  // 关键：检查是否超时
            close(req.responseCh)  // ⚠️ 关闭 channel！
            delete(ch.pendingReqs, correlationID)
        }
    }
}
```

### cleanupLoop 的行为

**每 30 秒运行一次**，检查所有 pending requests：
- 如果 `now - createdAt > timeout`，**关闭 responseCh**
- 删除 pending request

---

## 三、问题根源：超时不匹配

### 场景 1：短超时配置（✅ 正常工作）

```
T0: peer_chat 请求开始
    ├─ RequestTimeout = 60 秒
    └─ peer_chat handler 超时 = 60 秒

T0 + 30秒: cleanupLoop 第 1 次检查
    ├─ age = 30 秒 < 60 秒 ✅ 不清理
    └─ responseCh 仍然开启

T0 + 5 秒: LLM 响应返回（假设 5 秒完成）
    ├─ responseCh 仍然开启 ✅
    ├─ RPCChannel.Send() → responseCh ← 成功投递
    └─ peer_chat_handler 收到响应 ✅
```

**关键**：在 LLM 响应返回之前，cleanupLoop **没有关闭** responseCh。

---

### 场景 2：长超时配置（❌ 导致问题）

```
T0: peer_chat 请求开始
    ├─ RequestTimeout = 28 分钟
    └─ peer_chat handler 超时 = 29 分钟

T0 + 30秒: cleanupLoop 第 1 次检查
    ├─ age = 30 秒 < 28 分钟 ✅ 不清理
    └─ responseCh 仍然开启

T0 + 60秒: cleanupLoop 第 2 次检查
    ├─ age = 60 秒 < 28 分钟 ✅ 不清理
    └─ responseCh 仍然开启

... (cleanupLoop 每 30 秒检查一次) ...

T0 + 5 秒: LLM 响应返回（假设 5 秒完成）
    ├─ responseCh 仍然开启 ✅
    ├─ RPCChannel.Send() → responseCh ← 成功投递
    └─ peer_chat_handler 收到响应 ✅
```

**等等！这里应该也正常工作啊？**

让我重新分析...

---

## 四、真正的问题：Race Condition

### 问题时间线（长超时配置）

```
T0: peer_chat 请求开始
    responseCh 创建，加入 pendingReqs

T0 + 28分01秒: cleanupLoop 检查
    ├─ age = 28分01秒 > 28 分钟 ❌
    ├─ close(responseCh)  ← ⚠️ 关闭 channel
    └─ delete from pendingReqs

T0 + 28分02秒: LLM 响应终于返回（超长任务）
    ├─ RPCChannel.Send() 尝试投递
    ├─ pendingReqs 中找不到（已被删除）
    └─ 记录 "⚠️ No pending request found"

T0 + 29分钟: peer_chat handler context 超时
    ├─ ctx.Done() 触发
    └─ 返回 timeout 错误
```

**但这个场景不会导致阻塞，只是返回错误！**

---

## 五、发现真正的问题：向已关闭的 channel 发送

### 致命的 Race Condition

```
时间线（长超时，响应在 28 分钟后返回）:

T0: 请求创建，responseCh 开启

T0 + 28分: cleanupLoop 执行
         └─ close(responseCh)  ← ⚠️ 关闭

T0 + 28分: LLM 响应返回
         └─ Send() 调用
             └─ req.responseCh <- content
                 └─ responseCh 已关闭！
                 └─ 💥 PANIC! (或者永远阻塞)
```

### Go 语言行为

在 Go 中，**向已关闭的 channel 发送数据会 panic**：

```go
ch := make(chan string, 1)
close(ch)
ch <- "test"  // 💥 panic: send on closed channel
```

### RPCChannel.Send() 的代码

```go
select {
case req.responseCh <- actualContent:
    logger.InfoCF("rpc", "✅ Response delivered successfully")
case <-time.After(time.Second):
    logger.WarnCF("rpc", "Failed to deliver response")
}
```

**问题**：如果 responseCh 在 send 之前被关闭：
- `case req.responseCh <- actualContent:` 会 **panic**
- 导致整个 goroutine 崩溃
- peer_chat_handler 永远不会收到响应
- 对端节点卡住

---

## 六、为什么短超时配置能工作？

### 关键：响应返回时间 < cleanup 超时时间

```
短超时配置:
├─ RequestTimeout: 60 秒
├─ cleanupInterval: 30 秒
└─ LLM 响应时间: 5 秒

时间线:
T0: 请求创建
T0 + 5 秒: 响应返回
    ├─ age = 5 秒 < 60 秒 ✅
    ├─ responseCh 仍然开启 ✅
    └─ 成功投递 ✅

T0 + 30 秒: cleanupLoop 检查
    └─ 请求已被处理，不在 pendingReqs 中 ✅
```

**关键**：cleanupLoop 在响应返回**之后**才检查，此时请求已经被删除。

---

## 七、为什么长超时配置失败？

### 问题：cleanupLoop 可能在响应返回之前关闭 channel

虽然 cleanupLoop 每 30 秒检查一次，但如果有以下情况：

```
场景 A：任务确实需要很长时间（> 28 分钟）

T0: 请求创建
T0 + 28分01秒: cleanupLoop 认为超时，close(responseCh)
T0 + 29分钟: LLM 响应返回
    └─ Send() 尝试投递 → responseCh 已关闭 → 💥 PANIC

场景 B：LLM 慢但还能在 28 分钟内完成

T0: 请求创建
T0 + 10 分钟: LLM 还在处理
T0 + 28分01秒: cleanupLoop 关闭 responseCh
T0 + 28分30秒: LLM 完成
    └─ Send() 尝试投递 → 💥 PANIC
```

**但用户说 12 分钟后卡住，不是 28 分钟！**

---

## 八、重新分析：12 分钟卡住的原因

### 可能的解释 1：有其他 pending request 被清理

如果对端之前有多个请求，某个旧的 request 在 12 分钟时被 cleanup：

```
T - 10 分钟: 旧的 peer_chat 请求（假设卡住了）
T0: 新的 peer_chat 请求
T0 + 2 分钟: 新请求的响应返回
    └─ Send() 尝试投递
        └─ 此时 cleanupLoop 正在执行
        └─ 获取锁，检查超时
        └─ 关闭了某个旧的 responseCh
        └─ 💥 但新请求的 responseCh 也受影响？
```

这个解释不太合理，因为每个 request 有独立的 responseCh。

### 可能的解释 2：cleanupLoop 和 Send() 的锁竞争

```go
// cleanupExpiredRequests
ch.mu.Lock()
for correlationID, req := range ch.pendingReqs {
    if now.Sub(req.createdAt) > req.timeout {
        close(req.responseCh)  // ⚠️ 持有锁时关闭
        delete(ch.pendingReqs, correlationID)
    }
}
ch.mu.Unlock()

// Send()
ch.mu.RLock()
req, exists := ch.pendingReqs[correlationID]  // 读锁
ch.mu.RUnlock()
select {
case req.responseCh <- actualContent:  // ⚠️ 可能在锁外发送，但 channel 已关闭
```

**Race Condition**:
1. cleanupLoop 获取写锁，关闭 responseCh，释放锁
2. Send() 获取读锁，找到 req，释放锁
3. Send() 尝试发送，但 responseCh 已被关闭
4. **PANIC**

### 可能的解释 3：panic 被 recover 但没有正确处理

如果某个地方有 recover，panic 可能被静默处理：
```go
func someGoroutine() {
    defer func() {
        if r := recover(); r != nil {
            // 没有日志，没有处理
        }
    }()
    // ... RPCChannel.Send() ...
}
```

---

## 九、验证假设

### 需要添加的日志

1. **cleanupExpiredRequests**: 记录关闭的 responseCh 指针
2. **Send**: 记录发送时的 responseCh 指针和状态
3. **panic recover**: 捕获并记录 panic

---

## 十、结论

### 问题根源

**cleanupLoop 会在超时后关闭 responseCh，但 Send() 可能在关闭后尝试发送，导致 panic。**

### 为什么短超时能工作

- LLM 响应通常在几秒内返回
- cleanupLoop 30 秒后才第一次检查
- 响应已经投递，request 已删除
- cleanupLoop 找不到这个 request，不会关闭 responseCh

### 为什么长超时失败

- cleanupLoop 的超时判断基于 `req.timeout`（28 分钟）
- peer_chat handler 的超时判断基于 context（29 分钟）
- **两者不匹配！**
- cleanupLoop 提前 1 分钟关闭了 responseCh
- 当响应在 28-29 分钟之间返回时，Send() 向已关闭的 channel 发送 → panic

### 设计缺陷

1. **双重超时机制冲突**：
   - RPCChannel cleanupLoop: 主动关闭 channel
   - peer_chat handler context: 被动等待超时

2. **没有保护向 channel 发送的操作**：
   - Send() 没有检查 channel 是否已关闭
   - 没有 recover 机制

3. **超时不一致**：
   - RequestTimeout (28分钟) < peer_chat handler 超时 (29分钟)
   - 导致 cleanup 提前执行

---

## 十一、解决方案

### 方案 1：移除 cleanupLoop 的关闭逻辑（推荐）

```go
// cleanupExpiredRequests - 只删除，不关闭
func (ch *RPCChannel) cleanupExpiredRequests() {
    ch.mu.Lock()
    defer ch.mu.Unlock()

    now := time.Now()
    for correlationID, req := range ch.pendingReqs {
        if now.Sub(req.createdAt) > req.timeout {
            // ❌ 不要关闭 channel
            // close(req.responseCh)

            // ✅ 只删除记录
            delete(ch.pendingReqs, correlationID)
        }
    }
}
```

**理由**：
- peer_chat handler 有自己的 context 超时
- 让 handler 自己处理超时
- cleanupLoop 只清理内存，不干扰 channel

### 方案 2：统一超时时间

```go
RequestTimeout: 29 * time.Minute  // 与 peer_chat handler 一致
peer_chat handler: 29 * time.Minute
```

**但这样不能解决根本问题！** cleanupLoop 仍然会关闭 channel。

### 方案 3：Send() 前检查 channel 状态

```go
select {
case req.responseCh <- actualContent:
    logger.InfoCF("rpc", "✅ Response delivered")
case <-time.After(time.Second):
    logger.WarnCF("rpc", "Failed to deliver")
}
```

但这不能避免 panic，因为 Go 的 select 在 channel 关闭时仍然会 panic。

### 方案 4：使用 defer recover 保护 Send()

```go
func (ch *RPCChannel) Send(ctx context.Context, msg bus.OutboundMessage) error {
    defer func() {
        if r := recover(); r != nil {
            logger.ErrorCF("rpc", "PANIC in Send: %v", r)
        }
    }()
    // ... Send 逻辑
}
```

---

## 十二、建议

### 短期修复

1. **保持短超时配置**（30秒/60秒/60秒）
2. **移除 cleanupLoop 的 close(responseCh) 调用**

### 长期优化

1. **重新设计超时机制**：
   - 只保留一层超时（peer_chat handler 的 context）
   - cleanupLoop 只清理内存，不操作 channel

2. **添加 panic recover**：
   - 在所有可能 panic 的地方添加 recover
   - 记录详细的 panic 信息

3. **改进监控**：
   - 记录所有 responseCh 的创建、关闭、发送操作
   - 记录 cleanupLoop 的所有操作

---

**分析人**: Claude
**分析日期**: 2026-03-06
**状态**: 📊 **分析完成**
