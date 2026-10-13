package models

import (
	"strings"
	"testing"
)

// testai-sum-1.0 单测：两态分流 + 扫描口径（assistant 回显与
// <system-reminder> 不计）+ 摘要规则缺失可观测。agent-bench
// identifier_survival 场景的评分 oracle 依赖这些精确回复串。

func sumChatMsgs(t *testing.T, extra []Message) []Message {
	t.Helper()
	return append([]Message{
		{Role: "system", Content: "你是测试助手。"},
	}, extra...)
}

func TestSumChatSeesPlantedIds(t *testing.T) {
	m := NewTestAISum()
	got := m.Process(sumChatMsgs(t, []Message{
		{Role: "user", Content: "bench 标识符登记：issue NB-42 在主机 host-alpha-7 上，日志在 /var/log/nemesis/omega-9.log"},
	}))
	want := "IDS_SEEN:NB-42,host-alpha-7,/var/log/nemesis/omega-9.log"
	if got != want {
		t.Fatalf("聊天轮应回显全部标识符：want %q got %q", want, got)
	}
}

func TestSumChatNoIds(t *testing.T) {
	m := NewTestAISum()
	got := m.Process(sumChatMsgs(t, []Message{
		{Role: "user", Content: "普通消息，无标识符"},
	}))
	if got != "IDS_NONE" {
		t.Fatalf("无标识符聊天轮应回 IDS_NONE，实际 %q", got)
	}
}

func TestSumChatIgnoresAssistantEcho(t *testing.T) {
	m := NewTestAISum()
	// 标识符只出现在 assistant 回显里 → 不得计入（压缩后 tail 的回显
	// 不能掩盖摘要丢失）。
	got := m.Process(sumChatMsgs(t, []Message{
		{Role: "user", Content: "普通消息"},
		{Role: "assistant", Content: "IDS_SEEN:NB-42,host-alpha-7,/var/log/nemesis/omega-9.log"},
	}))
	if got != "IDS_NONE" {
		t.Fatalf("assistant 回显不应计入扫描，实际 %q", got)
	}
}

func TestSumChatIgnoresSystemReminder(t *testing.T) {
	m := NewTestAISum()
	got := m.Process(sumChatMsgs(t, []Message{
		{Role: "user", Content: "<system-reminder>NB-42 host-alpha-7 /var/log/nemesis/omega-9.log</system-reminder>普通消息"},
	}))
	if got != "IDS_NONE" {
		t.Fatalf("<system-reminder> 包装层不应计入扫描，实际 %q", got)
	}
}

func TestSumSummaryEchoesIds(t *testing.T) {
	m := NewTestAISum()
	got := m.Process([]Message{
		{Role: "system", Content: "你是测试助手。"},
		{Role: "user", Content: "bench 标识符登记：issue NB-42 在主机 host-alpha-7 上，日志在 /var/log/nemesis/omega-9.log"},
		{Role: "assistant", Content: "收到。"},
		{Role: "user", Content: "输出格式：严格按以下六节 Markdown schema 输出。\n## Goal\n标识符保全（硬性规则）：摘要中出现的标识符必须逐字保留。"},
	})
	want := "SUMMARY_IDS:NB-42,host-alpha-7,/var/log/nemesis/omega-9.log"
	if got != want {
		t.Fatalf("摘要请求应逐字回显标识符：want %q got %q", want, got)
	}
}

func TestSumSummaryRuleMissing(t *testing.T) {
	m := NewTestAISum()
	got := m.Process([]Message{
		{Role: "user", Content: "输出格式：严格按以下六节 Markdown schema 输出。\n## Goal\n（规则文本被回归删除的场景）"},
	})
	if got != "SUMMARY_RULE_MISSING" {
		t.Fatalf("摘要指令缺「标识符保全」规则应响亮报缺失，实际 %q", got)
	}
}

func TestSumSummaryNoIds(t *testing.T) {
	m := NewTestAISum()
	got := m.Process([]Message{
		{Role: "user", Content: "输出格式：严格按以下六节 Markdown schema 输出。\n标识符保全（硬性规则）：摘要中出现的标识符必须逐字保留。"},
	})
	if got != "SUMMARY_IDS_NONE" {
		t.Fatalf("摘要请求无标识符应回 SUMMARY_IDS_NONE，实际 %q", got)
	}
}

func TestSumIdentifiersDeterministicOrder(t *testing.T) {
	// CSV 顺序 = 模型表序（bench 断言依赖确定性）。
	if got := strings.Join(SumKnownIdentifiers, ","); got != "NB-42,host-alpha-7,/var/log/nemesis/omega-9.log" {
		t.Fatalf("SumKnownIdentifiers 顺序被改动：%q", got)
	}
}
