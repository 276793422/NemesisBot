# peer_chat Handler 实现改进计划

**目标**: 将 `llm_forward` 完整重构为 `peer_chat`
**创建日期**: 2026-03-05
**预计工作量**: 2-3 小时
**风险级别**: 中等

---

## 📊 问题分析

### 当前状态

| 组件 | 当前名称 | 状态 | 问题 |
|------|---------|------|------|
| Action Schema | `peer_chat` | ✅ 已更新 | 无问题 |
| Cluster Skill | `peer_chat` | ✅ 已更新 | 无问题 |
| Handler 文件 | `llm_forward_handler.go` | ❌ 旧名称 | 需要重命名 |
| Handler 结构体 | `LLMForwardHandler` | ❌ 旧名称 | 需要重命名 |
| 注册代码 | `llm_forward` | ❌ 旧名称 | 需要更新 |
| Payload 类型 | `LLMForwardPayload` | ❌ 旧名称 | 需要重命名 |
| 测试代码 | `llm_forward` | ❌ 旧名称 | 需要更新 |

### 功能差距

**旧实现 (llm_forward)**:
```go
type LLMForwardPayload struct {
    Channel    string            `json:"channel"`
    ChatID     string            `json:"chat_id"`
    Content    string            `json:"content"`
    SenderID   string            `json:"sender_id"`
    SessionKey string            `json:"session_key"`
    Metadata   map[string]string `json:"metadata"`
}
```

**新期望 (peer_chat)**:
```go
type PeerChatPayload struct {
    Type    string                 `json:"type"`     // chat|request|task|query
    Content string                 `json:"content"`   // 对话内容
    Context map[string]interface{} `json:"context"`   // 附加上下文
}
```

**问题**: 新旧参数格式不兼容！

---

## 🎯 改进策略

### 策略选择

#### 选项A: 完全重构（推荐）✅
- 直接使用新的 peer_chat 参数格式
- 修改 handler 内部逻辑以适配新参数
- **优点**: 语义清晰，符合设计理念
- **缺点**: 不兼容，需要大量修改

#### 选项B: 兼容性适配
- 同时支持新旧两种参数格式
- 内部进行格式转换
- **优点**: 向后兼容
- **缺点**: 代码复杂，维护困难

#### 选项C: 渐进式迁移
- 先改名称，保持参数不变
- 后续版本逐步迁移参数
- **优点**: 风险小
- **缺点**: 违背设计初衷

**推荐**: **选项A（完全重构）**
- 因为这是系统内部的 RPC 接口
- 外部调用方可以直接使用新格式
- 没有历史包袱，可以做得更好

---

## 📝 详细改进步骤

### 阶段1: Handler 文件重构（核心）

#### 步骤 1.1: 创建新的 handler 文件

**文件**: `module/cluster/rpc/peer_chat_handler.go`（新建）

**内容结构**:
```go
package rpc

import (
    "context"
    "encoding/json"
    "fmt"
    "time"

    "github.com/276793422/NemesisBot/module/bus"
    "github.com/276793422/NemesisBot/module/channels"
)

// PeerChatPayload represents the payload for peer_chat action
type PeerChatPayload struct {
    Type    string                 `json:"type"`     // chat|request|task|query
    Content string                 `json:"content"`   // 对话内容或任务描述
    Context map[string]interface{} `json:"context"`   // 附加上下文信息
}

// PeerChatResponse represents the response from peer_chat action
type PeerChatResponse struct {
    Response string                 `json:"response"` // 节点的响应内容
    Result   map[string]interface{} `json:"result,omitempty"` // 结构化结果
    Status   string                 `json:"status"`   // success|error|busy
}

// PeerChatHandler handles peer-to-peer chat and collaboration requests
type PeerChatHandler struct {
    cluster    Cluster
    rpcChannel *channels.RPCChannel
}

// NewPeerChatHandler creates a new peer chat handler
func NewPeerChatHandler(cluster Cluster, rpcChannel *channels.RPCChannel) *PeerChatHandler {
    return &PeerChatHandler{
        cluster:    cluster,
        rpcChannel: rpcChannel,
    }
}

// Handle handles a peer chat request
func (h *PeerChatHandler) Handle(payload map[string]interface{}) (map[string]interface{}, error) {
    h.cluster.LogRPCInfo("[PeerChat] Received request: type=%s", payload["type"])

    // 1. Parse payload
    var req PeerChatPayload
    if err := h.parsePayload(payload, &req); err != nil {
        h.cluster.LogRPCError("[PeerChat] Invalid payload: %v", err)
        return h.errorResponse("error", "invalid payload: "+err.Error()), nil
    }

    // 2. Validate
    if req.Content == "" {
        return h.errorResponse("error", "content is required"), nil
    }

    // 3. Route based on type
    switch req.Type {
    case "chat", "request", "task", "query":
        // These types all need LLM processing
        return h.handleLLMRequest(&req)
    default:
        // Default to task type
        req.Type = "task"
        return h.handleLLMRequest(&req)
    }
}

// handleLLMRequest processes LLM-based chat requests
func (h *PeerChatHandler) handleLLMRequest(req *PeerChatPayload) (map[string]interface{}, error) {
    // Extract chat_id and session_key from context
    chatID := "default"
    sessionKey := "default-session"

    if req.Context != nil {
        if v, ok := req.Context["chat_id"].(string); ok {
            chatID = v
        }
        if v, ok := req.Context["session_key"].(string); ok {
            sessionKey = v
        }
    }

    // Construct InboundMessage
    inbound := &bus.InboundMessage{
        Channel:    "rpc",
        ChatID:     chatID,
        Content:    req.Content,
        SessionKey: sessionKey,
    }

    // Set correlation ID for tracking
    correlationID := fmt.Sprintf("peer-chat-%d", time.Now().UnixNano())
    inbound.CorrelationID = correlationID

    // Send to RPCChannel
    ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
    defer cancel()

    respCh, err := h.rpcChannel.Input(ctx, inbound)
    if err != nil {
        h.cluster.LogRPCError("[PeerChat] Failed to process: %v", err)
        return h.errorResponse("error", "failed to process: "+err.Error()), nil
    }

    // Wait for response
    select {
    case response := <-respCh:
        return h.successResponse(response, nil), nil

    case <-ctx.Done():
        h.cluster.LogRPCError("[PeerChat] Timeout", nil)
        return h.errorResponse("error", "timeout"), nil
    }
}

// Helper methods
func (h *PeerChatHandler) parsePayload(payload map[string]interface{}, req *PeerChatPayload) error {
    // Parse Type
    if v, ok := payload["type"].(string); ok {
        req.Type = v
    } else {
        req.Type = "request" // Default
    }

    // Parse Content
    if v, ok := payload["content"].(string); ok {
        req.Content = v
    }

    // Parse Context
    if v, ok := payload["context"].(map[string]interface{}); ok {
        req.Context = v
    }

    return nil
}

func (h *PeerChatHandler) successResponse(content string, result map[string]interface{}) map[string]interface{} {
    response := map[string]interface{}{
        "status":   "success",
        "response": content,
    }
    if result != nil {
        response["result"] = result
    }
    return response
}

func (h *PeerChatHandler) errorResponse(status, errMsg string) map[string]interface{} {
    return map[string]interface{}{
        "status":   status,
        "response": errMsg,
    }
}
```

#### 步骤 1.2: 保留旧文件作为备份

**操作**:
- 暂时保留 `llm_forward_handler.go`
- 添加注释标记为 deprecated

---

### 阶段2: 注册代码更新

#### 步骤 2.1: 更新 handlers/llm.go

**文件**: `module/cluster/handlers/llm.go`

**修改前**:
```go
func RegisterLLMHandlers(...) {
    llmForwardHandler := handlerFactory(rpcChannel)
    registrar("llm_forward", llmForwardHandler)
    logger.LogRPCInfo("Registered LLM handlers: llm_forward")
}
```

**修改后**:
```go
func RegisterLLMHandlers(...) {
    peerChatHandler := handlerFactory(rpcChannel)
    registrar("peer_chat", peerChatHandler)
    logger.LogRPCInfo("Registered peer chat handler: peer_chat")
}
```

**同时需要更新**:
- 函数名：`RegisterLLMHandlers` → `RegisterPeerChatHandlers`
- 日志信息

#### 步骤 2.2: 更新 cluster.go 中的调用

**文件**: `module/cluster/cluster.go`

**修改前**:
```go
// Line 687-693
handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
    handler := rpc.NewLLMForwardHandler(c, rpcChannel)
    return handler.Handle
}

handlers.RegisterLLMHandlers(c.logger, c.rpcChannel, handlerFactory, registrar)
```

**修改后**:
```go
handlerFactory := func(rpcChannel *channels.RPCChannel) func(map[string]interface{}) (map[string]interface{}, error) {
    handler := rpc.NewPeerChatHandler(c, rpcChannel)
    return handler.Handle
}

handlers.RegisterPeerChatHandlers(c.logger, c.rpcChannel, handlerFactory, registrar)
```

---

### 阶段3: 参数兼容性处理

#### 步骤 3.1: 支持从 context 中提取旧参数

为了平滑过渡，handler 可以同时支持新旧格式：

```go
func (h *PeerChatHandler) parsePayload(payload map[string]interface{}, req *PeerChatPayload) error {
    // Parse Type
    req.Type = "request" // Default
    if v, ok := payload["type"].(string); ok {
        req.Type = v
    }

    // Parse Content
    req.Content = ""
    if v, ok := payload["content"].(string); ok {
        req.Content = v
    }

    // Parse Context
    req.Context = make(map[string]interface{})
    if v, ok := payload["context"].(map[string]interface{}); ok {
        req.Context = v
    }

    // 兼容旧格式：从直接字段提取
    if req.Context != nil {
        // 如果没有 chat_id，尝试从根级别获取
        if _, hasChatID := req.Context["chat_id"]; !hasChatID {
            if v, ok := payload["chat_id"].(string); ok {
                req.Context["chat_id"] = v
            }
        }
    } else {
        // 完全没有 context，从根级别构建
        if chatID, ok := payload["chat_id"].(string); ok {
            req.Context = map[string]interface{}{
                "chat_id": chatID,
            }
        }
    }

    return nil
}
```

#### 步骤 3.2: 更新文档说明参数迁移

在文档中添加迁移指南。

---

### 阶段4: 测试更新

#### 步骤 4.1: 更新单元测试

**文件**: `test/unit/cluster/rpc/llm_forward_handler_test.go`

**操作**:
1. 重命名为 `peer_chat_handler_test.go`
2. 更新所有测试用例
3. 添加新参数格式的测试

**新测试用例**:
```go
func TestPeerChatHandler_TaskType(t *testing.T)
func TestPeerChatHandler_ChatType(t *testing.T)
func TestPeerChatHandler_WithNewFormat(t *testing.T)
func TestPeerChatHandler_WithOldFormat(t *testing.T) // 兼容性测试
func TestPeerChatHandler_MissingContent(t *testing.T)
```

#### 步骤 4.2: 更新集成测试

**文件**: `test/integration/rpc/rpc_llm_integration_test.go`

**修改**:
- 更新 mock 数据
- 更新测试调用

---

### 阶段5: 文档更新

#### 步骤 5.1: 更新技术文档

创建迁移文档：`docs/PEER_CHAT_MIGRATION.md`

内容包括：
- 参数格式对比
- 迁移步骤
- 兼容性说明
- 常见问题

#### 步骤 5.2: 更新 API 文档

更新所有提到 `llm_forward` 的文档。

---

### 阶段6: 清理工作

#### 步骤 6.1: 删除旧文件（在确认稳定后）

**待删除**:
- `module/cluster/rpc/llm_forward_handler.go` → 删除
- 相关测试文件 → 删除

#### 步骤 6.2: 更新 gitignore

确保没有遗留的临时文件。

---

## ⚠️ 风险评估

### 高风险区域

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| 参数格式不兼容 | 现有调用失败 | 1. 暂时支持双格式<br>2. 提前通知用户<br>3. 详细的迁移文档 |
| Handler 注册失败 | RPC 功能不可用 | 1. 充分测试<br>2. 保留旧文件作为备份<br>3. 可以快速回滚 |
| 测试覆盖不足 | 边缘情况出错 | 1. 编写全面的测试<br>2. 手动测试各种场景<br>3. 添加集成测试 |

### 回滚计划

如果出现问题，可以立即回滚：
1. 恢复 `llm_forward_handler.go`
2. 恢复 `handlers/llm.go` 中的注册代码
3. 恢复 `cluster.go` 中的调用

**预计回滚时间**: 5-10 分钟

---

## 📋 检查清单

### 修改前检查

- [ ] 确认当前 `llm_forward` 的所有调用位置
- [ ] 备份相关代码
- [ ] 通知团队成员（如果有的话）

### 修改中检查

- [ ] 每个文件修改后立即编译测试
- [ ] 逐步更新，不一次性修改所有文件
- [ ] 保留旧文件作为参考

### 修改后验证

- [ ] 单元测试全部通过
- [ ] 集成测试通过
- [ ] 手动测试各种场景
- [ ] 检查日志输出是否正确
- [ ] 确认 action schema 正确

---

## 🎯 成功标准

### 功能验收

- [ ] `peer_chat` action 可以正常调用
- [ ] 支持 4 种对话类型 (chat/request/task/query)
- [ ] 返回格式符合新的 schema
- [ ] 日志清晰，易于调试

### 兼容性验收

- [ ] 旧格式参数仍能工作（过渡期）
- [ ] 新格式参数完全符合设计
- [ ] 错误处理友好

### 质量验收

- [ ] 所有测试通过
- [ ] 代码无警告
- [ ] 文档完整
- [ ] 性能无明显下降

---

## ⏱️ 时间估算

| 阶段 | 预计时间 | 备注 |
|------|---------|------|
| 阶段1: Handler 文件重构 | 60 分钟 | 核心工作 |
| 阶段2: 注册代码更新 | 20 分钟 | 相对简单 |
| 阶段3: 参数兼容性处理 | 30 分钟 | 需要仔细考虑 |
| 阶段4: 测试更新 | 40 分钟 | 保证质量 |
| 阶段5: 文档更新 | 20 分钟 | 重要的是清晰 |
| 阶段6: 清理工作 | 10 分钟 | 最后收尾 |
| **总计** | **约 3 小时** | 包含测试和验证 |

---

## 🚀 执行建议

### 推荐执行顺序

1. **先创建新文件，不删除旧文件**（安全）
2. **逐步更新，每次修改后立即测试**（降低风险）
3. **先更新单元测试，再更新集成测试**（快速反馈）
4. **最后才删除旧文件**（保留回滚能力）

### 不推荐的执行方式

- ❌ 一次性修改所有文件（难以定位问题）
- ❌ 先删除旧文件再创建新文件（失去回滚能力）
- ❌ 跳过测试直接上生产（危险）

---

## 📞 需要确认的问题

在开始执行前，请确认：

1. **参数格式**: 确认使用新的简化参数格式吗？
   ```json
   {"type": "task", "content": "...", "context": {...}}
   ```

2. **兼容性**: 是否需要支持旧的 `llm_forward` 参数格式？
   - 如果需要 → 增加兼容层（+30分钟）
   - 如果不需要 → 完全重构（更快）

3. **测试范围**: 是否需要手动测试实际节点间通信？
   - 需要 → 预留额外测试时间

4. **文档**: 是否需要用户迁移指南？

---

## ✅ 准备就绪

一旦你确认以上计划，我就可以开始执行！

**你可以告诉我**：
- "开始执行，按计划来" → 我开始逐步实施
- "先做阶段1和2" → 分步执行
- "我有点担心XXX，能调整吗？" → 我们可以修改计划
- "我想先看看具体代码再决定" → 我可以展示关键代码片段

准备好了就告诉我！🚀
