# TestAIServer 已知问题清单

## 文档说明

本文档记录 TestAIServer 的所有已知问题、限制和待实现功能。
**重要**: 在使用或测试 TestAIServer 前，请务必阅读此文档！

---

## 🔴 高优先级问题

### ISSUE-001: 不支持流式响应（Streaming Response）

**状态**: 🟡 临时解决（兼容模式）
**发现日期**: 2026-03-11
**影响范围**: 高 - 影响所有默认使用 stream=true 的客户端
**优先级**: P1 - 高

#### 问题描述

TestAIServer 当前不支持 OpenAI API 的流式响应（Server-Sent Events, SSE）。
许多 OpenAI 兼容的客户端工具默认使用 `stream=true`，例如：
- Cherry Studio
- OpenAI官方SDK（默认）
- 其他第三方客户端

#### 当前行为

**原始实现**（v1.0-v1.2）:
```json
// 请求 stream=true 时返回错误
{
  "error": {
    "message": "Streaming is not supported by test models",
    "type": "invalid_request_error",
    "code": "streaming_not_supported"
  }
}
```

**临时方案**（v1.3+）:
- 当客户端请求 `stream=true` 时，仍然返回非流式响应
- 在服务器日志中记录警告
- 客户端可以正常工作，但无法获得流式体验

#### 技术细节

**文件位置**: `handlers/handlers.go`

**代码片段**:
```go
// ⚠️ KNOWN ISSUE: 流式响应兼容性处理
if req.Stream {
    // 记录警告：客户端请求了流式响应，但我们返回非流式响应
    fmt.Printf("[WARNING] Client requested streaming (stream=true) but returning non-streaming response. Model: %s\n", req.Model)
}
// 继续返回非流式响应
```

#### 影响的客户端

✅ **可以使用的客户端**（临时方案后）:
- Cherry Studio
- OpenAI SDK（当 stream=true 时）
- Cursor
- 其他使用 stream=true 的工具

⚠️ **限制**:
- 无法获得逐字输出的体验
- 长响应需要等待完全生成
- 某些客户端可能检测到响应格式不一致

#### 正确实现方案（待开发）

OpenAI 流式响应格式：
```
data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"testai-1.1","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"testai-1.1","choices":[{"index":0,"delta":{"content":"好"},"finish_reason":null}]}

data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"testai-1.1","choices":[{"index":0,"delta":{"content":"的"},"finish_reason":null}]}

data: [DONE]
```

需要实现：
1. SSE（Server-Sent Events）支持
2. 响应分块传输
3. 正确的 HTTP headers（`Content-Type: text/event-stream`）
4. 流式 JSON 格式（`chat.completion.chunk`）

#### 解决方案优先级

**短期**（已完成）:
- [x] 兼容模式：忽略 stream 参数，返回非流式响应
- [x] 添加警告日志
- [x] 更新文档

**中期**（计划）:
- [ ] 实现基本的流式响应支持
- [ ] 支持 testai-1.1 和 testai-2.0 的流式输出
- [ ] 添加流式响应测试用例

**长期**（可选）:
- [ ] 优化流式响应性能
- [ ] 支持取消流式传输
- [ ] 添加流式响应的超时处理

#### 测试用例

**测试 1: Cherry Studio 测试**
```bash
# 使用 Cherry Studio 连接 testai-1.1
# 预期：可以正常对话，但不是逐字输出
```

**测试 2: curl 测试**
```bash
# 非流式（正常）
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"testai-1.1","messages":[{"role":"user","content":"测试"}],"stream":false}'

# 流式（兼容模式）
curl http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"testai-1.1","messages":[{"role":"user","content":"测试"}],"stream":true}'
# 预期：返回非流式响应，但可以正常工作
```

#### 相关文件

- `handlers/handlers.go` - 主要实现
- `models/test_models.go` - 模型实现
- `test/streaming_test.go` - 测试用例（待创建）
- `docs/KNOWN_ISSUES.md` - 本文档

#### 参考资料

- [OpenAI Streaming API](https://platform.openai.com/docs/api-reference/streaming)
- [Server-Sent Events](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events)

---

## 🟡 中优先级问题

### ISSUE-002: Token 计数不准确

**状态**: 🔴 未解决
**发现日期**: 2026-03-11
**影响范围**: 低 - 仅影响 usage 统计
**优先级**: P3 - 低

#### 问题描述

当前使用简单的字符计数来估算 token，不是真实的 tokenizer 实现。

#### 当前实现

```go
func (h *Handler) countTokens(messages []models.Message) int {
    count := 0
    for _, msg := range messages {
        count += len(msg.Content)  // 按字符计数，不准确
    }
    return count
}
```

#### 影响

- ❌ Token 统计不准确
- ❌ 与真实 OpenAI API 的 token 计数不一致
- ✅ 不影响功能，仅影响统计

#### 解决方案

需要实现真实的 tokenizer：
- 使用 tiktoken 库
- 或调用 OpenAI 的 tokenizer API

---

### ISSUE-003: 日志文件无自动清理

**状态**: 🔴 未解决
**发现日期**: 2026-03-11
**影响范围**: 中 - 长期运行会占用磁盘空间
**优先级**: P2 - 中

#### 问题描述

日志文件会持续累积，没有自动清理机制。

#### 影响

- 长期运行会占用大量磁盘空间
- 需要手动清理

#### 解决方案

- 实现日志轮转
- 自动删除超过 N 天的日志
- 限制日志目录总大小

---

## 🟢 低优先级问题

### ISSUE-004: 不支持 API Key 验证

**状态**: 🔵 设计如此
**影响范围**: 低 - 仅测试用途
**优先级**: P4 - 极低

#### 说明

TestAIServer 是测试服务器，不验证 API Key。这是设计决定，不是问题。

---

## 📊 问题统计

| 优先级 | 数量 | 状态 |
|--------|------|------|
| P1 - 高 | 1 | 🟡 临时解决 |
| P2 - 中 | 1 | 🔴 未解决 |
| P3 - 低 | 1 | 🔴 未解决 |
| P4 - 极低 | 1 | 🔵 设计如此 |

---

## 🔍 如何报告新问题

如果你发现新问题，请按以下格式记录：

```markdown
### ISSUE-XXX: 问题标题

**状态**: 🔴/🟡/🟢/🔵
**发现日期**: YYYY-MM-DD
**影响范围**: 高/中/低
**优先级**: P1/P2/P3/P4

#### 问题描述
[详细描述]

#### 当前行为
[当前如何表现]

#### 预期行为
[应该如何表现]

#### 解决方案
[如何修复]
```

---

## 📝 更新日志

| 日期 | 问题 | 操作 |
|------|------|------|
| 2026-03-11 | ISSUE-001 | 发现问题，实施临时方案 |
| 2026-03-11 | ISSUE-002 | 记录已知限制 |
| 2026-03-11 | ISSUE-003 | 记录已知限制 |
| 2026-03-11 | - | 创建已知问题清单 |

---

**最后更新**: 2026-03-11
**维护者**: Claude Code
**用途**: 问题跟踪和质量保证

---

## ⚠️ 重要提示

**在使用 TestAIServer 前，请务必**:
1. ✅ 阅读本已知问题清单
2. ✅ 确认你的客户端是否受影响
3. ✅ 检查是否有临时方案可用
4. ✅ 了解限制和影响

**如需真正的流式响应支持**，请等待 ISSUE-001 的完整实现。
