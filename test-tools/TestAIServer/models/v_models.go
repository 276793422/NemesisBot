package models

// V-batch scripted models (dsh-closure goal V1/V3, 2026-08-23):
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

// TestAI91 — V4 (B3) Claude Code delegation verification model.
//
// User sends <CC_DELEGATE> → ONE response with a single claude_code tool call
// whose prompt asks the child CLI to create cc_probe.txt (content
// CC_SUBTASK_OK) and reply DONE. When the tool result comes back (last
// message role=tool) the model reports what the delegation actually produced:
//   result mentions done / cc_probe.txt → "CC_DELEGATION_SUCCESS"
//   otherwise                          → "CC_DELEGATION_FAILED"
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
		if strings.Contains(c, "done") || strings.Contains(c, "cc_probe.txt") {
			return "CC_DELEGATION_SUCCESS"
		}
		return "CC_DELEGATION_FAILED"
	}
	if strings.Contains(last.Content, "<CC_DELEGATE>") {
		argsJSON, _ := json.Marshal(map[string]interface{}{
			"prompt": "Create a plain text file named cc_probe.txt in the current working directory with exactly this content: CC_SUBTASK_OK. Then reply with just the word DONE.",
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
	return "send <CC_DELEGATE> to trigger a claude_code delegation"
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
