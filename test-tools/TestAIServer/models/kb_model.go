package models

// testai-kb-1.0 — 知识库重定向（天工系统 v1）UAT 模型（2026-11-26）：
//
// 驱动协议（用户消息内嵌）：
//	<KB_TEST>{"op":"read","path":"..."}</KB_TEST>
//	<KB_TEST>{"op":"write","path":"...","content":"..."}</KB_TEST>
//
// 行为：
//  1. 触发轮按 op 发单个 read_file / write_file 工具调用（路径原样透传
//     ——知识库重定向裁决在 gateway 侧，模型对镜像一无所知，这正是被测
//     语义：LLM 只见项目视角路径）；
//  2. 全消息扫描「# 天工」存在性声明（gateway build_messages 注入面，
//     pointer 模式声明文本首行）→ 记录 KB_DECL；
//  3. 工具结果轮返回终端回复
//     "KB_DECL=<true|false> KB_RESULT=<工具结果前 400 字符>"
//     ——e2e 断言 KB_DECL=true（声明注入装配链路真实生效）+
//     KB_RESULT 含镜像内容（路径重定向真实生效）。
//
// 结果截断 400 字符：镜像 fixture 内容很短，截断只为防意外大结果冲爆
// WS 回复。

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

const kbDeclMarker = "# 天工"

type TestAIKb struct{}

func NewTestAIKb() *TestAIKb { return &TestAIKb{} }

func (m *TestAIKb) Name() string { return "testai-kb-1.0" }

func (m *TestAIKb) Delay() time.Duration { return 0 }

func (m *TestAIKb) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// 声明注入检测：任意消息出现「# 天工」即算看到（不挑注入形态）。
	declSeen := false
	for _, msg := range messages {
		if strings.Contains(msg.Content, kbDeclMarker) {
			declSeen = true
			break
		}
	}

	// 工具结果轮 → 终端回复（带声明检测结果）。
	if last.Role == "tool" {
		result := last.Content
		if len(result) > 400 {
			r := []rune(result)
			if len(r) > 400 {
				result = string(r[:400])
			}
		}
		return fmt.Sprintf("KB_DECL=%t KB_RESULT=%s", declSeen, result)
	}

	// 触发轮：解析 <KB_TEST>{...}</KB_TEST>。
	if last.Role != "user" {
		return "expect user message with <KB_TEST>"
	}
	i := strings.Index(last.Content, "<KB_TEST>")
	if i < 0 {
		if declSeen {
			return "KB_DECL=true KB_RESULT=(no trigger tag)"
		}
		return `send <KB_TEST>{"op":"read","path":"..."}</KB_TEST>`
	}
	rest := last.Content[i+len("<KB_TEST>"):]
	j := strings.Index(rest, "</KB_TEST>")
	if j < 0 {
		return "BAD_KB_TEST_TAG"
	}
	var req struct {
		Op      string `json:"op"`
		Path    string `json:"path"`
		Content string `json:"content"`
		OldText string `json:"old_text"`
		NewText string `json:"new_text"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(rest[:j])), &req); err != nil || req.Path == "" {
		return "BAD_KB_TEST_JSON"
	}
	switch req.Op {
	case "read":
		return buildSingleToolCall("read_file", map[string]interface{}{
			"path": req.Path,
		})
	case "write":
		return buildSingleToolCall("write_file", map[string]interface{}{
			"path":    req.Path,
			"content": req.Content,
		})
	case "edit":
		// edit 轮（横扫补强 2026-11-26）：old/new 由 trigger 带参，
		// 路径原样透传——重定向裁决在 gateway 侧。
		return buildSingleToolCall("edit_file", map[string]interface{}{
			"path":     req.Path,
			"old_text": req.OldText,
			"new_text": req.NewText,
		})
	default:
		return "BAD_KB_OP"
	}
}
