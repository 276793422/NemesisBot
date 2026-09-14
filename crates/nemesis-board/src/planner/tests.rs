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
