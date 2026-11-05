package handlers

import (
	"encoding/json"
	"net/http/httptest"
	"strings"
	"testing"

	"testaiserver/models"

	"github.com/gin-gonic/gin"
)

// parse SSE frames: "data: <json>\n\n" sequences; the final [DONE] frame is dropped.
func parseSSEChunks(t *testing.T, body string) []map[string]interface{} {
	t.Helper()
	var chunks []map[string]interface{}
	for _, frame := range strings.Split(body, "\n\n") {
		frame = strings.TrimSpace(frame)
		if !strings.HasPrefix(frame, "data: ") {
			continue
		}
		payload := strings.TrimPrefix(frame, "data: ")
		if payload == "[DONE]" {
			continue
		}
		var chunk map[string]interface{}
		if err := json.Unmarshal([]byte(payload), &chunk); err != nil {
			t.Fatalf("chunk is not valid JSON: %v\n%s", err, payload)
		}
		chunks = append(chunks, chunk)
	}
	return chunks
}

// BUG-20261105-039 回归：流式多工具调用必须逐调用分块且显式携带递增 index。
// 修复前形态（全部调用挤单块、无 index）会让按 index 分桶装配的消费端
// （nemesis-providers http_provider）全部塌进桶 0——id/name 互相覆盖、
// arguments 串联成非法 JSON。
func TestStreamingToolCallsEmittedPerCallWithIndex(t *testing.T) {
	gin.SetMode(gin.TestMode)

	calls := []models.ToolCall{
		{ID: "call-0", Type: "function", Function: &models.FunctionCall{Name: "web_fetch", Arguments: `{"url":"a"}`}},
		{ID: "call-1", Type: "function", Function: &models.FunctionCall{Name: "web_fetch", Arguments: `{"url":"b"}`}},
		{ID: "call-2", Type: "function", Function: &models.FunctionCall{Name: "web_fetch", Arguments: `{"url":"c"}`}},
	}

	w := httptest.NewRecorder()
	c, _ := gin.CreateTestContext(w)
	c.Request = httptest.NewRequest("POST", "/v1/chat/completions", nil)

	h := &Handler{}
	h.handleStreamingResponseWithTools(c, "testai-2.1", "", calls)

	var toolChunks []map[string]interface{}
	for _, chunk := range parseSSEChunks(t, w.Body.String()) {
		choices, _ := chunk["choices"].([]interface{})
		if len(choices) == 0 {
			continue
		}
		choice, _ := choices[0].(map[string]interface{})
		delta, _ := choice["delta"].(map[string]interface{})
		if tcs, ok := delta["tool_calls"].([]interface{}); ok && len(tcs) > 0 {
			if len(tcs) != 1 {
				t.Fatalf("expected exactly 1 tool call per chunk (OpenAI streaming contract), got %d: %v", len(tcs), chunk)
			}
			toolChunks = append(toolChunks, chunk)
		}
	}
	if len(toolChunks) != len(calls) {
		t.Fatalf("expected %d tool-call chunks, got %d", len(calls), len(toolChunks))
	}

	// 每个调用的 id/args 保真 + index 显式递增（index=0 必须显式出现，
	// 消费端缺 index 会静默落桶 0）
	for k, chunk := range toolChunks {
		delta := chunk["choices"].([]interface{})[0].(map[string]interface{})["delta"].(map[string]interface{})
		tc := delta["tool_calls"].([]interface{})[0].(map[string]interface{})
		if got := tc["id"].(string); got != calls[k].ID {
			t.Fatalf("chunk %d: id overwritten: got %q want %q", k, got, calls[k].ID)
		}
		if got := tc["function"].(map[string]interface{})["arguments"].(string); got != calls[k].Function.Arguments {
			t.Fatalf("chunk %d: arguments corrupted: got %q want %q", k, got, calls[k].Function.Arguments)
		}
		idx, ok := tc["index"].(float64)
		if !ok {
			t.Fatalf("chunk %d: tool call missing explicit index field: %v", k, tc)
		}
		if int(idx) != k {
			t.Fatalf("chunk %d: index = %v, want %d", k, int(idx), k)
		}
	}
}

// 非流式路径字节保真：ToolCall.Index 为 nil 时序列化必须省略 index 键
// （非流式响应报文与历史形态逐字节一致）；流式下显式 index=0 也必须出现。
func TestToolCallIndexWireFidelity(t *testing.T) {
	data, err := json.Marshal(models.ToolCall{
		ID:       "call-1",
		Type:     "function",
		Function: &models.FunctionCall{Name: "sleep", Arguments: `{"duration":30}`},
	})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(data), `"index"`) {
		t.Fatalf("nil index must be omitted (non-streaming wire unchanged), got %s", data)
	}

	zero := 0
	data, err = json.Marshal(models.ToolCall{ID: "call-2", Type: "function", Index: &zero})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(data), `"index":0`) {
		t.Fatalf("explicit zero index must be serialized, got %s", data)
	}
}
