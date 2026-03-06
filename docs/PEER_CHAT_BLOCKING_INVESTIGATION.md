# Peer Chat 阻塞问题调查进度

**创建日期**: 2026-03-06
**状态**: 🔍 调查中
**优先级**: 🔴 高

---

## 📋 问题描述

### 现象

**使用长超时配置时**（28/29/30分钟）：
- 对端日志：`✅ Response delivered successfully via Send`
- 对端日志：`Message sent to channel successfully`
- 对端**卡住12分钟**，没有任何日志输出
- 本端一直等待，收不到 TCP 响应

**使用短超时配置后**（30/60/60秒）：
- 一切正常工作

### 关键日志缺失

对端应该有但**没有出现**的日志：
```
[INFO] [PeerChat] Response received! correlation_id=..., response=...
[INFO] Response: action=peer_chat, from=..., to=..., id=..., payload=...
```

---

## 🔧 当前配置

### 超时配置（工作正常）

| 超时参数 | 文件 | 行数 | 值 |
|---------|------|------|-----|
| RPC Client 超时 | `module/cluster/rpc/client.go` | 194 | 30 秒 |
| peer_chat handler 超时 | `module/cluster/rpc/peer_chat_handler.go` | 117 | 60 秒 |
| RPCChannel RequestTimeout | `module/agent/loop.go` | 1593 | 60 秒 |
| RPCChannel CleanupInterval | `module/agent/loop.go` | 1594 | 30 秒 |

### 之前的问题配置

| 超时参数 | 值 | 问题 |
|---------|-----|------|
| RPC Client 超时 | 30 分钟 | ❌ |
| peer_chat handler 超时 | 29 分钟 | ❌ |
| RPCChannel RequestTimeout | 28 分钟 | ❌ |
| RPCChannel CleanupInterval | 30 秒 | ✅ |

---

## 🔍 分析过程

### 已排除的原因

#### ❌ 原因 1：cleanupLoop 提前关闭 channel

**分析**：
```go
// cleanupLoop 每 30 秒检查一次
ticker := time.NewTicker(ch.cleanupInterval)  // 30 秒

// cleanupExpiredRequests
for correlationID, req := range ch.pendingReqs {
    if now.Sub(req.createdAt) > req.timeout {  // req.timeout = 60秒
        close(req.responseCh)
        delete(ch.pendingReqs, correlationID)
    }
}
```

**时间线**：
```
T0: 请求创建
T0 + 30秒: cleanupLoop 检查 → age=30秒 < 60秒 → ✅ 不关闭
T0 + 60秒: cleanupLoop 检查 → age=60秒 不大于 60秒 → ✅ 不关闭
T0 + 90秒: cleanupLoop 检查 → age=90秒 > 60秒 → ❌ 关闭
```

**结论**：cleanupLoop **不会**提前关闭 channel，逻辑正确 ✅

#### ❌ 原因 2：cleanupLoop 关闭 channel 导致 Send() panic

**分析**：
- 如果 responseCh 被关闭，Send() 会 panic
- 但对端没有 panic 日志
- 对端只是卡住，没有崩溃

**结论**：不是 panic 导致的 ✅

#### ❌ 原因 3：TCP 层面的响应丢失

**分析**：
- 对端日志显示响应已成功投递到 respCh
- 问题是 peer_chat_handler 没有收到
- 不是 TCP 传输问题

**结论**：问题在应用层，不是传输层 ✅

---

## 🔴 关键发现

### 发现 1：peer_chat_handler 的 select 阻塞

**代码**：
```go
// peer_chat_handler.go line 129-137
select {
case response := <-respCh:
    h.cluster.LogRPCInfo("[PeerChat] Response received! correlation_id=%s, response=%s", correlationID, response)
    return h.successResponse(response, nil), nil

case <-ctx.Done():
    h.cluster.LogRPCError("[PeerChat] Timeout waiting for response (correlation_id=%s)", correlationID)
    return h.errorResponse("error", "timeout"), nil
}
```

**问题**：
- `[PeerChat] Response received!` 日志**没有出现**
- `[PeerChat] Timeout` 日志也**没有出现**
- 说明 select **两个分支都没有触发**

**可能原因**：
1. respCh 的数据没有被投递（但 Send 日志显示投递成功）
2. ctx.Done() 永远不触发（即使超时）
3. select 本身阻塞了

### 发现 2：响应流程中断

**正常流程**：
```
1. RPCChannel.Send() → respCh ← 投递成功 ✅
2. peer_chat_handler select → 收到响应 ✅
3. peer_chat_handler 返回 ← 应该有日志 ❌
4. server.handleRequest() 调用 sendMessage ← 应该有日志 ❌
5. conn.Send() → TCP 响应 ← 应该有日志 ❌
```

**实际情况**：
```
1. RPCChannel.Send() → respCh ← 投递成功 ✅
2. peer_chat_handler select ← 阻塞，两个分支都没触发 ❌
3. [卡住12分钟]
```

### 发现 3：阻塞发生在 dispatchOutbound 之后

**最后的日志**：
```
[INFO] channels: Message sent to channel successfully {channel=rpc, chat_id=default}
```

**来源**：
```go
// manager.go line 346
logger.InfoCF("channels", "Message sent to channel successfully", ...)
```

**之后应该发生**：
```
peer_chat_handler 的 select 收到响应
↓
[PeerChat] Response received! ← 这条日志缺失
↓
peer_chat_handler 返回
↓
server.go line 253: Response: action=peer_chat... ← 这条日志也缺失
↓
conn.Send() TCP 响应
```

---

## 🤔 可能的根因假设

### 假设 1：respCh 指针不匹配

**场景**：
- RPCChannel.Input() 创建了一个 respCh
- 返回给 peer_chat_handler
- 但 Send() 投递到了另一个 respCh

**验证方法**：
- 添加 respCh 指针地址日志
- 对比 Input() 返回的指针和 Send() 投递的指针

**状态**：❌ 未验证

### 假设 2：goroutine 泄漏或死锁

**场景**：
- peer_chat_handler 所在的 goroutine 进入某种死锁
- 或者被其他锁阻塞

**验证方法**：
- 添加 goroutine ID 追踪
- 在关键位置打印调用栈
- 检查 pprof goroutine 数量

**状态**：❌ 未验证

### 假设 3：Context 超时机制有问题

**场景**：
- context.WithTimeout() 创建的 context
- 在长超时配置下（29分钟）有某种 bug
- ctx.Done() 永远不触发

**验证方法**：
- 添加 ctx.Done() 的监控日志
- 检查 context 的 deadline 是否正确设置

**状态**：❌ 未验证

### 假设 4：有其他代码持有 respCh 引用

**场景**：
- 某个全局变量或缓存
- 持有 respCh 的引用
- 导致数据投递到错误的 channel

**验证方法**：
- 全局搜索 `respCh` 或 `responseCh` 的使用
- 检查是否有缓存或全局 map

**状态**：❌ 未验证

### 假设 5：race condition

**场景**：
- cleanupLoop 的锁和 Send() 的锁
- 在某种极端情况下产生竞态
- 导致数据投递失败

**验证方法**：
- 运行 `go test -race`
- 添加更多锁相关的日志

**状态**：❌ 未验证

---

## 🔧 待添加的调试日志

### 1. respCh 指针追踪

**位置**: `rpc_channel.go` Input() 方法
```go
logger.InfoCF("rpc", "RPCChannel.Input: created respCh=%p", respCh)
logger.InfoCF("rpc", "RPCChannel.Input: returning respCh=%p to caller", respCh)
```

**位置**: `rpc_channel.go` Send() 方法
```go
logger.InfoCF("rpc", "RPCChannel.Send: delivering to respCh=%p", req.responseCh)
```

**位置**: `peer_chat_handler.go`
```go
logger.LogRPCInfo("[PeerChat] waiting on respCh=%p", respCh)
```

### 2. Select 状态追踪

**位置**: `peer_chat_handler.go` select 之前
```go
logger.LogRPCInfo("[PeerChat] entering select, respCh=%p, ctx_deadline=%v", respCh, ctx.Deadline())
```

### 3. Goroutine ID 追踪

```go
import "runtime"

goroutineID := runtime.GetGoroutineId()
logger.LogRPCInfo("[PeerChat] goroutine_id=%d", goroutineID)
```

### 4. 调用栈快照

**在阻塞位置添加**：
```go
buf := make([]byte, 4096)
n := runtime.Stack(buf, false)
logger.LogRPCInfo("[PeerChat] stack trace:\n%s", string(buf[:n]))
```

---

## 📊 下一步排查计划

### 优先级 1：添加指针追踪日志

**目标**：验证 respCh 指针是否一致

**步骤**：
1. 在 Input() 返回时记录 respCh 指针
2. 在 Send() 投递时记录 responseCh 指针
3. 在 peer_chat_handler 等待时记录 respCh 指针
4. 对比三个指针是否相同

**预期结果**：
- 如果指针相同 → 排除指针不匹配
- 如果指针不同 → 找到根因！

### 优先级 2：使用 race detector

**命令**：
```bash
go test -race ./module/channels/...
go test -race ./module/cluster/rpc/...
go run -race ./nemesisbot
```

**目标**：发现潜在的竞态条件

### 优先级 3：添加 pprof 端点

**方法**：
```go
import _ "net/http/pprof"

go func() {
    log.Println(http.ListenAndServe("localhost:6060", nil))
}()
```

**使用**：
```bash
# 查看 goroutine 数量
curl http://localhost:6060/debug/pprof/goroutine?debug=1

# 查看 heap
curl http://localhost:6060/debug/pprof/heap?debug=1
```

### 优先级 4：重现问题

**步骤**：
1. 恢复长超时配置（28/29/30分钟）
2. 发送 peer_chat 请求
3. 等待问题出现
4. 收集完整日志
5. 使用 pprof 收集现场数据

---

## 📝 相关代码位置

### 关键文件

| 文件 | 关键方法 | 行数 |
|------|---------|------|
| `module/cluster/rpc/peer_chat_handler.go` | `handleLLMRequest` | 71-138 |
| `module/channels/rpc_channel.go` | `Input` | 271-303 |
| `module/channels/rpc_channel.go` | `Send` | 154-246 |
| `module/channels/rpc_channel.go` | `cleanupExpiredRequests` | 342-367 |
| `module/cluster/rpc/server.go` | `handleRequest` | 203-257 |
| `module/cluster/rpc/client.go` | `receiveResponseWithContext` | 336-389 |

### 配置位置

| 配置项 | 文件 | 行数 |
|--------|------|------|
| RequestTimeout | `module/agent/loop.go` | 1593 |
| CleanupInterval | `module/agent/loop.go` | 1594 |
| peer_chat 超时 | `module/cluster/rpc/peer_chat_handler.go` | 117 |
| Client 超时 | `module/cluster/rpc/client.go` | 194 |

---

## 🔗 相关文档

- `docs/RPC_TIMEOUT_DEEP_ANALYSIS.md` - 超时机制深度分析
- `docs/PEER_CHAT_TIMEOUT_CONFIG_UPDATE.md` - 超时配置更新文档
- `docs/PEER_CHAT_FINAL_VERIFICATION_REPORT.md` - 最终验证报告

---

## ✅ 已确认的事实

1. ✅ cleanupLoop 的逻辑是正确的，不会提前关闭 channel
2. ✅ 短超时配置（30/60/60秒）工作正常
3. ✅ 长超时配置（28/29/30分钟）会导致阻塞
4. ✅ RPCChannel.Send() 成功投递了响应
5. ✅ peer_chat_handler 的 select 没有收到响应
6. ✅ 不是 panic 导致的（对端没有崩溃）
7. ✅ 不是 TCP 传输问题

---

## ❓ 待解决的问题

1. ❓ 为什么 peer_chat_handler 的 select 两个分支都没触发？
2. ❓ 阻塞发生在什么位置？
3. ❓ 为什么短超时正常，长超时异常？
4. ❓ 是否有 goroutine 泄漏或死锁？
5. ❓ 是否有竞态条件？

---

## 📅 调查历史

### 2026-03-06

**最初假设**：cleanupLoop 提前关闭 channel，导致 Send() panic
**验证结果**：❌ 错误，cleanupLoop 逻辑正确

**第二假设**：向已关闭的 channel 发送导致 panic
**验证结果**：❌ 错误，没有 panic 日志

**第三假设**：respCh 指针不匹配
**验证结果**：❓ 未验证，需要添加日志

**当前状态**：需要添加更多调试日志来定位问题

---

**最后更新**: 2026-03-06
**负责人**: Claude
**下次更新**: 添加指针追踪日志后
