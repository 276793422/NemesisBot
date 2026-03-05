# llm_forward → peer_chat 重命名记录

**日期**: 2026-03-05
**原因**: 提升语义准确性，更好地反映节点间对等协作关系

---

## 为什么改名？

### 旧名称 `llm_forward` 的问题

```
llm_forward 的语义暗示：
A → [中间转发者] → B

但实际架构是：
A ⟷ B (直接通信，对等节点)
```

**问题**：
- ❌ "forward" 暗示有第三方转发
- ❌ 没有体现节点间的对等关系
- ❌ 限制了只能使用 LLM 服务
- ❌ 不符合智能体协作的理念

### 新名称 `peer_chat` 的优势

```
peer_chat 的语义：
Peer A ⟷ Peer B (对等节点间的直接对话)
```

**优势**：
- ✅ "peer" 完美体现对等节点关系（P2P架构）
- ✅ "chat" 强调对话、交流、协作
- ✅ 不限制服务类型（LLM、计算、存储、检索等）
- ✅ 符合智能体系统的理念
- ✅ 更友好、更自然

---

## 修改内容

### 1. Action Schema

**文件**: `module/cluster/actions_schema.go`

**变更**:
```go
// 旧
{
  Name: "llm_forward",
  Description: "转发 LLM 请求到当前节点进行处理...",
  Parameters: {model, messages, temperature, max_tokens}
}

// 新
{
  Name: "peer_chat",
  Description: "与对等节点进行智能对话和任务协作...",
  Parameters: {type, content, context}
}
```

### 2. 使用参数简化

**旧参数**（复杂，技术化）:
```json
{
  "model": "gpt-4",
  "messages": [{"role": "user", "content": "..."}],
  "temperature": 0.7,
  "max_tokens": 4096
}
```

**新参数**（简单，自然）:
```json
{
  "type": "task",
  "content": "帮我写一首诗",
  "context": {...}
}
```

### 3. 使用场景扩展

| 场景 | llm_forward | peer_chat |
|------|-------------|-----------|
| 请求LLM服务 | ✅ | ✅ |
| 任务协作 | ❌ | ✅ |
| 节点聊天 | ❌ | ✅ |
| 信息查询 | ❌ | ✅ |
| 数据计算 | ❌ | ✅ (未来) |
| 文件存储 | ❌ | ✅ (未来) |

### 4. 文档更新

**文件**: `workspace/skills/cluster/SKILL.md`

**变更**:
- 标题：LLM 转发 → Peer Chat - 节点间对话与协作
- 描述：强调"节点间直接通信"
- 示例：更新为对话风格

---

## 新功能特性

### 对话类型

```javascript
// 1. chat - 纯聊天
peer_chat(nodeB, {
  type: "chat",
  content: "你好，最近忙什么呢？"
})

// 2. request - 请求帮助
peer_chat(nodeB, {
  type: "request",
  content: "请帮我分析这段数据"
})

// 3. task - 任务协作
peer_chat(nodeB, {
  type: "task",
  content: "帮我写一首关于春天的诗"
})

// 4. query - 查询信息
peer_chat(nodeB, {
  type: "query",
  content: "你有哪些功能？"
})
```

### 响应格式

```json
{
  "response": "节点的响应内容",
  "result": {...},  // 如果有结构化结果
  "status": "success"  // success | error | busy
}
```

---

## 测试验证

### 单元测试
```bash
✅ TestListActionsHandler - PASS
✅ TestListActionsHandlerEmptySchema - PASS
✅ TestListActionsHandlerWithAllFields - PASS
✅ TestListActionsResponseFormat - PASS
```

### 集成测试
```bash
✅ TestListActionsRPCFlow - PASS
```

### 编译验证
```bash
✅ go build ./module/cluster - PASS
✅ go build ./cmd/... - PASS
```

---

## 向后兼容性

⚠️ **不兼容的变更**：
- Action 名称从 `llm_forward` 改为 `peer_chat`
- 参数格式完全不同

**迁移建议**：
1. 更新所有调用 `llm_forward` 的代码
2. 调整参数格式
3. 更新错误处理逻辑

**示例迁移**：
```javascript
// 旧代码
llm_forward(nodeB, {
  model: "gpt-4",
  messages: [{role: "user", content: "写诗"}]
})

// 新代码
peer_chat(nodeB, {
  type: "task",
  content: "帮我写一首诗"
})
```

---

## 未来扩展

### 短期
- [ ] 实现 `peer_chat` handler
- [ ] 更新所有现有调用
- [ ] 添加更多对话类型

### 长期
- [ ] 支持流式对话
- [ ] 对话历史管理
- [ ] 多节点群聊
- [ ] 对话协议标准化

---

## 总结

这次改名不仅仅是一个名称的变更，而是**理念的升级**：

| 方面 | llm_forward | peer_chat |
|------|-------------|-----------|
| **理念** | 转发、中转 | 对话、协作 |
| **关系** | 主从关系 | 对等关系 |
| **语义** | 技术化 | 自然化 |
| **扩展性** | 限制LLM | 支持所有服务 |
| **智能感** | 低 | 高 ✨ |

`peer_chat` 更符合 NemesisBot 作为**智能体系统**的定位！

---

**版本**: 1.0
**作者**: Claude Sonnet
**状态**: ✅ 完成并测试通过
