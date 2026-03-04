═══════════════════════════════════════════════════════════════
         RPC "hello" 请求完整流程详细描述
═══════════════════════════════════════════════════════════════

## 📌 场景设定

- Client 节点: bot-LAPTOP-FGO6HJ0E-20260304-120000
- Server 节点: bot-DESKTOP-ABC123-20260304-115000
- Server 地址: 192.168.1.100:21949

═══════════════════════════════════════════════════════════════

## 步骤 1: Client 发送请求

### 代码位置
nemesisbot/main.go:705-708

### 调用代码
response, err := clusterInstance.Call(n.ID, "hello", map[string]interface{}{
    "from": clusterInstance.GetNodeID(),
    "timestamp": time.Now().Format(time.RFC3339),
})

### 参数解析表

| 参数名 | 实际值 | 说明 |
|--------|--------|------|
| n.ID | "bot-DESKTOP-ABC123-20260304-115000" | 目标Server节点ID |
| "hello" | "hello" | RPC Action名称 |
| map{} | 见下方Payload表 | 请求数据负载 |

### Payload 组织表

| 字段名 | 值 | 类型 | 来源 |
|--------|-----|------|------|
| from | "bot-LAPTOP-FGO6HJ0E-20260304-120000" | string | clusterInstance.GetNodeID() |
| timestamp | "2026-03-04T12:00:00+08:00" | string | time.Now().Format(time.RFC3339) |

### Client 内部处理流程

| 步骤 | 操作 | 代码位置 |
|------|------|----------|
| 1 | 从连接池获取TCP连接 | client.go:442 c.pool.Get(peerID, address) |
| 2 | 创建 RPCMessage | client.go:266 transport.NewRequest(from, to, "hello", payload) |
| 3 | 发送JSON数据 | client.go:271 conn.Send(req) |
| 4 | 等待响应 | client.go:279 c.receiveResponseWithContext() |

### 生成的完整 JSON (Client → Server)

{
  "version": "1.0",
  "id": "msg-1709553600123456789-1234",
  "type": "request",
  "from": "bot-LAPTOP-FGO6HJ0E-20260304-120000",
  "to": "bot-DESKTOP-ABC123-20260304-115000",
  "action": "hello",
  "payload": {
    "from": "bot-LAPTOP-FGO6HJ0E-20260304-120000",
    "timestamp": "2026-03-04T12:00:00+08:00"
  },
  "timestamp": 1709553600
}

### JSON 字段来源对照表

| JSON字段 | 值示例 | 来源代码 |
|----------|--------|----------|
| version | "1.0" | 常量 RPCProtocolVersion |
| id | "msg-1709553600123456789-1234" | generateID() 自动生成 |
| type | "request" | RPCTypeRequest 常量 |
| from | "bot-LAPTOP-FGO6HJ0E-..." | c.cluster.GetNodeID() |
| to | "bot-DESKTOP-ABC123-..." | 参数 peerID |
| action | "hello" | 参数 "hello" |
| payload.from | "bot-LAPTOP-FGO6HJ0E-..." | clusterInstance.GetNodeID() |
| payload.timestamp | "2026-03-04T12:00:00+08:00" | time.Now().Format(time.RFC3339) |
| timestamp | 1709553600 | req.Timestamp = time.Now().Unix() |

═══════════════════════════════════════════════════════════════

## 步骤 2: Server 接收并处理请求

### 代码位置
module/cluster/rpc/server.go:202-241

### Server 处理流程表

| 步骤 | 操作 | 代码位置 | 日志输出 |
|------|------|----------|----------|
| 1 | 接收TCP连接 | server.go:126-140 | "Accepted connection from 192.168.1.100:xxxxx" |
| 2 | 解析JSON为RPCMessage | conn.go:160 | - |
| 3 | 验证消息格式 | rpc.go:84-106 | - |
| 4 | 记录接收日志 | server.go:203 | "Received request: action=hello, from=bot-LAPTOP-..." |
| 5 | 查找 handler | server.go:212 | - |
| 6 | ❌ Handler不存在 | server.go:215 | "No handler for action 'hello', returning default response" |
| 7 | 生成默认响应 | server.go:220-223 | - |

### 查找 Handler 逻辑

// server.go:211-213
s.mu.RLock()
handler, exists := s.handlers[req.Action]  // 查找 "hello"
s.mu.RUnlock()

if !exists {
    // 进入默认处理逻辑
}

### 已注册的 Handlers 表

| Action | Handler | 是否存在 |
|--------|---------|----------|
| "ping" | ✅ 已注册 | 是 |
| "get_capabilities" | ✅ 已注册 | 是 |
| "get_info" | ✅ 已注册 | 是 |
| "hello" | ❌ 未注册 | 否 ← 这就是问题所在！ |

### 默认响应生成逻辑

// server.go:220-223
defaultPayload := map[string]interface{}{
    "response": fmt.Sprintf("Resp: %v", req.Payload),
}
resp := transport.NewResponse(req, defaultPayload)

### Payload 格式化细节

| 操作 | 代码 | 结果 |
|------|------|------|
| 格式化 | fmt.Sprintf("Resp: %v", req.Payload) | "Resp: map[from:bot-LAPTOP-FGO6HJ0E-20260304-120000 timestamp:2026-03-04T12:00:00+08:00]" |

注意: %v 格式化 map 会输出类似 map[key:value key:value] 的字符串

═══════════════════════════════════════════════════════════════

## 步骤 3: Server 返回响应

### 响应创建代码
transport/rpc.go:56-67

func NewResponse(req *RPCMessage, payload map[string]interface{}) *RPCMessage {
    return &RPCMessage{
        Version:   RPCProtocolVersion,
        ID:        req.ID,              // ← 使用请求的相同ID
        Type:      RPCTypeResponse,     // ← type="response"
        From:      req.To,              // ← From变成请求的To
        To:        req.From,            // ← To变成请求的From
        Action:    req.Action,          // ← Action保持不变
        Payload:   payload,             // ← handler返回的payload
        Timestamp: 0,
    }
}

### 响应字段转换表

| 字段 | 请求值 | 响应值 | 转换规则 |
|------|--------|--------|----------|
| version | "1.0" | "1.0" | 保持不变 |
| id | "msg-xxx-1234" | "msg-xxx-1234" | 使用请求的相同ID |
| type | "request" | "response" | 改为response |
| from | "bot-LAPTOP-..." | "bot-DESKTOP-..." | 互换 (使用请求的to) |
| to | "bot-DESKTOP-..." | "bot-LAPTOP-..." | 互换 (使用请求的from) |
| action | "hello" | "hello" | 保持不变 |
| payload | 用户数据 | handler返回数据 | 由handler决定 |
| timestamp | 1709553600 | 1709553601 | 更新为当前时间 |

### 生成的完整 JSON (Server → Client)

无 handler 的情况 (默认响应):

{
  "version": "1.0",
  "id": "msg-1709553600123456789-1234",
  "type": "response",
  "from": "bot-DESKTOP-ABC123-20260304-115000",
  "to": "bot-LAPTOP-FGO6HJ0E-20260304-120000",
  "action": "hello",
  "payload": {
    "response": "Resp: map[from:bot-LAPTOP-FGO6HJ0E-20260304-120000 timestamp:2026-03-04T12:00:00+08:00]"
  },
  "timestamp": 1709553601
}

有 handler 的情况 (假设我添加了hello handler):

{
  "version": "1.0",
  "id": "msg-1709553600123456789-1234",
  "type": "response",
  "from": "bot-DESKTOP-ABC123-20260304-115000",
  "to": "bot-LAPTOP-FGO6HJ0E-20260304-120000",
  "action": "hello",
  "payload": {
    "greeting": "Hello! Received your greeting from bot-LAPTOP-FGO6HJ0E-20260304-120000",
    "timestamp": "2026-03-04T12:00:01+08:00",
    "node_id": "bot-DESKTOP-ABC123-20260304-115000",
    "status": "ok"
  },
  "timestamp": 1709553601
}

═══════════════════════════════════════════════════════════════

## 步骤 4: Client 接收响应

### 代码位置
module/cluster/rpc/client.go:318-371

### 接收流程表

| 步骤 | 操作 | 代码 | 条件 |
|------|------|------|------|
| 1 | 从TCP读取数据 | conn.Receive() | 等待数据 |
| 2 | 解析JSON为RPCMessage | json.Unmarshal() | 自动 |
| 3 | 检查ID是否匹配 | msg.ID == messageID | 核心匹配逻辑 |
| 4 | 如果匹配则返回 | return msg, nil | ✅ |
| 5 | 不匹配则继续等待 | continue | 等待下一条消息 |
| 6 | 超时处理 | timeout | 30秒后超时 |

### ID 匹配逻辑

// client.go:359
if msg.ID == messageID {  // messageID = "msg-xxx-1234"
    return msg, nil  // 找到对应的响应
}
// 不是我的响应，继续等待...

### 接收到的数据处理

// client.go:289-303
if response.Type == transport.RPCTypeError {
    return nil, fmt.Errorf("RPC error from peer: %s", response.Error)
}

responseData, err := json.Marshal(response.Payload)
if err != nil {
    return nil, fmt.Errorf("failed to marshal response: %w", err)
}

return responseData, nil  // 返回payload的JSON字节

### Client 输出日志

// main.go:713
log("INFO", "RPC -> %s: Response: %s", n.ID, string(response))

实际输出:
[2026-03-04 12:00:01] [INFO] RPC -> bot-DESKTOP-ABC123-20260304-115000: Response: {"response":"Resp: map[from:bot-LAPTOP-FGO6HJ0E-20260304-120000 timestamp:2026-03-04T12:00:00+08:00]"}

═══════════════════════════════════════════════════════════════

## 📊 完整流程总览表

| 阶段 | 方向 | JSON Type | 关键字段值 | 日志位置 |
|------|------|-----------|------------|----------|
| 发送请求 | Client → Server | request | action="hello" | daemon.log |
| 接收请求 | Server 接收 | - | action=hello | rpc.log |
| 查找handler | Server 内部 | - | exists=false | rpc.log |
| 生成响应 | Server 内部 | - | defaultPayload | rpc.log |
| 返回响应 | Server → Client | response | payload.response | rpc.log |
| 接收响应 | Client 接收 | - | id匹配成功 | daemon.log |

═══════════════════════════════════════════════════════════════

## 🗂️ 日志文件对应表

| 文件 | 位置 | 内容示例 |
|------|------|----------|
| daemon.log | workspace/logs/cluster/daemon.log | Client端日志: RPC调用和响应 |
| rpc.log | workspace/logs/cluster/rpc.log | Server端日志: 请求接收和处理 |
| discovery.log | workspace/logs/cluster/discovery.log | UDP发现日志 |

### 典型日志内容

daemon.log (Client端):
[2026-03-04 12:00:00] [INFO] RPC: Calling 1 nodes...
[2026-03-04 12:00:00] [INFO] RPC -> bot-DESKTOP-...: Starting RPC call...
[2026-03-04 12:00:01] [INFO] RPC -> bot-DESKTOP-...: Response: {"response":"Resp: map[...]"}

rpc.log (Server端):
[2026-03-04 12:00:00] [INFO] Received request: action=hello, from=bot-LAPTOP-..., id=msg-xxx-1234
[2026-03-04 12:00:00] [INFO] No handler for action 'hello', returning default response

═══════════════════════════════════════════════════════════════

## 🎯 关键要点

1. ID匹配机制: Client通过ID匹配请求和响应，所以ID必须相同
2. From/To互换: 响应中的From和To与请求相反
3. Payload灵活性:
   - 无handler时: {"response": "Resp: map[...]"}
   - 有handler时: 由handler自定义结构
4. 日志分离: Client和Server的日志在不同文件
5. Handler查找: 通过action字段查找对应的handler函数

═══════════════════════════════════════════════════════════════
