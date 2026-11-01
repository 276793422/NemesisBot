package models

// V-batch scripted models (2026-08-23):
//
//	testai-8.0 — big-output model: drives exec to produce 30KB/70KB tool
//	  results so prune (8KB-64KB band) and spill (>=64KB) can be asserted
//	  end-to-end against a real gateway; reacts to the spill locator by
//	  issuing a segmented read_file (offset/limit) so the read-back leg is
//	  exercised too.
//	testai-2.1 — parallel batch model: single response with 3 web_fetch
//	  tool_calls (U5 read-only concurrent batch), with staggered /slow
//	  delays so wall-clock ≈ max(delays) iff the batch ran concurrently.

import (
	"encoding/json"
	"fmt"
	"strings"
	"sync/atomic"
	"time"
)

// ---------------------------------------------------------------- testai-8.0

type TestAI80 struct{}

func NewTestAI80() *TestAI80 { return &TestAI80{} }

func (m *TestAI80) Name() string { return "testai-8.0" }

// Terminal replies the e2e test asserts on over WS.
const (
	pruneSeen  = "PRUNE_SEEN"
	readBackOK = "READBACK_OK"
)

func (m *TestAI80) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// Tool-result rounds: react to exec / read_file results by their markers.
	// Marker strings must match the Rust side exactly:
	//   spill locator: "已完整保存到：<path>。可用 read_file 工具按 offset/limit 分段读取…"
	//   read segment:  "[read_file 分段] path=… total_chars=… offset=…"
	//   prune marker:  "结果过长已截断"
	if last.Role == "tool" {
		switch {
		case strings.Contains(last.Content, "已完整保存到："):
			path := extractSpillPath(last.Content)
			if path == "" {
				return "SPILL_PATH_NOT_FOUND"
			}
			// Read a middle segment of the spilled file — exactly what the
			// marker promises. offset 30000 / limit 500 stays far under the
			// 64KB spill threshold so this result itself does not spill.
			return buildSingleToolCall("read_file", map[string]interface{}{
				"path":   path,
				"offset": 30000,
				"limit":  500,
			})
		case strings.Contains(last.Content, "[read_file 分段]"):
			return readBackOK
		case strings.Contains(last.Content, "结果过长已截断"):
			return pruneSeen
		default:
			return "UNEXPECTED_TOOL_RESULT"
		}
	}

	// User round: <BIG_OUT>{"command":"..."}</BIG_OUT> → exec it.
	if i := strings.Index(last.Content, "<BIG_OUT>"); i >= 0 {
		rest := last.Content[i+len("<BIG_OUT>"):]
		if j := strings.Index(rest, "</BIG_OUT>"); j >= 0 {
			var req struct {
				Command string `json:"command"`
			}
			if err := json.Unmarshal([]byte(strings.TrimSpace(rest[:j])), &req); err == nil && req.Command != "" {
				return buildSingleToolCall("exec", map[string]interface{}{
					"command": req.Command,
				})
			}
			return "BAD_BIG_OUT_TAG"
		}
	}
	return `send <BIG_OUT>{"command":"..."}</BIG_OUT> to drive a big exec output`
}

func (m *TestAI80) Delay() time.Duration { return 0 }

// extractSpillPath pulls the locator path out of the spill marker:
// …已完整保存到：<path>。可用 read_file 工具按 offset/limit 分段读取…
func extractSpillPath(toolResult string) string {
	const key = "已完整保存到："
	i := strings.Index(toolResult, key)
	if i < 0 {
		return ""
	}
	rest := toolResult[i+len(key):]
	if j := strings.Index(rest, "。"); j >= 0 {
		return strings.TrimSpace(rest[:j])
	}
	return strings.TrimSpace(rest)
}

// buildSingleToolCall is buildClusterRpcToolCall generalized to any tool.
func buildSingleToolCall(name string, args map[string]interface{}) string {
	argsJSON, _ := json.Marshal(args)
	response := ProcessedResponse{
		ToolCalls: []ToolCall{
			{
				ID:   fmt.Sprintf("call-%d", time.Now().UnixNano()),
				Type: "function",
				Function: &FunctionCall{
					Name:      name,
					Arguments: string(argsJSON),
				},
			},
		},
	}
	responseJSON, _ := json.Marshal(response)
	return string(responseJSON)
}

// ---------------------------------------------------------------- testai-2.1

type TestAI21 struct{}

func NewTestAI21() *TestAI21 { return &TestAI21{} }

func (m *TestAI21) Name() string { return "testai-2.1" }

func (m *TestAI21) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]

	// After the 3 tool results are appended the loop makes one more call whose
	// last message is a tool result — terminal reply.
	if last.Role == "tool" {
		return "PARALLEL_DONE"
	}

	// <PARALLEL>http://host:port</PARALLEL> → ONE response with 3 web_fetch
	// calls hitting /slow?secs=6|3|2: wall-clock ≈ 6s (not 11s) iff the agent
	// runs read-only batches concurrently.
	if i := strings.Index(last.Content, "<PARALLEL>"); i >= 0 {
		rest := last.Content[i+len("<PARALLEL>"):]
		if j := strings.Index(rest, "</PARALLEL>"); j >= 0 {
			base := strings.TrimRight(strings.TrimSpace(rest[:j]), "/")
			calls := make([]ToolCall, 0, 3)
			for k, secs := range []int{6, 3, 2} {
				argsJSON, _ := json.Marshal(map[string]interface{}{
					"url": fmt.Sprintf("%s/slow?secs=%d", base, secs),
				})
				calls = append(calls, ToolCall{
					ID:   fmt.Sprintf("call-p%d-%d", k, time.Now().UnixNano()),
					Type: "function",
					Function: &FunctionCall{
						Name:      "web_fetch",
						Arguments: string(argsJSON),
					},
				})
			}
			response := ProcessedResponse{ToolCalls: calls}
			responseJSON, _ := json.Marshal(response)
			return string(responseJSON)
		}
	}
	return "send <PARALLEL>http://host:port</PARALLEL> to trigger a 3-call web_fetch batch"
}

func (m *TestAI21) Delay() time.Duration { return 0 }

// TestAI90 — V2 (B4) memory auto-inject verification model.
//
// Pure conversational (no tool calls): the gateway's auto-inject
// (prefetch_memory_context) renders a "# Memory Context" section into the
// merged snapshot BEFORE the LLM call, so the model just reports what it
// actually received:
//   - last user message contains <MEM_CHECK>:
//       any message contains "# Memory Context" → "MEM_INJECT_SEEN"
//       otherwise                              → "MEM_NO_INJECT"
//
// The injected snapshot is ephemeral (a build_messages projection, never
// appended to conversation history), so scanning all messages sees exactly
// the CURRENT request — round A's snapshot is not re-sent in round B.
type TestAI90 struct{}

func NewTestAI90() *TestAI90 { return &TestAI90{} }

func (m *TestAI90) Name() string { return "testai-9.0" }

func (m *TestAI90) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]
	if last.Role == "user" && strings.Contains(last.Content, "<MEM_CHECK>") {
		for _, msg := range messages {
			if strings.Contains(msg.Content, "# Memory Context") {
				return "MEM_INJECT_SEEN"
			}
		}
		return "MEM_NO_INJECT"
	}
	return "MEM_UNRELATED_ROUND"
}

func (m *TestAI90) Delay() time.Duration { return 0 }

// TestAI91 — V4 (B3) claude CLI delegation verification model.
//
// User sends <DELEGATE> → ONE response with a single claude_code tool call
// whose prompt asks the child CLI to create cli_probe.txt (content
// SUBTASK_OK) and reply DONE. When the tool result comes back (last
// message role=tool) the model reports what the delegation actually produced:
//   result mentions done / cli_probe.txt → "DELEGATION_SUCCESS"
//   otherwise                          → "DELEGATION_FAILED"
//
// The authoritative assertions live in the Rust e2e (file on disk + request
// log); this marker stream only drives the conversation.
type TestAI91 struct{}

func NewTestAI91() *TestAI91 { return &TestAI91{} }

func (m *TestAI91) Name() string { return "testai-9.1" }

func (m *TestAI91) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	if last.Role == "tool" {
		c := strings.ToLower(last.Content)
		if strings.Contains(c, "done") || strings.Contains(c, "cli_probe.txt") {
			return "DELEGATION_SUCCESS"
		}
		return "DELEGATION_FAILED"
	}
	if strings.Contains(last.Content, "<DELEGATE>") {
		argsJSON, _ := json.Marshal(map[string]interface{}{
			"prompt": "Create a plain text file named cli_probe.txt in the current working directory with exactly this content: SUBTASK_OK. Then reply with just the word DONE.",
		})
		response := ProcessedResponse{ToolCalls: []ToolCall{{
			ID:   fmt.Sprintf("call-cc-%d", time.Now().UnixNano()),
			Type: "function",
			Function: &FunctionCall{
				Name:      "claude_code",
				Arguments: string(argsJSON),
			},
		}}}
		b, _ := json.Marshal(response)
		return string(b)
	}
	return "send <DELEGATE> to trigger a claude_code delegation"
}

func (m *TestAI91) Delay() time.Duration { return 0 }

// TestAI93 — Z1 (Phase4-d) session-fork projection verification model.
//
// Pure conversational (no tool calls). Replies with the number of user
// messages in the request: "Z1_USERS_<n>". The Rust e2e drives a 3-turn
// conversation in the source session, forks it at turn 2 while the gateway
// keeps running, then chats in the forked session — the fork's FIRST reply
// must be Z1_USERS_3 (2 copied turns + 1 new user message) and the source's
// next reply Z1_USERS_4, proving both the forked projection and that the
// source session was untouched. Authoritative assertions live in the Rust
// e2e (request_log AI.Request.md); this marker stream independently
// confirms what the provider actually received.
type TestAI93 struct{}

func NewTestAI93() *TestAI93 { return &TestAI93{} }

func (m *TestAI93) Name() string { return "testai-9.3" }

func (m *TestAI93) Process(messages []Message) string {
	// Count REAL user turns only: the gateway's merged context snapshot
	// (time/env + skills + policy) rides as an extra user-role
	// <system-reminder> message on every request — skip wrapper content so
	// the reported number is the semantic turn count.
	n := 0
	for _, msg := range messages {
		if msg.Role == "user" && !strings.Contains(msg.Content, "<system-reminder>") {
			n++
		}
	}
	return fmt.Sprintf("Z1_USERS_%d", n)
}

func (m *TestAI93) Delay() time.Duration { return 0 }

// TestAI92 — V5 (B5) inbox/steer verification model.
//
// User sends <B5_BUSY>http://host:port</B5_BUSY> → ONE response with a
// single web_fetch tool call to {base}/slow?secs=8 — an 8s tool round that
// keeps the session busy so the e2e harness can send a follow-up mid-turn.
// What the model reports afterwards reveals WHERE the follow-up landed:
//   - last message role=tool (turn 1 finishing, no steer injected)
//       → "B5_TURN1_DONE"
//   - last message role=user containing "STEER_INJECT" (the steer message
//     was injected as the newest user message right before this LLM call —
//     the tool boundary) → "B5_STEER_WITNESSED"
//   - last message role=user containing "QUEUE_TURN2" (a whole NEW turn
//     started with the queued message) → "B5_QUEUED_TURN2_OK"
//
// The authoritative assertions live in the Rust e2e (WS receipts + request
// log); this marker stream only drives the conversation.
type TestAI92 struct{}

func NewTestAI92() *TestAI92 { return &TestAI92{} }

func (m *TestAI92) Name() string { return "testai-9.2" }

func (m *TestAI92) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	if last.Role == "tool" {
		// Turn 1's slow tool round completed with nothing injected after it.
		return "B5_TURN1_DONE"
	}
	if last.Role == "user" {
		if strings.Contains(last.Content, "STEER_INJECT") {
			return "B5_STEER_WITNESSED"
		}
		if strings.Contains(last.Content, "QUEUE_TURN2") {
			return "B5_QUEUED_TURN2_OK"
		}
		if i := strings.Index(last.Content, "<B5_BUSY>"); i >= 0 {
			rest := last.Content[i+len("<B5_BUSY>"):]
			if j := strings.Index(rest, "</B5_BUSY>"); j >= 0 {
				base := strings.TrimRight(strings.TrimSpace(rest[:j]), "/")
				argsJSON, _ := json.Marshal(map[string]interface{}{
					"url": fmt.Sprintf("%s/slow?secs=8", base),
				})
				response := ProcessedResponse{ToolCalls: []ToolCall{{
					ID:   fmt.Sprintf("call-b5-%d", time.Now().UnixNano()),
					Type: "function",
					Function: &FunctionCall{
						Name:      "web_fetch",
						Arguments: string(argsJSON),
					},
				}}}
				b, _ := json.Marshal(response)
				return string(b)
			}
		}
	}
	return "send <B5_BUSY>http://host:port</B5_BUSY> to trigger a busy tool round"
}

func (m *TestAI92) Delay() time.Duration { return 0 }

// TestAI85 — devtool-upgrade 门 7 S1-S7 通用工具驱动模型。
//
// User sends <TOOL>{"name":"run_checks","args":{"scope":"test"}}</TOOL> →
// ONE response emitting exactly that tool call（一次标签 = 一次工具调用）。
// 工具结果轮（last role=tool）→ 终端文本 "TOOL_DRIVE_DONE"。未带标签 →
// 提示文本。S1 编码全流程冒烟的驱动器：run_checks / edit_file / exec /
// git / web_fetch 等任意已注册工具都能逐轮驱动——现有模型的硬编码行为
// 覆盖不到这些工具（5.0 只有 7 个 FILE_OP、8.0 只有 exec）。
type TestAI85 struct{}

func NewTestAI85() *TestAI85 { return &TestAI85{} }

func (m *TestAI85) Name() string { return "testai-8.5" }

func (m *TestAI85) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	if last.Role == "tool" {
		return "TOOL_DRIVE_DONE"
	}
	if last.Role == "user" {
		// <REPLY>text</REPLY> → 原样回显 text（多行支持）。视觉/前端冒烟用：
		// 让 assistant 消息携带任意 markdown（如 ```diff 围栏）走真实管线
		// （LLM → agent loop → chat_log → WS → 前端渲染），而不是只有固定文本。
		if i := strings.Index(last.Content, "<REPLY>"); i >= 0 {
			rest := last.Content[i+len("<REPLY>"):]
			if j := strings.Index(rest, "</REPLY>"); j >= 0 {
				return rest[:j]
			}
		}
		if i := strings.Index(last.Content, "<TOOL>"); i >= 0 {
			rest := last.Content[i+len("<TOOL>"):]
			if j := strings.Index(rest, "</TOOL>"); j >= 0 {
				var req struct {
					Name string                 `json:"name"`
					Args map[string]interface{} `json:"args"`
				}
				if err := json.Unmarshal([]byte(strings.TrimSpace(rest[:j])), &req); err == nil && req.Name != "" {
					if req.Args == nil {
						req.Args = map[string]interface{}{}
					}
					return buildSingleToolCall(req.Name, req.Args)
				}
				return "BAD_TOOL_TAG"
			}
		}
	}
	return `send <TOOL>{"name":"...","args":{...}}</TOOL> to drive any tool call`
}

func (m *TestAI85) Delay() time.Duration { return 0 }

// TestAI100 — Swarm M3 G5 交付线程验证模型。
//
// 固定输出五段结构化汇报（## 结论 / ## 交付物清单 / ## 自检 / ## 风险与未尽
// 事项 / ## 经验与坑——节名与 nemesis-board report::REPORT_FORMAT_SECTION
// 同源），不调工具。T21 用它驱动 worker dispatch：write_back_board_dispatch
// 解析出五段 → ctype='delivery' 首评 + in_progress→in_review；对照 testai-3.1
// 的无格式回复走「✅ worker 汇报完成」普通评论降级路径（T15）。
// 「经验与坑」段（M4.5）给评审 agent 的蒸馏纪律提供待审数据（真伪由评审
// 自判——本模型的锚点行不会被蒸馏入库，入库对象由 review_model 的
// <REVIEW_EXP> 槽位驱动）。
type TestAI100 struct{}

func NewTestAI100() *TestAI100 { return &TestAI100{} }

func (m *TestAI100) Name() string { return "testai-10.0" }

func (m *TestAI100) Process(messages []Message) string {
	return `## 结论
完成。T21 交付线程验证任务已按验收标准执行完毕。

## 交付物清单
- branch: uat/t21-delivery
- commits: a1b2c3d fix delivery thread, e4f5g6h add report format
- 改动文件: src/main.rs, src/report.rs

## 自检结果
对照验收标准逐条自检通过：汇报格式五段齐全，内容与任务描述一致。

## 风险与未尽事项
无。

## 经验与坑
- 坑（t29auth）：T29 汇报经验锚点：worker 侧经验段是数据，由验收 agent 判断后蒸馏。`
}

func (m *TestAI100) Delay() time.Duration { return 0 }

// ---------------------------------------------------------------- L21 stream-break 模型对
//
// testai-break-1.0 / testai-heal-1.0 —— agent-bench stream_break 场景
// （2026-11-01）的流中断模型对。实现 StreamBreaker 接口的模型在流式请求
// （req.Stream=true）且 ShouldBreak()=true 时，handler 发出 SSE 前缀
// （role 块 + partial 内容块）后**中途掐断传输**（http.ErrAbortHandler，
// chunked 帧无终止帧）——客户端读到 incomplete message 传输错误，对应
// nemesis-providers SSE 读错误 → transport_error → collect_stream_response
// Err 臂（partial 非空 → StreamInterrupted 携带 partial）。
//
// ⚠ 刻意不走「干净 EOF」形态：http_provider 的 J1 宽容层会把 EOF-无-
// [DONE] 合成终态块当成功收尾（2026-10-13 wal_crash 根修的刻意设计），
// 干净 EOF 的 partial 会被当完整回复上抛、中断根本不可见。
//
// 分工：
//   - testai-break-1.0 恒断——每次请求都中断。断言重试耗尽终局：错误
//     通知诚实上报（stream interrupted from provider 头行）+ partial
//     全文随注记框架保留。
//   - testai-heal-1.0 全局奇偶交替——奇数次请求中断、偶数次请求正常
//     完整回复。断言重试路径完整：断流后重试成功，用户拿到完整回复、
//     partial 不上抛。奇偶配对成立的前提是本模型被场景独占（bench
//     单线程逐场景串行请求），不设每会话状态。

// StreamBreaker 流中断模型契约（L21）。handler 在流式请求下先问
// ShouldBreak()——true 则走 handleStreamBreakResponse（partial 后无终态
// 收尾），false 走正常 Process 路径。
type StreamBreaker interface {
	Model
	ShouldBreak() bool
	PartialContent() string
}

// breakPartialContent 中断前发出的确定性 partial（bench 断言错误通知携带
// 全文的 marker）。刻意避开错误分类词表污染面（token/429/expired/
// unauthorized/password 等）。
const breakPartialContent = "BREAK_PARTIAL_A1：这是中断前的半截内容，"

// TestAIBreak 恒断模型。
type TestAIBreak struct{}

func NewTestAIBreak() *TestAIBreak          { return &TestAIBreak{} }
func (m *TestAIBreak) Name() string         { return "testai-break-1.0" }
func (m *TestAIBreak) Delay() time.Duration { return 0 }
func (m *TestAIBreak) ShouldBreak() bool    { return true }
func (m *TestAIBreak) PartialContent() string {
	return breakPartialContent
}

// Process 恒断路径不消费（handler 在 Process 之前短路进中断分支）；
// 非流式请求兜底返回提示文本。
func (m *TestAIBreak) Process(messages []Message) string {
	return "BREAK_UNREACHABLE（本模型只在流式请求下工作：恒中断）"
}

// TestAIHeal 奇偶交替模型：奇数次请求中断，偶数次请求完整回复。
type TestAIHeal struct {
	calls atomic.Int64
}

func NewTestAIHeal() *TestAIHeal           { return &TestAIHeal{} }
func (m *TestAIHeal) Name() string         { return "testai-heal-1.0" }
func (m *TestAIHeal) Delay() time.Duration { return 0 }

// ShouldBreak 全局请求计数奇偶：1/3/5… 中断，2/4/6… 完整。跨请求持久
// （进程生命周期）——场景串行的前提下每对请求恰好一断一成。
func (m *TestAIHeal) ShouldBreak() bool {
	return m.calls.Add(1)%2 == 1
}

func (m *TestAIHeal) PartialContent() string { return breakPartialContent }

// Process 偶数次（重试）请求的完整回复。marker 供 bench 断言「重试完整」；
// 刻意不含 BREAK_PARTIAL marker——bench 同时断言 partial 不泄漏进成功
// 回复。
func (m *TestAIHeal) Process(messages []Message) string {
	return "HEAL_RETRY_OK：重试后的完整回复"
}

// ---------------------------------------------------------------- L04 length-continuation 模型
//
// testai-lenbreak-1.0 —— agent-bench length_continuation 场景（2026-11-01）
// 的输出撞顶续行模型。两步确定性脚本：
//
//   - 轮 1（任务消息）：返回 9016 字符长正文。handler 非流式普通路径组装
//     usage.completion_tokens = len(content) ≥ agents.defaults.max_tokens
//     （bench config 8192）→ 主循环 handle_length_continuation 的 usage
//     数值判定（hit_cap）命中。刻意不伪造 finish_reason（恒 "stop"）——
//     同时覆盖「上游不给 length 信号、只有 usage 超限」的最难检测形态
//     （usage 判定优先于 finish_reason 回落，run_loop.rs §6.2）。
//   - 轮 2（最后一条 user 消息是 continue 提示 "Output limit reached"）：
//     返回短补全段 LENBREAK_PART_B OK——正常 usage 不触发，器官 7 终答。
//
// bench 断言终答含 LENBREAK_PART_B OK：续行判定失效时轮 1 长文直接成
// 终答（PART_B 永不出现 → 断言超时）；续写预算耗尽 / 空输出 guard 的
// 报错终局同样到不了 PART_B。
type TestAILenBreak struct{}

func NewTestAILenBreak() *TestAILenBreak     { return &TestAILenBreak{} }
func (m *TestAILenBreak) Name() string       { return "testai-lenbreak-1.0" }
func (m *TestAILenBreak) Delay() time.Duration { return 0 }

func (m *TestAILenBreak) Process(messages []Message) string {
	// 倒序找最后一条 user 消息：continue 提示是最新 user 轮（系统/assistant
	// 历史消息不参与判定——主循环每轮注入的时间 system 消息不影响）。
	for i := len(messages) - 1; i >= 0; i-- {
		if messages[i].Role != "user" {
			continue
		}
		if strings.Contains(messages[i].Content, "Output limit reached") {
			return "LENBREAK_PART_B OK：续写轮补全段"
		}
		return "LENBREAK_PART_A:" + strings.Repeat("x", 9000)
	}
	return "LENBREAK_NO_USER_MESSAGE"
}
