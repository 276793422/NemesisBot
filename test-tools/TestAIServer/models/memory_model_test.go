package models

import (
	"encoding/json"
	"strings"
	"testing"
	"time"
)

// TestAIMem 确定性搬运工模型单元测试（项2 P1 桩自身语义钉死）。
func memSingleCall(t *testing.T, raw string) (string, map[string]interface{}) {
	t.Helper()
	name, args := unwrapLoopCall(t, raw) // 复用同形解包（单调用形态一致）
	return name, args
}

func TestAIMemNameAndDelay(t *testing.T) {
	m := NewTestAIMem()
	if m.Name() != "testai-mem-1.0" {
		t.Fatalf("unexpected model name: %s", m.Name())
	}
	if m.Delay() != 0 {
		t.Fatalf("delay should be 0, got: %s", m.Delay())
	}
}

// MEMSTORE goal 轮 → 逐条 memory_store（episodic + bench tag，N 个并发调用）。
func TestAIMemStoreFanOut(t *testing.T) {
	m := NewTestAIMem()
	raw := m.Process([]Message{{Role: "user", Content: `MEMSTORE: ["事实甲 BENCHMEM-A1", "事实乙 BENCHMEM-A2", "事实丙 BENCHMEM-A3"]`}})
	var outer struct {
		ToolCalls []struct {
			Function struct {
				Name      string `json:"name"`
				Arguments string `json:"arguments"`
			} `json:"function"`
		} `json:"tool_calls"`
	}
	if err := json.Unmarshal([]byte(raw), &outer); err != nil || len(outer.ToolCalls) != 3 {
		t.Fatalf("expect 3 tool calls, got %d: %s", len(outer.ToolCalls), raw)
	}
	for i, call := range outer.ToolCalls {
		if call.Function.Name != "memory_store" {
			t.Fatalf("call %d should be memory_store, got %s", i, call.Function.Name)
		}
		var args map[string]interface{}
		if err := json.Unmarshal([]byte(call.Function.Arguments), &args); err != nil {
			t.Fatalf("arguments should be JSON: %v", err)
		}
		if args["memory_type"] != "episodic" {
			t.Fatalf("memory_type should be episodic, got %v", args["memory_type"])
		}
		tags, _ := args["tags"].([]interface{})
		if len(tags) != 1 || tags[0] != "bench" {
			t.Fatalf("tags should be [bench], got %v", args["tags"])
		}
	}
}

// MEMQUERY → memory_search；MEMFORGET → memory_forget delete_by_id。
func TestAIMemQueryAndForget(t *testing.T) {
	m := NewTestAIMem()
	name, args := memSingleCall(t, m.Process([]Message{{Role: "user", Content: "MEMQUERY: BENCHMEM-A1 的事实"}}))
	if name != "memory_search" {
		t.Fatalf("MEMQUERY should call memory_search, got %s", name)
	}
	if args["query"] != "BENCHMEM-A1 的事实" {
		t.Fatalf("query passthrough failed: %v", args["query"])
	}

	name, args = memSingleCall(t, m.Process([]Message{{Role: "user", Content: "MEMFORGET: 3f2a9c1e-0000-4000-8000-000000000001"}}))
	if name != "memory_forget" {
		t.Fatalf("MEMFORGET should call memory_forget, got %s", name)
	}
	if args["action"] != "delete_by_id" {
		t.Fatalf("action should be delete_by_id, got %v", args["action"])
	}
	if args["id"] != "3f2a9c1e-0000-4000-8000-000000000001" {
		t.Fatalf("id passthrough failed: %v", args["id"])
	}
}

// 工具回执轮 → MEMRECEIPT 拼接全部回执（多调用形态全量 echo）。
func TestAIMemReceiptEcho(t *testing.T) {
	m := NewTestAIMem()
	out := m.Process([]Message{
		{Role: "user", Content: `MEMSTORE: ["事实甲", "事实乙"]`},
		{Role: "tool", Content: "Episodic memory stored successfully (ID: aaa, session: s1)"},
		{Role: "tool", Content: "Episodic memory stored successfully (ID: bbb, session: s1)"},
	})
	if !strings.Contains(out, "MEMRECEIPT:") {
		t.Fatalf("receipt turn should start MEMRECEIPT:, got %s", out)
	}
	if !strings.Contains(out, "ID: aaa") || !strings.Contains(out, "ID: bbb") {
		t.Fatalf("receipt should contain both IDs, got %s", out)
	}
	if !strings.Contains(out, "---") {
		t.Fatalf("multiple receipts should be joined with ---, got %s", out)
	}
}

// 无 marker 的 user 轮 = 诚实等待文本（不假装执行）。
func TestAIMemNoMarker(t *testing.T) {
	m := NewTestAIMem()
	out := m.Process([]Message{{Role: "user", Content: "普通聊天，不带 marker"}})
	if strings.Contains(out, "tool_call") {
		t.Fatalf("no-marker turn should not emit tool calls: %s", out)
	}
	if !strings.Contains(out, "no marker") {
		t.Fatalf("no-marker turn should say waiting, got %s", out)
	}
}

// 接口形状锚定（编译期即红）。
var _ interface {
	Name() string
	Delay() time.Duration
	Process([]Message) string
} = NewTestAIMem()
