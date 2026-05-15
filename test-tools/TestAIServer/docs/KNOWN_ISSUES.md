# TestAIServer 已知问题清单

## 文档说明

本文档记录 TestAIServer 的所有已知问题、限制和待实现功能。
**重要**: 在使用或测试 TestAIServer 前，请务必阅读此文档！

---

## 🔴 高优先级问题

### ISSUE-001: 不支持流式响应（Streaming Response）

**状态**: ✅ 已解决
**发现日期**: 2026-03-11
**解决日期**: 2026-04-08
**影响范围**: 无（已修复）
**优先级**: P1 - 高

#### 问题描述

TestAIServer 之前不支持 OpenAI API 的流式响应（Server-Sent Events, SSE）。
许多 OpenAI 兼容的客户端工具默认使用 `stream=true`，例如：
- Cherry Studio
- OpenAI官方SDK（默认）
- 其他第三方客户端

#### 解决方案

**已实现完整的流式响应支持**（v2.0+）：
- ✅ 实现了 SSE（Server-Sent Events）支持
- ✅ 响应分块传输（逐字符发送）
- ✅ 正确的 HTTP headers（`Content-Type: text/event-stream`）
- ✅ 流式 JSON 格式（`chat.completion.chunk`）
- ✅ 支持工具调用的流式响应

#### 技术细节

**文件位置**: `handlers/handlers.go`

**实现函数**:
- `handleStreamingResponse()` - 普通流式响应（line 190-265）
- `handleStreamingResponseWithTools()` - 工具调用流式响应（line 280-378）
- `sendSSEChunk()` - SSE 数据块发送（line 267-278）

**支持的客户端**:
- ✅ Cherry Studio
- ✅ OpenAI SDK（stream=true）
- ✅ Cursor
- ✅ 其他使用 stream=true 的工具
- ✅ 所有 curl 测试

#### 流式响应格式

```
data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-1.1","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-1.1","choices":[{"index":0,"delta":{"content":"好"},"finish_reason":null}]}

data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1694268190,"model":"testai-1.1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}

data: [DONE]
```

#### 测试用例

**测试 1: curl 流式响应**
```bash
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"testai-2.0","messages":[{"role":"user","content":"测试"}],"stream":true}'
# 预期：逐字符输出响应
```

**测试 2: 工具调用流式响应**
```bash
curl -N http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"testai-5.0","messages":[{"role":"user","content":"<FILE_OP>{\"operation\":\"file_read\",\"path\":\"/test.txt\"}</FILE_OP>"}],"stream":true}'
# 预期：工具调用以流式返回
```

#### 相关文件

- `handlers/handlers.go` - 流式响应实现
- `models/test_models.go` - 模型实现
- `README.md` - 更新了流式响应文档
- `help_system.go` - 更新了帮助文档

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
| P1 - 高 | 1 | ✅ 已解决 |
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
| 2026-04-08 | ISSUE-001 | ✅ 完整实现流式响应支持 |

---

**最后更新**: 2026-04-08
**维护者**: Claude Code
**用途**: 问题跟踪和质量保证

---

## ⚠️ 重要提示

**在使用 TestAIServer 前，请务必**:
1. ✅ 阅读本已知问题清单
2. ✅ 确认你的客户端是否受影响
3. ✅ 检查是否有临时方案可用
4. ✅ 了解限制和影响

**流式响应支持**: ✅ 已完整实现（v2.0+）
