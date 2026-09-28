//! 提示词资产单测：段落池确定性组装、描述表结构不变量、子代理模板结构、
//! 纯渲染函数契约。

use super::*;

// ---------------------------------------------------------------------------
// 段落池渲染
// ---------------------------------------------------------------------------

#[test]
fn render_layer_is_deterministic() {
    // 同层两次渲染字节级一致（prompt cache 前缀稳定的根基）。
    let a = system::render_layer(system::Layer::Pre);
    let b = system::render_layer(system::Layer::Pre);
    assert_eq!(a, b);
    assert!(
        !a.is_empty(),
        "Pre 层不应为空（identity_base + safety_policy）"
    );
}

#[test]
fn render_layer_orders_segments_by_registry() {
    // 注册表顺序 = 渲染顺序：身份段在安全段之前。
    let pre = system::render_layer(system::Layer::Pre);
    let identity_pos = pre
        .find("交互式智能代理")
        .expect("identity_base 正文应在场");
    let safety_pos = pre.find("安全基线").expect("safety_policy 正文应在场");
    assert!(
        identity_pos < safety_pos,
        "Pre 层顺序应为 identity_base → safety_policy"
    );
}

#[test]
fn render_layer_trims_segment_edges() {
    // 段落首尾空白被裁掉——组装产物不依赖文件结尾换行的有无。
    let pre = system::render_layer(system::Layer::Pre);
    assert!(!pre.starts_with('\n'), "渲染结果不得以空白开头");
    assert!(!pre.starts_with(' '));
    assert!(!pre.ends_with('\n'), "渲染结果不得以空白结尾");
}

#[test]
fn post_layer_renders_all_behavior_segments_in_order() {
    let post = system::render_layer(system::Layer::Post);
    assert!(!post.is_empty(), "Post 层（行为段池）不应为空");

    // 注册表顺序 = 行为段渲染顺序；每段用一个独有短语做存在性锚点。
    let markers: &[&str] = &[
        "输出规范",
        "沟通方式",
        "群聊与频道运营",
        "决策姿态",
        "任务交付",
        "汇报纪律",
        "自主运行",
        "代码风格",
        "工具使用总则",
        "工具参数纪律",
        "执行模式",
        "工作区与记忆",
        "确认与授权",
        "内容边界",
        "称谓与语言",
    ];
    let mut last = 0usize;
    for m in markers {
        let pos = post
            .find(m)
            .unwrap_or_else(|| panic!("Post 层缺段落锚点：{m}"));
        assert!(pos >= last, "Post 层顺序错乱：{m} 出现在前一个锚点之前");
        last = pos;
    }
}

#[test]
fn all_segments_nonempty_and_within_budget() {
    // 池总量远低于软预算；单段为空说明文件被清空（include_str! 编译期
    // 嵌入，这里兜一道运行期断言）。
    let pre = system::render_layer(system::Layer::Pre);
    let post = system::render_layer(system::Layer::Post);
    assert!(pre.len() > 200, "Pre 层内容异常地短：{}B", pre.len());
    assert!(post.len() > 2_000, "Post 层内容异常地短：{}B", post.len());
    assert!(
        pre.len() + post.len() < system::SOFT_BUDGET_BYTES,
        "段落池总量 {}B 已逼近/超过软预算 {}B",
        pre.len() + post.len(),
        system::SOFT_BUDGET_BYTES
    );
}

// ---------------------------------------------------------------------------
// 工具描述查表结构不变量
// ---------------------------------------------------------------------------

/// 表条目数锚点：新增工具槽位必须同时补描述表（lean 必有，full 按频次），
/// 数量变化时此处显式红出来提醒对齐注册表清单。
const EXPECTED_TOOL_ENTRIES: usize = 60;

#[test]
fn tool_table_covers_expected_slots_with_valid_names() {
    assert_eq!(
        tools::table_len(),
        EXPECTED_TOOL_ENTRIES,
        "描述表条目数漂移：新增/删除工具槽位时同步维护本表"
    );
    for (name, lean, _) in tools::table_entries() {
        assert!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "工具名应为 snake_case：{name}"
        );
        let lean = lean.trim();
        assert!(!lean.is_empty(), "{name} 的 lean 描述为空");
        assert!(
            lean.len() <= 400,
            "{name} 的 lean 描述超预算：{}B",
            lean.len()
        );
    }
}

#[test]
fn full_description_starts_with_lean_first_sentence() {
    // full 首句 == lean 首句：与 tool_doc_folding 的首句折叠天然兼容
    // （折叠工具拿到的句子在两档下语义一致），也是「精简版 = 完整版摘要」
    // 的纪律锚。外部脚本生成的条目必须过这道门。
    for (name, lean, full) in tools::table_entries() {
        let Some(full) = full else { continue };
        assert_eq!(
            tools::first_sentence(full),
            tools::first_sentence(lean),
            "{name} 的 full 首句与 lean 首句不一致"
        );
    }
}

#[test]
fn description_for_serves_level_appropriate_text() {
    // 命中表项：lean 恒取 lean 文本；full 取 full 且以 lean 首句开头。
    let lean = tools::description_for("exec", tools::DescLevel::Lean).expect("exec 应命中描述表");
    let full = tools::description_for("exec", tools::DescLevel::Full).expect("exec 应命中描述表");
    assert_eq!(lean.trim(), tools::first_sentence(full).trim());
    assert!(full.len() > lean.len(), "full 档应比 lean 档更详尽");
    // 未命中 = None（调用方回落注册表原文）。
    assert_eq!(
        tools::description_for("no_such_tool", tools::DescLevel::Full),
        None
    );
}

// ---------------------------------------------------------------------------
// 内部调用点文案
// ---------------------------------------------------------------------------

#[test]
fn compact_merge_template_has_exactly_two_placeholders() {
    // render_compact_merge 对占位数量有硬契约（两个）；三个及以上会把
    // 第三段原文带进 prompt。这里钉死模板形态。
    let t = aux::COMPACT_MERGE_TEMPLATE;
    assert!(t.contains("{}"));
    let (_, rest) = t.split_once("{}").unwrap();
    assert!(rest.contains("{}"));
    let (_, rest2) = rest.split_once("{}").unwrap();
    assert!(!rest2.contains("{}"), "合并模板不得有多余占位");

    // 渲染产物按序包含两份输入。
    let rendered = aux::render_compact_merge("AAA", "BBB");
    let a = rendered.find("AAA").expect("摘要一应在渲染产物中");
    let b = rendered.find("BBB").expect("摘要二应在渲染产物中");
    assert!(a < b, "摘要一必须先于摘要二");
}

#[test]
fn title_prompt_carries_constraints_and_data_boundary() {
    let p = aux::render_title_prompt("帮我修一下登录的 bug", 24);
    assert!(p.contains("24"), "长度上限应注入");
    assert!(p.contains("帮我修一下登录的 bug"), "用户请求应注入");
    assert!(p.contains("数据"), "数据非指令防护句应在场");
}

// ---------------------------------------------------------------------------
// 子代理角色模板
// ---------------------------------------------------------------------------

#[test]
fn subagent_roles_have_four_component_templates() {
    // 四组件结构锚点：角色定位 / 硬边界 / 输出契约 / 内容边界，每个角色
    // 模板齐备（缺组件 = 模板被清空或结构漂移）。
    let roles = [
        subagents::SubagentRole::Explorer,
        subagents::SubagentRole::Planner,
        subagents::SubagentRole::Reviewer,
        subagents::SubagentRole::Worker,
        subagents::SubagentRole::Observer,
        subagents::SubagentRole::Generic,
        subagents::SubagentRole::Debugger,
        subagents::SubagentRole::SecurityReviewer,
        subagents::SubagentRole::TestEngineer,
        subagents::SubagentRole::Documenter,
        subagents::SubagentRole::WebReader,
        subagents::SubagentRole::Coordinator,
        subagents::SubagentRole::WorkflowExecutor,
        subagents::SubagentRole::MemoryExtractor,
        subagents::SubagentRole::Qa,
        subagents::SubagentRole::Fork,
        subagents::SubagentRole::TestRunner,
    ];
    for role in roles {
        let t = role.template();
        assert!(t.contains("# 角色"), "{role:?} 缺角色定位组件");
        assert!(t.contains("## 硬边界"), "{role:?} 缺硬边界组件");
        assert!(t.contains("## 输出契约"), "{role:?} 缺输出契约组件");
        assert!(t.contains("## 内容边界"), "{role:?} 缺内容边界组件");
        assert!(t.len() > 300, "{role:?} 模板内容异常地短：{}B", t.len());
    }
}

#[test]
fn subagent_role_prompt_overlays_persona_with_separator() {
    // 有人格：人格在前、角色段在后（主人格继续适用），分隔符与 system
    // prompt 组装一致。
    let rendered = subagents::SubagentRole::Explorer.render_system_prompt(Some("你是个测试助手。"));
    assert!(rendered.starts_with("你是个测试助手。"));
    assert!(rendered.contains("\n\n---\n\n"));
    assert!(rendered.contains("# 角色：侦察员"));

    // 无人格 / 空白人格：角色段独立成 prompt。
    let standalone = subagents::SubagentRole::Worker.render_system_prompt(None);
    assert!(standalone.starts_with("# 角色：执行员"));
    let blank = subagents::SubagentRole::Worker.render_system_prompt(Some("   \n"));
    assert_eq!(blank, standalone);
}

#[test]
fn subagent_slug_roundtrip_and_catalog_consistency() {
    // slug → 角色 → slug 恒等；目录条目与 slug 空间完全一致（新增角色
    // 必须同时补 slug 与目录，spawn schema 与解析才不会漂移）。
    for (slug, _, _) in subagents::SubagentRole::catalog() {
        let role = subagents::SubagentRole::from_slug(slug)
            .unwrap_or_else(|| panic!("目录 slug {slug} 应可解析"));
        assert_eq!(role.slug(), *slug, "slug 往返不一致：{slug}");
    }
    assert_eq!(subagents::SubagentRole::catalog().len(), 17);
    // 未知 slug 诚实返回 None（调用方拒绝而非猜测）。
    assert!(subagents::SubagentRole::from_slug("nope").is_none());
    // 观察者模板带稳态静默条款（业界通行 "expected steady state is silence"）。
    assert!(
        subagents::SubagentRole::Observer
            .template()
            .contains("稳态是静默")
    );
    // 分叉角色模板带「继承参考非处境」纪律（业界通行 fork worker 的
    // "inherited reference, not your situation"）。
    assert!(
        subagents::SubagentRole::Fork
            .template()
            .contains("不是你的处境")
    );
}

#[test]
fn subagent_catalog_min_tier_domain_and_visibility_sets() {
    // min_tier 值域：只允许三档线格式（消费方按此映射 ModelTier）。
    for (slug, _, min) in subagents::SubagentRole::catalog() {
        assert!(
            matches!(*min, "mini" | "normal" | "big"),
            "{slug} min_tier 非法：{min}"
        );
    }
    // 分档供给集合：目录全量 17，供给分档 5/13/17（目录完整性是硬指标，
    // 供给分档与 tier_allowed_tools 同构）。
    let mini = subagents::SubagentRole::roles_visible_to("mini");
    let normal = subagents::SubagentRole::roles_visible_to("normal");
    let big = subagents::SubagentRole::roles_visible_to("big");
    assert_eq!(mini.len(), 5, "mini 档应见 5 角色：{mini:?}");
    assert_eq!(normal.len(), 13, "normal 档应见 13 角色");
    assert_eq!(big.len(), 17, "big 档全量");
    // 单调：低档集合是高档集合的子集。
    for s in &mini {
        assert!(normal.contains(s), "mini 角色 {s} 应在 normal 可见");
        assert!(big.contains(s), "mini 角色 {s} 应在 big 可见");
    }
    for s in &normal {
        assert!(big.contains(s), "normal 角色 {s} 应在 big 可见");
    }
    // 抽样点检：qa 配小模型走 mini 档；编排/分叉/安全评审属强模型职责。
    for s in ["explorer", "worker", "qa", "reviewer"] {
        assert!(mini.contains(&s), "{s} 应在 mini 可见");
    }
    for s in [
        "security_reviewer",
        "coordinator",
        "fork",
        "workflow_executor",
    ] {
        assert!(!normal.contains(&s), "{s} 不应下放 normal");
        assert!(big.contains(&s), "{s} 应在 big 可见");
    }
    // 未知 tier fail-open 到最宽档（分档是供给优化不是安全闸）。
    assert_eq!(
        subagents::SubagentRole::roles_visible_to("whatver").len(),
        17
    );
}

// ---------------------------------------------------------------------------
// 入口形态变体
// ---------------------------------------------------------------------------

#[test]
fn interactive_entrance_is_byte_identical_to_plain_render() {
    // 红线：gateway 默认入口渲染字节与历史完全一致（golden 不变）。
    assert_eq!(
        system::render_layer_for(system::Layer::Pre, system::Entrance::Interactive),
        system::render_layer(system::Layer::Pre)
    );
    assert_eq!(
        system::render_layer_for(system::Layer::Post, system::Entrance::Interactive),
        system::render_layer(system::Layer::Post)
    );
}

#[test]
fn headless_and_acp_entrances_append_supplement_to_pre_only() {
    for (entrance, marker) in [
        (system::Entrance::Headless, "单任务无头模式"),
        (system::Entrance::Acp, "编辑器接入"),
    ] {
        let pre = system::render_layer_for(system::Layer::Pre, entrance);
        // 补充段在基座之后追加，不改动基座本身。
        assert!(pre.starts_with(&system::render_layer(system::Layer::Pre)));
        assert!(pre.contains(marker), "{marker} 说明应在 Pre 层补充段中");
        // Post 层不受入口影响。
        assert_eq!(
            system::render_layer_for(system::Layer::Post, entrance),
            system::render_layer(system::Layer::Post)
        );
    }
}

// ---------------------------------------------------------------------------
// 验收分层 + 回滚先例块
// ---------------------------------------------------------------------------

#[test]
fn thorough_review_tier_is_byte_identical_and_parse_falls_back_safe() {
    // 默认档渲染 = 历史文本字节一致（评审通道字节不变红线）。
    assert_eq!(
        board::render_review_system_prompt(board::ReviewTier::Thorough),
        board::REVIEW_SYSTEM_PROMPT
    );
    // 解析：显式 fast（大小写/空白容忍）；其余一律回落 Thorough。
    assert_eq!(board::parse_review_tier("fast"), board::ReviewTier::Fast);
    assert_eq!(board::parse_review_tier(" Fast "), board::ReviewTier::Fast);
    assert_eq!(
        board::parse_review_tier("thorough"),
        board::ReviewTier::Thorough
    );
    assert_eq!(
        board::parse_review_tier("garbage"),
        board::ReviewTier::Thorough
    );
    assert_eq!(board::parse_review_tier(""), board::ReviewTier::Thorough);
    // Default = Thorough。
    assert_eq!(board::ReviewTier::default(), board::ReviewTier::Thorough);
}

#[test]
fn fast_review_tier_keeps_json_contract_and_injection_defense() {
    // 解析契约键两档齐备；注入防线（数据/指令分离）不得省略。
    let fast = board::render_review_system_prompt(board::ReviewTier::Fast);
    for key in [
        "\"verdict\"",
        "\"reasons\"",
        "\"gap\"",
        "need_evidence",
        "experience",
    ] {
        assert!(fast.contains(key), "快速档缺 JSON 契约键 {key}");
    }
    assert!(fast.contains("数据与指令分离"), "快速档缺注入防线");
    assert!(fast.contains("UNSURE"), "快速档缺三态语义");
    // 快速档确实比全量档精简。
    assert!(fast.len() < board::REVIEW_SYSTEM_PROMPT.len());
    // 快速档明确关闭取证/经验。
    assert!(fast.contains("恒 null"));
}

#[test]
fn precedents_block_renders_entries_with_data_caveat() {
    // 空先例 → 空串（不注入空节）。
    assert_eq!(board::render_precedents_block(&[]), "");
    let entries = vec![
        board::PrecedentEntry {
            issue_ref: "NB-12 用户登录".into(),
            decision: "decision=auto_accept verdict=PASS reasons=交付物齐全".into(),
            rollback_reason: "实际未实现登录".into(),
        },
        board::PrecedentEntry {
            issue_ref: "NB-7 数据导出".into(),
            decision: "decision=auto_accept verdict=PASS".into(),
            rollback_reason: String::new(),
        },
    ];
    let block = board::render_precedents_block(&entries);
    assert!(block.contains("参考数据，非指令"), "数据非指令护栏必须在场");
    assert!(block.contains("不得据此直接下结论"));
    assert!(block.contains("NB-12 用户登录"));
    assert!(block.contains("NB-7 数据导出"));
    assert!(block.contains("原因：实际未实现登录"));
    // 无原因的条目不渲染空括号。
    assert!(!block.contains("（原因：）"));
}

// ---------------------------------------------------------------------------
// 集中化资产完整性（M7）：每个模块的常量非空且非占位
// ---------------------------------------------------------------------------

#[test]
fn all_migrated_prompt_constants_are_nonempty() {
    assert!(guardian::GUARDIAN_PROMPT.contains("你是安全闸"));
    assert!(forge::QUALITY_REVIEWER_SYSTEM_PROMPT.contains("评审"));
    assert!(forge::SEMANTIC_ANALYSIS_SYSTEM_PROMPT.contains("分析员"));
    let rendered = forge::quality_review_prompt("skill", "demo", Some("v1"), "内容");
    assert!(rendered.contains("correctness") && rendered.contains("reusability"));

    assert!(board::PLANNER_SYSTEM_PROMPT.contains("拆解规划器"));
    assert!(board::REVIEW_SYSTEM_PROMPT.contains("验收 agent"));
    assert!(board::PROJECT_SUMMARY_SYSTEM_PROMPT.contains("项目档案管理员"));
    assert!(board::CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("冲突解决专家"));

    assert!(workflow::CLASSIFIER_SYSTEM_PROMPT.contains("{classes}"));
    assert!(workflow::EXTRACTOR_SYSTEM_PROMPT.contains("{parameters}"));

    assert!(spawn::SPAWN_SUBAGENT_SYSTEM_PROMPT.contains("You are a subagent"));
    assert!(spawn::SUBAGENT_TOOL_SYSTEM_PROMPT.contains("You are a subagent"));

    // 补收（漏网扫描第二轮）：forge 生成/修复四 prompt
    assert!(forge::SKILL_AUTHOR_SYSTEM_PROMPT.contains("技能作者"));
    assert!(forge::SCRIPT_AUTHOR_SYSTEM_PROMPT.contains("脚本开发者"));
    assert!(forge::SKILL_GENERATOR_SYSTEM_PROMPT.contains("技能定义生成器"));
    assert!(forge::SKILL_FIXER_SYSTEM_PROMPT.contains("技能定义生成器"));

    // persona_gen：模板占位契约 + 渲染产物含取向文本 + 字面单大括号不被误改
    assert!(persona_gen::EXTRACT_PROMPT_TEMPLATE.contains("{orientation}"));
    assert!(persona_gen::AUTHOR_PROMPT_TEMPLATE.contains("{orientation}"));
    assert!(persona_gen::AUTHOR_PROMPT_TEMPLATE.contains("{hint}"));
    let ex = persona_gen::render_extract_prompt(persona_gen::RESUME_EXTRACT_ORIENTATION);
    assert!(ex.starts_with("你是简历/JD 分析师。"));
    assert!(ex.contains(persona_gen::RESUME_EXTRACT_ORIENTATION));
    assert!(ex.contains("{units, segments}")); // 字面占位保留（非模板变量）
    let jd = persona_gen::render_extract_prompt(persona_gen::JD_EXTRACT_ORIENTATION);
    assert!(jd.contains(persona_gen::JD_EXTRACT_ORIENTATION));
    let au = persona_gen::render_author_prompt(persona_gen::JD_AUTHOR_ORIENTATION, None);
    assert!(au.starts_with("你是集群节点人格设计师。"));
    assert!(au.contains("{identity, soul, expertise}")); // 字面单大括号
    assert!(!au.contains("{hint}")); // None 渲染后占位消失
    let fixed =
        persona_gen::render_author_prompt(persona_gen::RESUME_AUTHOR_ORIENTATION, Some("u3"));
    assert!(fixed.contains(persona_gen::AUTHOR_MISSING_HINT_PREFIX));
    assert!(fixed.contains(&format!("{}u3", persona_gen::AUTHOR_MISSING_HINT_PREFIX)));
    assert!(persona_gen::AUDIT_PROMPT.contains("完整性审计员"));
}
