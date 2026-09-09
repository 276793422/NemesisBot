package models

import (
	"strings"
	"testing"
)

// TestAIBoard 分流冒烟：三条链路（planner/reviewer/普通讨论）按 system
// prompt 特征词命中对应桩。真机验收 G1（拆解）/G6（验收）同链路。
func TestBoardRouting(t *testing.T) {
	m := NewTestAIBoard()
	cases := []struct {
		name    string
		system  string
		wantSub string
	}{
		{"planner 特征→计划 JSON", "你是 NemesisBot 看板的任务拆解规划器（planner）。", `"title"`},
		{"reviewer 特征→verdict JSON", "你是 NemesisBot 看板的验收 agent（reviewer）。", `"verdict"`},
		{"reviewer FAIL 锚点继承", "验收 agent（reviewer）。<REVIEW_FAIL>", `FAIL`},
		{"普通→文本", "", "收到"},
	}
	for _, c := range cases {
		msgs := []Message{{Role: "user", Content: "任务"}}
		if c.system != "" {
			msgs = append([]Message{{Role: "system", Content: c.system}}, msgs...)
		}
		got := m.Process(msgs)
		if !strings.Contains(got, c.wantSub) {
			t.Errorf("%s: 输出缺 %q，实际 %.80s", c.name, c.wantSub, got)
		}
	}
}
