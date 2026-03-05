# 连接池问题修复报告

## 问题描述

**现象**：Daemon 模式下，前 3 次调用成功，第 4 次开始失败：

```
[21:31:32] Call returned, err=<nil>  ✅ 成功
[21:32:32] Call returned, err=<nil>  ✅ 成功
[21:33:32] Call returned, err=<nil>  ✅ 成功
[21:34:32] Call returned, err=too many concurrent connections (max=3)  ❌ 失败
```

## 根本原因

**连接池的空闲清理机制导致的资源泄漏**：

### 问题流程

1. **第1次调用（21:31:32）**：
   - 建立连接 conn1
   - `nodeConns[peer] = 1`

2. **conn1 空闲30秒后被 idleMonitor 关闭**：
   ```go
   // conn.go:222
   if time.Since(lastUsed) > tc.idleTimeout {
       tc.Close()  // ← 关闭连接
       return
   }
   ```
   - 但是**没有通知连接池清理**
   - `nodeConns[peer]` 还是 `1`，连接池中还保留着 conn1

3. **第2次调用（21:32:32）**：
   - Get() 发现 conn1.IsActive() = false
   - 建立新连接 conn2
   - `nodeConns[peer] = 2`

4. **第3次调用（21:33:32）**：
   - conn2 被关闭，建立 conn3
   - `nodeConns[peer] = 3`

5. **第4次调用（21:34:32）**：
   - conn3 被关闭
   - `nodeConns[peer] = 3`（已达到上限）
   - **失败！** "too many concurrent connections (max=3)"

### 根本原因

**idleMonitor 关闭连接时，没有从连接池中清理，导致计数器没有减少。**

## 修复方案

**文件**: `module/cluster/transport/pool.go:126-137`

### 修复代码

```go
// ✅ Fix: Clean up inactive connection before checking limit
if exists && !conn.IsActive() {
    // Connection exists but is inactive (closed by idle monitor or error)
    // Remove it from pool and decrement counters
    delete(p.conns, key)
    p.activeConns--
    if p.nodeConns[nodeID] > 0 {
        p.nodeConns[nodeID]--
    }
}

// Check per-node limit
if p.nodeConns[nodeID] >= p.maxConnsPerNode {
    <-p.semaphore // Release semaphore slot
    return nil, fmt.Errorf("too many concurrent connections to node %s (max=%d)", nodeID, p.maxConnsPerNode)
}
```

### 修复逻辑

1. **检查旧连接状态**：`conn.IsActive()`
2. **如果不活跃**：
   - 从连接池删除：`delete(p.conns, key)`
   - 减少总连接数：`p.activeConns--`
   - 减少节点连接数：`p.nodeConns[nodeID]--`
3. **然后检查限制**：`p.nodeConns[nodeID] >= p.maxConnsPerNode`

## 测试验证

### 测试场景
- **5 次连续调用**
- **每次间隔 35 秒**
- **连接空闲超时 30 秒**
- **预期**：所有调用成功

### 测试结果

```
[Call 1/5] ✅ SUCCESS
[Call 2/5] ✅ SUCCESS
[Call 3/5] ✅ SUCCESS
[Call 4/5] ✅ SUCCESS
[Call 5/5] ✅ SUCCESS
```

**✅ 所有调用成功，证明修复有效！**

## 预期效果

修复后，daemon 模式下应该可以持续运行：

```
[21:31:32] Call returned, err=<nil>  ✅ 成功
[21:32:32] Call returned, err=<nil>  ✅ 成功
[21:33:32] Call returned, err=<nil>  ✅ 成功
[21:34:32] Call returned, err=<nil>  ✅ 成功（修复前失败）
[21:35:32] Call returned, err=<nil>  ✅ 成功
...
持续运行，不再失败
```

## 修改总结

### 本次修复（第4个问题）

**文件**: `module/cluster/transport/pool.go`
**修改**: 在 GetWithContext() 中添加不活跃连接清理逻辑
**新增代码**: 约 10 行

### 完整修复列表（所有4个问题）

1. ✅ **RateLimiter 死循环** - `client.go:85-91`
2. ✅ **Daemon 模式 handler 未注册** - `cluster.go:690-720`, `main.go:660-664`
3. ✅ **服务端响应日志缺失** - `server.go:228, 242, 253`
4. ✅ **连接池资源泄漏** - `pool.go:126-137`

## 编译状态

```bash
✅ nemesisbot.exe (27MB)
✅ 所有模块编译成功
```

---

**修复完成时间**: 2026-03-04 21:41
**测试状态**: ✅ 完全通过
**待验证**: 用户长时间运行测试
