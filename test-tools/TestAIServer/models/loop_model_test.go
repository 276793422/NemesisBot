package models

import (
	"encoding/json"
	"strings"
	"testing"
	"time"
)

// TestAILoop 三流对话机单元测试（项5 桩自身语义钉死）。
type loopCall struct {
	Name string                 `json:"-"`
	Args map[string]interface{} `json:"-"`
	Raw  string
}

// unwrapLoopCall 从 ProcessedResponse JSON 取单工具调用名+参数。
func unwrapLoopCall(t *testing.T, raw string) (string, map[string]interface{}) {
	t.Helper()
	var outer struct {
		ToolCalls []struct {
			Function struct {
				Name      string `json:"name"`
				Arguments string `json:"arguments"`
			} `json:"function"`
		} `json:"tool_calls"`
	}
	if err := json.Unmarshal([]byte(raw), &outer); err != nil || len(outer.ToolCalls) != 1 {
		t.Fatalf("expect exactly 1 tool call, got: %s", raw)
	}
	var args map[string]interface{}
	if err := json.Unmarshal([]byte(outer.ToolCalls[0].Function.Arguments), &args); err != nil {
		t.Fatalf("arguments should be JSON object, got: %s", outer.ToolCalls[0].Function.Arguments)
	}
	return outer.ToolCalls[0].Function.Name, args
}

func TestAILoopNameAndDelay(t *testing.T) {
	m := NewTestAILoop()
	if m.Name() != "testai-loop-1.0" {
		t.Fatalf("unexpected model name: %s", m.Name())
	}
	if m.Delay() != 0 {
		t.Fatalf("delay should be 0, got: %s", m.Delay())
	}
}

// 首轮（goal 无唤醒标记）→ loop_wakeup 6s 一次性排程。
func TestAILoopGoalTurnSchedules(t *testing.T) {
	m := NewTestAILoop()
	for _, goal := range []string{
		"bench loop 目标：LOOPBENCH:FINISH 收尾",
		"bench loop 目标：LOOPBENCH:PARK 续排程",
		"bench loop 目标：LOOPBENCH:ESTOP 急停链",
	} {
		name, args := unwrapLoopCall(t, m.Process([]Message{{Role: "user", Content: goal}}))
		if name != "loop_wakeup" {
			t.Fatalf("goal turn should call loop_wakeup, got %s", name)
		}
		if args["delay_secs"].(float64) != 6 {
			t.Fatalf("goal turn delay should be 6, got %v", args["delay_secs"])
		}
		if _, has := args["cron"]; has {
			t.Fatalf("goal turn must not set cron")
		}
	}
}

// FINISH 流唤醒轮 → loop_finish，summary 带 LOOPBENCH:DONE。
func TestAILoopFinishFlow(t *testing.T) {
	m := NewTestAILoop()
	msgs := []Message{
		{Role: "user", Content: "目标 LOOPBENCH:FINISH"},
		{Role: "tool", Content: "已排程：loop L1 将在 2026-11-16 唤醒本会话（一次性）"},
		{Role: "user", Content: "[循环:L1] 继续你的目标：LOOPBENCH:FINISH"},
	}
	name, args := unwrapLoopCall(t, m.Process(msgs))
	if name != "loop_finish" {
		t.Fatalf("FINISH wake turn should call loop_finish, got %s", name)
	}
	if s, _ := args["summary"].(string); !strings.Contains(s, "LOOPBENCH:DONE") {
		t.Fatalf("finish summary should carry LOOPBENCH:DONE, got %v", args["summary"])
	}
	// 收尾回执 → 终局确认文本。
	out := m.Process(append(msgs, Message{Role: "tool", Content: "loop L1 已收尾（done）。"}))
	if !strings.Contains(out, "LOOPBENCH:FINISHED") {
		t.Fatalf("finish receipt should yield LOOPBENCH:FINISHED, got %s", out)
	}
}

// PARK 流唤醒轮 → 再排程 4s + note（预算闸在 bench 侧——模型只管续跑）。
func TestAILoopParkFlowReschedules(t *testing.T) {
	m := NewTestAILoop()
	msgs := []Message{
		{Role: "user", Content: "目标 LOOPBENCH:PARK"},
		{Role: "user", Content: "[循环:L1] 继续你的目标：LOOPBENCH:PARK"},
	}
	name, args := unwrapLoopCall(t, m.Process(msgs))
	if name != "loop_wakeup" {
		t.Fatalf("PARK wake turn should re-schedule loop_wakeup, got %s", name)
	}
	if args["delay_secs"].(float64) != 4 {
		t.Fatalf("PARK wake delay should be 4, got %v", args["delay_secs"])
	}
	note, _ := args["note"].(string)
	if !strings.Contains(note, "LOOPBENCH:PROGRESS") {
		t.Fatalf("PARK wake should append progress note, got %q", note)
	}
}

// ESTOP 流：唤醒#1 → 15s 长排程（给 bench estop 窗口）；唤醒#2 → 收尾。
func TestAILoopEstopFlow(t *testing.T) {
	m := NewTestAILoop()
	base := []Message{
		{Role: "user", Content: "目标 LOOPBENCH:ESTOP"},
		{Role: "user", Content: "[循环:L1] 继续你的目标：LOOPBENCH:ESTOP"},
	}
	name, args := unwrapLoopCall(t, m.Process(base))
	if name != "loop_wakeup" {
		t.Fatalf("ESTOP wake#1 should re-schedule, got %s", name)
	}
	if args["delay_secs"].(float64) != 15 {
		t.Fatalf("ESTOP wake#1 delay should be 15 (long, bench estop window), got %v", args["delay_secs"])
	}
	// 唤醒#2（历史两条 [循环:）→ loop_finish ESTOP-DONE。
	wake2 := append(base, Message{Role: "user", Content: "[循环:L1] 继续你的目标：LOOPBENCH:ESTOP"})
	name, args = unwrapLoopCall(t, m.Process(wake2))
	if name != "loop_finish" {
		t.Fatalf("ESTOP wake#2 should finish, got %s", name)
	}
	if s, _ := args["summary"].(string); !strings.Contains(s, "LOOPBENCH:ESTOP-DONE") {
		t.Fatalf("ESTOP finish summary should carry LOOPBENCH:ESTOP-DONE, got %v", args["summary"])
	}
}

// 未绑定（bench loops.create 竞态窗口）→ loop_wakeup 原参重试。
func TestAILoopRetriesWhenUnbound(t *testing.T) {
	m := NewTestAILoop()
	out := m.Process([]Message{
		{Role: "user", Content: "目标 LOOPBENCH:FINISH"},
		{Role: "tool", Content: "本会话未绑定任何 loop（由人经 CLI/WSAPI 创建并绑定）"},
	})
	name, args := unwrapLoopCall(t, out)
	if name != "loop_wakeup" {
		t.Fatalf("unbound should retry loop_wakeup, got %s", name)
	}
	if args["delay_secs"].(float64) != 6 {
		t.Fatalf("retry should keep delay 6, got %v", args["delay_secs"])
	}
}

// 排程成功回执 → LOOPBENCH:SCHEDULED 终局文本。
func TestAILoopScheduledReceipt(t *testing.T) {
	m := NewTestAILoop()
	out := m.Process([]Message{
		{Role: "user", Content: "目标 LOOPBENCH:FINISH"},
		{Role: "tool", Content: "已排程：loop L1 将在 2026-11-16 唤醒本会话（一次性）"},
	})
	if !strings.Contains(out, "LOOPBENCH:SCHEDULED") {
		t.Fatalf("schedule receipt should yield LOOPBENCH:SCHEDULED, got %s", out)
	}
}

// 接口形状锚定（Name/Delay/Process 三方法齐备，编译期即红）。
var _ interface {
	Name() string
	Delay() time.Duration
	Process([]Message) string
} = NewTestAILoop()
