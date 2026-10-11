package models

import (
	"encoding/json"
	"strings"
	"testing"
)

// TestAISpawn 两轮对话机单元测试（T48 桩自身语义钉死）。
func TestAISpawnTwoRoundFlow(t *testing.T) {
	m := NewTestAISpawn()
	if m.Name() != "testai-spawn-1.0" {
		t.Fatalf("unexpected model name: %s", m.Name())
	}

	// 轮1：user 消息带 <SPAWN_PATH> → exec 工具调用，命令含目标路径。
	// cmd.exe 内置 del + 双引号 + 反斜杠（cmd /C 执行面；Remove-Item 在
	// cmd 下不存在——T48 首轮实证）。正斜杠路径必须转反斜杠。
	// 断言走两层 JSON 解包取命令原文（对转义层数零猜测，精确等值）。
	task := "修复任务 <SPAWN_PATH>C:/tmp/t48_approve_target.txt</SPAWN_PATH> 请处理"
	round1 := m.Process([]Message{{Role: "user", Content: task}})
	var outer struct {
		ToolCalls []struct {
			Function struct {
				Name      string `json:"name"`
				Arguments string `json:"arguments"`
			} `json:"function"`
		} `json:"tool_calls"`
	}
	if err := json.Unmarshal([]byte(round1), &outer); err != nil || len(outer.ToolCalls) == 0 {
		t.Fatalf("round1 should emit exec tool call, got: %s", round1)
	}
	var args struct {
		Command string `json:"command"`
	}
	if err := json.Unmarshal([]byte(outer.ToolCalls[0].Function.Arguments), &args); err != nil {
		t.Fatalf("round1 arguments should be JSON, got: %s", outer.ToolCalls[0].Function.Arguments)
	}
	if args.Command != `del "C:\tmp\t48_approve_target.txt"` {
		t.Fatalf("round1 command should be del with backslash path, got: %q", args.Command)
	}

	// 轮2a：tool 结果含 SECURITY BLOCKED（审批拒绝链）→ DENIED 标记。
	round2deny := m.Process([]Message{
		{Role: "user", Content: task},
		{Role: "assistant", Content: "", ToolCalls: nil},
		{Role: "tool", Content: "⛔ SECURITY BLOCKED [layer:abac|policy:process_rules] denied"},
	})
	if !strings.Contains(round2deny, "SPAWN_DENIED:C:/tmp/t48_approve_target.txt") {
		t.Fatalf("deny result should yield SPAWN_DENIED marker, got: %s", round2deny)
	}

	// 轮2b：tool 结果为命令真实执行（成功输出）→ COMPLETED 标记。
	round2ok := m.Process([]Message{
		{Role: "user", Content: task},
		{Role: "tool", Content: "exited with code 0"},
	})
	if !strings.Contains(round2ok, "SPAWN_COMPLETED:C:/tmp/t48_approve_target.txt") {
		t.Fatalf("exec result should yield SPAWN_COMPLETED marker, got: %s", round2ok)
	}
}

// 会话复用锚定：第二轮的路径取自当前任务（最后一条 user 消息），
// 不被历史任务的 SPAWN_PATH 污染。
func TestAISpawnAnchorsToCurrentTask(t *testing.T) {
	m := NewTestAISpawn()
	messages := []Message{
		{Role: "user", Content: "旧任务 <SPAWN_PATH>C:/tmp/old_target.txt</SPAWN_PATH>"},
		{Role: "assistant", Content: "SPAWN_COMPLETED:C:/tmp/old_target.txt"},
		{Role: "user", Content: "新任务 <SPAWN_PATH>C:/tmp/new_target.txt</SPAWN_PATH>"},
		{Role: "tool", Content: "exited with code 0"},
	}
	got := m.Process(messages)
	if !strings.Contains(got, "SPAWN_COMPLETED:C:/tmp/new_target.txt") {
		t.Fatalf("must anchor to current task path, got: %s", got)
	}
	if strings.Contains(got, "old_target") {
		t.Fatalf("must not leak historical task path, got: %s", got)
	}
}

// 无标记任务走 SKIP 分支（不产生工具调用）。
func TestAISpawnSkipsTaskWithoutMarker(t *testing.T) {
	m := NewTestAISpawn()
	got := m.Process([]Message{{Role: "user", Content: "普通任务，无标记"}})
	if got != "SPAWN_SKIP_NO_PATH" {
		t.Fatalf("expected SPAWN_SKIP_NO_PATH, got: %s", got)
	}
}
