package models

// testai-docslift-1.0 — 工作区指令水合（docs lift）驱动模型（2026-10-13，
// T3 生态与地基 L10 docs_lift 场景）：
//
//	扫描**全部**请求消息（含 system——AGENTS.md 指令链经 merged
//	<system-reminder> 注入消息上行，角色不限于 user）中的
//	DOCSLIFT_PROTOCOL_V1 标记：
//	  命中 n 次 → "DOCS_OK:<n>"（工作区指令文件真进了 LLM 请求）；
//	  未命中   → "DOCS_MISSING:1"（指令未水合——臂 A 的期望）。
//
// 探针用户消息本身不含标记（marker 只能经 AGENTS.md 水合到达），
// 权威断言在 Rust 侧 bench 场景（双臂 A/B 对：无文件→MISSING、
// 落盘文件→OK）。工作区指令链每轮 build 从磁盘重读（messages.rs
// H5 合并注入），本模型零状态、纯函数式判定。

import (
	"fmt"
	"strings"
	"time"
)

const (
	docsLiftMarker      = "DOCSLIFT_PROTOCOL_V1"
	docsLiftMissingMark = "DOCS_MISSING:1"
)

type TestAIDocsLift struct{}

func NewTestAIDocsLift() *TestAIDocsLift { return &TestAIDocsLift{} }

func (m *TestAIDocsLift) Name() string { return "testai-docslift-1.0" }

func (m *TestAIDocsLift) Process(messages []Message) string {
	count := 0
	for _, msg := range messages {
		count += strings.Count(msg.Content, docsLiftMarker)
	}
	if count > 0 {
		return fmt.Sprintf("DOCS_OK:%d", count)
	}
	return docsLiftMissingMark
}

func (m *TestAIDocsLift) Delay() time.Duration { return 0 }
