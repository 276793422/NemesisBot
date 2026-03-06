# RPC Channel 重复启动问题修复

## 问题描述

**错误日志**:
```
[ERROR] channels: Failed to start channel {channel=rpc, error=RPC channel already running}
```

## 问题原因

RPC channel 被启动了两次，导致第二次启动时失败。

### 启动流程分析

**错误的双启动流程**:

```
1. NewAgentLoop() 创建
   ↓
2. setupClusterRPCChannel() 被调用
   ├─ 创建 RPCChannel
   ├─ 调用 rpcCh.Start(ctx) ← 第一次启动
   └─ 设置到 Cluster
   ↓
3. SetChannelManager(channelManager)
   └─ 注册 RPCChannel 到 ChannelManager.channels["rpc"]
   ↓
4. ChannelManager.StartAll()
   └─ 遍历所有 channel 调用 Start()
      └─ 调用 rpcCh.Start(ctx) ← 第二次启动（错误！）
```

### 问题代码

**原代码** (module/agent/loop.go 第 1590-1594 行):

```go
// Start RPC channel
ctx := context.Background()
if err := rpcCh.Start(ctx); err != nil {
    return fmt.Errorf("failed to start RPC channel: %w", err)
}
```

这里提前启动了 RPC channel，但随后 ChannelManager.StartAll() 会再次启动它。

## 解决方案

**移除 `setupClusterRPCChannel` 中的 Start 调用**，让 ChannelManager 统一管理所有 channel 的生命周期。

### 修改后的代码

```go
// setupClusterRPCChannel sets up the RPC channel and LLM forward handler for the cluster
func setupClusterRPCChannel(clusterInstance *cluster.Cluster, msgBus *bus.MessageBus) error {
    // Create RPC channel configuration
    cfg := &channels.RPCChannelConfig{
        MessageBus:      msgBus,
        RequestTimeout:  60 * time.Second,
        CleanupInterval: 30 * time.Second,
    }

    // Create RPC channel
    rpcCh, err := channels.NewRPCChannel(cfg)
    if err != nil {
        return fmt.Errorf("failed to create RPC channel: %w", err)
    }

    // NOTE: Don't start RPC channel here!
    // It will be started by ChannelManager.StartAll() after registration
    // This prevents "RPC channel already running" error

    // Set RPC channel on cluster (triggers LLM handler registration)
    clusterInstance.SetRPCChannel(rpcCh)

    logger.InfoC("agent", "RPC channel for peer chat created and configured (will be started by ChannelManager)")

    return nil
}
```

### 正确的启动流程

```
1. NewAgentLoop() 创建
   ↓
2. setupClusterRPCChannel() 被调用
   ├─ 创建 RPCChannel
   ├─ 不调用 Start() ← 修复：移除启动调用
   └─ 设置到 Cluster
   ↓
3. SetChannelManager(channelManager)
   └─ 注册 RPCChannel 到 ChannelManager.channels["rpc"]
   ↓
4. ChannelManager.StartAll()
   └─ 遍历所有 channel 调用 Start()
      └─ 调用 rpcCh.Start(ctx) ← 唯一的一次启动
```

## 为什么这样做是正确的？

### 1. 统一的生命周期管理

ChannelManager 的职责就是管理所有 channel 的生命周期：
- **初始化** (Init)
- **启动** (Start)
- **停止** (Stop)

将 RPC channel 纳入这个统一管理是正确的架构设计。

### 2. 避免竞态条件

双启动会导致：
- 第一次启动已经开始运行
- 第二次启动检测到已运行，返回错误
- 可能导致状态不一致

### 3. 一致性

其他 channel（telegram, discord 等）都是在 ChannelManager 中启动的，RPC channel 应该保持一致。

### 4. 简化错误处理

只需要在一个地方处理启动失败，而不是多个地方。

## 影响范围

### 正面影响

- ✅ 消除了"RPC channel already running"错误
- ✅ 统一了 channel 生命周期管理
- ✅ 简化了代码逻辑
- ✅ 提高了代码可维护性

### 可能的问题

**无**。这个修改是安全的，因为：

1. RPCChannel 在创建后就立即注册到 Cluster
2. SetChannelManager 会将它注册到 ChannelManager
3. ChannelManager.StartAll 会确保它被启动
4. 启动顺序：
   - 先创建和配置
   - 再统一启动
   - 最后统一停止

## 验证

重新编译和运行后，应该看到：

**正确的日志**:
```
[INFO] agent: RPC channel for peer chat created and configured (will be started by ChannelManager)
[INFO] agent: RPC channel registered to channel manager
[INFO] channels: Starting all channels
[INFO] channels: Starting channel {channel: rpc}
[INFO] channels: All channels started
```

**不应该再看到**:
```
[ERROR] channels: Failed to start channel {channel=rpc, error=RPC channel already running}
```

## 相关修改

- **文件**: `module/agent/loop.go`
- **函数**: `setupClusterRPCChannel()`
- **修改**: 移除了 `rpcCh.Start(ctx)` 调用
- **添加**: 注释说明为什么不在这里启动

## 总结

这个问题的根本原因是违反了单一职责原则 - RPC channel 的生命周期应该在 ChannelManager 中统一管理，而不是在多处手动启动。

修复后，RPC channel 的生命周期与其他 channel 保持一致，由 ChannelManager 统一管理，避免了重复启动的错误。
