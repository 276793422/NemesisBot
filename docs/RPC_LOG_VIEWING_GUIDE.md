# RPC 响应内容日志查看指南

## 📋 当前状态

### 日志文件位置

```
{工作目录}/logs/cluster/rpc.log
```

**工作目录通常是**：
- 默认配置：`{项目根目录}/.nemesisbot/`
- 示例：`C:\AI\NemesisBot\NemesisBot\.nemesisbot\logs\cluster\rpc.log`

### 当前日志输出

#### 已记录的日志

1. **接收请求** (server.go:204)
   ```
   [INFO] Received request: action=ping, from=node-1, id=req-123
   ```

2. **发送响应** (server.go:240) - DEBUG 级别
   ```
   [DEBUG] Sending response: action=ping, id=req-123
   ```
   ⚠️ **注意**：不包含响应内容

3. **Handler 错误** (server.go:232)
   ```
   [ERROR] Handler error for action 'llm_forward': timeout
   ```

#### 未记录的内容

- ❌ 响应的具体内容（payload）
- ❌ 成功响应的详细信息
- ❌ 响应的错误详情（除了错误消息）

---

## 🔍 如何查看日志

### 方法 1: 实时查看 RPC 日志

```bash
# 进入项目目录
cd C:\AI\NemesisBot\NemesisBot

# 实时查看 RPC 日志
tail -f .nemesisbot/logs/cluster/rpc.log
```

### 方法 2: 查看最近的日志

```bash
# 查看最近 100 行
tail -n 100 .nemesisbot/logs/cluster/rpc.log

# 查看所有日志
cat .nemesisbot/logs/cluster/rpc.log
```

### 方法 3: 搜索特定内容

```bash
# 搜索特定 action
grep "action=ping" .nemesisbot/logs/cluster/rpc.log

# 搜索特定节点
grep "from=node-1" .nemesisbot/logs/cluster/rpc.log

# 搜索错误
grep "\[ERROR\]" .nemesisbot/logs/cluster/rpc.log
```

---

## 💡 建议的改进

### 改进方案 1: 添加响应内容日志

修改 `server.go:238-241`，添加响应内容日志：

```go
// Send success response
resp := transport.NewResponse(req, result)
s.cluster.LogRPCDebug("Sending response: action=%s, id=%s", req.Action, req.ID)

// ✅ 新增：记录响应内容
if resp.Type == transport.RPCTypeResponse {
    s.cluster.LogRPCInfo("Response payload: action=%s, id=%s, payload=%+v", req.Action, req.ID, resp.Payload)
}

s.sendMessage(conn, resp)
```

### 改进方案 2: 将发送响应改为 INFO 级别

修改 `server.go:240`：

```go
// 将 LogRPCDebug 改为 LogRPCInfo，这样默认就能看到
s.cluster.LogRPCInfo("Sending response: action=%s, id=%s, payload=%+v", req.Action, req.ID, result)
```

---

## 📊 当前日志级别说明

| 级别 | 方法 | 当前用途 |
|------|------|---------|
| **INFO** | `LogRPCInfo` | 重要信息：请求接收、Handler 注册 |
| **ERROR** | `LogRPCError` | 错误信息：Handler 执行失败 |
| **DEBUG** | `LogRPCDebug` | 调试信息：发送响应（可能看不到） |

---

## 🎯 查看完整 RPC 流程

### 如果你想看完整的请求-响应流程

1. **查看 RPC 日志**
   ```bash
   tail -f .nemesisbot/logs/cluster/rpc.log
   ```

2. **查看所有 cluster 日志**
   ```bash
   tail -f .nemesisbot/logs/cluster/*.log
   ```

3. **启用 DEBUG 级别日志**（需要修改代码或配置）

---

## ✅ 快速检查

### 检查日志文件是否存在

```bash
# 检查日志目录
ls -la .nemesisbot/logs/cluster/

# 应该看到：
# discovery.log
# rpc.log
```

### 查看 RPC 日志内容

```bash
# 查看最后 20 行
tail -n 20 .nemesisbot/logs/cluster/rpc.log
```

---

**说明**：当前代码中，响应的具体内容（payload）没有被记录到日志中。如果需要查看完整的响应内容，需要修改代码添加日志输出。
