//! planner 纯逻辑测试（Swarm M1）：解析容错 / 校验规则 / 环检测 / 提示词。

use super::*;

// ---------------------------------------------------------------------------
// parse_plan： happy path + 容错
// ---------------------------------------------------------------------------

#[test]
fn parse_plan_happy_path() {
    let raw = r#"[
        {"title":"修编译错误","description":"先让 cargo build 过","required_role":"worker",
         "required_tags":["rust"],"acceptance_criteria":"cargo build 成功","depends_on":[]},
        {"title":"跑测试","description":"全量测试绿","required_role":"worker",
         "required_tags":[],"acceptance_criteria":"cargo test 全绿","depends_on":[0]}
    ]"#;
    let plan = parse_plan(raw).expect("合法拆解应通过");
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].title, "修编译错误");
    assert_eq!(plan[1].depends_on, vec![0]);
    assert_eq!(plan[1].required_tags, Vec::<String>::new());
}

#[test]
fn parse_plan_tolerates_markdown_fence_and_prose() {
    let raw = "好的，以下是拆解：\n```json\n[{\"title\":\"任务A\",\"depends_on\":[]}]\n```\n以上。";
    let plan = parse_plan(raw).expect("围栏+杂散文字应被容忍");
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].title, "任务A");
}

/// P4（2026-10-13 编写陷阱防线）拆解管线 sanity：AC 内的 ` # ` 注释锚点
/// 原样透传——planner 产出的 AC 文本逐字落 PlannedSubIssue，不剥离注释
/// （注释剥离是 anchor 解析层的职责，拆解层不做二次加工）。
#[test]
fn parse_plan_preserves_comment_anchor_ac_verbatim() {
    let raw = r#"[
        {"title":"跑测试","description":"全量绿","acceptance_criteria":"[CHECK] exec:cargo test # 跑全部用例\n[CHECK] re:^测试通过 # 汇报须含测试结论","depends_on":[]}
    ]"#;
    let plan = parse_plan(raw).expect("带注释锚点的拆解应通过");
    assert_eq!(plan.len(), 1);
    assert_eq!(
        plan[0].acceptance_criteria,
        "[CHECK] exec:cargo test # 跑全部用例\n[CHECK] re:^测试通过 # 汇报须含测试结论"
    );
    // 透传文本喂回锚点解析器 → pattern/命令只取 ` # ` 前段（端到端闭环）。
    let (anchors, _, rejected) = crate::parse_anchors(&plan[0].acceptance_criteria);
    assert_eq!(anchors.len(), 2, "{anchors:?}");
    assert!(rejected.is_empty());
    assert_eq!(anchors[0].target, "cargo test");
    assert_eq!(anchors[1].pattern.as_deref(), Some("^测试通过"));
}

#[test]
fn parse_plan_missing_array_is_error() {
    let err = parse_plan("我觉得这个任务不需要拆解。").expect_err("无 JSON 数组必须报错");
    assert!(
        err.message.contains("JSON 数组"),
        "回灌文本须指明问题: {err}"
    );
}

#[test]
fn parse_plan_broken_json_is_error() {
    let err = parse_plan(r#"[{"title":"A",}]"#).expect_err("坏 JSON 必须报错");
    assert!(
        err.message.contains("JSON 解析失败"),
        "回灌文本须可自纠: {err}"
    );
}

#[test]
fn parse_plan_empty_array_is_error() {
    let err = parse_plan("[]").expect_err("空拆解必须报错");
    assert!(err.message.contains("至少"), "回灌文本须说明要求: {err}");
}

#[test]
fn parse_plan_defaults_fill_optional_fields() {
    let plan = parse_plan(r#"[{"title":"只有标题"}]"#).expect("只有 title 应通过（serde 宽容）");
    assert_eq!(plan[0].description, "");
    assert_eq!(plan[0].required_role, "");
    assert!(plan[0].required_tags.is_empty());
    assert!(plan[0].depends_on.is_empty());
}

// ---------------------------------------------------------------------------
// parse_plan：校验规则
// ---------------------------------------------------------------------------

#[test]
fn parse_plan_rejects_over_limit() {
    let items: Vec<String> = (0..=MAX_SUBISSUES)
        .map(|i| format!(r#"{{"title":"任务{i}","depends_on":[]}}"#))
        .collect();
    let raw = format!("[{}]", items.join(","));
    let err = parse_plan(&raw).expect_err("超上限必须报错");
    assert!(err.message.contains("上限"), "回灌文本须说明上限: {err}");
}

#[test]
fn parse_plan_rejects_empty_title() {
    let err = parse_plan(r#"[{"title":"  "}]"#).expect_err("空标题必须报错");
    assert!(err.message.contains("title"), "回灌文本须指明字段: {err}");
}

#[test]
fn parse_plan_rejects_out_of_range_dep() {
    let err = parse_plan(r#"[{"title":"A","depends_on":[5]}]"#).expect_err("越界依赖必须报错");
    assert!(
        err.message.contains("序号 5"),
        "回灌文本须指明越界序号: {err}"
    );
}

#[test]
fn parse_plan_rejects_self_dependency() {
    let err = parse_plan(r#"[{"title":"A","depends_on":[0]}]"#).expect_err("自引用必须报错");
    assert!(
        err.message.contains("自引用"),
        "回灌文本须指明自引用: {err}"
    );
}

#[test]
fn parse_plan_rejects_cycle() {
    let raw = r#"[
        {"title":"A","depends_on":[1]},
        {"title":"B","depends_on":[0]}
    ]"#;
    let err = parse_plan(raw).expect_err("二环必须报错");
    assert!(err.message.contains("循环依赖"), "回灌文本须指明环: {err}");
    assert!(err.message.contains("0 -> 1 -> 0"), "环成员须可读: {err}");
}

#[test]
fn parse_plan_allows_diamond_dependencies() {
    // 菱形（A→B, A→C, B→D, C→D）不是环，必须放行。
    let raw = r#"[
        {"title":"A","depends_on":[]},
        {"title":"B","depends_on":[0]},
        {"title":"C","depends_on":[0]},
        {"title":"D","depends_on":[1,2]}
    ]"#;
    let plan = parse_plan(raw).expect("菱形依赖不是环");
    assert_eq!(plan.len(), 4);
}

// ---------------------------------------------------------------------------
// 提示词构造
// ---------------------------------------------------------------------------

#[test]
fn planner_user_prompt_renders_sections() {
    let prompt = build_planner_user_prompt(
        "实现登录",
        "登录接口 + 前端表单",
        Some("null 用户名返回 400"),
        &[],
        None,
    );
    assert!(prompt.contains("# 父任务"));
    assert!(prompt.contains("标题：实现登录"));
    assert!(prompt.contains("登录接口 + 前端表单"));
    assert!(prompt.contains("null 用户名返回 400"));
    assert!(!prompt.contains("团队经验"), "空经验不得渲染该段");
    assert!(prompt.ends_with("只输出 JSON 数组。"));
}

#[test]
fn planner_user_prompt_injects_team_experience() {
    let prompt = build_planner_user_prompt(
        "标题",
        "",
        None,
        &["cargo 镜像用 rsproxy".to_string()],
        None,
    );
    assert!(prompt.contains("（未提供）"), "空描述/验收须占位");
    assert!(prompt.contains("# 团队经验"));
    assert!(prompt.contains("- cargo 镜像用 rsproxy"));
}

#[test]
fn retry_prompt_carries_error_and_prev_output() {
    let err = parse_plan("不是 JSON").expect_err("构造错误");
    let prompt = build_retry_prompt("上一次的坏输出", &err);
    assert!(prompt.contains("无法通过校验"));
    assert!(prompt.contains("上一次的坏输出"));
    assert!(prompt.contains("JSON 数组"));
}

#[test]
fn retry_prompt_truncates_long_prev_output() {
    let err = PlanParseError {
        message: "e".to_string(),
    };
    let long = "x".repeat(5000);
    let prompt = build_retry_prompt(&long, &err);
    assert!(prompt.len() < long.len(), "超长原输出须截断");
    assert!(prompt.contains("已截断"));
}

/// 回归锁（2026-10-11 真机实证）：裸字节切片 `&prev_output[..4000]` 在
/// 中文输出上第 4000 字节落在多字节字符内部 → panic → 整个规划 tokio
/// 任务无声死亡、看板零留痕（用户项目两次「建了不动」的根因）。截断
/// 必须走 rune 边界安全版。
#[test]
fn retry_prompt_truncates_long_cjk_output_at_char_boundary() {
    let long = "中".repeat(3000); // 9000 字节 > 4000，且字节 4000 恰在「中」内部
    let err = PlanParseError {
        message: "e".to_string(),
    };
    let prompt = build_retry_prompt(&long, &err); // 修复前此处 panic
    assert!(prompt.contains("（已截断）"));
    assert!(prompt.contains('中'));
    // 截断头部必须完整落在字符边界上（按 rune 边界回退，不得撕裂「中」）
    let idx = prompt.find("…（已截断）").expect("截断注记在 prompt 中");
    let _head = &prompt[..idx]; // 非边界则此处 panic
}

// ---------------------------------------------------------------------------
// system prompt 静态自检（提示词与解析器契约一致：改一处必须同步另一处）
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_declares_the_schema_fields() {
    for field in [
        "title",
        "description",
        "required_role",
        "required_tags",
        "acceptance_criteria",
        "depends_on",
    ] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(field),
            "system prompt 必须声明字段 {field}（与 PlannedSubIssue schema 同步）"
        );
    }
    assert!(
        PLANNER_SYSTEM_PROMPT.contains(&format!("不超过 {MAX_SUBISSUES} 个")),
        "system prompt 的数量上限声明须与 MAX_SUBISSUES 常量同步"
    );
}

/// 验收标准自主可达纪律（2026-10-16 NB-121 三道人工门禁根修）防误删快照：
/// planner prompt 必须携带「AC 每条须自主 agent 可达成并留证」硬约束 +
/// 三类禁止形态（人工签字/外部资源/长时实测）+ 等价证据改写指引。
/// 删段则拆解器再次发明自主链路永远无法闭环的门禁，验收阶段必然弹人工。
#[test]
fn system_prompt_declares_ac_autonomous_reachability() {
    for marker in ["自主可达", "人工签字", "等价证据", "待人工签核"] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含验收标准自主可达纪律标记 {marker}"
        );
    }
}

/// P2（B1）防误删快照：planner prompt 必须携带 `[CHECK]` 锚点指令段
/// （与 `crate::anchor::ANCHOR_PREFIX` 及 anchor 解析器同步演化——删段
/// 则 planner 不再产出锚点，P2 双检退化回纯语义）。
#[test]
fn system_prompt_declares_check_anchor_instructions() {
    for marker in ["[CHECK]", "file:", "contains:", "re:", "相对路径"] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含锚点指令标记 {marker}（与 anchor 模块同步）"
        );
    }
}

/// §6.1（2026-10-12 exec 锚点激活）防误删快照：exec 锚点产出指令（对可运行
/// 时验证的子任务**应**产出，按交付物技术栈选命令）+ 命令纪律（只读/测试类，
/// 黑名单拒绝破坏性形态）+ 跨节点安全说明 + 环境依赖缺失诚实跳过语义；
/// 叠加输入/输出资源声明纪律（[READ]/[TOUCH] 与 depends_on 互相印证）。
/// 删段则 exec 产出激励与输入清单声明回归无提示词闸。
#[test]
fn system_prompt_declares_exec_gate_and_resource_declarations() {
    for marker in [
        "应优先为本形态选一条命令",
        "按交付物技术栈选",
        "只读/测试类",
        "黑名单",
        "跨节点安全",
        "诚实跳过（不计失败）",
        "[READ]",
        "输入/输出文件清单",
        "互相印证",
    ] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含 exec 产出指令/资源声明标记 {marker}"
        );
    }
}

/// P1 拓扑纪律防误删快照（2026-09-12 双端真机 S2 根修）：planner prompt
/// 必须声明「远端任务禁止 file: 锚点」——删掉这段 planner 会继续给远端
/// 子任务配 file: 锚点（硬闸会拦，但每次派发都炸成诚实错误，规划面劣化）。
#[test]
fn system_prompt_declares_anchor_topology_discipline() {
    for marker in ["拓扑纪律", "远端", "只允许 `[CHECK] re:` 形态"] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含锚点拓扑纪律标记 {marker}（远端任务禁 file: 锚点）"
        );
    }
}

/// F5（件②）父单锚点下放纪律防误删快照：父任务 AC 含锚点时必须由子任务
/// 继承——删掉这段 planner 拆解会丢父任务锚点（父单收口评审锚点必败）。
#[test]
fn system_prompt_declares_anchor_downgrade_discipline() {
    for marker in ["父单锚点下放", "至少被一个子任务", "永远无法通过核验"] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含父单锚点下放纪律标记 {marker}"
        );
    }
}

/// P4（2026-10-13 编写陷阱防线）防误删快照：planner prompt 必须携带
/// ` # ` 注释语法与反例——删掉这段 planner 会继续把描述文本写进 re:
/// pattern（核验几乎必然失败 → 无谓重派，与 anchor 解析器的显式注释
/// 切分同步演化）。
#[test]
fn system_prompt_declares_anchor_comment_syntax() {
    for marker in [" # ", "反例", "只用前半段匹配"] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含锚点注释语法标记 {marker}（与 anchor 模块同步）"
        );
    }
}

/// 编排者数量优先级防缩编快照（2026-10-09 web-FPS 拆解三连跑偏固化）：
/// planner 默认数量档（3-7 为佳/宁少勿滥）不得压过父任务显式数量指定——
/// 真机实证：glm-5.3-flash 对硬性「恰好 8-10 单」的父单三次按默认档缩编
/// （subs=4/1/5），模型把系统默认当硬约束、冲突时取保守端。删掉优先级
/// 声明则缩编回归无提示词闸。
#[test]
fn system_prompt_declares_parent_count_precedence() {
    for marker in [
        "以父任务的指定为准",
        "不得自行缩编",
        "只适用于父任务未指定数量",
    ] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含父任务数量优先级声明 {marker}（拆解缩编三连跑偏固化）"
        );
    }
}

/// 压测③/⑦（2026-09-30 三设备）防误删快照：需求保真对照拆解（漏项/外卡
/// 双向闸）+ 拆解自检段（输出前逐项核对，锚点下放第 3 次强化）——删段则
/// 保真度回归无提示词闸（场景 A 漏 2 章/场景 B 7 张外卡的教训固化）。
#[test]
fn system_prompt_declares_requirement_fidelity_and_self_check() {
    for marker in [
        "需求保真",
        "清单之外的内容不得拆出子任务",
        "拆解自检",
        "漏项即拆解不合格",
        "外卡即拆解不合格",
    ] {
        assert!(
            PLANNER_SYSTEM_PROMPT.contains(marker),
            "system prompt 必须含需求保真/自检标记 {marker}（压测③⑦固化）"
        );
    }
}

/// 职能框架（M1）同步钉：完整形态 `planner_system_prompt()`（基础契约 +
/// 拆解方法论段）必须 (a) 在 schema 里声明 `required_profession` 字段，
/// (b) 值域清单逐 slug 覆盖 nemesis-prompts 内置目录 CATALOG——prompt 是
/// 字面文本（const 不能插值），目录增删时本测试红出来强制同步。
#[test]
fn planner_prompt_declares_profession_field_and_full_slug_catalog() {
    let p = super::super::planner_system_prompt();
    assert!(
        p.contains("required_profession"),
        "完整 planner prompt 必须声明 required_profession 字段"
    );
    assert!(
        p.contains("无职能需求填空串"),
        "required_profession 的空值语义必须声明"
    );
    for m in nemesis_prompts::professions::meta::CATALOG {
        assert!(
            p.contains(&format!("`{}`", m.slug)),
            "planner prompt 值域清单缺内置职能 `{}`（与 CATALOG 同步）",
            m.slug
        );
    }
    // 精确匹配纪律（无继承）必须传达到 planner。
    assert!(
        p.contains("精确匹配"),
        "职能匹配语义（精确匹配无继承）必须出现在 planner 方法论段"
    );
}

/// planner 提示词软预算钉（对齐 nemesis-prompts 对主 prompt
/// `SOFT_BUDGET_BYTES` 的测试先例）：完整形态必须低于预算——lens/变体规则
/// 等新增段落叠加把体量推过线时，本测试红出来强制治理（超限运行期只 WARN
/// 不截断；这里是防患闸门）。
#[test]
fn planner_prompt_under_soft_budget() {
    let p = super::super::planner_system_prompt();
    assert!(
        p.len() < nemesis_prompts::board::PLANNER_SOFT_BUDGET_BYTES,
        "planner 提示词 {} 字节超过软预算 {} 字节——提示词资产失控，检查新增长度",
        p.len(),
        nemesis_prompts::board::PLANNER_SOFT_BUDGET_BYTES
    );
}

/// R-10（goal P4）：集群画像注入——有画像时渲染专用段+约束语，无则不渲染。
#[test]
fn planner_user_prompt_renders_cluster_profile() {
    let profile = "- Alex [role=worker tags=python]\n- Bob [role=worker tags=node]";
    let prompt = build_planner_user_prompt("标题", "描述", None, &[], Some(profile));
    assert!(prompt.contains("# 可用执行节点"));
    assert!(prompt.contains("Alex"));
    assert!(prompt.contains("拆解必须"), "约束语必须出现");
}

#[test]
fn planner_user_prompt_no_profile_no_section() {
    let prompt = build_planner_user_prompt("标题", "描述", None, &[], None);
    assert!(!prompt.contains("可用执行节点"), "无画像不得渲染空段");
}

/// R-9：[TOUCH] 行解析（去重/trim/非 TOUCH 行忽略）。
#[test]
fn test_parse_touch_paths() {
    let ac = "[TOUCH] client/game.js\n[TOUCH] server/server.py\n普通文字行\n[TOUCH] client/game.js";
    let paths = super::super::parse_touch_paths(ac);
    assert_eq!(paths, vec!["client/game.js", "server/server.py"]);
    assert!(super::super::parse_touch_paths("no touch here").is_empty());
}

// ---------------------------------------------------------------------------
// E6 共享文件分析（看板项目档案 goal）：并行写同路径 → 回灌重试
// ---------------------------------------------------------------------------

/// 两个子任务 [TOUCH] 同一路径且无依赖边 → 校验失败，回灌文本点名双方与路径。
#[test]
fn shared_touch_without_dep_edge_is_rejected() {
    let raw = r#"[
        {"title":"改协议头","acceptance_criteria":"[TOUCH] common.h","depends_on":[]},
        {"title":"改实现","acceptance_criteria":"[TOUCH] common.h","depends_on":[]}
    ]"#;
    let err = parse_plan(raw).expect_err("并行写同文件必须拦下回灌");
    assert!(
        err.message.contains("共享文件冲突"),
        "回灌须指明冲突类型: {err}"
    );
    assert!(
        err.message.contains("common.h"),
        "回灌须点名冲突路径: {err}"
    );
    assert!(
        err.message.contains("第 0") && err.message.contains("第 1"),
        "回灌须点名双方序号: {err}"
    );
    assert!(
        err.message.contains("depends_on"),
        "回灌须给出修正方向: {err}"
    );
}

/// 同路径但有依赖边（串行链）→ 放行：后者基线已含前者合入。
#[test]
fn shared_touch_with_dep_edge_is_accepted() {
    let raw = r#"[
        {"title":"改协议头","acceptance_criteria":"[TOUCH] common.h","depends_on":[]},
        {"title":"改实现","acceptance_criteria":"[TOUCH] common.h","depends_on":[0]}
    ]"#;
    let plan = parse_plan(raw).expect("有依赖边的串行写同文件应放行");
    assert_eq!(plan.len(), 2);
}

/// 反向依赖边（后者 depend 前者）同样放行（串行即安全，方向无关）。
#[test]
fn shared_touch_reverse_dep_edge_is_accepted() {
    let raw = r#"[
        {"title":"A","acceptance_criteria":"[TOUCH] f.rs","depends_on":[1]},
        {"title":"B","acceptance_criteria":"[TOUCH] f.rs","depends_on":[]}
    ]"#;
    parse_plan(raw).expect("任一方向依赖边都应放行");
}

/// 不同路径互不干扰；同子任务内重复声明同一路径不误报。
#[test]
fn distinct_paths_and_intra_sub_dup_are_accepted() {
    let raw = r#"[
        {"title":"A","acceptance_criteria":"[TOUCH] a.rs\n[TOUCH] a.rs","depends_on":[]},
        {"title":"B","acceptance_criteria":"[TOUCH] b.rs","depends_on":[]}
    ]"#;
    parse_plan(raw).expect("不同路径/单内重复声明不应拦");
}

/// 多处冲突一次性全列（回灌自纠一轮修完，不挤牙膏）。
#[test]
fn multiple_shared_touch_violations_all_reported() {
    let raw = r#"[
        {"title":"A","acceptance_criteria":"[TOUCH] x.h\n[TOUCH] y.h","depends_on":[]},
        {"title":"B","acceptance_criteria":"[TOUCH] x.h","depends_on":[]},
        {"title":"C","acceptance_criteria":"[TOUCH] y.h","depends_on":[]}
    ]"#;
    let err = parse_plan(raw).expect_err("两组冲突必须全报");
    assert!(
        err.message.contains("x.h") && err.message.contains("y.h"),
        "两组冲突都须点名: {err}"
    );
    assert_eq!(
        err.message.matches("没有依赖边").count(),
        2,
        "冲突对数应全部列出: {err}"
    );
}

/// 三方同写一路径 → 列出全部无依赖边对（3 对）。
#[test]
fn three_way_shared_touch_reports_all_pairs() {
    let raw = r#"[
        {"title":"A","acceptance_criteria":"[TOUCH] z.h","depends_on":[]},
        {"title":"B","acceptance_criteria":"[TOUCH] z.h","depends_on":[]},
        {"title":"C","acceptance_criteria":"[TOUCH] z.h","depends_on":[]}
    ]"#;
    let err = parse_plan(raw).expect_err("三方同写必须拦下");
    assert_eq!(
        err.message.matches("没有依赖边").count(),
        3,
        "0-1/0-2/1-2 三对全列: {err}"
    );
}

/// prompt 同步声明共享文件纪律（规则 6）与锁文件/生成物独立成单。
#[test]
fn system_prompt_declares_shared_file_discipline() {
    let p = PLANNER_SYSTEM_PROMPT;
    assert!(p.contains("共享文件纪律"), "prompt 须含共享文件规则: ");
    assert!(p.contains("锁文件"), "prompt 须含锁文件独立成单: ");
}

// -----------------------------------------------------------------------
// required_profession（集群专业职能框架 M2，D9 三臂）
// -----------------------------------------------------------------------

use crate::planner::{parse_plan, parse_plan_lenient};

/// 臂 1（严格常规路径）：职能格式非法 → PlanParseError 回灌（不产计划）。
#[test]
fn profession_format_invalid_rejected_in_strict_parse() {
    // 缺冒号形态错（大写）。
    let raw = r#"[{"title":"子甲","required_profession":"Dev:CPP"}]"#;
    let err = parse_plan(raw).unwrap_err();
    assert!(err.message.contains("required_profession"), "{err}");
    assert!(err.message.contains("格式非法"), "{err}");
    // 非法字符（空格）。
    let err = parse_plan(r#"[{"title":"子甲","required_profession":"dev cpp"}]"#).unwrap_err();
    assert!(err.message.contains("格式非法"), "{err}");
    // 冒号双段非法。
    let err = parse_plan(r#"[{"title":"子甲","required_profession":"dev:cpp:win"}]"#).unwrap_err();
    assert!(err.message.contains("格式非法"), "{err}");
}

/// 臂 1 补充：合法 slug（含目录外合法形态）→ 保留派发（匹配端诚实找
/// 不到；D9 明文不拦用户自定义职能）。
#[test]
fn profession_legal_but_unknown_slug_preserved() {
    let plan = parse_plan(r#"[{"title":"子甲","required_profession":"dev:cuda"}]"#).unwrap();
    assert_eq!(plan[0].required_profession, "dev:cuda");
    let plan = parse_plan(r#"[{"title":"子甲","required_profession":"dev:cpp"}]"#).unwrap();
    assert_eq!(plan[0].required_profession, "dev:cpp");
}

/// 臂 1 补充：缺字段 / 空串 / 空白 = 无职能需求（serde default 零迁移）。
#[test]
fn profession_absent_or_blank_means_none() {
    let plan = parse_plan(r#"[{"title":"子甲"}]"#).unwrap();
    assert_eq!(plan[0].required_profession, "");
    let plan = parse_plan(r#"[{"title":"子甲","required_profession":"  "}]"#).unwrap();
    assert_eq!(plan[0].required_profession, "  ");
}

/// 臂 2（末轮宽和）：格式非法 → 置空收进计划 + 降级明细返回；其余校验
/// 照常严校（title 空、依赖越界仍报错）。
#[test]
fn lenient_final_round_downgrades_invalid_profession() {
    let raw = r#"[
        {"title":"子甲","required_profession":"BAD SLUG"},
        {"title":"子乙","required_profession":"dev:cpp"},
        {"title":"子丙"}
    ]"#;
    let lp = parse_plan_lenient(raw).unwrap();
    assert_eq!(lp.plan.len(), 3);
    assert_eq!(lp.plan[0].required_profession, "", "非法职能置空");
    assert_eq!(lp.plan[1].required_profession, "dev:cpp", "合法项原样保留");
    assert_eq!(lp.plan[2].required_profession, "");
    assert_eq!(
        lp.downgraded,
        vec![(0, "BAD SLUG".to_string())],
        "降级明细 = (序号, 原始值)"
    );

    // 宽和臂不放宽其他校验。
    let err = parse_plan_lenient(r#"[{"title":"","required_profession":"BAD"}]"#).unwrap_err();
    assert!(err.message.contains("title 为空"), "{err}");
    let err = parse_plan_lenient(r#"[{"title":"子甲","depends_on":[5]}]"#).unwrap_err();
    assert!(err.message.contains("depends_on"), "{err}");
    // 宽和臂对整体非 JSON 的失败面与严格版一致。
    assert!(parse_plan_lenient("not json").is_err());
}

/// 严格可过的计划在宽和臂下零降级原样通过（宽和 = 严格超集，末轮直用
/// 宽和版对成功路径无差）。
#[test]
fn lenient_is_superset_of_strict() {
    let raw = r#"[
        {"title":"子甲","required_profession":"architecture","acceptance_criteria":"[CHECK] file:docs/prd.md exists:## 验收标准"},
        {"title":"子乙","required_profession":"dev","depends_on":[0]}
    ]"#;
    let strict = parse_plan(raw).unwrap();
    let lenient = parse_plan_lenient(raw).unwrap();
    assert_eq!(strict, lenient.plan);
    assert!(lenient.downgraded.is_empty());
}

// -----------------------------------------------------------------------
// required_capabilities（NB-122 批 2026-10-16，环境能力硬闸）
// -----------------------------------------------------------------------

/// 严格臂：能力词形态非法（含空格/空串/超 64 字符）→ PlanParseError 回灌。
#[test]
fn capabilities_format_invalid_rejected_in_strict_parse() {
    // 含空格（节点宣告的 capabilities 都是单段词，含空格永不命中）。
    let err =
        parse_plan(r#"[{"title":"子甲","required_capabilities":["chrome browser"]}]"#).unwrap_err();
    assert!(err.message.contains("required_capabilities"), "{err}");
    // 空串项。
    let err = parse_plan(r#"[{"title":"子甲","required_capabilities":[""]}]"#).unwrap_err();
    assert!(err.message.contains("required_capabilities"), "{err}");
    // 超 64 字符。
    let long = "x".repeat(65);
    let raw = format!(r#"[{{"title":"子甲","required_capabilities":["{long}"]}}]"#);
    let err = parse_plan(&raw).unwrap_err();
    assert!(err.message.contains("required_capabilities"), "{err}");
}

/// 缺字段 / 空数组 / 合法短词 = 零摩擦（serde default；合法词原样保留）。
#[test]
fn capabilities_absent_or_empty_means_none() {
    let plan = parse_plan(r#"[{"title":"子甲"}]"#).unwrap();
    assert!(plan[0].required_capabilities.is_empty());
    let plan = parse_plan(r#"[{"title":"子甲","required_capabilities":[]}]"#).unwrap();
    assert!(plan[0].required_capabilities.is_empty());
    let plan =
        parse_plan(r#"[{"title":"子甲","required_capabilities":["chrome","docker"]}]"#).unwrap();
    assert_eq!(plan[0].required_capabilities, vec!["chrome", "docker"]);
}

/// 宽和臂：非法项剔除、合法项保留；不记 downgrades（该台账是
/// required_profession 专属）；其余校验照常严校。
#[test]
fn lenient_final_round_drops_invalid_capabilities() {
    let raw = r#"[
        {"title":"子甲","required_capabilities":["chrome","bad word","","docker","LONGWORD_PLACEHOLDER"]},
        {"title":"子乙","required_capabilities":["rust"]}
    ]"#;
    let raw = raw.replace("LONGWORD_PLACEHOLDER", &"x".repeat(65));
    let lp = parse_plan_lenient(&raw).unwrap();
    assert_eq!(
        lp.plan[0].required_capabilities,
        vec!["chrome", "docker"],
        "非法项剔除（空/含空白/超长），合法项保留"
    );
    assert_eq!(lp.plan[1].required_capabilities, vec!["rust"]);
    assert!(lp.downgraded.is_empty(), "能力词不进职能降级台账");
}

/// 提示词同步钉：完整 planner prompt 必须声明 `required_capabilities`
/// 字段（schema）+ 硬条件语义 + 词表纪律（拆解纪律第 9 条）；user 段集群
/// 画像约束行带 caps 词表来源。缺任一段，拆解端就不知道这个词表的存在。
#[test]
fn planner_prompt_declares_required_capabilities_contract() {
    let p = super::super::planner_system_prompt();
    assert!(p.contains("required_capabilities"), "schema 必须声明字段");
    assert!(
        p.contains("环境能力硬闸"),
        "拆解纪律必须声明硬条件语义（无节点具备=无法派发）"
    );
    assert!(p.contains("caps="), "词表纪律必须指向集群画像 caps= 词源");
    assert!(
        p.contains("改按第 8 条拆「装配环境」前置子任务"),
        "普遍缺能力时必须走装配前置子单路径（第 8 条联动）"
    );
    let user = crate::planner::build_planner_user_prompt(
        "标题",
        "描述",
        None,
        &[],
        Some("节点甲 role=worker tags=rust caps=chrome,docker"),
    );
    assert!(
        user.contains("required_capabilities 只能从上述节点 caps="),
        "user 段约束必须声明能力词表来源"
    );
}

// ---------------------------------------------------------------------------
// C1/D2（profession-capability-evolution 2026-10-14）：方法论段落内容钉
// ---------------------------------------------------------------------------

/// C1：dev 子单禁令→锚点翻译规则必须出现在完整 planner prompt（方法论段）。
/// 同一禁令在开发前置必读、拆解锚点、评审分镜三处同源生效——拆解侧缺文
/// 本条规则，翻译链就断在第一环。
#[test]
fn system_prompt_declares_dev_ban_anchor_translation() {
    let p = super::super::planner_system_prompt();
    // 规则 7 三要素：读哪（架构工件禁令节）、译成什么（引用编号的
    // [CHECK] re: 锚点）、缺席语义（跳过不造空锚点）。
    assert!(p.contains("禁令与约束清单"), "缺「禁令与约束清单」出处声明");
    assert!(
        p.contains("翻译为该子单 acceptance_criteria 里的锚点行"),
        "缺禁令→锚点翻译规则"
    );
    assert!(
        p.contains("引用禁令编号"),
        "锚点必须引用禁令编号（评审分镜核对的抓手）"
    );
    assert!(
        p.contains("禁令清单缺席时跳过"),
        "禁令清单缺席语义必须是跳过（不造空锚点）"
    );
}

/// D2：多变体规则（分歧轴三分 + 分歧声明 + 定稿单裁决工件）必须出现在
/// 完整 planner prompt——变体拆解形态是方法论段的编排规则，不在基础
/// 契约里（基础契约只管 JSON schema）。
#[test]
fn system_prompt_declares_variant_and_finalize_rules() {
    let p = super::super::planner_system_prompt();
    // 三条分歧轴（D7：定向不采样——少任何一条，变体退化为随机文档）。
    for axis in ["最保守解读", "字面最大化解读", "重新框定"] {
        assert!(p.contains(axis), "变体分歧轴缺「{axis}」");
    }
    // 分歧声明节（变体的自我声明——没有它讨论无从聚焦）。
    assert!(p.contains("分歧声明"), "变体头部强制分歧声明节未声明");
    // 定稿单裁决工件：「需求评审记录」节 + 胜者定稿路径。
    assert!(
        p.contains("需求评审记录"),
        "定稿单「需求评审记录」工件未声明"
    );
    assert!(p.contains("docs/prd.md"), "胜者定稿路径 docs/prd.md 未声明");
    // 需求模糊触发条件（何时拆变体——不拆变体的判据也要传达）。
    assert!(p.contains("需求模糊"), "变体触发条件（需求模糊判定）未声明");
}
