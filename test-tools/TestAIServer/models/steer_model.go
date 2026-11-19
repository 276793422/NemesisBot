package models

import (
	"strings"
	"time"
)

// TestAISteer — 子代理飞行中驾驶（项3）E2E 桩（agent-bench subagent_steer 场景）。
//
// 单模型双角色（spawn schema 无模型选择参数——子代理继承父模型，故 master
// 与子代理共用本模型，按消息面区分角色）：
//
//	master 判定：任一消息含 <STEER_E2E>（master 会话全历史可见；
//	            子代理任务文本刻意不含该标记）
//	child  判定：其余（任务文本带 STEER_CHILD_TASK 标记）
//
// master 对话机：
//
//	轮1 user 带 <STEER_E2E> → spawn 工具调用 {task: STEER_CHILD_TASK…, background: true}
//	    （2026-11-19 park 化：master 回合停在 park 等待、无中间回复——bench
//	    以 subagents.list 出现本会话 running 行为 spawn 受理信号）
//	续行 tool（子代理完成回灌，真实工具结果含 STEER_OK…）→ MASTER_ACK_CHILD_DONE
//	续行 tool（无 STEER_OK，理论上不达）       → SPAWNED_OK
//
// 子代理对话机（每轮 1s Delay 给 bench 留出 steer 窗口）：
//
//	任一 user 消息含 [父代理指令] → STEER_OK:<指令原文>（终局——bench 断言面）
//	保活轮 ≥12 → STEER_TIMEOUT（诚实失败：steer 未到达，bench 按含 STEER_OK 判负）
//	其余 → list_dir 保活（路径轮换 8 条——turn_guard 读侧重复 nudge 按
//	       精确 (tool,args) 计数，同参连击 3 次即触发，轮换即免疫；
//	       steer 到达前典型只走 2-4 轮，8 条轮换绰绰有余）
//
// bench 断言链（scenarios.rs）：master 首回合固定中间消息（spawn 受理）→
// subagents.list 出现 running 行 → WSAPI subagents.steer → 轮询该行至终态 →
// summary 含 STEER_OK 且含注入指令子串（子代理真的在下一轮吃到了注入）→
// master 续行终局 MASTER_ACK_CHILD_DONE（回灌链通）。
type TestAISteer struct{}

func NewTestAISteer() *TestAISteer { return &TestAISteer{} }

func (m *TestAISteer) Name() string { return "testai-steer-1.0" }

// Delay 每轮 1s：子代理轮距 1s，bench 在 spawn 受理后 ~1s 内完成 steer，
// 注入落在子代理第 2-4 轮的轮边界 claim。
func (m *TestAISteer) Delay() time.Duration { return 1 * time.Second }

func (m *TestAISteer) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	for _, msg := range messages {
		if strings.Contains(msg.Content, "<STEER_E2E>") {
			return m.masterTurn(messages)
		}
	}
	return m.childTurn(messages)
}

func (m *TestAISteer) masterTurn(messages []Message) string {
	last := messages[len(messages)-1]
	switch last.Role {
	case "tool":
		// spawn 回执轮（Background sub-agent accepted | __BG_SPAWN__…）
		// → SPAWNED_OK——注意这只在【续行】时被消费：首回合在
		// __BG_SPAWN__ marker 处被 loop 固定中间消息收尾，模型不说话；
		// 子代理完成回灌续行时真实工具结果进历史，模型在此收尾。
		if strings.Contains(last.Content, "STEER_OK") {
			return "MASTER_ACK_CHILD_DONE"
		}
		return "SPAWNED_OK"
	case "user":
		// 兜底（理论上 user 轮只会出现一次：首轮派单）→ spawn。
		return buildSingleToolCall("spawn", map[string]interface{}{
			"task":       "STEER_CHILD_TASK 调查工作区根目录结构，汇报你看到的条目后即完成。",
			"background": true,
		})
	}
	return "MASTER_IDLE"
}

// steerKeepalivePaths 保活轮换路径（read-only 白名单内工具，无副作用；
// 每条路径只走一次 → 精确 (tool,args) 计数恒 1，turn_guard 免疫）。
var steerKeepalivePaths = [8]string{
	".", "memory", "scripts", "skills", "logs", "config", "images", "uploads",
}

func (m *TestAISteer) childTurn(messages []Message) string {
	// steer 消费判定：任一 user 消息带 [父代理指令] 前缀（loop 侧
	// steer_subagent 的固定形态），取前缀后的指令原文回显。
	for _, msg := range messages {
		if msg.Role != "user" {
			continue
		}
		if i := strings.Index(msg.Content, "[父代理指令]"); i >= 0 {
			instr := strings.TrimSpace(msg.Content[i+len("[父代理指令]"):])
			return "STEER_OK:" + instr
		}
	}
	// 保活轮上限（tool 结果数）：12 轮 ≈12s 仍无 steer = 诚实失败。
	toolRounds := 0
	for _, msg := range messages {
		if msg.Role == "tool" {
			toolRounds++
		}
	}
	if toolRounds >= 12 {
		return "STEER_TIMEOUT:no steer landed in 12 rounds"
	}
	return buildSingleToolCall("list_dir", map[string]interface{}{
		"path": steerKeepalivePaths[toolRounds%len(steerKeepalivePaths)],
	})
}
