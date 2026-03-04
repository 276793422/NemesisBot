# RPC Handlers 深度检查报告

## 🔍 检查范围

- Handler 注册流程的完整性
- 生命周期管理
- 并发安全性
- 资源清理
- 错误处理
- 边界条件

---

## 🚨 发现的问题

### 问题 1: ⚠️ Custom Handlers 未被注册（功能缺失）

**严重程度**: 中等
**影响**: hello handler 不可用

**详情**:
```go
// handlers/custom.go 中定义了 RegisterCustomHandlers
func RegisterCustomHandlers(logger Logger, getNodeID func() string, registrar Registrar) {
    registrar("hello", func(payload map[string]interface{}) (map[string]interface{}, error) {
        // ... hello handler 实现
    })
}

// ❌ 但是没有任何地方调用 RegisterCustomHandlers！
```

**影响范围**:
- `hello` handler 无法接收 RPC 请求
- 用户之前提到的 daemon cluster auto 模式中的 hello 请求无法工作

**建议**:
1. 在 `registerLLMHandlers()` 中也注册 custom handlers，或
2. 在 Server.Start() 中注册 custom handlers，或
3. 提供单独的 `RegisterCustomHandlers()` 调用

---

### 问题 2: 🔴 潜在的死锁风险（严重）

**严重程度**: 严重
**影响**: 可能导致系统挂起

**详情**:
```go
// cluster.go: SetRPCChannel
func (c *Cluster) SetRPCChannel(rpcCh *channels.RPCChannel) {
    c.mu.Lock()           // 🔒 获取写锁
    defer c.mu.Unlock()

    c.rpcChannel = rpcCh

    if c.running && c.rpcServer != nil {
        c.registerLLMHandlers()  // 在持有写锁时调用 ⚠️
    }
}

func (c *Cluster) registerLLMHandlers() {
    // ...
    handlers.RegisterLLMHandlers(c.logger, c.rpcChannel, handlerFactory, registrar)
    // 其中 registrar 调用 c.RegisterRPCHandler(...)
}

func (c *Cluster) RegisterRPCHandler(...) error {
    c.mu.RLock()          // 🔒 尝试获取读锁
    // ...
    c.mu.RUnlock()
}
```

**死锁分析**:
```
调用链:
SetRPCChannel()
  → 持有 c.mu.Lock() (写锁)
  → 调用 registerLLMHandlers()
    → 调用 RegisterRPCHandler()
      → 尝试 c.mu.RLock() (读锁)
      → 💥 死锁！写锁状态下无法获取读锁
```

**实际运行情况**:
- 如果测试通过了，说明可能：
  1. 这个代码路径从未被执行过
  2. Go 的 RWMutex 在某些情况下允许降级
  3. 或者测试没有覆盖这个场景

**建议**:
在调用 `registerLLMHandlers()` 之前释放锁：
```go
func (c *Cluster) SetRPCChannel(rpcCh *channels.RPCChannel) {
    c.mu.Lock()
    c.rpcChannel = rpcCh
    wasRunning := c.running
    hasServer := c.rpcServer != nil
    c.mu.Unlock()

    // 在锁外调用 registerLLMHandlers
    if wasRunning && hasServer {
        c.registerLLMHandlers()
    }
}
```

---

### 问题 3: ⚠️ RPCChannel 生命周期管理缺失（资源泄漏）

**严重程度**: 中等
**影响**: 资源可能未正确释放

**详情**:
```go
// loop.go: setupClusterRPCChannel
func setupClusterRPCChannel(clusterInstance *cluster.Cluster, msgBus *bus.MessageBus) error {
    rpcCh, err := channels.NewRPCChannel(cfg)
    if err != nil {
        return err
    }

    ctx := context.Background()
    if err := rpcCh.Start(ctx); err != nil {
        return err
    }

    clusterInstance.SetRPCChannel(rpcCh)
    // ❌ 没有保存 rpcCh 的引用
    // ❌ 没有 defer rpcCh.Stop()
    return nil
}
```

**问题**:
- `rpcCh` 是局部变量，函数返回后无法访问
- `RPCChannel.Stop()` 从未被调用
- goroutines 可能泄漏

**影响**:
- Agent 重新加载时可能泄漏资源
- 多次调用可能创建多个 RPCChannel

**建议**:
```go
// 选项 1: 在 Cluster 中管理生命周期
type Cluster struct {
    // ...
    rpcChannel *channels.RPCChannel
}

// 在 Cluster.Stop() 中调用 rpcChannel.Stop()

// 选项 2: 保存引用以便清理
// 需要在 loop.go 中保存 rpcCh 引用，在 agent 停止时清理
```

---

### 问题 4: ℹ️ 重复的日志前缀

**严重程度**: 轻微
**影响**: 日志可读性

**详情**:
```go
// cluster.go
c.logger.RPCInfo("RPCChannel not ready, skipping LLM handler registration")

// handlers/custom.go
logger.LogRPCInfo("Hello handler: Received hello from %s at %s", from, timestamp)
```

`logger.LogRPCInfo` 实际上已经包含 "RPC" 前缀，日志中可能出现：
```
[RPC] RPCChannel not ready...  ← 重复
```

**建议**:
统一日志格式，要么：
1. Logger 方法名不带 RPC 前缀：`logger.Info()`
2. 日志内容不带 RPC 前缀：`"RPCChannel not ready..."`

---

## 📊 问题汇总表

| 问题 | 严重程度 | 状态 | 影响 | 建议优先级 |
|------|---------|------|------|-----------|
| Custom handlers 未注册 | ⚠️ 中 | ❌ 未解决 | hello handler 不可用 | P1 |
| 潜在死锁风险 | 🔴 严重 | ❌ 未解决 | 可能导致系统挂起 | P0（紧急） |
| RPCChannel 生命周期 | ⚠️ 中 | ❌ 未解决 | 资源泄漏 | P1 |
| 日志格式重复 | ℹ️ 轻微 | ❌ 未解决 | 日志可读性 | P3 |

---

## 🔧 推荐修复方案

### P0 - 立即修复：死锁风险

```go
// cluster.go: SetRPCChannel
func (c *Cluster) SetRPCChannel(rpcCh *channels.RPCChannel) {
    c.mu.Lock()
    c.rpcChannel = rpcCh

    // 保存状态
    wasRunning := c.running
    hasServer := c.rpcServer != nil

    c.mu.Unlock()  // 🔑 释放锁后再调用

    // 在锁外调用 registerLLMHandlers
    if wasRunning && hasServer {
        c.registerLLMHandlers()
    }
}
```

### P1 - 高优先级：注册 Custom Handlers

```go
// cluster.go: registerLLMHandlers
func (c *Cluster) registerLLMHandlers() {
    if c.rpcChannel == nil {
        return
    }

    registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
        if err := c.RegisterRPCHandler(action, handler); err != nil {
            c.logger.RPCError("Failed to register LLM handler '%s': %v", action, err)
        }
    }

    // 注册 LLM handlers
    handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
        handler := rpc.NewLLMForwardHandler(c, rpcChannel)
        return handler.Handle
    }
    handlers.RegisterLLMHandlers(c.logger, c.rpcChannel, handlerFactory, registrar)

    // ✅ 同时注册 custom handlers（新增）
    handlers.RegisterCustomHandlers(c.logger, c.GetNodeID, registrar)
}
```

### P1 - 高优先级：RPCChannel 生命周期管理

```go
// cluster.go
func (c *Cluster) Stop() error {
    // ... existing stop logic ...

    // 停止 RPCChannel
    if c.rpcChannel != nil {
        ctx := context.Background()
        if err := c.rpcChannel.Stop(ctx); err != nil {
            c.logger.RPCError("Failed to stop RPC channel: %v", err)
        }
        c.rpcChannel = nil
    }

    // ... rest of stop logic ...
}
```

---

## ✅ 已验证正常的部分

| 检查项 | 状态 | 说明 |
|--------|------|------|
| Default handlers 注册 | ✅ 正常 | ping, get_capabilities, get_info |
| LLM handlers 注册 | ✅ 正常 | llm_forward 通过 factory 注册 |
| 类型接口一致性 | ✅ 正常 | rpc.Cluster 接口完整 |
| 单元测试覆盖 | ✅ 覯盖 | 17/17 测试通过 |
| 集成测试 | ✅ 通过 | RPC 功能正常 |
| 编译 | ✅ 成功 | 无编译错误 |

---

## 🎯 风险评估

**高风险**:
- 🔴 死锁问题：如果在生产环境中触发 SetRPCChannel() 且 Server 正在运行，系统会挂起

**中风险**:
- ⚠️ 资源泄漏：长时间运行可能泄漏 goroutines 和连接
- ⚠️ 功能缺失：hello handler 不可用

**低风险**:
- ℹ️ 日志格式：仅影响可读性

---

## 📝 总结

发现了 **4 个需要修复的问题**，其中 **1 个是严重的（死锁风险）**，**2 个是中等的**，**1 个是轻微的**。

建议**优先修复 P0 级别的死锁问题**，因为可能导致生产环境系统挂起。

---

**检查时间**: 2026-03-04
**检查者**: Claude
**状态**: ⚠️ 发现多个需要修复的问题
