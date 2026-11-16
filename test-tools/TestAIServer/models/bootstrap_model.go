package models

import (
	"strings"
	"time"
)

// TestAIBootstrap — agent 自举插件（项6）E2E 桩（agent-bench plugin_bootstrap 场景）。
//
// 单模型对话机，按 goal 内嵌 marker 分三条流：
//
//	BOOTFLOW:HAPPY  plugin_search → plugin_propose（bench 以 WSAPI
//	                approval.respond 应人批审批卡）→ 工具结果含
//	                installed_and_loaded（热装语义）→ 立即调用
//	                plugin.textstat.textstat（同会话新工具可用性）→
//	                BOOTFLOW:HAPPY-DONE:<工具结果>
//	BOOTFLOW:WL     plugin_search → plugin_propose → allowed_sources
//	                白名单外结构化拒绝（「白名单」）→ BOOTFLOW:WL-REFUSED
//	BOOTFLOW:ESTOP  急停期间（bench 先 estop.trigger）：run_loop 轮顶急停闸
//	                直接终局「已急停 (E-STOP)」（模型不被调用）——bench 直接
//	                断言该回复；本模型保留 ESTOP-BLOCKED 兜底臂（若急停恰好
//	                落在首轮 LLM 之后，工具分发层拒绝含 ESTOP 字样）。
//
// 工具结果嗅探（顺序敏感：成功态在前，避免 propose 成功结果里的 slug
// 键被搜索结果分支误捕）：
//   - "installed_and_loaded" / "已热装载" → 调 textstat
//   - "chars"（textstat 输出 {"chars":..}）→ 终局
//   - "白名单" → WL-REFUSED
//   - "急停" / "ESTOP" → ESTOP-BLOCKED
//   - 含 "slug"（搜索结果行）→ propose
type TestAIBootstrap struct{}

func NewTestAIBootstrap() *TestAIBootstrap { return &TestAIBootstrap{} }

func (m *TestAIBootstrap) Name() string  { return "testai-bootstrap-1.0" }
func (m *TestAIBootstrap) Delay() time.Duration { return 0 }

func (m *TestAIBootstrap) Process(messages []Message) string {
	if len(messages) == 0 {
		return ""
	}
	last := messages[len(messages)-1]
	switch last.Role {
	case "user":
		// 首轮（goal）：先检索源里有什么（自举第一步 = 发现）。
		return buildSingleToolCall("plugin_search", map[string]interface{}{"query": "textstat"})
	case "tool":
		return m.toolTurn(last.Content)
	}
	return "BOOTFLOW:IDLE"
}

func (m *TestAIBootstrap) toolTurn(content string) string {
	switch {
	case strings.Contains(content, "installed_and_loaded") || strings.Contains(content, "已热装载"):
		// 提案被批准且已热装载——同一会话立刻用上新工具。
		return buildSingleToolCall("plugin.textstat.textstat", map[string]interface{}{
			"text": "bootstrap e2e hello",
		})
	case strings.Contains(content, "chars"):
		// textstat 真实执行结果（{"chars":N,...}）——全链收尾。
		return "BOOTFLOW:HAPPY-DONE:" + content
	case strings.Contains(content, "白名单"):
		return "BOOTFLOW:WL-REFUSED"
	case strings.Contains(content, "急停") || strings.Contains(content, "ESTOP"):
		return "BOOTFLOW:ESTOP-BLOCKED"
	}
	if strings.Contains(content, "slug") {
		// 搜索命中（结果行带 slug 字段）→ 提案安装（论证段上审批卡）。
		return buildSingleToolCall("plugin_propose", map[string]interface{}{
			"slug": "textstat",
			"reason": "能力缺口：需要本地文本统计工具（字符/词/行计数）——" +
				"bootstrap-src 源中的 TextStat 插件恰好补上，请批准安装",
		})
	}
	return "BOOTFLOW:STUCK:" + content
}
