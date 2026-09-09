package models

import (
	"strings"
	"time"
)

// TestAIBoard — Swarm 真机验收用 master 全职责组合测试模型。
//
// 背景：planner-1.0 / review-1.0 是单格式桩（各只会回一种 JSON），而
// master 的拆解（planner）/ 验收（review）/ 主持人讨论走同一个
// defaults.llm——单一专用桩无法同时喂三条链路，真机验收时被迫来回
// 切模型（G6 切 review 后忘切回，G1 复验拆解即炸）。
//
// 本模型按 system prompt 特征词分流到对应专用桩（完全复用其 Process，
// 含全部锚点分支与注入探针，零逻辑重复）：
//   - 含「任务拆解规划器（planner）」→ TestAIPlanner（固定 3 子任务计划）
//   - 含「验收 agent（reviewer）」  → TestAIReview（verdict 三态+锚点）
//   - 其他 → 固定普通文本（主持人讨论发言；无格式断言场景）
//
// 两个特征词分别来自 nemesis-board PLANNER_SYSTEM_PROMPT /
// REVIEW_SYSTEM_PROMPT 的首句自我介绍，为各桩输入文本独有（回灌重试
// 消息带的旧输出 JSON 不含特征词，system prompt 始终在 messages[0]，
// 分流不受重试轮次影响）。
type TestAIBoard struct {
	planner *TestAIPlanner
	review  *TestAIReview
}

func NewTestAIBoard() *TestAIBoard {
	return &TestAIBoard{planner: NewTestAIPlanner(), review: NewTestAIReview()}
}

func (m *TestAIBoard) Name() string { return "testai-board-1.0" }

func (m *TestAIBoard) Process(messages []Message) string {
	var buf strings.Builder
	for _, msg := range messages {
		buf.WriteString(msg.Content)
		buf.WriteString("\n")
	}
	input := buf.String()

	switch {
	case strings.Contains(input, "任务拆解规划器（planner）"):
		return m.planner.Process(messages)
	case strings.Contains(input, "验收 agent（reviewer）"):
		return m.review.Process(messages)
	default:
		return "收到。当前集群协作状态正常，如需推进任务请继续；我会按看板流程跟进。"
	}
}

func (m *TestAIBoard) Delay() time.Duration { return 0 }
