# Hello RPC 请求服务端处理流程完整分析

## 📋 分析概述

**分析日期**: 2026-03-04
**模式**: `nemesisbot daemon cluster auto`
**请求**: 客户端向服务端发送 `hello` RPC 请求
**目的**: 完整追踪服务端如何处理请求并返回响应

---

## 🔄 完整处理流程

### 阶段 1: 服务端启动与 Handler 注册

```
t0: Cluster.Start()
    ↓
t1: rpc.Server.Start(port)
    ↓
t2: Server.registerDefaultHandlers()
    ├─ 注册 ping, get_capabilities, get_info
    └─ ↓
t3: 注册完成后，Server 开始监听
    ↓
t4: Server.acceptLoop() 启动
    └─ 等待客户端连接...
```

**注册的 handlers**（在 Server.Start 中）:
- `ping` → 响应 status 和 node_id
- `get_capabilities` → 返回能力列表
- `get_info` → 返回节点信息和在线节点
- `hello` → **Custom handler**（在 SetRPCChannel 时注册）

---

### 阶段 2: 客户端连接与请求发送

```
Client (node-A)                    Server (node-B)
     |                                    |
     |  1. TCP 连接                       |
     |---------------------------------->|
     |                                    |
     |  2. 发送 RPC 请求                  |
     |    type: "request"                |
     |    action: "hello"                |
     |    from: "node-A"                 |
     |    id: "req-001"                  |
     |    payload: {                     |
     |      "from": "node-A",            |
     |      "timestamp": "2026-03-04..." |
     |    }                               |
     |---------------------------------->|
     |                                    |
     |                                    |  t5: Server 接收到请求
     |                                    |
     |  3. 接收 RPC 响应                  |
     |<----------------------------------|
     |                                    |
```

---

### 阶段 3: 服务端接收请求（详细流程）

#### Step 3.1: TCP 连接接受

**代码位置**: `rpc/server.go:123-142`

```go
// acceptLoop 接受连接
func (s *Server) acceptLoop() {
    for {
        conn, err := s.listener.Accept()  // ← 接受 TCP 连接
        if err != nil {
            // ... 错误处理
        }

        // 创建 goroutine 处理连接
        go s.handleConnection(conn)
    }
}
```

**日志输出**:
```
[INFO] Accepted connection from 192.168.1.100:54321
```

---

#### Step 3.2: 创建连接处理

**代码位置**: `rpc/server.go:145-200`

```go
func (s *Server) handleConnection(netConn net.Conn) {
    remoteAddr := netConn.RemoteAddr().String()

    // 创建 TCPConn wrapper
    tc := transport.NewTCPConn(netConn, config)
    tc.Start()

    // 添加到连接池
    s.conns[remoteAddr] = tc

    // 消息处理循环
    for {
        select {
        case msg, ok := <-tc.Receive():  // ← 接收消息
            if !ok { return }

            if msg.Type == transport.RPCTypeRequest {
                s.handleRequest(tc, msg)  // ← 处理请求
            }
        }
    }
}
```

---

#### Step 3.3: 请求处理

**代码位置**: `rpc/server.go:202-242`

```go
func (s *Server) handleRequest(conn *transport.TCPConn, req *transport.RPCMessage) {
    // 🔍 Step 1: 记录接收到的请求
    s.cluster.LogRPCInfo("Received request: action=%s, from=%s, id=%s",
        req.Action, req.From, req.ID)

    // 🔍 Step 2: 更新连接的节点 ID
    if conn.GetNodeID() == "" {
        conn.SetNodeID(req.From)
    }

    // 🔍 Step 3: 查找 handler
    s.mu.RLock()
    handler, exists := s.handlers[req.Action]  // ← 在 map 中查找
    s.mu.RUnlock()

    // 🔍 Step 4: 如果没有找到 handler
    if !exists {
        s.cluster.LogRPCInfo("No handler for action '%s', returning default response", req.Action)
        defaultPayload := map[string]interface{}{
            "response": fmt.Sprintf("Resp: %v", req.Payload),
        }
        resp := transport.NewResponse(req, defaultPayload)
        s.sendMessage(conn, resp)
        return
    }

    // 🔍 Step 5: 调用 handler
    result, err := handler(req.Payload)  // ← 调用 hello handler

    // 🔍 Step 6: 处理错误
    if err != nil {
        s.cluster.LogRPCError("Handler error for action '%s': %v", req.Action, err)
        resp := transport.NewError(req, err.Error())
        s.sendMessage(conn, resp)
        return
    }

    // 🔍 Step 7: 发送成功响应
    resp := transport.NewResponse(req, result)
    s.cluster.LogRPCDebug("Sending response: action=%s, id=%s", req.Action, req.ID)
    s.sendMessage(conn, resp)  // ← 发送回客户端
}
```

---

### 阶段 4: Hello Handler 执行（核心逻辑）

**代码位置**: `handlers/custom.go:19-49`

```go
registrar("hello", func(payload map[string]interface{}) (map[string]interface{}, error) {
    // 📦 Step 1: 提取请求参数
    from := ""
    if fromVal, ok := payload["from"].(string); ok {
        from = fromVal  // ← 提取 "from" 字段
    }

    timestamp := ""
    if tsVal, ok := payload["timestamp"].(string); ok {
        timestamp = tsVal  // ← 提取 "timestamp" 字段
    }

    // 📝 Step 2: 记录接收日志
    logger.LogRPCInfo("Hello handler: Received hello from %s at %s", from, timestamp)

    // 🔨 Step 3: 构建响应数据
    response := map[string]interface{}{
        "greeting":  fmt.Sprintf("Hello! Received your greeting from %s", from),
        "timestamp": time.Now().Format(time.RFC3339),  // ← 当前时间
        "node_id":   getNodeID(),                      // ← 本节点 ID
        "status":    "ok",
    }

    // 📝 Step 4: 记录发送日志
    logger.LogRPCInfo("Hello handler: Sending response to %s", from)

    // ✅ Step 5: 返回响应
    return response, nil
})
```

---

### 阶段 5: 响应返回给客户端

```
Server (node-B)                    Client (node-A)
     |                                    |
     |  5. 发送 RPC 响应                  |
     |    type: "response"               |
     |    action: "hello"                |
     |    id: "req-001"                   |
     |    payload: {                     |
     |      "greeting": "Hello! Received...", |
     |      "timestamp": "2026-03-04T18:...", |
     |      "node_id": "node-B",           |
     |      "status": "ok"                 |
     |    }                               |
     |---------------------------------->|
     |                                    |
     |                                    |  t6: Client 接收响应
```

---

## 📊 数据流转详解

### 请求数据结构

```json
{
  "type": "request",
  "action": "hello",
  "from": "node-A",
  "id": "req-001",
  "payload": {
    "from": "node-A",
    "timestamp": "2026-03-04T18:30:00Z"
  }
}
```

### 响应数据结构

```json
{
  "type": "response",
  "action": "hello",
  "from": "node-B",
  "id": "req-001",
  "payload": {
    "greeting": "Hello! Received your greeting from node-A",
    "timestamp": "2026-03-04T18:30:05.123Z",  // 服务端当前时间
    "node_id": "node-B",
    "status": "ok"
  }
}
```

---

## 📝 日志输出记录

### 服务端日志输出（按时间顺序）

```
t1: [INFO] Accepted connection from 192.168.1.100:54321
t2: [INFO] Received request: action=hello, from=node-A, id=req-001
t3: [INFO] Hello handler: Received hello from node-A at 2026-03-04T18:30:00Z
t4: [INFO] Hello handler: Sending response to node-A
t5: [DEBUG] Sending response: action=hello, id=req-001
```

**日志文件位置**:
```
.nemesisbot/logs/cluster/rpc.log
```

---

## 🔍 关键处理点

### 1. Handler 查找

```go
// 在 Server 的 handlers map 中查找
s.mu.RLock()
handler, exists := s.handlers["hello"]  // ← 查找 "hello" handler
s.mu.RUnlock()
```

**如果找到了**: 执行 handler 函数
**如果没找到**: 返回默认响应 `{response: "Resp: {...}"}`

---

### 2. Hello Handler 参数提取

```go
// 从 payload 中提取参数
payload = map[string]interface{}{
    "from": "node-A",
    "timestamp": "2026-03-04T18:30:00Z"
}

// 提取 "from"
from := payload["from"].(string)  // ← "node-A"

// 提取 "timestamp"
timestamp := payload["timestamp"].(string)  // ← "2026-03-04T18:30:00Z"
```

**如果参数不存在**: 使用空字符串 `""`

---

### 3. 响应构建

```go
response := map[string]interface{}{
    "greeting":  "Hello! Received your greeting from node-A",
    "timestamp": "2026-03-04T18:30:05.123Z",  // ← 服务端当前时间
    "node_id":   "node-B",                    // ← 服务端的节点 ID
    "status":    "ok",
}
```

---

## 🎯 完整时序图

```
Client                     Server                   Handlers
  |                          |                         |
  |--[TCP Connect]---------->|                         |
  |                          |                         |
  |--[RPC Request: hello]--->|                         |
  |                          |                         |
  |                          |--[Find handler]-------->|
  |                          |                         |
  |                          |<--[hello handler]-------|
  |                          |                         |
  |                          |--[Execute handler]----->|
  |                          |  Extract payload        |
  |                          |  Build response         |
  |                          |  Log response           |
  |                          |<--[Return result]-------|
  |                          |                         |
  |<--[RPC Response]---------|                         |
  |                          |                         |
```

---

## 📊 Handler 注册时机

### Default Handlers
**注册时机**: `Server.Start()` 时
```go
// rpc/server.go:69
s.registerDefaultHandlers()
```
- `ping`
- `get_capabilities`
- `get_info`

### Custom Handlers (包括 hello)
**注册时机**: `SetRPCChannel()` 被调用时
```go
// cluster.go:682
handlers.RegisterCustomHandlers(c.logger, c.GetNodeID, registrar)
```

这个调用发生在：
1. `loop.go` 创建 RPCChannel 后
2. 调用 `clusterInstance.SetRPCChannel(rpcCh)`
3. 在 `registerLLMHandlers()` 中注册

---

## ⚠️ 重要注意事项

### 1. Hello Handler 必须先注册

如果 `hello` handler 没有注册：
```
t2: [INFO] Received request: action=hello, from=node-A, id=req-001
t3: [INFO] No handler for action 'hello', returning default response
```

响应会变成：
```json
{
  "type": "response",
  "payload": {
    "response": "Resp: map[from:node-A timestamp:2026-03-04T18:30:00Z]"
  }
}
```

### 2. 日志级别问题

**INFO 级别** (可以看到):
- 接收请求
- Hello handler 的处理日志

**DEBUG 级别** (可能看不到):
- "Sending response"

建议：将 "Sending response" 改为 INFO 级别

### 3. 响应内容未记录

当前代码只记录：
```
[DEBUG] Sending response: action=hello, id=req-001
```

**没有记录响应的实际内容**，即：
- greeting
- timestamp
- node_id
- status

---

## 🧪 验证方法

### 查看日志验证处理流程

```bash
# 实时查看 RPC 日志
tail -f .nemesisbot/logs/cluster/rpc.log

# 应该看到：
# [INFO] Accepted connection from xxx:xxxxx
# [INFO] Received request: action=hello, from=node-A, id=req-xxx
# [INFO] Hello handler: Received hello from node-A at ...
# [INFO] Hello handler: Sending response to node-A
# [DEBUG] Sending response: action=hello, id=req-xxx
```

---

## 📝 总结

### 服务端处理流程概要

1. **启动阶段**:
   - Server 启动并监听端口
   - 注册 default handlers
   - 等待连接

2. **连接阶段**:
   - 接受客户端 TCP 连接
   - 创建连接处理 goroutine
   - 进入消息循环

3. **请求处理阶段**:
   - 从 TCP 连接接收消息
   - 解析 RPC 消息
   - 查找对应的 handler
   - 调用 handler 处理请求

4. **Hello Handler 执行**:
   - 提取 "from" 和 "timestamp" 参数
   - 记录接收日志
   - 构建响应（greeting, timestamp, node_id, status）
   - 记录发送日志
   - 返回响应

5. **响应返回**:
   - 将响应包装成 RPC 消息
   - 通过 TCP 连接发送回客户端
   - 记录发送日志

### 关键代码位置

| 步骤 | 文件 | 行号 | 说明 |
|------|------|------|------|
| 接收连接 | rpc/server.go | 123-142 | acceptLoop |
| 处理连接 | rpc/server.go | 145-200 | handleConnection |
| 处理请求 | rpc/server.go | 202-242 | handleRequest |
| Hello 逻辑 | handlers/custom.go | 19-49 | Hello handler |

---

**分析完成时间**: 2026-03-04
**状态**: ✅ 完成
