package models

import (
	"bytes"
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
//   - <PLAN_ANCHOR>：子任务验收标准带合法 [CHECK] 锚点行（文件锚点指向
//     uat-t2/pass/subN.md，由 UAT 驱动预置；另含对交付文本的正则锚点）
//     （验证 P2 锚点全过 → 进语义项，T2-1）
//   - <PLAN_REANCHOR>：子任务验收标准只带 re: 型交付文本正则锚点（跨节点
//     安全）。P1 拓扑硬闸（reject_remote_file_anchors）上线后 file: 锚点
//     不可远端派发，本形态是远端锚点链路 e2e 的合法计划（T30① 全过正流）
//   - <PLAN_ANCHOR_EVIL>：子任务验收标准带路径不安全锚点行（`..` 穿越 /
//     绝对路径）+ 一条合法锚点（验证解析期拒绝 + 告警回落语义，T2-3）
//   - <PLAN_ANCHOR_MIXED>：子任务验收标准带无法解析的 [CHECK] 坏行（空
//     目标）+ 合法锚点（验证坏行静默回落语义、不告警不 FAIL，T2-4）
//   - <PLAN_PARALLEL>：3 个无依赖就绪子任务——0/1 互斥对（[TOUCH] 同一
//     路径 shared/a.txt）、2 独立（[TOUCH] solo/c.txt）。供 T-sched-1 /
//     T-res-2 的 D0 准入 + R-9 touch_paths 互斥多 worker 联验：互斥对不
//     并发、独立对可分散两机
//   - <PLAN_PROF>：5 子任务职能链（0→1→2→3→4），required_profession 依
//     次为 product/architecture/dev:cpp/test-whitebox/test-blackbox（职能
//     框架 M7 U1：逐单派发给宣告职能的节点 + B 端契约渲染）
//   - <PLAN_PROF_BAD>：子任务2 带 required_profession "dev;cpp"（分号，
//     slug 语法非法）→ 首轮解析失败；输入含回灌锚点「你上一次的输出无法
//     通过校验」时自愈为 "dev:cpp"（验证回灌重试自愈，U4 前半）
//   - <PLAN_PROF_BAD_STUBBORN>：同 <PLAN_PROF_BAD> 但自愈轮仍输出非法值
//     → 3 轮耗尽走末轮宽和：单子单降级（职能清空）+ 评论留痕 + 其余子单
//     照常派发（U4 后半）
//   - <PLAN_PROF_USER>：1 子任务，required_profession=myfamily:myspec
//     （用户自定义职能，U7：announce 携带 + B 端从本节点磁盘渲染档案）
//   - <PLAN_PROF_UI_TEXT>：1 子任务 ui-design，描述带 <UAT_UI_TEXT>
//     （U6 图像降级：B 无图像模型，文本线交付）
//   - <PLAN_PROF_UI_IMG>：1 子任务 ui-design，描述带 <UAT_UI_IMG>
//     （U5 图像全链：generate_image → board_asset publish → 资产取回
//     vision 评审）
//
// 确定性输出，零随机、零延迟。
type TestAIPlanner struct{}

// plannerSub 拆解子任务结构（json.Marshal 保证输出恒为合法 JSON 数组）。
// RequiredProfession omitempty：既有形态（无职能）不发射该键，与职能
// 框架 M2 之前的响应字节保持一致（serde default 侧兼容，双保险）。
type plannerSub struct {
	Title              string   `json:"title"`
	Description        string   `json:"description"`
	RequiredRole       string   `json:"required_role"`
	RequiredTags       []string `json:"required_tags"`
	RequiredProfession string   `json:"required_profession,omitempty"`
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

	// P2 锚点系列开关（T2 组 UAT）：子任务验收标准携带 [CHECK] 行。
	title := plannerExtractTitle(input)
	switch {
	case strings.Contains(input, "<PLAN_PARALLEL>"):
		return plannerMarshal(plannerParallelPlan(title))
	case strings.Contains(input, "<PLAN_TAGS>"):
		return plannerMarshal(plannerTagsPlan(title))
	case strings.Contains(input, "<PLAN_PROF_BAD_STUBBORN>"):
		// 坚持非法：回灌轮输入只带 prev（不带原始任务文本），所以内嵌
		// marker 必须是完整激活标记 <PLAN_PROF_BAD_STUBBORN>——否则回灌
		// 轮落入 BAD 分支被误自愈。3 轮恒命中 → 走末轮宽和（降级+评论）。
		return plannerMarshal(plannerProfPlan(title, "dev;cpp", "<PLAN_PROF_BAD_STUBBORN>"))
	case strings.Contains(input, "<PLAN_PROF_BAD>"):
		// 回灌重试轮（锚点 = board build_retry_prompt 首句；marker 经
		// prev 输出回流——build_retry_prompt 不携带原始任务文本）自愈为
		// 合法 dev:cpp；首轮输出非法 dev;cpp 并把 marker 内嵌进 prev
		// （子任务2 描述），保证回灌轮分支可达。
		if strings.Contains(input, "你上一次的输出无法通过校验") {
			return plannerMarshal(plannerProfPlan(title, "dev:cpp", ""))
		}
		return plannerMarshal(plannerProfPlan(title, "dev;cpp", "<PLAN_PROF_BAD>"))
	case strings.Contains(input, "<PLAN_PROF>"):
		return plannerMarshal(plannerProfChainPlan(title))
	case strings.Contains(input, "<PLAN_PROF_USER>"):
		return plannerMarshal(plannerProfSinglePlan(title, "myfamily:myspec",
			"U7 用户自定义职能子单：按本节点职能档案交付。",
			"回复包含：职能=myfamily:myspec"))
	case strings.Contains(input, "<PLAN_PROF_UI_TEXT>"):
		return plannerMarshal(plannerProfSinglePlan(title, "ui-design",
			"UI 交付子单（文本线）。<UAT_UI_TEXT>",
			"回复包含：UI图已交付"))
	case strings.Contains(input, "<PLAN_PROF_UI_IMG>"):
		return plannerMarshal(plannerProfSinglePlan(title, "ui-design",
			"UI 交付子单（图像线）。<UAT_UI_IMG>",
			"回复包含：UI图已交付"))
	case strings.Contains(input, "<PLAN_ANCHOR_EVIL>"):
		return plannerMarshal(plannerAnchorPlan(title, "evil"))
	case strings.Contains(input, "<PLAN_ANCHOR_MIXED>"):
		return plannerMarshal(plannerAnchorPlan(title, "mixed"))
	case strings.Contains(input, "<PLAN_REANCHOR>"):
		return plannerMarshal(plannerAnchorPlan(title, "reanchor"))
	case strings.Contains(input, "<PLAN_ANCHOR>"):
		return plannerMarshal(plannerAnchorPlan(title, "pass"))
	}

	return plannerMarshal(plannerDefaultPlan(title, expNote))
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

// plannerParallelPlan R-9/D0 多 worker 联验计划（<PLAN_PARALLEL>）：
// 3 个全就绪子任务（无依赖），全部 worker 角色、空标签。0/1 是互斥对
// （[TOUCH] shared/a.txt 同路径），2 独立（solo/c.txt）。预期调度行为：
// 互斥对不并发（R-9 闸串行或错开）、独立单可派到另一台 worker。
func plannerParallelPlan(parentTitle string) []plannerSub {
	return []plannerSub{
		{
			Title:              parentTitle + " · 子任务1：写共享文件 A",
			Description:        "先执行命令 sleep 90（阻塞 90 秒，模拟长任务），然后在工作区创建 shared 目录并写文件 a.txt（内容：A 完成）。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "回复包含：A完成\n[TOUCH] shared/a.txt",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务2：覆盖写共享文件 A",
			Description:        "向 shared/a.txt 追加一行（内容：B 完成）——与子任务1 写同一路径，必须等其完成后再做。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "回复包含：B完成\n[TOUCH] shared/a.txt",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务3：写独立文件 C",
			Description:        "在工作区创建 solo 目录并写文件 c.txt（内容：C 完成）。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "回复包含：C完成\n[TOUCH] solo/c.txt",
			DependsOn:          []int{},
		},
	}
}

// plannerMarshal 序列化计划（恒为合法 JSON 数组文本）。SetEscapeHTML(false)：
// 默认转义会把 < > 写成 < >——U4 依赖 marker 字面量随 prev 输出
// 回流（回灌轮 Contains 探测），必须保持原字符。其余计划不含 <>&，字节不变。
func plannerMarshal(subs []plannerSub) string {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(subs); err != nil {
		// 结构体序列化不会失败；防御性兜底仍返回可被 parse_plan 拒绝的文本。
		return "plan 序列化失败"
	}
	return strings.TrimRight(buf.String(), "\n")
}

// plannerAnchorPlan P2 锚点系列计划（T2 组 UAT）：3 子任务链 0→1→2，
// 验收标准 = 普通文字行 + [CHECK] 锚点行。mode：
//   - "pass"：全部合法锚点。文件锚点指向 uat-t2/pass/subN.md（UAT 驱动
//     在 A 端评审 workspace 预置）；子任务1 另含交付文本正则锚点（匹配
//     测试 worker 的固定汇报文本「集群协作状态正常」）。
//   - "reanchor"：纯 re: 型交付文本正则锚点（无 file:，跨节点安全——
//     P1 拓扑硬闸下远端派发的合法形态）。子任务1 两条（集群协作状态正常
//     + 收到），子任务2/3 各一条；全部命中测试 worker 固定汇报文本。
//   - "evil"：每子任务带两条路径不安全锚点（`..` 穿越 / 绝对路径——跨
//     平台都会被解析期拒绝的形态）+ 一条合法锚点（uat-t2/evil/subN.md，
//     驱动预置）。期望：不安全行解析期拒绝 + 告警评论，合法锚点照跑，
//     验收不炸不短路 FAIL。
//   - "mixed"：每子任务带一条无法解析的 [CHECK] 坏行（空目标路径）+
//     一条合法锚点（uat-t2/mixed/subN.md，驱动预置）。期望：坏行静默
//     回落语义项（不告警），合法锚点照跑。
func plannerAnchorPlan(parentTitle string, mode string) []plannerSub {
	var acs [3]string
	switch mode {
	case "reanchor":
		acs[0] = "产出包含实施要点的说明文本。\n" +
			"[CHECK] re:集群协作状态正常\n" +
			"[CHECK] re:收到"
		acs[1] = "核心改动落地且实现说明写明改动点。\n" +
			"[CHECK] re:集群协作状态正常"
		acs[2] = "检查通过并给出结论性总结。\n" +
			"[CHECK] re:收到"
	case "pass":
		acs[0] = "产出包含实施要点的说明文本。\n" +
			"[CHECK] file:uat-t2/pass/sub1.md exists\n" +
			"[CHECK] file:uat-t2/pass/sub1.md contains:锚点测试\n" +
			"[CHECK] re:集群协作状态正常"
		acs[1] = "核心改动落地且实现说明写明改动点。\n" +
			"[CHECK] file:uat-t2/pass/sub2.md exists"
		acs[2] = "检查通过并给出结论性总结。\n" +
			"[CHECK] file:uat-t2/pass/sub3.md exists"
	case "evil":
		acs[0] = "产出包含实施要点的说明文本。\n" +
			"[CHECK] file:../outside-secret.txt exists\n" +
			"[CHECK] file:/abs/path/probe.txt exists\n" +
			"[CHECK] file:uat-t2/evil/sub1.md exists"
		acs[1] = "核心改动落地且实现说明写明改动点。\n" +
			"[CHECK] file:../outside-secret.txt exists\n" +
			"[CHECK] file:/abs/path/probe.txt exists\n" +
			"[CHECK] file:uat-t2/evil/sub2.md exists"
		acs[2] = "检查通过并给出结论性总结。\n" +
			"[CHECK] file:../outside-secret.txt exists\n" +
			"[CHECK] file:/abs/path/probe.txt exists\n" +
			"[CHECK] file:uat-t2/evil/sub3.md exists"
	case "mixed":
		acs[0] = "产出包含实施要点的说明文本。\n" +
			"[CHECK] file: exists\n" +
			"[CHECK] file:uat-t2/mixed/sub1.md exists"
		acs[1] = "核心改动落地且实现说明写明改动点。\n" +
			"[CHECK] file: exists\n" +
			"[CHECK] file:uat-t2/mixed/sub2.md exists"
		acs[2] = "检查通过并给出结论性总结。\n" +
			"[CHECK] file: exists\n" +
			"[CHECK] file:uat-t2/mixed/sub3.md exists"
	}
	deps := [3][]int{{}, {0}, {1}}
	subs := make([]plannerSub, 3)
	for i := range subs {
		subs[i] = plannerSub{
			Title:              parentTitle + " · 子任务" + string(rune('1'+i)),
			Description:        "锚点测试子任务。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: acs[i],
			DependsOn:          deps[i],
		}
	}
	return subs
}

// plannerTagsPlan E 标签授予联验计划（<PLAN_TAGS>）：3 个无依赖就绪子单，
// 子1 空标签（正常匹配基线），子2/子3 声明幽灵标签 ghost-e2e（无人持有）
// ——确认波内：子2 匹配失败走兜底（dispatch_fallback 开）→ 派出 + 授予
// 标签；子3 在子2 之后重估 → granted_tags 投影合并 → 正常匹配（不再走
// 兜底）。验证 E 台账 + 同类第二单正常匹配闭环。（无依赖：链式计划会被
// 依赖闸卡在 in_review 前序单上——auto_accept=false 时前序永不为 done。）
func plannerTagsPlan(parentTitle string) []plannerSub {
	return []plannerSub{
		{
			Title:              parentTitle + " · 子任务1：基线",
			Description:        "回复：E1完成",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "回复包含：E1完成",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务2：幽灵标签单",
			Description:        "回复：E2完成",
			RequiredRole:       "worker",
			RequiredTags:       []string{"ghost-e2e"},
			AcceptanceCriteria: "回复包含：E2完成",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务3：同类第二单",
			Description:        "回复：E3完成",
			RequiredRole:       "worker",
			RequiredTags:       []string{"ghost-e2e"},
			AcceptanceCriteria: "回复包含：E3完成",
			DependsOn:          []int{},
		},
	}
}

// plannerProfChainPlan 职能框架 U1 全流水线计划（<PLAN_PROF>）：5 子任务
// 依赖链 0→1→2→3→4，required_profession 覆盖首批六职能中的五个（PM →
// 架构 → dev:cpp → 白盒 → 黑盒）。每单验收锚点 = 交付文本正则（re: 型，
// 跨节点安全）+ 职能名回显——评审既验交付又验「B 端确实收到了职能契约」
// （worker 汇报含职能名 = system prompt 注入成功的客观旁证）。
func plannerProfChainPlan(parentTitle string) []plannerSub {
	profs := []string{"product", "architecture", "dev:cpp", "test-whitebox", "test-blackbox"}
	tasks := []struct{ brief, deliver string }{
		{"需求分析", "回复：产品需求要点已产出，职能=product"},
		{"技术架构", "回复：架构方案已产出，职能=architecture"},
		{"核心实现", "回复：核心实现已落地，职能=dev:cpp"},
		{"白盒验证", "回复：白盒检查已通过，职能=test-whitebox"},
		{"黑盒验收", "回复：黑盒验收已通过，职能=test-blackbox"},
	}
	subs := make([]plannerSub, len(profs))
	for i := range profs {
		deps := []int{}
		if i > 0 {
			deps = []int{i - 1}
		}
		subs[i] = plannerSub{
			Title:              parentTitle + " · 子任务" + string(rune('1'+i)) + "：" + tasks[i].brief,
			Description:        "职能测试子任务：" + tasks[i].brief + "。",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			RequiredProfession: profs[i],
			AcceptanceCriteria: "回复包含：职能=" + profs[i] + "\n[CHECK] re:职能=" + profs[i],
			DependsOn:          deps,
		}
	}
	return subs
}

// plannerProfSinglePlan 单子任务职能计划（U7/U6/U5）：1 子任务、无依赖、
// required_profession=prof、描述/验收标准由调用方给定（UAT 标记随描述
// 进 worker 任务提示词）。confirm 波即派——依赖闸无前序。
func plannerProfSinglePlan(parentTitle, prof, desc, ac string) []plannerSub {
	return []plannerSub{
		{
			Title:              parentTitle + " · 子任务1：职能交付",
			Description:        desc,
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			RequiredProfession: prof,
			AcceptanceCriteria: ac,
			DependsOn:          []int{},
		},
	}
}

// plannerProfPlan 职能非法枚举计划（<PLAN_PROF_BAD> / <PLAN_PROF_BAD_STUBBORN>）：
// 3 子任务链 0→1→2，子任务2 required_profession = prof 参数（合法值 =
// 自愈轮输出；"dev;cpp" = 非法，slug 语法非法 → 解析期拒绝）。
// marker 非空时把该标记（完整激活标记，含 STUBBORN 形态）内嵌进子任务2
// 描述——build_retry_prompt 只回携带上一次输出（不带原始任务文本），
// 回灌轮分支探测的是 prev 里的内嵌标记，必须与激活标记同形。自愈轮
// marker 为空（合法计划无需回流标记）。
func plannerProfPlan(parentTitle string, prof string, marker string) []plannerSub {
	desc2 := "回复：P2完成"
	if marker != "" {
		desc2 = "回复：P2完成 " + marker
	}
	return []plannerSub{
		{
			Title:              parentTitle + " · 子任务1：合法职能单",
			Description:        "回复：P1完成",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			RequiredProfession: "product",
			AcceptanceCriteria: "回复包含：P1完成",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务2：职能单",
			Description:        desc2,
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			RequiredProfession: prof,
			AcceptanceCriteria: "回复包含：P2完成",
			DependsOn:          []int{},
		},
		{
			Title:              parentTitle + " · 子任务3：普通单",
			Description:        "回复：P3完成",
			RequiredRole:       "worker",
			RequiredTags:       []string{},
			AcceptanceCriteria: "回复包含：P3完成",
			DependsOn:          []int{},
		},
	}
}
