package models

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

// TestAI11 - 立即返回固定响应
type TestAI11 struct{}

func NewTestAI11() *TestAI11 {
	return &TestAI11{}
}

func (m *TestAI11) Name() string {
	return "testai-1.1"
}

func (m *TestAI11) Process(messages []Message) string {
	return "好的，我知道了"
}

func (m *TestAI11) Delay() time.Duration {
	return 0
}

// TestAI12 - 延迟 30 秒后返回固定响应
type TestAI12 struct{}

func NewTestAI12() *TestAI12 {
	return &TestAI12{}
}

func (m *TestAI12) Name() string {
	return "testai-1.2"
}

func (m *TestAI12) Process(messages []Message) string {
	return "好的，我知道了"
}

func (m *TestAI12) Delay() time.Duration {
	return 30 * time.Second
}

// TestAI13 - 延迟 300 秒后返回固定响应
type TestAI13 struct{}

func NewTestAI13() *TestAI13 {
	return &TestAI13{}
}

func (m *TestAI13) Name() string {
	return "testai-1.3"
}

func (m *TestAI13) Process(messages []Message) string {
	return "好的，我知道了"
}

func (m *TestAI13) Delay() time.Duration {
	return 300 * time.Second
}

// TestAI20 - 原样返回用户消息
type TestAI20 struct{}

func NewTestAI20() *TestAI20 {
	return &TestAI20{}
}

func (m *TestAI20) Name() string {
	return "testai-2.0"
}

func (m *TestAI20) Process(messages []Message) string {
	// 返回最后一条用户消息
	if len(messages) > 0 {
		return messages[len(messages)-1].Content
	}
	return ""
}

func (m *TestAI20) Delay() time.Duration {
	return 0
}

// TestAI30 - 返回工具调用以触发 peer_chat
// 功能：
// 1. 检测消息中的 <PEER_CHAT>{"peer_id":"xxx","content":"yyy"}</PEER_CHAT> 标记
// 2. 如果检测到，返回 cluster_rpc 工具调用（JSON 格式）
// 3. 否则，返回用户消息
type TestAI30 struct{}

func NewTestAI30() *TestAI30 {
	return &TestAI30{}
}

func (m *TestAI30) Name() string {
	return "testai-3.0"
}

func (m *TestAI30) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}

	lastMsg := messages[len(messages)-1].Content

	// 检查是否包含 <PEER_CHAT> 标记
	if strings.Contains(lastMsg, "<PEER_CHAT>") && strings.Contains(lastMsg, "</PEER_CHAT>") {
		// 提取标记内的 JSON
		start := strings.Index(lastMsg, "<PEER_CHAT>") + len("<PEER_CHAT>")
		end := strings.Index(lastMsg, "</PEER_CHAT>")

		if end > start {
			jsonStr := strings.TrimSpace(lastMsg[start:end])

			// 解析 JSON
			var req struct {
				PeerID  string `json:"peer_id"`
				Content string `json:"content"`
			}

			if err := json.Unmarshal([]byte(jsonStr), &req); err == nil {
				// 构造工具调用响应
				toolCallID := fmt.Sprintf("call-%d", time.Now().UnixNano())

				// 构造 cluster_rpc 的参数
				args := map[string]interface{}{
					"peer_id": req.PeerID,
					"action":  "peer_chat",
					"data": map[string]interface{}{
						"type":    "chat",
						"content": req.Content,
					},
				}

				argsJSON, _ := json.Marshal(args)

				// 返回工具调用 JSON
				response := ProcessedResponse{
					Content: "",
					ToolCalls: []ToolCall{
						{
							ID:   toolCallID,
							Type: "function",
							Function: &FunctionCall{
								Name:      "cluster_rpc",
								Arguments: string(argsJSON),
							},
						},
					},
				}

				responseJSON, _ := json.Marshal(response)
				return string(responseJSON)
			}
		}
	}

	// 普通消息，直接返回
	return lastMsg
}

func (m *TestAI30) Delay() time.Duration {
	return 0
}

// TestAI42 - 调用客户端 sleep 工具，休眠 30 秒
// 功能：
// 1. 第一轮：返回 sleep 工具调用（30 秒）
// 2. 第二轮：返回 "工作完成"
type TestAI42 struct{}

func NewTestAI42() *TestAI42 {
	return &TestAI42{}
}

func (m *TestAI42) Name() string {
	return "testai-4.2"
}

func (m *TestAI42) Process(messages []Message) string {
	// 检查是否是第二轮：最后一条消息是 tool 消息
	if len(messages) > 0 {
		lastMsg := messages[len(messages)-1]
		if lastMsg.Role == "tool" {
			// 第二轮：返回最终响应
			return "工作完成"
		}
	}

	// 第一轮：返回 sleep 工具调用
	toolCallID := fmt.Sprintf("call-%d", time.Now().UnixNano())

	// 构造 sleep 工具调用的参数
	args := map[string]interface{}{
		"duration": 30, // 30 秒
	}
	argsJSON, _ := json.Marshal(args)

	// 返回工具调用 JSON
	response := ProcessedResponse{
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   toolCallID,
				Type: "function",
				Function: &FunctionCall{
					Name:      "sleep",
					Arguments: string(argsJSON),
				},
			},
		},
	}

	responseJSON, _ := json.Marshal(response)
	return string(responseJSON)
}

func (m *TestAI42) Delay() time.Duration {
	return 0
}

// TestAI43 - 调用客户端 sleep 工具，休眠 300 秒
// 功能：
// 1. 第一轮：返回 sleep 工具调用（300 秒）
// 2. 第二轮：返回 "工作完成"
type TestAI43 struct{}

func NewTestAI43() *TestAI43 {
	return &TestAI43{}
}

func (m *TestAI43) Name() string {
	return "testai-4.3"
}

func (m *TestAI43) Process(messages []Message) string {
	// 检查是否是第二轮：最后一条消息是 tool 消息
	if len(messages) > 0 {
		lastMsg := messages[len(messages)-1]
		if lastMsg.Role == "tool" {
			// 第二轮：返回最终响应
			return "工作完成"
		}
	}

	// 第一轮：返回 sleep 工具调用
	toolCallID := fmt.Sprintf("call-%d", time.Now().UnixNano())

	// 构造 sleep 工具调用的参数
	args := map[string]interface{}{
		"duration": 300, // 300 秒（5 分钟）
	}
	argsJSON, _ := json.Marshal(args)

	// 返回工具调用 JSON
	response := ProcessedResponse{
		Content: "",
		ToolCalls: []ToolCall{
			{
				ID:   toolCallID,
				Type: "function",
				Function: &FunctionCall{
					Name:      "sleep",
					Arguments: string(argsJSON),
				},
			},
		},
	}

	responseJSON, _ := json.Marshal(response)
	return string(responseJSON)
}

func (m *TestAI43) Delay() time.Duration {
	return 0
}
