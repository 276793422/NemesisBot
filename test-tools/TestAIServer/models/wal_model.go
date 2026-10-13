package models

// testai-wal-1.0 — turn WAL 崩溃注入驱动模型（2026-10-13，L8 验收）：
//
//	用户轮（首轮）：发出 sleep{"duration":20} 长工具调用——gateway 被
//	  taskkill /F 注入崩溃时，WAL 里留下 admitted+started 两行断头记录
//	  （finished 永远等不到）。
//	用户轮（续轮）：最后一条 user 消息带「⟳ 系统续轮」标记（gateway
//	  重启后恢复链合成的续轮文案）→ 返回 WAL_RESUMED:OK 终态回复，
//	  供 e2e 断言「重启 → 补 interrupted → 续轮 → 回复落盘」全链闭合。
//	tool 轮：返回 WAL_TOOL_DONE（正常流不该走到——进程早被杀了）。

import (
	"strings"
	"time"
)

const (
	walResumeMarker = "⟳ 系统续轮"
	walResumedReply = "WAL_RESUMED:OK"
)

type TestAIWal struct{}

func NewTestAIWal() *TestAIWal { return &TestAIWal{} }

func (m *TestAIWal) Name() string { return "testai-wal-1.0" }

func (m *TestAIWal) Process(messages []Message) string {
	if len(messages) == 0 {
		return "no messages"
	}
	last := messages[len(messages)-1]

	if last.Role == "tool" {
		return "WAL_TOOL_DONE"
	}

	if strings.Contains(last.Content, walResumeMarker) {
		return walResumedReply
	}

	// 首轮：长 sleep，制造「执行中被杀」的窗口。
	// 参数名=seconds（gateway 注册的 loop_tools.SleepTool 用 seconds，
	// nemesis-tools 的同名工具用 duration 但不是注册生效的那个）。
	return buildSingleToolCall("sleep", map[string]interface{}{
		"seconds": 20,
	})
}

func (m *TestAIWal) Delay() time.Duration { return 0 }
