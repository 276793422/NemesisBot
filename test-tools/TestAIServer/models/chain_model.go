package models

// testai-chain-step-1.0 / testai-chain-code-1.0 —— agent-bench codemode_chain
// 场景（2026-11-01，L17 上下文经济学）的对比模型对：
//
//	同一条 3 步有依赖工具链（read chain_a.txt → write chain_b.txt（内容
//	= 读到的 CHAIN-A-VALUE 行 + "-B"）→ read chain_b.txt 验证），两种
//	驱动形态各跑一个独立会话：
//
//   - testai-chain-step-1.0（逐轮臂）：每轮一个工具调用，步进判定基于
//     上一轮 tool 结果内容（控制流 + 数据流依赖）——历史累积 3 条 tool
//     结果 + 4 次 LLM 请求。
//   - testai-chain-code-1.0（code_run 臂）：单脚本内串行 3 个 tool()，
//     完成值携带验证结果——2 次 LLM 请求。
//
// bench 断言（Rust 侧）：两臂效果面等价（chain_b.txt 内容 =
// CHAIN-A-VALUE-B）+ logs.session_usage 对比（requests：code 臂 < steps 臂；
// input_tokens 同向）+ 两臂墙钟入记分卡。
//
// 步进计数数 messages 里 role=tool 的条数（总数=当前步），不猜 write
// 回执文案。

import (
	"fmt"
	"strings"
	"time"
)

const chainFixtureLine = "CHAIN-A-VALUE"

// TestAIChainStep 逐轮臂模型。
type TestAIChainStep struct{}

func NewTestAIChainStep() *TestAIChainStep { return &TestAIChainStep{} }

func (m *TestAIChainStep) Name() string       { return "testai-chain-step-1.0" }
func (m *TestAIChainStep) Delay() time.Duration { return 0 }

func (m *TestAIChainStep) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// 数已完成的工具轮。
	toolCount := 0
	for _, msg := range messages {
		if msg.Role == "tool" {
			toolCount++
		}
	}

	if last.Role == "tool" {
		switch toolCount {
		case 1:
			// step1 结果（read chain_a.txt）→ step2：写入提取行 + "-B"
			//（数据流依赖：上一步读到的内容决定下一步写入值）。
			payload := chainLineOf(last.Content)
			if payload == "" {
				return "CHAIN_STEP1_BAD:" + clip(last.Content, 200)
			}
			return buildSingleToolCall("write_file", map[string]interface{}{
				"path":    "chain_b.txt",
				"content": payload + "-B",
			})
		case 2:
			// step2 结果（write 回执）→ step3：read 回验证。
			return buildSingleToolCall("read_file", map[string]interface{}{"path": "chain_b.txt"})
		case 3:
			// step3 结果：效果面判定。
			if strings.Contains(last.Content, chainFixtureLine+"-B") {
				return "CHAIN_STEPS_DONE"
			}
			return "CHAIN_STEP3_MISMATCH:" + clip(last.Content, 200)
		default:
			return "CHAIN_STEP_OVERFLOW:toolCount=" + fmt.Sprint(toolCount)
		}
	}

	// 任务轮 → step1。
	return buildSingleToolCall("read_file", map[string]interface{}{"path": "chain_a.txt"})
}

// TestAIChainCode code_run 臂模型。
type TestAIChainCode struct{}

func NewTestAIChainCode() *TestAIChainCode { return &TestAIChainCode{} }

func (m *TestAIChainCode) Name() string       { return "testai-chain-code-1.0" }
func (m *TestAIChainCode) Delay() time.Duration { return 0 }

func (m *TestAIChainCode) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// code_run 结果轮：完成值判定（脚本内 verify 失败也会落到 FAILED 分支）。
	if last.Role == "tool" {
		if strings.Contains(last.Content, "CHAIN_CODE_OK") {
			return "CHAIN_CODE_DONE"
		}
		return "CHAIN_CODE_FAILED:" + clip(last.Content, 800)
	}

	// 任务轮 → 单脚本编排 3 步（同链同依赖，收进一次工具调用）。
	script := `const a = tool("read_file", JSON.stringify({path: "chain_a.txt"}));` +
		`const line = a.split("\n").find(l => l.includes("` + chainFixtureLine + `"));` +
		`if (!line) throw new Error("fixture line missing: " + a);` +
		`tool("write_file", JSON.stringify({path: "chain_b.txt", content: line.trim() + "-B"}));` +
		`const b = tool("read_file", JSON.stringify({path: "chain_b.txt"}));` +
		`b.includes("` + chainFixtureLine + `-B") ? "CHAIN_CODE_OK:" + b : "CHAIN_CODE_MISMATCH:" + b`
	return buildSingleToolCall("code_run", map[string]interface{}{"script": script})
}

// chainLineOf 从工具结果文本提取含 CHAIN-A-VALUE 的行（read_file 原样
// 返回文件内容；带注记前缀的形态也能提取成功）。
func chainLineOf(content string) string {
	for _, ln := range strings.Split(content, "\n") {
		if strings.Contains(ln, chainFixtureLine) {
			return strings.TrimSpace(ln)
		}
	}
	return ""
}

// clip 结果文本截断（失败诊断带上限现场）。
func clip(s string, n int) string {
	if len(s) > n {
		return s[:n]
	}
	return s
}
