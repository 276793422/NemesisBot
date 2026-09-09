package models

import (
	"encoding/json"
	"strings"
	"time"
)

// TestAIPlanner - Swarm M1 看板拆解 planner 测试模型。
//
// 供 board issue.plan 两段式流程的集成测试使用：对任意输入返回固定合法的
// 拆解 JSON 数组（3 个子任务：0 无依赖 / 1 依赖 [0] / 2 依赖 [1]），
// 覆盖 G1（立即可派子单）与 G2（依赖闸 + 完成后自动补派）两条链路。
//
// 行为开关（扫描用户消息）：
//   - 默认：返回合法 3 子任务 JSON（标题内嵌父任务标题，取「标题：」行，
//     缺省用「父任务」；中文安全截断 40 字符）
//   - <PLAN_BAD>：返回纯文本（无 JSON 数组）→ 首次解析失败；重试提示词
//     不含该标记 → 第二轮自动成功（验证回灌重试 1 次自愈）
//   - <PLAN_DEAD>：返回循环依赖计划（0↔1）；且输入含「打断环路」（回灌
//     错误文本独有锚点——system prompt 描述纪律时也会出现「循环依赖」
//     四字，不能用作锚点，否则首轮即毒化）时继续返回循环 → 3 轮全败
//     （验证 board.plan_failed 路径）
//
// 确定性输出，零随机、零延迟。
type TestAIPlanner struct{}

// plannerSub 拆解子任务结构（json.Marshal 保证输出恒为合法 JSON）。
type plannerSub struct {
	Title              string   `json:"title"`
	Description        string   `json:"description"`
	RequiredRole       string   `json:"required_role"`
	RequiredTags       []string `json:"required_tags"`
	AcceptanceCriteria string   `json:"acceptance_criteria"`
	DependsOn          []int    `json:"depends_on"`
}

func NewTestAIPlanner() *TestAIPlanner {
	return &TestAIPlanner{}
}

func (m *TestAIPlanner) Name() string {
	return "testai-planner-1.0"
}

func (m *TestAIPlanner) Process(messages []Message) string {
	input := plannerInputText(messages)

	// M4.5 注入探针：输入含「历史沉淀」（planner 经验段头独有片段
	// 「# 团队经验（历史沉淀，拆解时参考）」；无注入时提示词不存在该词，
	// system prompt 与任务文本也不含）→ 子任务 1 描述回显锚点。plan_ready
	// 的 payload 随即可断言注入真的进了 prompt（T29）。注意不能用
	// 「团队过往经验」——那是派发注入段头（team_memory render），planner
	// 段头是「团队经验」，措辞不同。
	expNote := ""
	if strings.Contains(input, "历史沉淀") {
		expNote = "（T29PLANEXP：planner 收到团队经验注入）"
	}

	// <PLAN_DEAD>：循环依赖计划；回灌错误文本（含独有锚点「打断环路」，
	// system prompt 无此词）到达时保持循环（全败路径）。
	if strings.Contains(input, "<PLAN_DEAD>") || strings.Contains(input, "打断环路") {
		return plannerMarshal([]plannerSub{
			{
				Title:              "子任务 A：先行步骤",
				Description:        "被循环依赖卡住的先行子任务。",
				RequiredRole:       "worker",
				RequiredTags:       []string{},
				AcceptanceCriteria: "不存在（用于验证循环依赖检测）",
				DependsOn:          []int{1},
			},
			{
				Title:              "子任务 B：后续步骤",
				Description:        "回指子任务 A 形成循环。",
				RequiredRole:       "worker",
				RequiredTags:       []string{},
				AcceptanceCriteria: "不存在（用于验证循环依赖检测）",
				DependsOn:          []int{0},
			},
		})
	}

	// <PLAN_BAD>：纯文本输出（无 JSON 数组）→ 首轮解析失败。
	if strings.Contains(input, "<PLAN_BAD>") {
		return "抱歉，我暂时无法拆解这个任务。"
	}

	return plannerMarshal(plannerDefaultPlan(plannerExtractTitle(input), expNote))
}

func (m *TestAIPlanner) Delay() time.Duration {
	return 0
}

// plannerInputText 拼接全部消息文本（planner 请求是单用户消息或回灌重试
// 单消息，全量扫描足够确定性）。
func plannerInputText(messages []Message) string {
	var buf strings.Builder
	for _, msg := range messages {
		buf.WriteString(msg.Content)
		buf.WriteString("\n")
	}
	return buf.String()
}

// plannerExtractTitle 从 planner 用户提示词中提取「标题：」行的父任务标题；
// 缺省返回「父任务」。中文安全：按 rune 截断到 40 字符。
func plannerExtractTitle(input string) string {
	title := "父任务"
	for _, line := range strings.Split(input, "\n") {
		if strings.HasPrefix(line, "标题：") {
			if t := strings.TrimSpace(strings.TrimPrefix(line, "标题：")); t != "" {
				title = t
			}
			break
		}
	}
	runes := []rune(title)
	if len(runes) > 40 {
		title = string(runes[:40])
	}
	return title
}

// plannerDefaultPlan 固定 3 子任务计划：0 无依赖（confirm 即派出，G1）、
// 1 依赖 [0]、2 依赖 [1]（依赖闸 + 补派，G2）。全部 worker 角色、空标签，
// 让匹配器对任意在线 worker 节点可命中。expNote（M4.5 注入探针）非空时
// 追加到子任务 1 描述。
func plannerDefaultPlan(parentTitle string, expNote string) []plannerSub {
	return []plannerSub{
		{
			Title:              parentTitle + " · 子任务1：调研与准备",
			Description:        "阅读与该任务相关的代码和资料，产出实施要点清单；本阶段不修改生产代码。" + expNote,
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "产出包含实施要点与涉及文件清单的说明文本。",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务2：实现主体",
			Description:        "依据准备阶段的要点清单完成核心实现；只做清单内范围，不顺手扩scope。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "核心改动落地且实现说明写明改动点。",
			DependsOn:          []int{0},
		},
		{
			Title:              parentTitle + " · 子任务3：验证与收尾",
			Description:        "运行检查确认无回归，汇总最终结果与遗留事项。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "检查通过并给出结论性总结。",
			DependsOn:          []int{1},
		},
	}
}

// plannerMarshal 序列化计划（json.Marshal 保证恒为合法 JSON 数组文本）。
func plannerMarshal(subs []plannerSub) string {
	data, err := json.Marshal(subs)
	if err != nil {
		// 结构体序列化不会失败；防御性兜底仍返回可被 parse_plan 拒绝的文本。
		return "plan 序列化失败"
	}
	return string(data)
}
