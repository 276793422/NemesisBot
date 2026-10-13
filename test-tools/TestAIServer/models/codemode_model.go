package models

// testai-codemode-1.0 — code_run 脚本编排驱动模型（2026-10-13，T2 codemode）：
//
//	用户轮：消息带 <CODEMODE> 标记 → 发出单个 code_run 工具调用，脚本
//	  读 codemode_in.txt（上游播种 "1"）→ 数值 +41 → 写 codemode_out.txt
//	  （"42"）→ 完成值 "CODEMODE_OK:42"。读-改-写三步编排收进**一次**
//	  工具调用，正是 codemode 的经济学动机面。
//	tool 轮：完成值含 "CODEMODE_OK:42" → "CODEMODE_DONE:42"（脚本侧
//	  成功）；否则 "CODEMODE_FAILED:" + 结果全文（脚本错误/拒绝文本
//	  原样上抛，供上游失败面断言）。
//	无标记用户轮 → 提示语。
//
// 权威断言在 Rust 侧 bench 场景（codemode_out.txt 内容 = "42" 效果面 +
// 最终回复 marker）；本模型只驱动会话。

import (
	"strings"
	"time"
)

const (
	codemodeMarker   = "<CODEMODE>"
	codemodeOKMark   = "CODEMODE_OK:42"
	codemodeDoneMark = "CODEMODE_DONE:42"
)

type TestAICodeMode struct{}

func NewTestAICodeMode() *TestAICodeMode { return &TestAICodeMode{} }

func (m *TestAICodeMode) Name() string { return "testai-codemode-1.0" }

func (m *TestAICodeMode) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// tool 轮：完成值判定（成功 marker 精确匹配——脚本内数值算错也会
	// 落到 FAILED 分支，不只测「跑完了」）。
	if last.Role == "tool" {
		if strings.Contains(last.Content, codemodeOKMark) {
			return codemodeDoneMark
		}
		content := last.Content
		if len(content) > 800 {
			content = content[:800]
		}
		return "CODEMODE_FAILED:" + content
	}

	// 用户轮：<CODEMODE> → code_run 编排调用。
	if strings.Contains(last.Content, codemodeMarker) {
		script := `const a = tool("read_file", JSON.stringify({path: "codemode_in.txt"}));` +
			`const n = Number(a.trim()) + 41;` +
			`tool("write_file", JSON.stringify({path: "codemode_out.txt", content: String(n)}));` +
			`"CODEMODE_OK:" + n`
		return buildSingleToolCall("code_run", map[string]interface{}{"script": script})
	}
	return `send <CODEMODE> to trigger a code_run orchestration`
}

func (m *TestAICodeMode) Delay() time.Duration { return 0 }
