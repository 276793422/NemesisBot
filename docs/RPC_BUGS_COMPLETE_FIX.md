# RPC 问题完整修复报告

## 问题总览

**现象**: `nemesisbot daemon cluster auto` 模式下，RPC 调用卡住，无法完成

**根本原因**: 两个 Bug
1. RateLimiter.Acquire() 对新 peer 死循环
2. Daemon 模式下 hello handler 没有被注册

---

## 完整调用流程

```
main.go (daemon)
    ↓ line 705
clusterInstance.Call(peerID, "hello", payload)
    ↓ line 442
cluster.go: CallWithContext(context.Background(), ...)
    ↓ line 458
cluster.logger.RPCInfo("Calling %s: action=%s", ...)
    ↓ line 459
c.rpcClient.CallWithContext(ctx, peerID, action, payload)
    ↓ line 212  ← Bug #1: 卡在这里
client.rateLimiter.Acquire(ctx, peerID)
    ↓ line 223
c.cluster.LogRPCInfo("Found peer %s", peerID)
    ↓ line 235
c.cluster.LogRPCInfo("Peer %s is online", peerID)
    ↓ line 270
c.cluster.LogRPCInfo("Attempting to connect...")
    ↓ line 279
c.cluster.LogRPCInfo("Connected to peer...")
    ↓ line 286
c.cluster.LogRPCInfo("Sending request...")
    ↓ line 293
c.cluster.LogRPCInfo("Request sent successfully...")
    ↓ line 296
c.receiveResponseWithContext(ctx, conn, req.ID)
    ↓
服务端处理 (server.go:203)
    ↓ line 228
s.cluster.LogRPCInfo("Response: action=%s, from=%s, to=%s, id=%s, payload=%+v", ...)
    ↓
返回到客户端
    ↓ line 303
c.cluster.LogRPCInfo("Received response from %s: type=%s, id=%s", ...)
    ↓
返回到 main.go:709
log("DEBUG", "RPC -> %s: Call returned, err=%v", ...)
```

---

## Bug #1: RateLimiter 死循环

### 问题代码
**文件**: `module/cluster/rpc/client.go:84-165`

### 原因
```go
func (rl *RateLimiter) Acquire(ctx context.Context, peerID string) error {
    // ❌ 没有初始化新 peer

    for {
        if rl.tokens[peerID] > 0 {  // ❌ 新 peer 返回 0
            rl.tokens[peerID]--
            return nil
        }
        // ❌ 死循环：永远不会有 token
        select {
        case <-time.After(100 * time.Millisecond):
            continue
        }
    }
}
```

### 修复
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

### 修复位置
- **文件**: `module/cluster/rpc/client.go:85-91`

---

## Bug #2: Daemon 模式下 hello handler 未注册

### 原因分析

**Agent 模式**:
```
loop.go:1540 - clusterInstance.SetRPCChannel(rpcCh)
    ↓
cluster.go:653 - c.registerLLMHandlers()
    ↓
cluster.go:682 - handlers.RegisterCustomHandlers(c.logger, c.GetNodeID, registrar)
    ↓
handlers/custom.go:21 - registrar("hello", ...)  ← hello handler 被注册
```

**Daemon 模式**:
```
main.go:657 - clusterInstance.Start()
    ↓
❌ 没有调用 SetRPCChannel()
    ↓
❌ 没有调用 registerLLMHandlers()
    ↓
❌ hello handler 没有被注册！
```

### 修复

#### 步骤 1: 添加 RegisterBasicHandlers 方法
**文件**: `module/cluster/cluster.go:690`

```go
// RegisterBasicHandlers registers basic RPC handlers (default and custom)
// This can be called directly in daemon mode where RPCChannel is not available
func (c *Cluster) RegisterBasicHandlers() error {
	c.mu.RLock()
	serverRunning := c.running
	c.mu.RUnlock()

	if !serverRunning {
		return fmt.Errorf("cluster not running")
	}

	// Create a registrar function
	registrar := func(action string, handler func(map[string]interface{}) (map[string]interface{}, error)) {
		if err := c.RegisterRPCHandler(action, handler); err != nil {
			c.logger.RPCError("Failed to register handler '%s': %v", action, err)
		}
	}

	// Register default handlers (ping, get_capabilities, get_info)
	handlers.RegisterDefaultHandlers(
		c.logger,
		c.GetNodeID,
		c.GetCapabilities,
		c.GetOnlinePeers,
		registrar,
	)

	// Register custom handlers (hello, etc.)
	handlers.RegisterCustomHandlers(c.logger, c.GetNodeID, registrar)

	return nil
}
```

#### 步骤 2: 在 Daemon 模式下调用
**文件**: `nemesisbot/main.go:660-664`

```go
// Start cluster
log("INFO", "Starting cluster service...")
if err := clusterInstance.Start(); err != nil {
    log("ERROR", "Failed to start cluster: %v", err)
    os.Exit(1)
}

// ✅ 新增：注册基本 RPC handlers（包括 hello）
log("INFO", "Registering RPC handlers...")
if err := clusterInstance.RegisterBasicHandlers(); err != nil {
    log("ERROR", "Failed to register RPC handlers: %v", err)
    os.Exit(1)
}
```

---

## 修复验证

### 编译验证
```bash
$ go build ./module/...
✅ 编译成功

$ go build -o nemesisbot.exe ./nemesisbot/
✅ 主程序编译成功
```

### 代码验证

#### Bug #1 修复验证
```bash
$ grep -A 5 "Initialize peer tokens" module/cluster/rpc/client.go
// Initialize peer tokens if not exists
rl.mu.Lock()
if _, exists := rl.tokens[peerID]; !exists {
    rl.tokens[peerID] = rl.maxTokens
    rl.requests[peerID] = []time.Time{}
}
```
✅ 修复已应用

#### Bug #2 修复验证
```bash
$ grep "RegisterBasicHandlers" nemesisbot/main.go
clusterInstance.RegisterBasicHandlers()
```
✅ 调用已添加

---

## 预期结果

修复后，应该看到完整的日志流程：

### daemon.log
```
[DEBUG] RPC -> bot-localhost: Starting RPC call...
[DEBUG] RPC -> bot-localhost: Calling clusterInstance.Call()
[DEBUG] RPC -> bot-localhost: Call returned, err=<nil>  ← 应该出现
[INFO] RPC -> bot-localhost: Response: {...}  ← 应该出现
```

### rpc.log (客户端 - bot-Yaoguai)
```
[INFO] Registered default handlers: ping, get_capabilities, get_info  ← 新增
[INFO] Registered custom handlers: hello  ← 新增
[INFO] Calling bot-localhost: action=hello
[INFO] Found peer bot-localhost  ← 应该出现
[INFO] Peer bot-localhost is online  ← 应该出现
[INFO] Peer bot-localhost addresses: [192.168.137.42]  ← 应该出现
[INFO] Attempting to connect to peer bot-localhost at [192.168.137.42:21949]  ← 应该出现
[INFO] Connected to peer bot-localhost at 192.168.137.42:21949  ← 应该出现
[INFO] Sending request action=hello to peer bot-localhost (id=msg-xxx)  ← 应该出现
[INFO] Request sent successfully to peer bot-localhost, waiting for response (id=msg-xxx)  ← 应该出现
[INFO] Received response from bot-localhost: type=response, id=msg-xxx  ← 应该出现
```

### rpc.log (服务端 - bot-localhost)
```
[INFO] Registered default handlers: ping, get_capabilities, get_info  ← 新增
[INFO] Registered custom handlers: hello  ← 新增
[INFO] Accepted connection from 192.168.137.1:xxxxx  ← 应该出现
[INFO] Received request: action=hello, from=bot-Yaoguai, id=msg-xxx  ← 应该出现
[INFO] Hello handler: Received hello from bot-Yaoguai at ...  ← 应该出现
[INFO] Hello handler: Sending response to bot-Yaoguai  ← 应该出现
[INFO] Response: action=hello, from=bot-Yaoguai, to=bot-localhost, id=msg-xxx, payload=map[greeting:Hello! Received your greeting from bot-Yaoguai node_id:bot-localhost status:ok timestamp:...]  ← 应该出现
```

---

## 修改文件清单

### 修改的文件
1. `module/cluster/rpc/client.go`
   - 添加 RateLimiter 初始化逻辑（line 85-91）

2. `module/cluster/cluster.go`
   - 添加 RegisterBasicHandlers 方法（line 690-720）

3. `nemesisbot/main.go`
   - 在 daemon 模式下调用 RegisterBasicHandlers（line 660-664）

### 没有修改的文件
- `module/cluster/rpc/server.go` - 响应日志（已在之前修复）
- `module/cluster/handlers/custom.go` - hello handler 定义（已存在）
- `module/cluster/handlers/default.go` - default handlers（已存在）

---

## 总结

### 问题根源
两天的问题，两个根本原因：
1. **RateLimiter Bug**: 新 peer 永远得不到 tokens，死循环
2. **Handler 注册缺失**: Daemon 模式没有注册 hello handler

### 我的错误
1. **测试不充分**: 没有在实际环境中测试完整调用链
2. **没有追踪到底**: 没有发现 daemon 模式缺少 handler 注册

### 修复完成
- ✅ Bug #1: RateLimiter 初始化修复
- ✅ Bug #2: Daemon 模式 handler 注册修复
- ✅ 编译验证通过
- ⏳ 待用户实际运行测试

---

**修复完成时间**: 2026-03-04 21:30
**修改文件**: 3 个文件
**新增代码**: 约 40 行
**编译状态**: ✅ 成功
**测试状态**: ⏳ 待用户验证
