package models

import (
	"strings"
	"testing"
)

// TestAI31 看板任务卡分级冒烟（2026-09-13 T30①/T15/T29③ 三方实证）：
// 带 [CHECK] 锚点的任务卡 → boardAckText（不回显）；无锚点任务卡与普通
// 消息 → 终端回显契约（T15 marker / T29③ 经验注入断言依赖整卡回显）。
func TestAI31BoardCardGuard(t *testing.T) {
	m := NewTestAI31()
	card := "# 看板任务 NB-1\n\n## 验收标准\n产出说明文本。\n[CHECK] re:集群协作状态正常\n"
	plainCard := "# 看板任务 NB-2\n\n## 验收标准\n回显包含经验段。\n团队过往经验\nT29 经验锚点\n"
	cases := []struct {
		name    string
		content string
		wantAck bool
	}{
		{"锚点任务卡→固定汇报", card, true},
		{"无锚点任务卡→整卡回显", plainCard, false},
		{"普通消息→回显", "hello marker", false},
	}
	for _, c := range cases {
		got := m.Process([]Message{{Role: "user", Content: c.content}})
		if c.wantAck && got != boardAckText {
			t.Errorf("%s: 应返回固定汇报，实际 %.80s", c.name, got)
		}
		if !c.wantAck && got != c.content {
			t.Errorf("%s: 应整卡回显，实际 %.80s", c.name, got)
		}
		if strings.Contains(got, "<PEER_CHAT>") {
			t.Errorf("%s: 不应触发 peer_chat 路由", c.name)
		}
	}
}
