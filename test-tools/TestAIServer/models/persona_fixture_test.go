package models

import (
	"encoding/json"
	"strings"
	"testing"
)

// TestAI14（人格生成夹具）单元测试：阶段识别 + 各阶段 JSON 契约。

// 阶段1 提示词样本（摘自 nemesis extract_prompt 的可识别特征）。
const fixtureExtractSystem = "你是简历/JD 分析师。把输入【穷尽】拆成信息单元。"

// 阶段2 user 消息样本：信息单元清单（pretty JSON 形状）。
const fixtureAuthorUser = "【原始输入】\n某 JD 全文\n\n【信息单元清单】\n" +
	`{
  "units": [
    {
      "id": "u1",
      "key_entities": [
        "Vue3",
        "TypeScript"
      ]
    },
    {
      "id": "u3",
      "key_entities": [
        "NB-SIM-MISSING"
      ]
    }
  ],
  "segments": []
}`

// 阶段3 提示词样本（摘自 audit_prompt 的可识别特征）。
const fixtureAuditSystem = "你是完整性审计员，任务是【找漏洞】。"

func fixtureMessages(parts ...string) []Message {
	msgs := make([]Message, 0, len(parts))
	for i, p := range parts {
		role := "user"
		if i == 0 {
			role = "system"
		}
		msgs = append(msgs, Message{Role: role, Content: p})
	}
	return msgs
}

func mustJSON(t *testing.T, s string) map[string]interface{} {
	t.Helper()
	var v map[string]interface{}
	if err := json.Unmarshal([]byte(s), &v); err != nil {
		t.Fatalf("输出必须是合法 JSON: %v\n%s", err, s)
	}
	return v
}

func TestAI14StageExtract(t *testing.T) {
	m := NewTestAI14()
	out := m.Process(fixtureMessages(fixtureExtractSystem, "要求：精通 Vue3 + TypeScript，熟悉 Vite 与 Webpack"))
	v := mustJSON(t, out)

	units, ok := v["units"].([]interface{})
	if !ok || len(units) != 3 {
		t.Fatalf("expected 3 units, got %v", v["units"])
	}
	segs, ok := v["segments"].([]interface{})
	if !ok || len(segs) != 2 {
		t.Fatalf("expected 2 segments, got %v", v["segments"])
	}
	// s2 是刻意被跳过的段（unit_count=0 → 段落硬缺口）。
	s2, _ := segs[1].(map[string]interface{})
	if s2["unit_count"].(float64) != 0 {
		t.Fatalf("s2 unit_count must be 0, got %v", s2["unit_count"])
	}
	// u3 是刻意缺口实体。
	u3, _ := units[2].(map[string]interface{})
	ents := u3["key_entities"].([]interface{})
	if len(ents) != 1 || ents[0] != simMissingEntity {
		t.Fatalf("u3 entities must be [%s], got %v", simMissingEntity, ents)
	}
	// u1 实体必须来自输入（Vue3 / TypeScript 属于输入里的前几个 ASCII 词）。
	u1, _ := units[0].(map[string]interface{})
	u1ents := u1["key_entities"].([]interface{})
	if len(u1ents) != 2 || u1ents[0] != "Vue3" {
		t.Fatalf("u1 entities should derive from input, got %v", u1ents)
	}
}

func TestAI14StageExtractPureChineseFallback(t *testing.T) {
	m := NewTestAI14()
	out := m.Process(fixtureMessages(fixtureExtractSystem, "负责核心业务系统的架构设计与团队管理工作，追求高质量交付。"))
	v := mustJSON(t, out)
	units := v["units"].([]interface{})
	u1, _ := units[0].(map[string]interface{})
	ents := u1["key_entities"].([]interface{})
	if len(ents) != 2 {
		t.Fatalf("纯中文输入也应兜底出 2 个实体，got %v", ents)
	}
}

func TestAI14StageAuthor(t *testing.T) {
	m := NewTestAI14()
	out := m.Process(fixtureMessages("你是人格创作师。", fixtureAuthorUser))
	v := mustJSON(t, out)

	if v["role"] != "worker" {
		t.Fatalf("role must be worker, got %v", v["role"])
	}
	for _, f := range []string{"node_name", "display_name", "identity_md", "soul_md"} {
		if s, _ := v[f].(string); strings.TrimSpace(s) == "" {
			t.Fatalf("%s 必须非空（validate 契约）", f)
		}
	}
	// expertise_md 是 required 字段但允许空串（"无则空字符串"）。
	if _, exists := v["expertise_md"]; !exists {
		t.Fatal("expertise_md 字段必须存在（schema required）")
	}
	identity, _ := v["identity_md"].(string)
	if !strings.Contains(identity, "Vue3") || !strings.Contains(identity, "TypeScript") {
		t.Fatalf("identity_md 必须嵌入清单实体（u1 Covered 前提），got: %s", identity)
	}
	if strings.Contains(identity, simMissingEntity) {
		t.Fatalf("identity_md 不得包含刻意缺口实体 %s（u3 Missing 前提）", simMissingEntity)
	}
}

func TestAI14StageAudit(t *testing.T) {
	m := NewTestAI14()
	out := m.Process(fixtureMessages(fixtureAuditSystem, "【信息单元清单】…【生成的人格产物】…"))
	v := mustJSON(t, out)
	entries, ok := v["entries"].([]interface{})
	if !ok || len(entries) != 0 {
		t.Fatalf("audit 夹具应返回空 entries（审计兜底路径），got %v", v["entries"])
	}
}

func TestAI14StageDetectionOrder(t *testing.T) {
	m := NewTestAI14()
	// 审计消息也含【信息单元清单】（audit user 拼了单元清单+产物）——审计特征必须优先。
	out := m.Process(fixtureMessages(fixtureAuditSystem, "【信息单元清单】\n[]\n\n【生成的人格产物】\n…"))
	if !strings.Contains(out, `"entries"`) {
		t.Fatalf("审计特征应优先于创作特征，got %s", out)
	}
}
