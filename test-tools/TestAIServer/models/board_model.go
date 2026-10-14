package models

import (
	"encoding/json"
	"regexp"
	"strconv"
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
// peer_chat 对话机（campaign 48h soak 追加，与上述 marker 互斥）：
//   - 轮1 最后一条 user 消息带 <PEER_CHAT>{"peer_id":..,"content":..}</PEER_CHAT>
//     → cluster_rpc 工具调用（契约与 testai-3.0 一致：target/message 对齐
//     工具 schema）。
//   - 轮2（续行）输入含 <PEER_CHAT>（历史 user 消息保留标记）且末条是
//     tool 结果 → 固定前缀「对端回复：」+ 原文转述（前缀即驱动断言锚点）。
//     排在 board_issue tool 分支之前——建单锚点与 peer_chat 互斥，前置
//     只为防误入 BOARD_ISSUE_TOOL_UNEXPECTED 兜底。
//
// 职能框架 M7 追加两台对话机（都要求整个输入含对应标记才激活，既有
// 流程零扰动）：
//   - <UAT_UI_IMG>（U5）：轮1 user 带 marker → generate_image 工具调用；
//     轮2 tool 结果含 ui.png（path 锚点）→ board_asset publish；轮3 tool
//     结果含 published → 交付文本「UI图已交付」+ 原样内嵌 publish 结果
//     （含 AssetTokenBundle JSON——A 端 M5 评审取图链路的输入）。
//   - <UAT_UI_TEXT>（U6）：user 带 marker → 固定文本交付「UI图已交付」
//     （D7 默认态：无图像模型时文本线完整验收）。
//
// 职能契约回显（U1/U2/U3/U7）：输入含职能后缀稳定标记「执行职能：…（slug）」
// （render_profession_suffix 产物首行）→ 交付文本回显「职能=<slug>」。
// A 端验收 re: 锚点据此客观断言「契约真的进了 B 端 system prompt」——
// 证据走交付正流，不走日志旁路。仅在以上对话机都不命中时兜底生效。
//
// 锚点文本与 nemesisbot/src/board_issue_tool.rs 的产出严格对应：
// create 确认「已建单 {number}：」；plan 结果为 execute_plan_chain 的
// JSON（含 plan_id）。流程异常（空 moderator / planner 失败）落到
// BOARD_ISSUE_TOOL_UNEXPECTED，由测试侧断言失败暴露。
func (m *TestAIBoard) masterProcess(messages []Message) string {
	var buf strings.Builder
	for _, msg := range messages {
		buf.WriteString(msg.Content)
		buf.WriteString("\n")
	}
	input := buf.String()

	if len(messages) > 0 {
		last := messages[len(messages)-1]
		switch {
		case last.Role == "user":
			if strings.Contains(last.Content, "[取证请求 board_selfcheck:") {
				return boardSelfcheckEvidence
			}
			// soak peer_chat 对话机（轮1）：<PEER_CHAT> 标记 → cluster_rpc。
			if args, ok := extractPeerChatArgs(last.Content); ok {
				return buildSingleToolCall("cluster_rpc", map[string]interface{}{
					"target":  args.PeerID,
					"message": args.Content,
				})
			}
			// 职能框架 M7 U5：UI 子单图像对话机（轮1）。
			if strings.Contains(input, "<UAT_UI_IMG>") {
				return buildSingleToolCall("generate_image", map[string]interface{}{
					"prompt": "登录页高保真线框：顶部品牌区、中部表单、底部主按钮",
					"output": "board_uat/ui.png",
				})
			}
			// 职能框架 M7 U6：文本线交付（D7 默认态）。
			if strings.Contains(input, "<UAT_UI_TEXT>") {
				return "UI图已交付（文本线）：登录页布局=顶部品牌区/中部表单/底部主按钮，配色=主色蓝灰，组件=输入框×2+按钮×1。"
			}
			// 职能框架 D2（U9）：定稿单对话机（轮1）——向本单 issue 讨论
			// 线程发 @all 集群需求评审邀请（多变体摘要 + 请各节点发表实质
			// 意见）。thread_id 从任务卡首行「# 看板任务 NB-n」提取。
			if strings.Contains(input, "<UAT_D2_FINALIZE>") {
				return buildSingleToolCall("board_discuss", map[string]interface{}{
					"thread_kind": "issue",
					"thread_id":   extractTaskCardIssueNumber(input),
					"content":     "@all 需求评审邀请：本单聚合三条竞争解读变体（A 保守 / B 最大化 / C 重新框定），请各节点结合自身职能视角发表实质意见（支持/反对/修改建议均可），稍后本单将读取线程形成裁决并定稿。",
				})
			}
			if title, ok := extractBoardIssueTitle(last.Content); ok {
				return buildSingleToolCall("board_issue", map[string]interface{}{
					"subcommand":  "create",
					"title":       title,
					"description": "由 <BOARD_ISSUE> 标记驱动建单（TestAIServer 集成测试）",
				})
			}
		case last.Role == "tool":
			// 职能框架 D2（U9）：定稿单对话机（轮2+）——按工具结果推进
			// 发帖→sleep→读线程→（空则再等）→裁决定稿汇报。
			if strings.Contains(input, "<UAT_D2_FINALIZE>") {
				return d2FinalizeToolTurn(input, last.Content)
			}
			// soak peer_chat 续行（轮2）：历史含 <PEER_CHAT> 且末条是 tool
			// 结果 → 固定前缀转述对端回复（防落入 BOARD_ISSUE 兜底）。
			if strings.Contains(input, "<PEER_CHAT>") {
				return "对端回复：" + last.Content
			}
			if n := extractCreatedIssueNumber(last.Content); n != "" {
				return buildSingleToolCall("board_issue", map[string]interface{}{
					"subcommand": "plan",
					"issue":      n,
				})
			}
			if strings.Contains(last.Content, "plan_id") {
				return "BOARD_ISSUE_FLOW_DONE"
			}
			// 职能框架 M7 U5：图像对话机（轮2/轮3）。锚点用 ui.png 文件名
			// （generate_image 结果 path 是绝对路径，Windows 下反斜杠——
			// 文件名本身平台无关）；publish 结果含 "published"。
			if strings.Contains(input, "<UAT_UI_IMG>") {
				if strings.Contains(last.Content, "ui.png") && strings.Contains(last.Content, "\"path\"") {
					return buildSingleToolCall("board_asset", map[string]interface{}{
						"action": "publish",
						"path":   "images/board_uat/ui.png",
					})
				}
				if strings.Contains(last.Content, "published") {
					return "UI图已交付（图像线）。\n" + last.Content
				}
				// 图像链路中断（工具未注册/端点失败）：诚实文本交付，
				// 不带 bundle——评审走纯文本锚点，UAT 断言按失败暴露。
				return "UI图已交付（图像线降级文本）：工具链路未走通。"
			}
			return "BOARD_ISSUE_TOOL_UNEXPECTED"
		}
	}
	// 职能契约回显（兜底位——以上对话机都不命中才生效；A 端 master 对话
	// 无职能后缀，恒走 boardAckText，零行为变化）。
	if slug := extractProfessionSlug(input); slug != "" {
		return "收到。职能=" + slug + "。" + boardAckText
	}
	return boardAckText
}

// extractProfessionSlug 从提示词全文抠职能后缀标记「执行职能：<label>（<slug>）」
// 里的 slug（render_profession_suffix 契约首行/未知 slug 块首行同构）。
// 多处命中取首个（契约段先于方法论段）。无命中返回空串（不回显）。
func extractProfessionSlug(input string) string {
	const marker = "执行职能："
	re := regexp.MustCompile(`[（(]([a-z0-9_-]+(?::[a-z0-9_-]+)?)[）)]`)
	rest := input
	for {
		i := strings.Index(rest, marker)
		if i < 0 {
			return ""
		}
		line := rest[i+len(marker):]
		if j := strings.IndexAny(line, "\n"); j >= 0 {
			line = line[:j]
		}
		if m := re.FindStringSubmatch(line); m != nil {
			return m[1]
		}
		rest = rest[i+len(marker):]
	}
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

// peerChatReq <PEER_CHAT> 标记内嵌 JSON 的形状（与 testai-3.0 契约一致）。
type peerChatReq struct {
	PeerID  string `json:"peer_id"`
	Content string `json:"content"`
}

// extractPeerChatArgs 从 user 消息抠 <PEER_CHAT>{...}</PEER_CHAT> 标记并解析。
// 无标记 / 缺闭合标签 / JSON 非法 → ok=false（落回普通文本分支）。
func extractPeerChatArgs(content string) (peerChatReq, bool) {
	const open, close = "<PEER_CHAT>", "</PEER_CHAT>"
	i := strings.Index(content, open)
	if i < 0 {
		return peerChatReq{}, false
	}
	rest := content[i+len(open):]
	j := strings.Index(rest, close)
	if j < 0 {
		return peerChatReq{}, false
	}
	var req peerChatReq
	if err := json.Unmarshal([]byte(strings.TrimSpace(rest[:j])), &req); err != nil {
		return peerChatReq{}, false
	}
	return req, true
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

// extractTaskCardIssueNumber 从任务卡提示词抠看板单号（board.rs
// build_dispatch_prompt 首行契约「# 看板任务 NB-n」）。定稿对话机每轮都要
// thread_id，而 sleep/读线程的工具结果不带单号——从全会话输入重复提取
// （任务卡在首条 user 消息里，轮轮都在）。无命中返回 0（不应发生；后续
// 工具调用会诚实失败，UAT 断言按失败暴露）。
func extractTaskCardIssueNumber(input string) int {
	re := regexp.MustCompile(`看板任务\s*NB-(\d+)`)
	if m := re.FindStringSubmatch(input); m != nil {
		if n, err := strconv.Atoi(m[1]); err == nil {
			return n
		}
	}
	return 0
}

// d2FinalizeToolTurn — <UAT_D2_FINALIZE> 定稿对话机的工具结果推进器：
//   - post 结果「Posted to」（board_discuss 写确认）→ sleep（等应答节点发帖）；
//   - sleep 结果「Slept for」→ 读线程（content 缺席 = 读模式）；
//   - 读结果「no messages yet」（空线程，等待中的正常中间态）→ 再 sleep。
//     注意该分支必须先于「Thread issue:」判定——空线程文本同样以
//     「Thread issue:N has no messages yet」开头（format_comment_list 契约）；
//   - 读结果「Thread issue:N (M messages)」（有应答）→ 裁决定稿汇报
//     （AC 锚点「需求评审记录」「定稿」+ 应答原样引用——证据走交付正流）；
//   - 其余 → D2_FINALIZE_TOOL_UNEXPECTED（UAT 断言按失败暴露）。
//
// sleep 参数名必须用 `seconds`：B 端 cluster loop 注册的是
// agent::loop_tools::SleepTool（schema required=["seconds"]），args_validator
// 先于宽容 execute 拦截；nemesis-tools 版 sleep（duration）不在集群 worker
// 工具面。实录 2026-10-14：发 `duration` 被校验拒 → UNEXPECTED → 锚点 FAIL
// → 重派烧穿线程配额 → 转人工停车（U9 首跑红根因）。
func d2FinalizeToolTurn(input, toolResult string) string {
	switch {
	case strings.Contains(toolResult, "Posted to"):
		return buildSingleToolCall("sleep", map[string]interface{}{"seconds": 3})
	case strings.Contains(toolResult, "Slept for"):
		return buildSingleToolCall("board_discuss", map[string]interface{}{
			"thread_kind": "issue",
			"thread_id":   extractTaskCardIssueNumber(input),
		})
	case strings.Contains(toolResult, "no messages yet"):
		return buildSingleToolCall("sleep", map[string]interface{}{"seconds": 3})
	case strings.Contains(toolResult, "Thread issue:"):
		return "需求评审记录：\n" + toolResult +
			"\n裁决：综合各节点职能视角的实质意见，采纳支持最多且实现风险最小的解读为胜者并定稿 docs/prd.md；" +
			"未采纳变体中的合理点以附录形式并入。评审闭环，定稿完成。"
	default:
		return "D2_FINALIZE_TOOL_UNEXPECTED"
	}
}

func (m *TestAIBoard) Delay() time.Duration { return 0 }
