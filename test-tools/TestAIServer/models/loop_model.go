package models

import (
	"fmt"
	"strings"
	"time"
)

// TestAILoop — 自配速 durable loop（项5）E2E 桩（agent-bench durable_loop 场景）。
//
// 单模型对话机，按 goal 内嵌 marker 分三条流：
//
//	LOOPBENCH:FINISH  首轮排程(6s) → 唤醒轮 loop_finish（正常自配速收尾链）
//	LOOPBENCH:PARK    首轮排程(6s) → 唤醒轮再排程(4s)（bench 侧
//	                  budget.max_turns_total=1 → 第二次 sweep 预算耗尽 → 停车场留痕）
//	LOOPBENCH:ESTOP   首轮排程(6s) → 唤醒#1 长排程(15s，给 bench estop 窗口留余量)
//	                  → 唤醒#2 loop_finish（estop 冻结-保留-释放补投链）
//
// 唤醒轮判定：最后一条 user 消息含唤醒标记「[循环:」（驱动器注入前缀，
// nemesis-loop::wake::WAKE_MARKER 同源；goal 首轮不含）。唤醒次数按全历史
// 「[循环:」出现次数计（ESTOP 流第 2 次唤醒才收尾——冻结期注入停止的旁证）。
//
// 未绑定重试：bench 先 chat.send 占位 goal、紧接 WSAPI loops.create——模型
// 首轮 loop_wakeup 可能抢在 create 落盘之前（真实竞态），tool 结果含
// 「未绑定」时原参重试（顺带 E2E 覆盖工具诚实报错路径）。
type TestAILoop struct{}

func NewTestAILoop() *TestAILoop { return &TestAILoop{} }

func (m *TestAILoop) Name() string { return "testai-loop-1.0" }

func (m *TestAILoop) Delay() time.Duration { return 0 }

func (m *TestAILoop) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	switch last.Role {
	case "tool":
		return m.toolTurn(last.Content)
	case "user":
		if strings.Contains(last.Content, "[循环:") {
			return m.wakeTurn(messages)
		}
		return m.goalTurn()
	}
	return "LOOPBENCH:IDLE"
}

// loopFlow 扫全部 user 消息取流 marker（唤醒消息引用 goal 原文 → 每轮可见）。
func loopFlow(messages []Message) string {
	for _, msg := range messages {
		if msg.Role != "user" {
			continue
		}
		switch {
		case strings.Contains(msg.Content, "LOOPBENCH:FINISH"):
			return "FINISH"
		case strings.Contains(msg.Content, "LOOPBENCH:PARK"):
			return "PARK"
		case strings.Contains(msg.Content, "LOOPBENCH:ESTOP"):
			return "ESTOP"
		}
	}
	return ""
}

// loopWakeupCall 组 loop_wakeup 工具调用（一次性 delay 排程；note 可空）。
func loopWakeupCall(delay int, note string) string {
	args := map[string]interface{}{"delay_secs": delay}
	if note != "" {
		args["note"] = note
	}
	return buildSingleToolCall("loop_wakeup", args)
}

// goalTurn 首轮：统一 6s 一次性排程（自配速——下次何时醒由 agent 定）。
func (m *TestAILoop) goalTurn() string {
	return loopWakeupCall(6, "")
}

// wakeTurn 唤醒轮：按流分流（收尾 / 续排程 / estop 链二次判定）。
func (m *TestAILoop) wakeTurn(messages []Message) string {
	wakes := 0
	for _, msg := range messages {
		if msg.Role == "user" && strings.Contains(msg.Content, "[循环:") {
			wakes++
		}
	}
	switch loopFlow(messages) {
	case "FINISH":
		return buildSingleToolCall("loop_finish", map[string]interface{}{
			"summary": "LOOPBENCH:DONE 目标达成，循环正常收尾",
		})
	case "PARK":
		return loopWakeupCall(4, fmt.Sprintf("LOOPBENCH:PROGRESS 第 %d 次唤醒完成", wakes))
	case "ESTOP":
		if wakes >= 2 {
			return buildSingleToolCall("loop_finish", map[string]interface{}{
				"summary": "LOOPBENCH:ESTOP-DONE 急停释放后补投收尾",
			})
		}
		// 唤醒#1：长排程（15s）——bench 在此窗口触发 estop，冻结期
		// sweep 零动作、唤醒留到期，release 后补投。
		return loopWakeupCall(15, "LOOPBENCH:PROGRESS 急停前最后一次排程")
	}
	return "LOOPBENCH:UNKNOWN-FLOW"
}

// toolTurn 工具回执轮：排程成功 → 终局文本；未绑定（bench create 竞态）
// → 原参重试；收尾回执 → 终局确认。
func (m *TestAILoop) toolTurn(content string) string {
	switch {
	case strings.Contains(content, "未绑定"):
		return loopWakeupCall(6, "")
	case strings.Contains(content, "已排程"):
		return "LOOPBENCH:SCHEDULED 下次唤醒已安排"
	case strings.Contains(content, "已收尾"):
		return "LOOPBENCH:FINISHED 循环已收尾"
	}
	return "LOOPBENCH:TOOL-UNKNOWN"
}
