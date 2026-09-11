package models

import (
	"encoding/json"
	"strings"
	"testing"
)

// Swarm M1 planner 测试模型单元测试。除模型自身行为外，用例内置一个
// parse_plan 约束的 Go 侧镜像校验（合法 JSON / 非空 / 上限 / title 非空 /
// depends_on 界内 / 不自引用 / 无循环）——Rust 侧 parse_plan 另有独立单测，
// 两侧约束同步演进。

// plannerMirrorValidate 镜像 crates/nemesis-board/src/planner.rs parse_plan
// 的核心校验；返回违规列表（空 = 通过）。
func plannerMirrorValidate(t *testing.T, raw string) []map[string]interface{} {
	t.Helper()
	start := strings.Index(raw, "[")
	end := strings.LastIndex(raw, "]")
	if start < 0 || end < start {
		t.Fatalf("output has no JSON array: %s", raw)
	}
	var subs []map[string]interface{}
	if err := json.Unmarshal([]byte(raw[start:end+1]), &subs); err != nil {
		t.Fatalf("output is not valid JSON array: %v\nraw: %s", err, raw)
	}
	if len(subs) == 0 {
		t.Fatal("empty plan array")
	}
	if len(subs) > 20 {
		t.Fatalf("plan exceeds max subissues: %d", len(subs))
	}
	titleOf := func(i int) string {
		s, _ := subs[i]["title"].(string)
		return s
	}
	for i, sub := range subs {
		if strings.TrimSpace(titleOf(i)) == "" {
			t.Fatalf("sub %d has empty title", i)
		}
		depsRaw, ok := sub["depends_on"].([]interface{})
		if !ok {
			depsRaw = []interface{}{}
		}
		for _, d := range depsRaw {
			dep := int(d.(float64))
			if dep >= len(subs) {
				t.Fatalf("sub %d depends_on %d out of bounds (len=%d)", i, dep, len(subs))
			}
			if dep == i {
				t.Fatalf("sub %d depends on itself", i)
			}
		}
	}
	// 环检测（DFS 三色）。
	color := make([]int, len(subs))
	var visit func(i int) bool
	visit = func(i int) bool {
		if color[i] == 1 {
			return true
		}
		if color[i] == 2 {
			return false
		}
		color[i] = 1
		depsRaw, _ := subs[i]["depends_on"].([]interface{})
		for _, d := range depsRaw {
			if visit(int(d.(float64))) {
				return true
			}
		}
		color[i] = 2
		return false
	}
	for i := range subs {
		if visit(i) {
			t.Fatalf("plan has dependency cycle:\n%s", raw)
		}
	}
	return subs
}

// plannerPromptFixture 模拟 build_planner_user_prompt 的输出形态。
func plannerPromptFixture(title string) string {
	return "# 父任务\n标题：" + title + "\n描述：\n示例描述\n\n# 整体验收标准\n（未提供）\n\n请拆解上述父任务，只输出 JSON 数组。"
}

func TestPlannerModelRegistrationShape(t *testing.T) {
	m := NewTestAIPlanner()
	if m.Name() != "testai-planner-1.0" {
		t.Fatalf("unexpected model name: %s", m.Name())
	}
	if m.Delay() != 0 {
		t.Fatalf("planner must be zero-delay, got %v", m.Delay())
	}
}

func TestPlannerDefaultPlanMirrorValidAndShape(t *testing.T) {
	m := NewTestAIPlanner()
	raw := m.Process([]Message{{Role: "user", Content: plannerPromptFixture("搭建 CI 门禁")}})
	subs := plannerMirrorValidate(t, raw)
	if len(subs) != 3 {
		t.Fatalf("default plan must have 3 subs, got %d", len(subs))
	}
	depsOf := func(i int) []int {
		depsRaw, _ := subs[i]["depends_on"].([]interface{})
		out := make([]int, 0, len(depsRaw))
		for _, d := range depsRaw {
			out = append(out, int(d.(float64)))
		}
		return out
	}
	// 依赖形状：0 无依赖 / 1 依赖 [0] / 2 依赖 [1]——G1 立即可派 + G2 依赖补派。
	if len(depsOf(0)) != 0 {
		t.Fatalf("sub 0 must have no deps, got %v", depsOf(0))
	}
	if len(depsOf(1)) != 1 || depsOf(1)[0] != 0 {
		t.Fatalf("sub 1 must depend on [0], got %v", depsOf(1))
	}
	if len(depsOf(2)) != 1 || depsOf(2)[0] != 1 {
		t.Fatalf("sub 2 must depend on [1], got %v", depsOf(2))
	}
	// 全部 worker 角色、空标签 → 匹配器对任意在线 worker 可命中。
	for i, sub := range subs {
		if sub["required_role"] != "worker" {
			t.Fatalf("sub %d role must be worker, got %v", i, sub["required_role"])
		}
		if tags, ok := sub["required_tags"].([]interface{}); !ok || len(tags) != 0 {
			t.Fatalf("sub %d tags must be empty, got %v", i, sub["required_tags"])
		}
	}
	// 标题内嵌父任务标题（测试可断言 plan 来源于该输入）。
	if !strings.Contains(subs[0]["title"].(string), "搭建 CI 门禁") {
		t.Fatalf("sub titles must embed parent title, got %v", subs[0]["title"])
	}
}

func TestPlannerTitleFallbackWhenMissing(t *testing.T) {
	m := NewTestAIPlanner()
	raw := m.Process([]Message{{Role: "user", Content: "拆解一下这个任务"}})
	subs := plannerMirrorValidate(t, raw)
	if !strings.Contains(subs[0]["title"].(string), "父任务") {
		t.Fatalf("missing 标题 line must fall back to 父任务, got %v", subs[0]["title"])
	}
}

func TestPlannerTitleTruncatedByRuneNotByte(t *testing.T) {
	m := NewTestAIPlanner()
	long := strings.Repeat("集", 60) // 60 个中文字符 → 截到 40 rune
	raw := m.Process([]Message{{Role: "user", Content: plannerPromptFixture(long)}})
	subs := plannerMirrorValidate(t, raw)
	title := subs[0]["title"].(string)
	// 40 rune 父标题 + 「 · 子任务1：调研与准备」后缀 → 总 rune 数应有限且不含乱码。
	got := strings.Count(title, "集")
	if got != 40 {
		t.Fatalf("parent title must truncate to 40 runes, got %d 集 in %q", got, title)
	}
}

func TestPlannerBadMarkerPlainOutputThenRetrySucceeds(t *testing.T) {
	m := NewTestAIPlanner()
	bad := m.Process([]Message{{Role: "user", Content: "<PLAN_BAD>" + plannerPromptFixture("坏首輪任务")}})
	if strings.Contains(bad, "[") {
		t.Fatalf("<PLAN_BAD> output must not contain JSON array, got %s", bad)
	}
	// 回灌重试提示词（build_retry_prompt 形态）不含标记 → 第二轮出合法计划。
	retry := "你上一次的输出无法通过校验：输出中找不到 JSON 数组（应以 '[' 开头、']' 结尾）。请只输出 JSON 数组本身。\n\n上一次输出：\n" + bad + "\n\n请修正后重新输出：只输出符合格式的 JSON 数组，不要任何其他文字或解释。"
	good := m.Process([]Message{{Role: "user", Content: retry}})
	plannerMirrorValidate(t, good)
}

func TestPlannerDeadMarkerCyclicPersistsAcrossRetry(t *testing.T) {
	m := NewTestAIPlanner()
	first := m.Process([]Message{{Role: "user", Content: "<PLAN_DEAD>" + plannerPromptFixture("死循环任务")}})
	// 注意：不能用 plannerMirrorValidate——循环依赖是该输出的意图行为，
	// 镜像校验器会正确地拒绝它。直接反序列化断言环存在。
	var subs []map[string]interface{}
	start := strings.Index(first, "[")
	end := strings.LastIndex(first, "]")
	if start < 0 || end < start {
		t.Fatalf("<PLAN_DEAD> output has no JSON array: %s", first)
	}
	if err := json.Unmarshal([]byte(first[start:end+1]), &subs); err != nil {
		t.Fatalf("<PLAN_DEAD> output not JSON: %v", err)
	}
	if len(subs) != 2 {
		t.Fatalf("<PLAN_DEAD> plan must have 2 subs, got %d", len(subs))
	}
	// 回灌错误文本（生产 detect_cycle 形态，含独有锚点「打断环路」）→
	// 模型继续输出循环计划（三轮全败路径）。
	retry := "你上一次的输出无法通过校验：depends_on 存在循环依赖：0 -> 1 -> 0。请打断环路后重新输出。\n\n上一次输出：\n" + first
	again := m.Process([]Message{{Role: "user", Content: retry}})
	var subsAgain []map[string]interface{}
	start = strings.Index(again, "[")
	end = strings.LastIndex(again, "]")
	if err := json.Unmarshal([]byte(again[start:end+1]), &subsAgain); err != nil {
		t.Fatalf("cyclic retry output not JSON: %v", err)
	}
	d0, _ := subsAgain[0]["depends_on"].([]interface{})
	if len(d0) != 1 || int(d0[0].(float64)) != 1 {
		t.Fatalf("retry must keep the cycle (sub0 -> 1), got %v", d0)
	}
}

// TestPlannerSystemPromptCycleMentionDoesNotPoison 防回归：生产
// PLANNER_SYSTEM_PROMPT 的拆解纪律行含「不得形成循环依赖」字样（同步自
// crates/nemesis-board/src/planner.rs），首轮请求即带该文本——模型不得把
// 它误当回灌锚点返回循环计划（2026-09-09 uat T21/T22 假败的根因）。
func TestPlannerSystemPromptCycleMentionDoesNotPoison(t *testing.T) {
	m := NewTestAIPlanner()
	system := "你是 NemesisBot 看板的任务拆解规划器（planner）。\n\n# 拆解纪律\n" +
		"3. depends_on 只允许引用本数组内的序号，且不得形成循环依赖。"
	raw := m.Process([]Message{
		{Role: "system", Content: system},
		{Role: "user", Content: plannerPromptFixture("带纪律提示词的任务")},
	})
	subs := plannerMirrorValidate(t, raw) // 内含环检测——循环计划在此直接红
	if len(subs) != 3 {
		t.Fatalf("system prompt mentioning cycles must not poison round 1: got %d subs", len(subs))
	}
}

func TestPlannerMultimodalPartsIgnored(t *testing.T) {
	// content 数组形态（Parts）下文本拼进 Content（types.go UnmarshalJSON）；
	// planner 只看拼接文本，不应因此输出异常。
	m := NewTestAIPlanner()
	msg := Message{Role: "user"}
	msgContent := plannerPromptFixture("数组内容任务")
	_ = json.Unmarshal([]byte(`{"role":"user","content":[{"type":"text","text":"`+
		strings.ReplaceAll(strings.ReplaceAll(msgContent, `\`, `\\`), `"`, `\"`)+
		`"}]}`), &msg)
	raw := m.Process([]Message{msg})
	plannerMirrorValidate(t, raw)
}

// TestPlannerAnchorMarkers P2 锚点系列开关（T2 组 UAT 夹具契约）：
// <PLAN_ANCHOR>/<PLAN_ANCHOR_EVIL>/<PLAN_ANCHOR_MIXED> 三标记的输出形态
// 钉死——合法锚点行 / 不安全锚点行（`..`、绝对路径）/ 坏行（空目标）。
func TestPlannerAnchorMarkers(t *testing.T) {
	m := NewTestAIPlanner()
	cases := []struct {
		marker      string
		wantCheckLn int // 每子任务 [CHECK] 行数（普通文字行不计）
	}{
		{"<PLAN_ANCHOR>", 0}, // 子任务1=3 条、子任务2/3=1 条 → 特判在下方
		{"<PLAN_ANCHOR_EVIL>", 3},
		{"<PLAN_ANCHOR_MIXED>", 2},
	}
	for _, tc := range cases {
		raw := m.Process([]Message{{Role: "user", Content: plannerPromptFixture("锚点任务 " + tc.marker)}})
		start := strings.Index(raw, "[")
		end := strings.LastIndex(raw, "]")
		if start < 0 || end < start {
			t.Fatalf("%s output has no JSON array: %s", tc.marker, raw)
		}
		var subs []map[string]interface{}
		if err := json.Unmarshal([]byte(raw[start:end+1]), &subs); err != nil {
			t.Fatalf("%s output not JSON: %v", tc.marker, err)
		}
		if len(subs) != 3 {
			t.Fatalf("%s plan must have 3 subs, got %d", tc.marker, len(subs))
		}
		if tc.marker == "<PLAN_ANCHOR>" {
			// 子任务1：2 文件锚点 + 1 交付正则；子任务2/3：各 1 文件锚点。
			ac0, _ := subs[0]["acceptance_criteria"].(string)
			ac1, _ := subs[1]["acceptance_criteria"].(string)
			if strings.Count(ac0, "[CHECK]") != 3 || !strings.Contains(ac0, "uat-t2/pass/sub1.md") {
				t.Fatalf("sub1 anchors unexpected: %s", ac0)
			}
			if strings.Count(ac1, "[CHECK]") != 1 || !strings.Contains(ac1, "uat-t2/pass/sub2.md") {
				t.Fatalf("sub2 anchors unexpected: %s", ac1)
			}
			continue
		}
		ac0, _ := subs[0]["acceptance_criteria"].(string)
		if strings.Count(ac0, "[CHECK]") != tc.wantCheckLn {
			t.Fatalf("%s sub1 [CHECK] lines = %d, want %d: %s", tc.marker, strings.Count(ac0, "[CHECK]"), tc.wantCheckLn, ac0)
		}
		if tc.marker == "<PLAN_ANCHOR_EVIL>" {
			if !strings.Contains(ac0, "../outside-secret.txt") || !strings.Contains(ac0, "/abs/path/probe.txt") {
				t.Fatalf("evil anchors missing unsafe forms: %s", ac0)
			}
		}
		if tc.marker == "<PLAN_ANCHOR_MIXED>" && !strings.Contains(ac0, "[CHECK] file: exists") {
			t.Fatalf("mixed anchor missing empty-target bad line: %s", ac0)
		}
	}
}
