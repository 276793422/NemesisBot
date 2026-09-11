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
//   - 其他 → masterProcess（<BOARD_ISSUE> 标记对话机 / 固定普通文本）
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
		return m.masterProcess(messages)
	}
}

// masterProcess — 主 agent（master）对话桩。除固定普通文本（主持人讨论
// 发言；无格式断言场景）外，支持 <BOARD_ISSUE> 标记驱动的「一句话建单 →
// AI 拆解」三步对话机（全自动流转 P3 agent 工具路径的集成测试驱动）：
//
//	轮1 最后一条是 user 消息且带 <BOARD_ISSUE>标题</BOARD_ISSUE>
//	    → board_issue create 工具调用
//	轮2 最后一条是 tool 结果且含「已建单 NB-n」（create 确认文本锚点）
//	    → board_issue plan 工具调用（引用该单号）
//	轮3 最后一条是 tool 结果且含 plan 链 JSON（plan_id 字段锚点）
//	    → 固定收尾文本 BOARD_ISSUE_FLOW_DONE
//
// 另支持 P4 B2b 自检取证请求：最后一条 user 消息含「[取证请求
// board_selfcheck:」（board_review.rs build_selfcheck_prompt 的路由前缀，
// marker 仅供人读，实际路由凭 SelfcheckRegistry）→ 回固定取证文本，逐项
// 回报并附带 <SELFCHK_EVIDENCE_OK> 完成标记（命中 review 桩二段 PASS 分支）。
//
// 锚点文本与 nemesisbot/src/board_issue_tool.rs 的产出严格对应：
// create 确认「已建单 {number}：」；plan 结果为 execute_plan_chain 的
// JSON（含 plan_id）。流程异常（空 moderator / planner 失败）落到
// BOARD_ISSUE_TOOL_UNEXPECTED，由测试侧断言失败暴露。
func (m *TestAIBoard) masterProcess(messages []Message) string {
	if len(messages) > 0 {
		last := messages[len(messages)-1]
		switch {
		case last.Role == "user":
			if strings.Contains(last.Content, "[取证请求 board_selfcheck:") {
				return boardSelfcheckEvidence
			}
			if title, ok := extractBoardIssueTitle(last.Content); ok {
				return buildSingleToolCall("board_issue", map[string]interface{}{
					"subcommand":  "create",
					"title":       title,
					"description": "由 <BOARD_ISSUE> 标记驱动建单（TestAIServer 集成测试）",
				})
			}
		case last.Role == "tool":
			if n := extractCreatedIssueNumber(last.Content); n != "" {
				return buildSingleToolCall("board_issue", map[string]interface{}{
					"subcommand": "plan",
					"issue":      n,
				})
			}
			if strings.Contains(last.Content, "plan_id") {
				return "BOARD_ISSUE_FLOW_DONE"
			}
			return "BOARD_ISSUE_TOOL_UNEXPECTED"
		}
	}
	return boardAckText
}

// boardAckText — 无标记场景的固定普通回复（主持人讨论；保持原样）。
const boardAckText = "收到。当前集群协作状态正常，如需推进任务请继续；我会按看板流程跟进。"

// boardSelfcheckEvidence — P4 B2b 自检取证的固定回报文本：逐项回报 +
// <SELFCHK_EVIDENCE_OK> 完成标记（review 桩二段验收 PASS 分支的输入锚点）。
const boardSelfcheckEvidence = "取证回报：\n" +
	"1. 交付物已按验收标准逐项落实，关键输出如下原文：\n" +
	"   「收到。当前集群协作状态正常，如需推进任务请继续；我会按看板流程跟进。」\n" +
	"2. 交付后未做任何改动，工作区状态与交付时一致。\n" +
	"3. 以上逐项与验收标准对照无缺口，取证完成 <SELFCHK_EVIDENCE_OK>"

// extractBoardIssueTitle 从 user 消息抠 <BOARD_ISSUE>…</BOARD_ISSUE> 标题；
// 缺闭合标签取剩余全文；抠出为空白 → 默认标题。ok=false = 消息无标记。
func extractBoardIssueTitle(content string) (string, bool) {
	const open = "<BOARD_ISSUE>"
	i := strings.Index(content, open)
	if i < 0 {
		return "", false
	}
	rest := content[i+len(open):]
	title := rest
	if j := strings.Index(rest, "</BOARD_ISSUE>"); j >= 0 {
		title = rest[:j]
	}
	title = strings.TrimSpace(title)
	if title == "" {
		title = "自动流转测试单"
	}
	return title, true
}

// extractCreatedIssueNumber 从 board_issue create 工具结果抠「已建单
// NB-n」的单号（含 NB- 前缀）；无锚点返回空串。
func extractCreatedIssueNumber(toolResult string) string {
	const key = "已建单 NB-"
	i := strings.Index(toolResult, key)
	if i < 0 {
		return ""
	}
	rest := toolResult[i+len(key):]
	digits := make([]byte, 0, 4)
	for k := 0; k < len(rest) && rest[k] >= '0' && rest[k] <= '9'; k++ {
		digits = append(digits, rest[k])
	}
	if len(digits) == 0 {
		return ""
	}
	return "NB-" + string(digits)
}

func (m *TestAIBoard) Delay() time.Duration { return 0 }
