package models

import (
	"encoding/json"
	"strings"
	"testing"
)

// TestAIBoard 分流冒烟：三条链路（planner/reviewer/普通讨论）按 system
// prompt 特征词命中对应桩。真机验收 G1（拆解）/G6（验收）同链路。
func TestBoardRouting(t *testing.T) {
	m := NewTestAIBoard()
	cases := []struct {
		name    string
		system  string
		wantSub string
	}{
		{"planner 特征→计划 JSON", "你是 NemesisBot 看板的任务拆解规划器（planner）。", `"title"`},
		{"reviewer 特征→verdict JSON", "你是 NemesisBot 看板的验收 agent（reviewer）。", `"verdict"`},
		{"reviewer FAIL 锚点继承", "验收 agent（reviewer）。<REVIEW_FAIL>", `FAIL`},
		{"普通→文本", "", "收到"},
	}
	for _, c := range cases {
		msgs := []Message{{Role: "user", Content: "任务"}}
		if c.system != "" {
			msgs = append([]Message{{Role: "system", Content: c.system}}, msgs...)
		}
		got := m.Process(msgs)
		if !strings.Contains(got, c.wantSub) {
			t.Errorf("%s: 输出缺 %q，实际 %.80s", c.name, c.wantSub, got)
		}
	}
}

// mustToolCall 断言 raw 是恰好 1 个工具调用的响应 JSON，返回 (工具名, args)。
func mustToolCall(t *testing.T, raw string) (string, string) {
	t.Helper()
	var resp ProcessedResponse
	if err := json.Unmarshal([]byte(raw), &resp); err != nil {
		t.Fatalf("响应不是合法 JSON: %v，原文 %.200s", err, raw)
	}
	if len(resp.ToolCalls) != 1 || resp.ToolCalls[0].Function == nil {
		t.Fatalf("期望恰好 1 个工具调用，实际 %.200s", raw)
	}
	return resp.ToolCalls[0].Function.Name, resp.ToolCalls[0].Function.Arguments
}

// <BOARD_ISSUE> 标记三步对话机：建单 → plan → 收尾（全自动流转 P3 IT 驱动）。
func TestBoardIssueMarkerFlow(t *testing.T) {
	m := NewTestAIBoard()

	// 轮1：user 带 <BOARD_ISSUE>标题</BOARD_ISSUE> → board_issue create。
	r1 := m.Process([]Message{
		{Role: "system", Content: "你是 master。"},
		{Role: "user", Content: "<BOARD_ISSUE>修复登录页崩溃</BOARD_ISSUE>"},
	})
	name, args := mustToolCall(t, r1)
	if name != "board_issue" || !strings.Contains(args, `"subcommand":"create"`) || !strings.Contains(args, `修复登录页崩溃`) {
		t.Fatalf("轮1 期望 board_issue create（标题透传），实际 name=%s args=%s", name, args)
	}

	// 轮2：create 工具结果（「已建单 NB-7」锚点）→ board_issue plan。
	r2 := m.Process([]Message{
		{Role: "system", Content: "你是 master。"},
		{Role: "user", Content: "<BOARD_ISSUE>修复登录页崩溃</BOARD_ISSUE>"},
		{Role: "tool", Content: "已建单 NB-7：修复登录页崩溃\n状态 todo · 优先级 1 · 项目 None\n可继续用 board_issue plan 对它做 AI 拆解。"},
	})
	name, args = mustToolCall(t, r2)
	if name != "board_issue" || !strings.Contains(args, `"subcommand":"plan"`) || !strings.Contains(args, `"issue":"NB-7"`) {
		t.Fatalf("轮2 期望 board_issue plan（NB-7），实际 name=%s args=%s", name, args)
	}

	// 轮3：plan 工具结果（plan 链 JSON，plan_id 锚点）→ 收尾标记。
	r3 := m.Process([]Message{
		{Role: "system", Content: "你是 master。"},
		{Role: "user", Content: "<BOARD_ISSUE>修复登录页崩溃</BOARD_ISSUE>"},
		{Role: "tool", Content: "已建单 NB-7：修复登录页崩溃"},
		{Role: "tool", Content: "{\n  \"status\": \"planned\",\n  \"plan_id\": \"plan-abc\",\n  \"subs\": 3\n}"},
	})
	if !strings.Contains(r3, "BOARD_ISSUE_FLOW_DONE") {
		t.Fatalf("轮3 输出缺收尾标记，实际 %.200s", r3)
	}
}

// 标记解析边界：缺闭合标签 / 空标题 / 无标记不误触发 / 异常工具结果诚实暴露。
func TestBoardIssueMarkerEdgeCases(t *testing.T) {
	m := NewTestAIBoard()

	// 缺闭合标签 → 取剩余全文当标题。
	r := m.Process([]Message{{Role: "user", Content: "<BOARD_ISSUE>只写了一半"}})
	name, args := mustToolCall(t, r)
	if name != "board_issue" || !strings.Contains(args, `只写了一半`) {
		t.Fatalf("缺闭合标签应取剩余全文当标题，实际 name=%s args=%s", name, args)
	}

	// 空标题 → 默认标题。
	r = m.Process([]Message{{Role: "user", Content: "<BOARD_ISSUE></BOARD_ISSUE>"}})
	_, args = mustToolCall(t, r)
	if !strings.Contains(args, `"title":"自动流转测试单"`) {
		t.Fatalf("空标题应落默认标题，实际 args=%s", args)
	}

	// 历史里带标记但最后一条是无标记用户消息 → 固定文本（不重复建单）。
	r = m.Process([]Message{
		{Role: "user", Content: "<BOARD_ISSUE>旧任务</BOARD_ISSUE>"},
		{Role: "tool", Content: "已建单 NB-2：旧任务"},
		{Role: "tool", Content: `{"plan_id": "plan-x"}`},
		{Role: "user", Content: "现在帮我看看集群状态"},
	})
	if !strings.Contains(r, boardAckText) {
		t.Fatalf("后续无标记消息应回固定文本，实际 %.200s", r)
	}

	// 异常工具结果（无锚点）→ BOARD_ISSUE_TOOL_UNEXPECTED 诚实暴露。
	r = m.Process([]Message{
		{Role: "user", Content: "<BOARD_ISSUE>任务</BOARD_ISSUE>"},
		{Role: "tool", Content: "board 服务未就绪（moderator agent 未运行），请稍后再试"},
	})
	if !strings.Contains(r, "BOARD_ISSUE_TOOL_UNEXPECTED") {
		t.Fatalf("异常工具结果应落 UNEXPECTED，实际 %.200s", r)
	}

	// extractCreatedIssueNumber：「已建单 NB-」后无数字 → 空串。
	if got := extractCreatedIssueNumber("已建单 NB-：x"); got != "" {
		t.Fatalf("无数字单号应返回空串，实际 %q", got)
	}
}
