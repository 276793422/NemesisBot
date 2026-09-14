package models

import (
	"strings"
	"testing"
)

// 冲突硬解桩（P5/T-mrg-3/4/6）：契约固定——全覆盖产出/二进制择边/锁文件
// 理由/恒败标记/评审通道兜底。

func solverPrompt(conflicts string) []Message {
	return []Message{
		{Role: "system", Content: "你是合并冲突硬解器…只允许处置列出的冲突文件"},
		{Role: "user", Content: "## 任务上下文\n\n看板单：NB-1 测试\n\n## 冲突文件\n" + conflicts},
	}
}

func TestConflictSolverCoversAllFilesWithDeterministicMerge(t *testing.T) {
	m := NewTestAIConflictSolver()
	prompt := solverPrompt("\n### src/a.rs\n\n我方（当前仓库）：\nours line\n\n对方（worker 变更集）：\ntheirs line\n\n基线（共同祖先）：\nbase line\n\n### src/b.rs\n\n我方（当前仓库）：\nA\n\n对方（worker 变更集）：\nB\n\n基线（共同祖先）：\nC\n")
	out := m.Process(prompt)
	if !strings.Contains(out, `"src/a.rs"`) || !strings.Contains(out, `"src/b.rs"`) {
		t.Fatalf("产出必须覆盖全部冲突文件: %s", out)
	}
	if !strings.Contains(out, `"action":"merge"`) {
		t.Fatalf("文本冲突必须 merge: %s", out)
	}
	if !strings.Contains(out, "resolved-by-ai-stub: src/a.rs") {
		t.Fatalf("合并内容必须确定性: %s", out)
	}
	// 合法 JSON 对象（带 resolutions 数组）。
	if !strings.HasPrefix(out, `{"resolutions":[`) {
		t.Fatalf("输出必须是 resolutions JSON: %s", out)
	}
}

func TestConflictSolverPicksSideForBinary(t *testing.T) {
	m := NewTestAIConflictSolver()
	prompt := solverPrompt("\n### assets/logo.bin\n\n（二进制文件：我方 6 字节 / 对方 7 字节。只能择边。）\n")
	out := m.Process(prompt)
	if !strings.Contains(out, `"action":"theirs"`) {
		t.Fatalf("二进制必须择边: %s", out)
	}
	if strings.Contains(out, "resolved-by-ai-stub") {
		t.Fatalf("二进制不得发明内容: %s", out)
	}
}

func TestConflictSolverReasonCarriesRegenPhrase(t *testing.T) {
	m := NewTestAIConflictSolver()
	prompt := solverPrompt("\n### Cargo.lock\n\n我方（当前仓库）：\nold\n\n对方（worker 变更集）：\nnew\n\n基线（共同祖先）：\nbase\n")
	out := m.Process(prompt)
	if !strings.Contains(out, "建议重新生成") {
		t.Fatalf("锁文件理由必须含「建议重新生成」（本桩恒带）: %s", out)
	}
}

func TestConflictSolverBadMarkerAlwaysFails(t *testing.T) {
	m := NewTestAIConflictSolver()
	prompt := solverPrompt("<CONFLICT_SOLVE_BAD>\n### src/a.rs\n\n内容")
	out := m.Process(prompt)
	if strings.Contains(out, "resolutions") {
		t.Fatalf("恒败桩不得产出 resolutions: %s", out)
	}
	// 回灌重试（错误反馈文本）后仍败。
	retry := append(prompt, Message{Role: "user", Content: "上一次输出无法解析（错误），请修正后重新输出"})
	if out2 := m.Process(retry); strings.Contains(out2, "resolutions") {
		t.Fatalf("重试后仍必须恒败: %s", out2)
	}
}

func TestConflictSolverDelegatesReviewChannel(t *testing.T) {
	m := NewTestAIConflictSolver()
	// 评审提示词（无冲突段、无坏标记）→ 委托 TestAIReview 默认 PASS。
	out := m.Process([]Message{{Role: "user", Content: "请验收以下交付……交付物清单：xxx"}})
	if !strings.Contains(out, `"verdict":"PASS"`) {
		t.Fatalf("评审通道必须出合法验收 JSON: %s", out)
	}
}
