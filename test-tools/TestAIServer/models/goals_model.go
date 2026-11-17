package models

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
	"time"
)

// TestAIGoals — 目标系统（goals）E2E 桩（agent-bench goal_gate 场景）。
//
// 单模型双角色对话机：同一模型既当执行 agent 又当目标评估闸（goals 无
// eval_model 配置时评估走主模型），按最后一条 user 消息判别角色：
//
//   - 评估闸：内容含「你是目标评估闸」（build_eval_prompt 判别头，
//     nemesis-goals::gate 同源）→ 按 goal.md 内嵌流 marker + 「第 N 轮」
//     出一行 verdict JSON；
//   - 执行轮 goal_round 注入：内容含「<goal_round」（build_round_message
//     注入块）→ 回声缺口（bench 用它断言 gap 注入真实到达 agent）；
//   - 普通执行轮：按流 marker 出 ACK 文本；GOALS-BENCH-D 带「删除」时
//     组 goal delete 工具调用（权限拒绝流——人建 goal 删除被诚实拒）。
//
// 四条流（goal.md 内容即流 marker）：
//
//	GOALS-BENCH-A  评估恒 DONE（round 0 直接收口——done 判定流）
//	GOALS-BENCH-B  round 0 CONTINUE（缺口 GOALS-BENCH-B-GAP-ONE）→ round≥1 DONE（续轮流）
//	GOALS-BENCH-C  round 0 CONTINUE（缺口 GOALS-BENCH-C-GAP-ONE）；round≥1 永不达——
//	               bench 用 max_rounds=1，第二轮评估前预算闸先停车（预算耗尽流）
//	GOALS-BENCH-D  评估恒 DONE；执行轮按「删除」出 goal delete（权限拒绝流）
//
// 工具回执轮：goal delete 撞人建 goal → 错误回执含「人工创建」→ 回声
// 错误原文（bench 断言诚实拒绝面）；其余回执出固定确认。
type TestAIGoals struct{}

func NewTestAIGoals() *TestAIGoals { return &TestAIGoals{} }

func (m *TestAIGoals) Name() string { return "testai-goals-1.0" }

func (m *TestAIGoals) Delay() time.Duration { return 0 }

var goalsRoundRe = regexp.MustCompile(`第 (\d+) 轮`)

func (m *TestAIGoals) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	switch last.Role {
	case "tool":
		return m.toolTurn(last.Content)
	case "user":
		if strings.Contains(last.Content, "你是目标评估闸") {
			return m.gateTurn(last.Content)
		}
		return m.agentTurn(messages, last.Content)
	}
	return "GOALS-GATE:IDLE"
}

// gateTurn 评估闸：按流 marker + 轮数出 verdict JSON（parse_verdict 契约：
// 一行 {"verdict":...} 或 {"verdict":"CONTINUE","gap":...}）。
func (m *TestAIGoals) gateTurn(prompt string) string {
	round := 0
	if mm := goalsRoundRe.FindStringSubmatch(prompt); mm != nil {
		round, _ = strconv.Atoi(mm[1])
	}
	switch {
	case strings.Contains(prompt, "GOALS-BENCH-A"):
		return `{"verdict":"DONE"}`
	case strings.Contains(prompt, "GOALS-BENCH-B"):
		if round >= 1 {
			return `{"verdict":"DONE"}`
		}
		return `{"verdict":"CONTINUE","gap":"GOALS-BENCH-B-GAP-ONE 缺计算机检查节"}`
	case strings.Contains(prompt, "GOALS-BENCH-C"):
		// round≥1 到不了这里：bench max_rounds=1，预算闸先于评估停车。
		return `{"verdict":"CONTINUE","gap":"GOALS-BENCH-C-GAP-ONE 缺性能数据"}`
	case strings.Contains(prompt, "GOALS-BENCH-D"):
		return `{"verdict":"DONE"}`
	}
	return `{"verdict":"CONTINUE","gap":"GOALS-GATE:UNKNOWN-FLOW"}`
}

// goalsFlow 扫全历史 user 消息取流 marker（goal_round 注入不含流名，
// 流身份只在首轮 chat 文本里）。
func goalsFlow(messages []Message) string {
	for _, msg := range messages {
		if msg.Role != "user" {
			continue
		}
		for _, f := range []string{"GOALS-BENCH-A", "GOALS-BENCH-B", "GOALS-BENCH-C", "GOALS-BENCH-D"} {
			if strings.Contains(msg.Content, f) {
				return f
			}
		}
	}
	return ""
}

// agentTurn 执行轮：goal_round 注入 → 回声缺口；D 流带「删除」→ goal
// delete 工具调用；其余 → 流 ACK 文本。
func (m *TestAIGoals) agentTurn(messages []Message, content string) string {
	if strings.Contains(content, "<goal_round") {
		echo := content
		if len(echo) > 200 {
			echo = echo[:200]
		}
		return "GOALS-GATE:ROUND-ECHO " + echo
	}
	flow := goalsFlow(messages)
	if flow == "GOALS-BENCH-D" && strings.Contains(content, "删除") {
		return buildSingleToolCall("goal", map[string]interface{}{"action": "delete"})
	}
	if flow != "" {
		return flow + "-ACK 已开始推进目标"
	}
	return "GOALS-GATE:UNKNOWN-TURN"
}

// toolTurn 工具回执轮：权限拒绝（人建 goal）→ 回声错误原文；其余确认。
func (m *TestAIGoals) toolTurn(content string) string {
	if strings.Contains(content, "人工创建") {
		return "GOALS-BENCH-D-REFUSED " + content
	}
	return fmt.Sprintf("GOALS-GATE:TOOL-OK %s", firstRunes(content, 80))
}

// firstRunes 取前 n 个 rune（错误回执截断回声，防超长）。
func firstRunes(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n])
}
