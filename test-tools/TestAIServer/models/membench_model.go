package models

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// TestAIMem（testai-mem-1.0）：记忆质量基准的确定性搬运工模型（项2 P1）。
//
// 记忆存取全部在 gateway 侧真实发生（memory_store/search/forget 四工具 →
// MemoryManager），模型只做确定性翻译——无采样随机性，bench 评分差的是
// gateway 侧工程质量。
//
// 对话机（按最后一条 user 消息的 marker 分流）：
//   - "MEMSTORE: [fact1, fact2, ...]"（JSON 字符串数组）→ 逐条 memory_store
//     （episodic + bench tag，一次并发多调用）；
//   - "MEMQUERY: <query>" → memory_search {query}；
//   - "MEMFORGET: <id>" → memory_forget {action: delete_by_id, id}；
//   - 工具回执轮（最后一条消息 role=tool）→ 终局文本
//     "MEMRECEIPT: <全部回执拼接>"（echo——bench 断言消费 gateway 产出）。
//
// 审批注记：agent 侧 memory_store/forget 过 GatewayMemoryGate 人工审批
// （永不静默跳过）——bench 侧由 auto-approve 任务批复（approval.pending 轮询
// → approval.respond），这是 e2e 形态的人批，非产品放水。
type TestAIMem struct{}

func NewTestAIMem() *TestAIMem { return &TestAIMem{} }

func (m *TestAIMem) Name() string { return "testai-mem-1.0" }

func (m *TestAIMem) Delay() time.Duration { return 0 }

// buildMultiToolCalls 一次响应携带 N 个工具调用（并行批量形态）。
func buildMultiToolCalls(name string, argsList []map[string]interface{}) string {
	calls := make([]ToolCall, 0, len(argsList))
	for _, args := range argsList {
		argsJSON, _ := json.Marshal(args)
		calls = append(calls, ToolCall{
			ID:   fmt.Sprintf("call-%d-%d", time.Now().UnixNano(), len(calls)),
			Type: "function",
			Function: &FunctionCall{
				Name:      name,
				Arguments: string(argsJSON),
			},
		})
	}
	responseJSON, _ := json.Marshal(ProcessedResponse{ToolCalls: calls})
	return string(responseJSON)
}

func (m *TestAIMem) Process(messages []Message) string {
	if len(messages) == 0 {
		return "MEMRECEIPT: (no messages)"
	}

	// 工具回执轮：拼接全部 tool 消息回显（bench 从回执断言 gateway 产出）。
	last := messages[len(messages)-1]
	if last.Role == "tool" {
		var b strings.Builder
		b.WriteString("MEMRECEIPT: ")
		first := true
		for _, msg := range messages {
			if msg.Role == "tool" {
				if !first {
					b.WriteString("\n---\n")
				}
				b.WriteString(msg.Content)
				first = false
			}
		}
		return b.String()
	}

	// goal 轮：按最后一条 user 消息 marker 分流。
	if !strings.HasPrefix(last.Role, "user") {
		return "MEMRECEIPT: (waiting for user marker)"
	}
	content := last.Content

	if rest, ok := markerPayload(content, "MEMSTORE:"); ok {
		var facts []string
		if err := json.Unmarshal([]byte(rest), &facts); err != nil || len(facts) == 0 {
			return "MEMRECEIPT: MEMSTORE parse error: " + rest
		}
		argsList := make([]map[string]interface{}, 0, len(facts))
		for _, fact := range facts {
			argsList = append(argsList, map[string]interface{}{
				"memory_type": "episodic",
				"content":     fact,
				"tags":        []string{"bench"},
			})
		}
		return buildMultiToolCalls("memory_store", argsList)
	}

	if rest, ok := markerPayload(content, "MEMQUERY:"); ok {
		return buildSingleToolCall("memory_search", map[string]interface{}{
			"query": rest,
		})
	}

	if rest, ok := markerPayload(content, "MEMFORGET:"); ok {
		return buildSingleToolCall("memory_forget", map[string]interface{}{
			"action": "delete_by_id",
			"id":     rest,
		})
	}

	return "MEMRECEIPT: (no marker in user message)"
}

// markerPayload 取 "MARKER:" 之后的剩余文本（trim；不存在 = ok=false）。
func markerPayload(content, marker string) (string, bool) {
	idx := strings.Index(content, marker)
	if idx < 0 {
		return "", false
	}
	return strings.TrimSpace(content[idx+len(marker):]), true
}
