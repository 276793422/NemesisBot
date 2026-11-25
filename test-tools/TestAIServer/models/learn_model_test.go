package models

import (
	"strings"
	"testing"
)

// TestAILearn 阶段检测单元测试。核心回归：transcript 累积协议下，阶段 N 的
// prompt 同时含全部 ≤N 的标记（内嵌前几轮往返原文）——阶段检测必须逆序取
// 最大命中号，否则阶段 ≥2 全部误路由回阶段 1（2026-11-24 真机彩排实证）。

func learnMsgs(content string) []Message {
	return []Message{{Role: "user", Content: content}}
}

func TestAILearnNameAndDelay(t *testing.T) {
	m := NewTestAILearn()
	if m.Name() != "testai-learn-1.0" || m.Delay() != 0 {
		t.Fatalf("unexpected name/delay: %s %v", m.Name(), m.Delay())
	}
}

func TestAILearnPhase1Proposals(t *testing.T) {
	m := NewTestAILearn()
	out := m.Process(learnMsgs("# 学习对话阶段 1/4：提议\n\n以下是本批学习证据包\n\n请判断"))
	if !strings.Contains(out, `"proposals"`) || !strings.Contains(out, `"id":"l1"`) {
		t.Fatalf("阶段 1 应返回 proposals 脚本: %s", out)
	}
}

// 回归锁：阶段 2 的 prompt 内嵌阶段 1 往返（标记 1 出现在消息中部），
// 检测必须命中阶段 2（返回 items/insight 形态），不得被阶段 1 分支截胡。
func TestAILearnPhase2WithEmbeddedPhase1Transcript(t *testing.T) {
	m := NewTestAILearn()
	embedded := "# 学习对话阶段 1/4：提议\n\n（前几轮往返原文……）\n### assistant\n{\"proposals\":[{\"id\":\"l1\"}]}"
	phase2 := "# 学习对话阶段 2/4：定形\n\n以下是前几轮对话记录：\n" + embedded + "\n\n对每个 proposal 决定交付形态。"
	out := m.Process(learnMsgs(phase2))
	if !strings.Contains(out, `"form":"insight"`) {
		t.Fatalf("阶段 2（内嵌阶段 1 往返）应返回定形脚本，不得误路由回阶段 1: %s", out)
	}
}

// 同理：阶段 4 的 prompt 内嵌阶段 1/2/3 全部往返，检测必须命中阶段 4。
func TestAILearnPhase4WithEmbeddedEarlierTranscripts(t *testing.T) {
	m := NewTestAILearn()
	phase4 := "# 学习对话阶段 4/4：定稿\n\n以下是前几轮对话记录：\n" +
		"# 学习对话阶段 1/4：提议\n……\n# 学习对话阶段 2/4：定形\n……\n# 学习对话阶段 3/4：对账\n……\n\n请定稿。"
	out := m.Process(learnMsgs(phase4))
	if !strings.Contains(out, `"insights"`) {
		t.Fatalf("阶段 4（内嵌更早往返）应返回定稿脚本: %s", out)
	}
}

func TestAILearnEmptyProposalsNothing(t *testing.T) {
	m := NewTestAILearn()
	out := m.Process(learnMsgs("# 学习对话阶段 1/4：提议\n\n证据包 <LEARN_NOTHING>\n请判断"))
	if !strings.Contains(out, `"proposals":[]`) || !strings.Contains(out, "empty_reason") {
		t.Fatalf("<LEARN_NOTHING> 应返回空 proposals + 理由: %s", out)
	}
}

func TestAILearnGarbageProse(t *testing.T) {
	m := NewTestAILearn()
	out := m.Process(learnMsgs("# 学习对话阶段 1/4：提议\n\n<LEARN_GARBAGE>"))
	if strings.HasPrefix(strings.TrimSpace(out), "{") {
		t.Fatalf("<LEARN_GARBAGE> 应返回非 JSON 散文: %s", out)
	}
}

func TestAILearnOutOfScript(t *testing.T) {
	m := NewTestAILearn()
	out := m.Process(learnMsgs("普通消息，无阶段标记"))
	if !strings.Contains(out, "out of script") {
		t.Fatalf("无标记应返回 out of script 兜底: %s", out)
	}
}
