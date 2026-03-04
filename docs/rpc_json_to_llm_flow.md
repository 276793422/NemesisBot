═══════════════════════════════════════════════════════════════
         RPC 消息从 JSON 到 LLM 的完整解析流程
═══════════════════════════════════════════════════════════════

## 📋 问题确认

**你的理解完全正确！**流程应该是：

1. RPC Server 接收 TCP 数据流
2. 从二进制帧解析 JSON
3. JSON 反序列化为 RPCMessage 结构体
4. 根据 action 字段查找对应的 handler
5. Handler 被调用，处理 payload
6. Handler 调用 RPCChannel.Input()
7. 数据最终发送到 LLM

═══════════════════════════════════════════════════════════════

## 🔄 完整代码流程追踪

### 层次 1: TCP 网络层接收数据

**代码位置**: `server.go:122-141`

```go
// server.go:123
func (s *Server) acceptLoop() {
    for {
        conn, err := s.listener.Accept()  // ← TCP 三次握手完成
        if err != nil {
            // ...
        }

        // 为每个连接启动一个 goroutine
        go s.handleConnection(conn)  // ← 进入下一层
    }
}
```

**输入**: TCP 流 (二进制数据)
**输出**: net.Conn 对象

---

### 层次 2: TCP 连接封装

**代码位置**: `server.go:144-199`

```go
// server.go:145
func (s *Server) handleConnection(netConn net.Conn) {
    // 2.1 创建 TCPConn 包装器
    config := &transport.TCPConnConfig{
        NodeID:           "",
        Address:          remoteAddr,
        ReadBufferSize:   100,
        SendBufferSize:   100,
        SendTimeout:      s.sendTimeout,
        IdleTimeout:      s.idleTimeout,
        HeartbeatInterval: 0,
    }

    tc := transport.NewTCPConn(netConn, config)
    tc.Start()  // ← 启动读写 goroutine

    // 2.2 消息处理循环
    for {
        select {
        case msg, ok := <-tc.Receive():  // ← 从接收通道获取消息
            if !ok {
                return
            }

            // 2.3 只处理请求类型消息
            if msg.Type == transport.RPCTypeRequest {
                s.handleRequest(tc, msg)  // ← 进入下一层
            }
        }
    }
}
```

**输入**: net.Conn (原始 TCP 连接)
**输出**: *RPCMessage (已解析的消息)

---

### 层次 3: 二进制帧解析 (JSON 在这一层被提取)

**代码位置**: `conn.go:130-171`

```go
// conn.go:130
func (tc *TCPConn) readLoop() {
    defer tc.wg.Done()

    fr := NewFrameReader(tc.conn)  // ← 创建帧读取器

    for {
        // 3.1 读取帧数据 (二进制)
        data, err := fr.ReadFrame()  // ← frame.go:105-107
        if err != nil {
            tc.Close()
            return
        }

        // 3.2 更新活跃时间
        tc.lastUsed.Store(time.Now())

        // 3.3 解析 JSON 为 RPCMessage (关键步骤！)
        var msg RPCMessage
        if err := json.Unmarshal(data, &msg); err != nil {
            // 无效消息，跳过
            continue
        }

        // 3.4 发送到接收通道
        select {
        case tc.recvChan <- &msg:  // ← 发送到通道
        default:
            // 通道满了，丢弃消息
        }
    }
}
```

**帧格式**: `[4字节长度(大端序)] + [JSON数据]`

**帧读取代码**: `frame.go:104-108`

```go
// frame.go:104
func (fr *FrameReader) ReadFrame() ([]byte, error) {
    return DecodeFrame(fr.reader)  // ← frame.go:53-89
}

// frame.go:53-89
func DecodeFrame(r io.Reader) ([]byte, error) {
    // 1. 读取 4 字节长度头
    header := make([]byte, 4)
    io.ReadFull(r, header)

    // 2. 解析长度 (大端序)
    dataLen := binary.BigEndian.Uint32(header)

    // 3. 读取数据部分
    data := make([]byte, dataLen)
    io.ReadFull(r, data)

    return data, nil  // ← 返回 JSON 字节
}
```

**输入**: TCP 流 (二进制帧)
**输出**: RPCMessage 结构体

**JSON 解析前后对比**:

```
TCP 接收的二进制数据:
[0x00 0x00 0x01 0x5A] [{"version":"1.0","id":"msg-xxx",...}]
 ↑ 4字节长度头   ↑ JSON数据

解析为 RPCMessage:
{
    Version:   "1.0",
    ID:        "msg-xxx",
    Type:      "request",
    From:      "bot-A",
    To:        "bot-B",
    Action:    "llm_forward",
    Payload:   {...},
    Timestamp: 1709553600
}
```

---

### 层次 4: Handler 查找与分发

**代码位置**: `server.go:201-241`

```go
// server.go:202
func (s *Server) handleRequest(conn *transport.TCPConn, req *transport.RPCMessage) {
    // 4.1 记录日志
    s.cluster.LogRPCInfo("Received request: action=%s, from=%s, id=%s",
        req.Action, req.From, req.ID)

    // 4.2 更新连接的节点 ID
    if conn.GetNodeID() == "" {
        conn.SetNodeID(req.From)
    }

    // 4.3 查找 handler (关键步骤！)
    s.mu.RLock()
    handler, exists := s.handlers[req.Action]  // ← 根据 action 查找
    s.mu.RUnlock()

    if !exists {
        // 4.4 没有 handler，返回默认响应
        s.cluster.LogRPCInfo("No handler for action '%s'", req.Action)
        defaultPayload := map[string]interface{}{
            "response": fmt.Sprintf("Resp: %v", req.Payload),
        }
        resp := transport.NewResponse(req, defaultPayload)
        s.sendMessage(conn, resp)
        return
    }

    // 4.5 调用 handler (进入下一层)
    result, err := handler(req.Payload)  // ← payload 是 map[string]interface{}
    if err != nil {
        resp := transport.NewError(req, err.Error())
        s.sendMessage(conn, resp)
        return
    }

    // 4.6 发送成功响应
    resp := transport.NewResponse(req, result)
    s.sendMessage(conn, resp)
}
```

**handlers map 结构**:

```go
// server.go:20
handlers map[string]RPCHandler

// RPCHandler 类型 (server.go:36)
type RPCHandler func(payload map[string]interface{}) (map[string]interface{}, error)
```

**已注册的 handlers**:

```go
// server.go:249-286
func (s *Server) registerDefaultHandlers() {
    // Handler 1: "ping"
    s.RegisterHandler("ping", func(payload map[string]interface{}) (map[string]interface{}, error) {
        return map[string]interface{}{
            "status": "ok",
            "node_id": s.cluster.GetNodeID(),
        }, nil
    })

    // Handler 2: "get_capabilities"
    s.RegisterHandler("get_capabilities", func(payload map[string]interface{}) (map[string]interface{}, error) {
        caps := s.cluster.GetCapabilities()
        return map[string]interface{}{
            "capabilities": caps,
        }, nil
    })

    // Handler 3: "get_info"
    s.RegisterHandler("get_info", func(payload map[string]interface{}) (map[string]interface{}, error) {
        peers := s.cluster.GetOnlinePeers()
        // ...
        return map[string]interface{}{
            "node_id": s.cluster.GetNodeID(),
            "peers":    peerInfos,
        }, nil
    })
}

// loop.go:1540-1546 - LLMForwardHandler 注册
llmForwardHandler := clusterrpc.NewLLMForwardHandler(clusterInstance, rpcCh)
clusterInstance.RegisterRPCHandler("llm_forward", llmForwardHandler.Handle)
```

**Handler 查找表**:

| Action | Handler | 位置 |
|--------|---------|------|
| "ping" | ✅ 已注册 | server.go:251-256 |
| "get_capabilities" | ✅ 已注册 | server.go:259-264 |
| "get_info" | ✅ 已注册 | server.go:267-285 |
| "llm_forward" | ✅ 已注册 | loop.go:1540-1546 |
| "hello" | ✅ 已注册 (我添加的) | rpc_handlers.go:14-40 |

**输入**: RPCMessage 结构体
**输出**: 调用具体的 handler 函数

---

### 层次 5: LLMForwardHandler 处理

**代码位置**: `llm_forward_handler.go:48-130`

```go
// llm_forward_handler.go:50
func (h *LLMForwardHandler) Handle(payload map[string]interface{}) (map[string]interface{}, error) {
    h.cluster.LogRPCInfo("[LLMForward] Received request", nil)

    // 5.1 解析 RPC Payload (第一次转换)
    var req LLMForwardPayload
    if err := h.parsePayload(payload, &req); err != nil {
        return h.errorResponse("invalid payload: " + err.Error()), nil
    }

    // 验证必填字段
    if req.ChatID == "" {
        return h.errorResponse("chat_id is required"), nil
    }
    if req.Content == "" {
        return h.errorResponse("content is required"), nil
    }

    // 5.2 构造 InboundMessage (第二次转换)
    inbound := bus.InboundMessage{
        Channel:    "rpc",
        ChatID:     req.ChatID,
        Content:    req.Content,
        SenderID:   req.SenderID,
        SessionKey: req.SessionKey,
        Metadata:   req.Metadata,
    }

    // 5.3 调用 RPCChannel.Input() (第三次转换 - 关键！)
    ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
    defer cancel()

    respCh, err := h.rpcChannel.Input(ctx, &inbound)  // ← 进入下一层
    if err != nil {
        return h.errorResponse("failed to process: " + err.Error()), nil
    }

    // 5.4 等待 LLM 响应
    h.cluster.LogRPCInfo("[LLMForward] Waiting for LLM response", nil)

    select {
    case response, ok := <-respCh:
        if !ok {
            return h.errorResponse("LLM processing timeout"), nil
        }
        return h.successResponse(response), nil

    case <-ctx.Done():
        return h.errorResponse("LLM processing timeout (60s)"), nil
    }
}
```

**Payload 解析代码**:

```go
// llm_forward_handler.go:132-145
func (h *LLMForwardHandler) parsePayload(payload map[string]interface{}, req *LLMForwardPayload) error {
    // 转换为 JSON 字节
    payloadBytes, err := json.Marshal(payload)
    if err != nil {
        return fmt.Errorf("failed to marshal payload: %w", err)
    }

    // 从 JSON 字节解析为结构体
    if err := json.Unmarshal(payloadBytes, req); err != nil {
        return fmt.Errorf("failed to unmarshal payload: %w", err)
    }

    return nil
}
```

**数据转换链**:

```
map[string]interface{} (RPC Payload)
    ↓ json.Marshal()
[]byte (JSON 字节)
    ↓ json.Unmarshal()
LLMForwardPayload (结构体)
    ↓ 提取字段
bus.InboundMessage (结构体)
    ↓ rpcChannel.Input()
<-chan string (响应通道)
```

**输入**: map[string]interface{} (RPC Payload)
**输出**: 调用 RPCChannel.Input()

---

### 层次 6: RPCChannel.Input() - CorrelationID 生成

**代码位置**: `rpc_channel.go:183-216`

```go
// rpc_channel.go:183
func (ch *RPCChannel) Input(ctx context.Context, inbound *bus.InboundMessage) (<-chan string, error) {
    // 6.1 生成 CorrelationID (关键！)
    if inbound.CorrelationID == "" {
        inbound.CorrelationID = generateCorrelationID()  // ← "rpc-1709553600123456789"
    }
    inbound.Channel = ch.Name()  // 设置为 "rpc"

    // 6.2 创建响应通道
    respCh := make(chan string, 1)

    // 6.3 注册待处理请求
    ch.mu.Lock()
    ch.pendingReqs[inbound.CorrelationID] = &pendingRequest{
        correlationID: inbound.CorrelationID,
        responseCh:    respCh,
        createdAt:     time.Now(),
        timeout:       ch.getRequestTimeout(inbound.Metadata),
    }
    ch.mu.Unlock()

    // 6.4 发送到 MessageBus (进入 LLM 处理流程)
    ch.base.bus.PublishInbound(*inbound)  // ← 这是关键！

    return respCh, nil  // 返回响应通道
}
```

**pendingReqs map 结构**:

```go
// rpc_channel.go:26
pendingReqs map[string]*pendingRequest  // correlation_id → request

type pendingRequest struct {
    correlationID string
    responseCh    chan string
    createdAt     time.Time
    timeout       time.Duration
}
```

**输入**: InboundMessage 结构体
**输出**:
- 发送到 MessageBus
- 返回响应通道

---

### 层次 7: MessageBus → AgentLoop → LLM

**代码位置**: `loop.go:506-519`

```go
// loop.go:506
func (al *AgentLoop) processMessage(msg bus.InboundMessage, agent *agent.Agent, ...) {
    // ...

    // 7.1 将 CorrelationID 添加到 context (关键！)
    if msg.CorrelationID != "" {
        ctx = context.WithValue(ctx, "correlation_id", msg.CorrelationID)
    }

    // 7.2 调用 AgentLoop
    result, err := al.runAgentLoop(ctx, agent, processOptions{
        SessionKey:      msg.SessionKey,
        Channel:         msg.Channel,
        ChatID:          msg.ChatID,
        UserMessage:     msg.Content,
        // ...
    })

    // ...
}
```

**进入 LLM 处理流程**:
- LLM 接收用户消息
- LLM 调用工具（包括 MessageTool）
- LLM 生成回复

---

### 层次 8: MessageTool 添加 CorrelationID 前缀

**代码位置**: `message.go:92-99`

```go
// message.go:92
finalContent := content
if channel == "rpc" {
    // 8.1 从 context 读取 CorrelationID
    if correlationID := getCorrelationIDFromContext(ctx); correlationID != "" {
        // 8.2 添加前缀到内容
        finalContent = fmt.Sprintf("[rpc:%s] %s", correlationID, content)
    }
}

// 8.3 发送响应
t.sendCallback(channel, chatID, finalContent)
```

**Content 格式变化**:

```
LLM 生成的原始内容:
"AI is a technology that..."

添加 CorrelationID 前缀后:
"[rpc:rpc-1709553600123456789] AI is a technology that..."
```

---

### 层次 9: MessageBus → RPCChannel.outboundListener()

**代码位置**: `rpc_channel.go:234-278`

```go
// rpc_channel.go:234
func (ch *RPCChannel) outboundListener(ctx context.Context) {
    for {
        select {
        case msg, ok := <-ch.base.bus.OutboundChannel():
            if !ok {
                return
            }

            // 9.1 只处理来自 "rpc" channel 的消息
            if msg.Channel != ch.Name() {
                continue
            }

            // 9.2 提取 CorrelationID (关键！)
            correlationID := extractCorrelationID(msg.Content)
            if correlationID == "" {
                continue  // 不是 RPC 响应，跳过
            }

            // 9.3 查找待处理请求
            ch.mu.RLock()
            req, exists := ch.pendingReqs[correlationID]
            ch.mu.RUnlock()

            if exists {
                // 9.4 移除 CorrelationID 前缀
                actualContent := removeCorrelationID(msg.Content)

                // 9.5 发送到响应通道
                select {
                case req.responseCh <- actualContent:
                    // 成功发送
                case <-time.After(time.Second):
                    // 超时
                }

                // 9.6 从 pendingReqs 删除
                ch.mu.Lock()
                delete(ch.pendingReqs, correlationID)
                ch.mu.Unlock()
            }
        }
    }
}
```

**CorrelationID 提取代码**:

```go
// rpc_channel.go:346-357
func extractCorrelationID(content string) string {
    if !strings.HasPrefix(content, "[rpc:") {
        return ""
    }

    // 查找结束的 "]"
    end := strings.Index(content, "]")
    if end == -1 {
        return ""
    }

    // 提取 "rpc:xxx" 中的 "xxx"
    return content[5:end]  // "[rpc:xxx]" → "xxx"
}
```

---

### 层次 10: 返回到 LLMForwardHandler

**代码位置**: `llm_forward_handler.go:107-119`

```go
// llm_forward_handler.go:107
select {
case response, ok := <-respCh:  // ← 从 rpcChannel.Input() 返回的通道
    if !ok {
        return h.errorResponse("LLM processing timeout"), nil
    }

    // 10.1 构造成功响应
    return h.successResponse(response), nil

case <-ctx.Done():
    return h.errorResponse("LLM processing timeout (60s)"), nil
}

// llm_forward_handler.go:148-152
func (h *LLMForwardHandler) successResponse(content string) map[string]interface{} {
    return map[string]interface{}{
        "success": true,
        "content": content,  // ← LLM 的实际响应
    }
}
```

---

### 层次 11: RPC Server 发送响应

**代码位置**: `server.go:238-240`

```go
// server.go:238
resp := transport.NewResponse(req, result)  // ← result 是 handler 返回的
s.sendMessage(conn, resp)

// server.go:244-245
func (s *Server) sendMessage(conn *transport.TCPConn, msg *transport.RPCMessage) error {
    return conn.Send(msg)
}
```

---

### 层次 12: TCP 发送 (JSON 序列化 + 帧编码)

**代码位置**: `conn.go:230-248`

```go
// conn.go:230
func (tc *TCPConn) Send(msg *RPCMessage) error {
    // 12.1 JSON 序列化
    data, err := msg.Bytes()  // ← json.Marshal(msg)
    if err != nil {
        return fmt.Errorf("failed to marshal message: %w", err)
    }

    // 12.2 发送到写 goroutine
    select {
    case tc.sendChan <- data:  // ← 发送 JSON 字节
        return nil
    case <-time.After(tc.sendTimeout):
        return ErrSendTimeout
    }
}
```

**写 goroutine**: `conn.go:174-204`

```go
// conn.go:183
case data, ok := <-tc.sendChan:
    if !ok {
        return
    }

    // 设置写超时
    if tc.sendTimeout > 0 {
        tc.conn.SetWriteDeadline(time.Now().Add(tc.sendTimeout))
    }

    // 写入帧 (长度头 + 数据)
    if err := WriteFrame(tc.conn, data); err != nil {
        tc.Close()
        return
    }
```

**帧编码**: `frame.go:30-49`

```go
// frame.go:30
func EncodeFrame(data []byte) ([]byte, error) {
    dataLen := uint32(len(data))

    // 分配缓冲区: 4字节头 + 数据
    frame := make([]byte, 4 + dataLen)

    // 写入长度头 (大端序)
    binary.BigEndian.PutUint32(frame[:4], dataLen)

    // 复制数据
    copy(frame[4:], data)

    return frame, nil
}
```

---

### 层次 13: 通过 TCP 发送回客户端

```
响应被编码为帧，通过 TCP 发送回 Bot A
```

═══════════════════════════════════════════════════════════════

## 📊 数据格式转换总结表

| 层次 | 输入格式 | 操作 | 输出格式 | 代码位置 |
|------|---------|------|---------|----------|
| 1 | TCP 流 | Accept() | net.Conn | server.go:125 |
| 2 | net.Conn | NewTCPConn() | TCPConn | server.go:159 |
| 3 | TCP 流 | ReadFrame() | []byte (帧数据) | conn.go:146 |
| 3-续 | []byte | json.Unmarshal() | RPCMessage | conn.go:160 |
| 4 | RPCMessage | handlers[action] | handler 函数 | server.go:212 |
| 5 | map[string]interface{} | parsePayload() | LLMForwardPayload | llm_forward_handler.go:133 |
| 5-续 | LLMForwardPayload | 构造 InboundMessage | bus.InboundMessage | llm_forward_handler.go:72 |
| 6 | InboundMessage | Input() | + CorrelationID | rpc_channel.go:189-190 |
| 6-续 | InboundMessage | PublishInbound() | → MessageBus | rpc_channel.go:213 |
| 7 | InboundMessage | context.WithValue() | ctx (含 correlation_id) | loop.go:509 |
| 8 | string (LLM 回复) | 添加前缀 | "[rpc:xxx] content" | message.go:97 |
| 9 | OutboundMessage | extractCorrelationID() | correlationID | rpc_channel.go:247 |
| 9-续 | OutboundMessage | removeCorrelationID() | 实际内容 | rpc_channel.go:261 |
| 10 | string | successResponse() | map[string]interface{} | llm_forward_handler.go:148 |
| 11 | map[string]interface{} | NewResponse() | RPCMessage | server.go:238 |
| 12 | RPCMessage | msg.Bytes() | []byte (JSON) | conn.go:236 |
| 12-续 | []byte | EncodeFrame() | []byte (帧) | frame.go:31 |
| 13 | []byte (帧) | conn.Write() | TCP 流 | conn.go:194 |

═══════════════════════════════════════════════════════════════

## 🎯 关键点总结

### 1. JSON 解析发生在第 3 层
- **位置**: `conn.go:160`
- **代码**: `json.Unmarshal(data, &msg)`
- **转换**: `[]byte` → `RPCMessage`

### 2. Handler 查找发生在第 4 层
- **位置**: `server.go:212`
- **代码**: `handler, exists := s.handlers[req.Action]`
- **转换**: 根据 `action` 字段查找对应的处理函数

### 3. Handler 调用发生在第 5 层
- **位置**: `server.go:229`
- **代码**: `result, err := handler(req.Payload)`
- **转换**: `RPCMessage.Payload` → `map[string]interface{}` → `LLMForwardPayload`

### 4. CorrelationID 生成发生在第 6 层
- **位置**: `rpc_channel.go:190`
- **代码**: `inbound.CorrelationID = generateCorrelationID()`
- **用途**: 匹配 RPC 请求和响应

### 5. 进入 LLM 处理在第 7 层
- **位置**: `loop.go:512`
- **代码**: `al.runAgentLoop(ctx, agent, {...})`
- **传递**: CorrelationID 通过 context 传递

### 6. CorrelationID 前缀添加在第 8 层
- **位置**: `message.go:97`
- **代码**: `fmt.Sprintf("[rpc:%s] %s", correlationID, content)`
- **格式**: `[rpc:rpc-xxx] 实际响应`

### 7. 响应匹配发生在第 9 层
- **位置**: `rpc_channel.go:257`
- **代码**: `req, exists := ch.pendingReqs[correlationID]`
- **匹配**: 通过 CorrelationID 找到对应的请求

═══════════════════════════════════════════════════════════════
