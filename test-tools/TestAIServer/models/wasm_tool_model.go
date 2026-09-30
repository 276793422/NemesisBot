package models

// testai-wasm-1.0 — WASM 插件工具驱动模型（2026-09-30，插件完整链路测试）：
//
//	用户轮：消息带 <TOOL>{"name":"<工具全名>","args":{...}}</TOOL> 标记 →
//	  发出对应 tool_call（任意工具名，含 plugin.<slug>.<base> 形态的
//	  WASM 插件工具——既有脚本模型只会发 exec/read_file 等内置名）。
//	tool 轮：返回 "WASM_TOOL_RESULT:" + 工具结果全文（截 800 字符），
//	  使最终回复携带工具结果原文，供上游断言（出站凭据脱敏、工作区
//	  围栏拒绝文本、is_error 错误文本）。
//	无标记用户轮 → 提示语。
//
// 截断取 800：fixture 文件控制在几百字节，preview 全文必然在截断线内，
// 「最终回复不含凭据原文」的断言不会因截断而空转。

import (
	"encoding/json"
	"strings"
	"time"
)

const (
	wasmToolMarker     = "<TOOL>"
	wasmToolMarkerEnd  = "</TOOL>"
	wasmToolResultCap  = 800
	wasmToolResultMark = "WASM_TOOL_RESULT:"
)

type TestAIWasmTool struct{}

func NewTestAIWasmTool() *TestAIWasmTool { return &TestAIWasmTool{} }

func (m *TestAIWasmTool) Name() string { return "testai-wasm-1.0" }

func (m *TestAIWasmTool) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	// tool 轮：原样回带工具结果（bridge 侧已完成出站复扫，模型拿到的是
	// 脱敏后的文本——回带即证明）。
	if last.Role == "tool" {
		content := last.Content
		if len(content) > wasmToolResultCap {
			content = content[:wasmToolResultCap]
		}
		return wasmToolResultMark + content
	}

	// 用户轮：<TOOL>{...}</TOOL> → 任意工具调用
	if i := strings.Index(last.Content, wasmToolMarker); i >= 0 {
		rest := last.Content[i+len(wasmToolMarker):]
		if j := strings.Index(rest, wasmToolMarkerEnd); j >= 0 {
			var req struct {
				Name string                 `json:"name"`
				Args map[string]interface{} `json:"args"`
			}
			if err := json.Unmarshal([]byte(strings.TrimSpace(rest[:j])), &req); err == nil && req.Name != "" {
				return buildSingleToolCall(req.Name, req.Args)
			}
			return "BAD_TOOL_TAG"
		}
	}
	return `send <TOOL>{"name":"...","args":{...}}</TOOL> to drive a tool call`
}

func (m *TestAIWasmTool) Delay() time.Duration { return 0 }
