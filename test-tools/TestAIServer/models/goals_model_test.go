package models

import (
	"strings"
	"testing"
)

// TestAIGoals 双角色对话机单元测试（goals 桩自身语义钉死）。

func goalsMsgs(role, content string) []Message {
	return []Message{{Role: role, Content: content}}
}

// 评估闸判别头必须与 nemesis-goals::gate::build_eval_prompt 同源。
const goalsGateHead = "你是目标评估闸（独立评审，不采信执行方的自我申报）"

func TestAIGoalsNameAndDelay(t *testing.T) {
	m := NewTestAIGoals()
	if m.Name() != "testai-goals-1.0" || m.Delay() != 0 {
		t.Fatalf("unexpected name/delay: %s %v", m.Name(), m.Delay())
	}
}

func TestAIGoalsGateFlowA(t *testing.T) {
	m := NewTestAIGoals()
	prompt := goalsGateHead + "\n\n# 长期目标（goal.md 全文）\nGOALS-BENCH-A 目标一\n\n# 轮数进度\n第 0 轮 / 上限 256（达到上限将停车）"
	out := m.Process(goalsMsgs("user", prompt))
	if out != `{"verdict":"DONE"}` {
		t.Fatalf("A 流 round 0 应恒 DONE: %s", out)
	}
}

func TestAIGoalsGateFlowBContinueThenDone(t *testing.T) {
	m := NewTestAIGoals()
	head := goalsGateHead + "\n\n# 长期目标（goal.md 全文）\nGOALS-BENCH-B 目标二\n\n# 轮数进度\n"
	r0 := m.Process(goalsMsgs("user", head+"第 0 轮 / 上限 256"))
	if !strings.Contains(r0, `"verdict":"CONTINUE"`) || !strings.Contains(r0, "GOALS-BENCH-B-GAP-ONE") {
		t.Fatalf("B 流 round 0 应 CONTINUE 带 gap: %s", r0)
	}
	r1 := m.Process(goalsMsgs("user", head+"第 1 轮 / 上限 256"))
	if r1 != `{"verdict":"DONE"}` {
		t.Fatalf("B 流 round 1 应 DONE: %s", r1)
	}
}

func TestAIGoalsGateFlowCAlwaysContinue(t *testing.T) {
	m := NewTestAIGoals()
	prompt := goalsGateHead + "\n\n# 长期目标\nGOALS-BENCH-C 目标三\n\n# 轮数进度\n第 0 轮 / 上限 1"
	out := m.Process(goalsMsgs("user", prompt))
	if !strings.Contains(out, "GOALS-BENCH-C-GAP-ONE") {
		t.Fatalf("C 流应 CONTINUE 带 gap: %s", out)
	}
}

func TestAIGoalsAgentAckAndRoundEcho(t *testing.T) {
	m := NewTestAIGoals()
	ack := m.Process(goalsMsgs("user", "GOALS-BENCH-B: 会话开始"))
	if !strings.Contains(ack, "GOALS-BENCH-B-ACK") {
		t.Fatalf("普通执行轮应 ACK: %s", ack)
	}
	echo := m.Process(goalsMsgs("user", "<goal_round id=\"G1\" round=\"1\" max=\"256\">\n目标未达成，评估闸判定继续。缺口：\nGOALS-BENCH-B-GAP-ONE 缺计算机检查节\n</goal_round>"))
	if !strings.Contains(echo, "GOALS-GATE:ROUND-ECHO") || !strings.Contains(echo, "GOALS-BENCH-B-GAP-ONE") {
		t.Fatalf("注入轮应回声缺口: %s", echo)
	}
}

func TestAIGoalsAgentDeleteToolCall(t *testing.T) {
	m := NewTestAIGoals()
	out := m.Process(goalsMsgs("user", "GOALS-BENCH-D: 请删除本会话的目标"))
	// arguments 是 JSON-in-JSON（\" 转义），按裸词断言动作与工具名。
	if !strings.Contains(out, `"goal"`) || !strings.Contains(out, "delete") {
		t.Fatalf("D 流删除轮应组 goal delete 工具调用: %s", out)
	}
	// 同流不带「删除」= ACK 不出工具调用。
	ack := m.Process(goalsMsgs("user", "GOALS-BENCH-D: 会话开始"))
	if strings.Contains(ack, "tool_calls") {
		t.Fatalf("D 流非删除轮不应出工具调用: %s", ack)
	}
}

func TestAIGoalsToolRefusalEcho(t *testing.T) {
	m := NewTestAIGoals()
	out := m.Process(goalsMsgs("tool", "目标 G1 由人工创建，agent 不可删除——请与用户确认"))
	if !strings.Contains(out, "GOALS-BENCH-D-REFUSED") || !strings.Contains(out, "人工创建") {
		t.Fatalf("权限拒绝回执应原样回声: %s", out)
	}
}
