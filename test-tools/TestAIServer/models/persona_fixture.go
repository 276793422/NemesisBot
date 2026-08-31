package models

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"time"
)

// TestAI14 - 人格生成三阶段夹具（W1.5 D①：覆盖率报告机制验证）。
//
// 模拟一个"能力一般、有系统性缺口"的模型，按提示词特征识别管线阶段并给出
// 确定性 JSON 回复（nemesis 的 chat_json 走内容兜底路径解析）：
//
//   - 阶段1（信息单元提取，system 含「简历/JD 分析师」）：从输入抽取 ASCII
//     实体词，产出 3 单元/2 段落。其中 u3 的 key_entities 是刻意不出现在
//     产物里的 NB-SIM-MISSING（→ 程序校验 Missing）；s2 段 unit_count=0
//     （→ 段落硬缺口）。
//   - 阶段2（人格创作，user 含【信息单元清单】）：从单元清单里抄
//     key_entities 嵌进产物文本（跳过 NB-SIM-MISSING）→ u1/u2 Covered、
//     u3 Missing。
//   - 阶段3（对抗审计，system 含「完整性审计员/找漏洞」）：返回空 entries
//     （走"审计兜底"路径）。
//
// 预期管线结局：max_attempts 次尝试均无法补齐 u3/s2，最终【不硬 Err】，
// 返回带缺口报告的 pkg（missing≥1 + segment_gaps≥1）——验证
// "覆盖校验失败返回报告而非失败"的机制。质量终审挂起（goal 外长期项）。
type TestAI14 struct{}

func NewTestAI14() *TestAI14 {
	return &TestAI14{}
}

func (m *TestAI14) Name() string {
	return "testai-1.4"
}

func (m *TestAI14) Delay() time.Duration {
	return 0
}

// 刻意缺口实体：不会出现在任何产物文本里 → 程序字面校验必判 Missing。
const simMissingEntity = "NB-SIM-MISSING"

var (
	// asciiTokenRe 抽取输入里的英文技术词（Vue3 / TypeScript / CI/CD…）。
	asciiTokenRe = regexp.MustCompile(`[A-Za-z][A-Za-z0-9+#.]{1,19}`)
	// keyEntRe 在阶段2 的 user 消息（pretty JSON）里圈出每个 key_entities 数组。
	keyEntRe = regexp.MustCompile(`(?s)"key_entities":\s*\[(.*?)\]`)
	// strRe 从数组片段里抠出各个实体字符串。
	strRe = regexp.MustCompile(`"([^"]+)"`)
)

func (m *TestAI14) Process(messages []Message) string {
	joined := make([]string, 0, len(messages))
	last := ""
	for _, msg := range messages {
		joined = append(joined, msg.Content)
		if strings.TrimSpace(msg.Content) != "" {
			last = msg.Content
		}
	}
	all := strings.Join(joined, "\n")

	switch {
	case strings.Contains(all, "完整性审计员") || strings.Contains(all, "找漏洞"):
		return `{"entries":[]}`
	case strings.Contains(all, "【信息单元清单】"):
		return m.author(last)
	default:
		return m.extract(last)
	}
}

// extract 阶段1：确定性产出带缺口的单元清单。
func (m *TestAI14) extract(input string) string {
	seen := map[string]bool{}
	var entities []string
	for _, tok := range asciiTokenRe.FindAllString(input, -1) {
		t := strings.TrimRight(tok, ".")
		if len(t) < 2 || seen[t] {
			continue
		}
		seen[t] = true
		entities = append(entities, t)
		if len(entities) == 4 {
			break
		}
	}
	for len(entities) < 4 {
		// 纯中文输入兜底：实体词只要求能在阶段2 产物里字面出现即可。
		entities = append(entities, fmt.Sprintf("SIM-ENT-%d", len(entities)+1))
	}

	clip := input
	if r := []rune(clip); len(r) > 40 {
		clip = string(r[:40]) + "…"
	}
	out := map[string]interface{}{
		"units": []interface{}{
			map[string]interface{}{
				"id": "u1", "content": "模拟单元1（" + clip + "）", "unit_type": "skill",
				"relevance": "high", "disposition": "identity", "key_entities": entities[:2],
			},
			map[string]interface{}{
				"id": "u2", "content": "模拟单元2（工程方法论）", "unit_type": "methodology",
				"relevance": "high", "disposition": "soul", "key_entities": entities[2:],
			},
			map[string]interface{}{
				"id": "u3", "content": "模拟单元3（刻意缺口）", "unit_type": "project",
				"relevance": "medium", "disposition": "identity", "key_entities": []string{simMissingEntity},
			},
		},
		"segments": []interface{}{
			map[string]interface{}{"id": "s1", "label": "模拟-正常段", "unit_count": 2},
			map[string]interface{}{"id": "s2", "label": "模拟-被跳过段", "unit_count": 0},
		},
	}
	b, _ := json.Marshal(out)
	return string(b)
}

// author 阶段2：抄单元清单里的 key_entities 嵌进产物（跳过刻意缺口实体）。
func (m *TestAI14) author(userMsg string) string {
	var keep []string
	for _, arr := range keyEntRe.FindAllStringSubmatch(userMsg, -1) {
		for _, s := range strRe.FindAllStringSubmatch(arr[1], -1) {
			if strings.Contains(s[1], simMissingEntity) {
				continue
			}
			keep = append(keep, s[1])
		}
	}
	if len(keep) == 0 {
		keep = []string{"SIM-FALLBACK"}
	}
	entityLine := strings.Join(keep, "、")

	out := map[string]interface{}{
		"node_name":    "sim-fixture-node",
		"display_name": "模拟人格（夹具）",
		"emoji":        "🧪",
		"role":         "worker",
		"category":     "development",
		"tags":         []string{"sim", "fixture", "persona-gen"},
		"identity_md":  fmt.Sprintf("## 定位\n\n模拟夹具人格，覆盖实体：%s。\n\n## 业务领域\n\n领域实体：%s。\n\n## 专长\n\n专长含实体 %s。\n\n## 方法论与性格\n\n模拟方法论。", entityLine, entityLine, entityLine),
		"soul_md":      fmt.Sprintf("## 工作哲学\n\n哲学含实体 %s。\n\n## 行为准则\n\n模拟准则。\n\n## 沟通风格\n\n模拟风格。\n\n## 边界\n\n模拟边界。", entityLine),
		"expertise_md": "",
	}
	b, _ := json.Marshal(out)
	return string(b)
}
