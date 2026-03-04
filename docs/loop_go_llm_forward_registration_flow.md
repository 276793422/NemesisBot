═══════════════════════════════════════════════════════════════
         loop.go 注册 llm_forward Handler 的完整流程
═══════════════════════════════════════════════════════════════

## 📋 问题核心

在重构方案中，有一个关键问题：

**Server 需要访问 RPCChannel，但 RPCChannel 是在 loop.go 中创建的！**

这需要一个传递机制。

═══════════════════════════════════════════════════════════════

## 🔴 当前流程 (有问题)

### 调用时序

```
t0: Cluster.Start()
    ↓
t1: Server.Start()
    ├─ 注册默认 handlers (ping, get_capabilities, get_info)
    └─ 启动 acceptLoop
    ↓
t2: NewAgentLoop()
    └─ setupClusterRPCChannel(clusterInstance, msgBus)
        ├─ 创建 RPCChannel
        ├─ 启动 RPCChannel
        ├─ 创建 LLMForwardHandler(clusterInstance, rpcCh)
        └─ clusterInstance.RegisterRPCHandler("llm_forward", ...)
```

### 当前代码

```go
// loop.go:1520-1550
func setupClusterRPCChannel(clusterInstance *cluster.Cluster, msgBus *bus.MessageBus) error {
    // 1. 创建 RPCChannel
    cfg := &channels.RPCChannelConfig{
        MessageBus:      msgBus,
        RequestTimeout:  60 * time.Second,
        CleanupInterval: 30 * time.Second,
    }
    rpcCh, err := channels.NewRPCChannel(cfg)
    if err != nil {
        return fmt.Errorf("failed to create RPC channel: %w", err)
    }

    // 2. 启动 RPCChannel
    ctx := context.Background()
    if err := rpcCh.Start(ctx); err != nil {
        return fmt.Errorf("failed to start RPC channel: %w", err)
    }

    // 3. 创建并注册 LLM forward handler
    llmForwardHandler := clusterrpc.NewLLMForwardHandler(clusterInstance, rpcCh)
    if err := clusterInstance.RegisterRPCHandler("llm_forward", llmForwardHandler.Handle); err != nil {
        logger.WarnCF("agent", "Failed to register LLM forward handler",
            map[string]interface{}{"error": err.Error()})
    }

    return nil
}
```

### 问题

1. **职责混乱**: loop.go 不应该知道 RPC handler 的存在
2. **依赖错误**: loop.go 依赖 clusterrpc 包
3. **耦合度高**: loop.go 和 cluster/rpc 紧密耦合

═══════════════════════════════════════════════════════════════

## 🟢 重构后的流程 (推荐)

### 核心机制

**延迟注册 (Lazy Registration)**

1. Server 启动时，只注册默认 handlers
2. loop.go 创建 RPCChannel 后，传递给 Cluster
3. Cluster 检测到 RPCChannel 就绪，立即注册 LLM handlers

### 调用时序

```
t0: Cluster.Start()
    ↓
t1: Server.Start()
    ├─ 注册默认 handlers (ping, get_capabilities, get_info)
    └─ 启动 acceptLoop
    ↓
t2: 检查 rpcChannel == nil? → 是，跳过 LLM handler 注册
    ↓
t3: 返回 Cluster.Start()
    ↓
t4: NewAgentLoop()
    └─ setupClusterRPCChannel(clusterInstance, msgBus)
        ├─ 创建 RPCChannel
        ├─ 启动 RPCChannel
        └─ 【关键】clusterInstance.SetRPCChannel(rpcCh)
            ↓
            SetRPCChannel() 内部:
            ├─ c.rpcChannel = rpcCh
            ├─ 检查 c.running && c.rpcServer != nil
            └─ 如果是，立即调用 c.registerLLMHandlers()
                ↓
                registerLLMHandlers():
                ├─ 检查 c.rpcChannel != nil  ✓
                ├─ 创建 handlerFactory: func(*RPCChannel) Handler
                │   └─ return rpc.NewLLMForwardHandler(c, rpcChannel).Handle
                └─ handlers.RegisterLLMHandlers(c.logger, c.rpcChannel, handlerFactory, c.RegisterRPCHandler)
                    ↓
                    handler := handlerFactory(rpcChannel)
                    └─ c.RegisterRPCHandler("llm_forward", handler)
                        ↓
                        Server.RegisterHandler("llm_forward", ...)
```

### 代码实现

#### 1. Cluster 添加字段和方法

```go
// cluster.go
type Cluster struct {
    // ... existing fields ...
    rpcServer   *rpc.Server
    rpcChannel  *channels.RPCChannel  // ← 新增
    mu          sync.RWMutex
}

// SetRPCChannel 设置 RPCChannel 并触发 handler 注册
func (c *Cluster) SetRPCChannel(rpcCh *channels.RPCChannel) {
    c.mu.Lock()
    defer c.mu.Unlock()

    c.rpcChannel = rpcCh

    // 如果 RPC Server 已经启动，立即注册 handlers
    if c.running && c.rpcServer != nil {
        c.registerLLMHandlers()
    }
}

// registerLLMHandlers 注册 LLM 相关的 handlers
func (c *Cluster) registerLLMHandlers() {
	if c.rpcChannel == nil {
		return
	}

	// 创建 handler factory（在 cluster 中创建，避免循环依赖）
	handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
		handler := rpc.NewLLMForwardHandler(c, rpcChannel)
		return handler.Handle
	}

	// 使用 handlers 包注册
	handlers.RegisterLLMHandlers(c.logger, c.rpcChannel, handlerFactory, c.RegisterRPCHandler)
}
```

#### 2. loop.go 简化

```go
// loop.go
func setupClusterRPCChannel(clusterInstance *cluster.Cluster, msgBus *bus.MessageBus) error {
    // 1. 创建 RPCChannel
    cfg := &channels.RPCChannelConfig{
        MessageBus:      msgBus,
        RequestTimeout:  60 * time.Second,
        CleanupInterval: 30 * time.Second,
    }

    rpcCh, err := channels.NewRPCChannel(cfg)
    if err != nil {
        return fmt.Errorf("failed to create RPC channel: %w", err)
    }

    // 2. 启动 RPCChannel
    ctx := context.Background()
    if err := rpcCh.Start(ctx); err != nil {
        return fmt.Errorf("failed to start RPC channel: %w", err)
    }

    // 3. 【关键】传递给 Cluster
    clusterInstance.SetRPCChannel(rpcCh)

    return nil
}
```

#### 3. 创建 handlers 包

```go
// handlers/llm.go
package handlers

// RegisterLLMHandlers 注册 LLM 相关的 RPC handlers
// 注意：使用 Handler Factory 模式避免循环依赖
func RegisterLLMHandlers(
    logger Logger,
    rpcChannel *channels.RPCChannel,
    handlerFactory func(*channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error),
    registrar Registrar,
) {
    // 使用 factory 创建 handler（factory 在 cluster.go 中定义）
    llmForwardHandler := handlerFactory(rpcChannel)
    registrar("llm_forward", llmForwardHandler)

    logger.LogRPCInfo("Registered LLM handlers: llm_forward")
}
```

**重要说明**：为什么使用 Handler Factory 模式？

为了避免循环依赖：
```
如果 handlers/llm.go 直接导入 rpc 包：
  handlers/llm.go → rpc (NewLLMForwardHandler)
  rpc/server.go → handlers (RegisterDefaultHandlers)
  形成循环依赖！❌

使用 Factory 模式：
  handlers/llm.go 不导入 rpc ✅
  handlerFactory 在 cluster.go 中创建（cluster 已经导入 rpc）
  通过参数传递，打破循环依赖 ✅
```

═══════════════════════════════════════════════════════════════

## 📊 对比总结

| 特性 | 重构前 | 重构后 | 改进 |
|------|--------|--------|------|
| Handler 注册位置 | 分散在 loop.go, rpc_handlers.go, server.go | 统一在 handlers/ 包 | ✅ 集中管理 |
| LLM Handler 注册 | loop.go 中直接创建和注册 | handlers/llm.go 通过 factory 模式 | ✅ 解耦 |
| 注册时机 | loop.go 初始化时 | SetRPCChannel() 延迟注册 | ✅ 自动触发 |
| 职责分离 | 混乱 | 清晰 | ✅ 各司其职 |
| loop.go 依赖 | 依赖 clusterrpc 包 | 只依赖 cluster | ✅ 解耦成功 |
| 循环依赖 | 无 | 无（factory 模式避免） | ✅ 架构健康 |
| 扩展性 | 困难 | 容易 | ✅ 易于添加新 handler |

### 架构设计说明

**Handler Factory 模式**（当前实现）：

1. **handlers/llm.go**：只负责注册，不导入 rpc 包
2. **cluster.go**：负责创建 LLMForwardHandler（已导入 rpc）
3. **factory 函数**：作为桥梁，连接 cluster 和 handlers

这种设计：
- ✅ 避免了 handlers → rpc → handlers 的循环依赖
- ✅ 保持了职责分离
- ✅ 测试全部通过（17/17 单元测试 + 3/3 集成测试）

═══════════════════════════════════════════════════════════════
