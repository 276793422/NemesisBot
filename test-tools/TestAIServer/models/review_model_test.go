package models

import (
	"strings"
	"testing"
)

// 跨轮缺口清账契约（2026-11-05 机械对账闸配套）：验收提示词「待清账缺口」
// 注入段在场时，评审输出必须带 gap_closure 逐条处置清单内编号——宿主
// check_gap_closure 机械闸缺一条即判输出无效、回灌重试，3 轮全败即
// 「评审自身失败」（mrg-a ET 实录）。桩按节锚定提取，PASS→closed 附证据，
// 非终局→open 免证据；线程历史 ❌ 评论里的【评审差距】清单不触发
// （结转重编号，多收即宿主判「虚构编号」）。

func ledgerPrompt(anchor string) []Message {
	return []Message{
		{Role: "system", Content: "你是看板评审。只输出符合格式的 JSON 对象。"},
		{Role: "user", Content: "## 任务上下文\n\n看板单：NB-7 测试\n\n## 待清账缺口（上一轮评审判定的未解决项）\n\n- G1: 缺少回归测试\n- G2: 文档未更新\n\n" + anchor + "\n\n交付物清单：FILE_EDIT_DONE 已按任务要求完成文件编辑，交付完成。"},
	}
}

func TestReviewPassClosesPromptLedgerGaps(t *testing.T) {
	out := NewTestAIReview().Process(ledgerPrompt(""))
	if !strings.Contains(out, `"gap_closure"`) {
		t.Fatalf("提示词带待清账缺口节时输出必须带 gap_closure: %s", out)
	}
	for _, id := range []string{`"id":"G1"`, `"id":"G2"`} {
		if !strings.Contains(out, id) {
			t.Fatalf("必须逐条处置 %s: %s", id, out)
		}
	}
	if strings.Count(out, `"status":"closed"`) != 2 {
		t.Fatalf("PASS 分支必须全部 closed: %s", out)
	}
	if strings.Count(out, `"evidence"`) != 2 {
		t.Fatalf("closed 必须逐条附证据（机械闸必填）: %s", out)
	}
}

func TestReviewNonFinalVerdictOpensGapsWithoutEvidence(t *testing.T) {
	out := NewTestAIReview().Process(ledgerPrompt("<REVIEW_UNSURE>"))
	if !strings.Contains(out, `"gap_closure"`) {
		t.Fatalf("UNSURE 也要对账（open 挂起）: %s", out)
	}
	if strings.Count(out, `"status":"open"`) != 2 {
		t.Fatalf("非终局判定必须 open: %s", out)
	}
	if strings.Contains(out, `"evidence"`) {
		t.Fatalf("open 免证据: %s", out)
	}
}

func TestReviewThreadLedgerDoesNotTriggerClosure(t *testing.T) {
	// 线程历史 ❌ 评论带【评审差距】清单，但本轮提示词没有待清账缺口节
	// （无结转）→ 输出不得带 gap_closure（旧格式，缺字段宽容降级）。
	out := NewTestAIReview().Process([]Message{
		{Role: "system", Content: "你是看板评审。只输出符合格式的 JSON 对象。"},
		{Role: "user", Content: "## 线程\n\n❌ 评审未通过\n\n【评审差距】（逐条编号，下一轮评审须逐条对账）\n\n- G1: 旧编号不应被本轮处置\n\n交付物清单：全部完成。"},
	})
	if strings.Contains(out, "gap_closure") {
		t.Fatalf("无待清账缺口节时不得凭历史清单出账（虚构编号会被机械闸拒）: %s", out)
	}
}

func TestReviewNoLedgerKeepsLegacyOutput(t *testing.T) {
	out := NewTestAIReview().Process([]Message{
		{Role: "user", Content: "交付物清单：全部完成。"},
	})
	if strings.Contains(out, "gap_closure") {
		t.Fatalf("无清单语境输出必须保持旧格式（omitempty 缺省）: %s", out)
	}
}
