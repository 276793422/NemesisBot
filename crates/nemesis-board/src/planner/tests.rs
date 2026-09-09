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
    assert!(err.message.contains("JSON 数组"), "回灌文本须指明问题: {err}");
}

#[test]
fn parse_plan_broken_json_is_error() {
    let err = parse_plan(r#"[{"title":"A",}]"#).expect_err("坏 JSON 必须报错");
    assert!(err.message.contains("JSON 解析失败"), "回灌文本须可自纠: {err}");
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
    assert!(err.message.contains("序号 5"), "回灌文本须指明越界序号: {err}");
}

#[test]
fn parse_plan_rejects_self_dependency() {
    let err = parse_plan(r#"[{"title":"A","depends_on":[0]}]"#).expect_err("自引用必须报错");
    assert!(err.message.contains("自引用"), "回灌文本须指明自引用: {err}");
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
    let prompt = build_planner_user_prompt("标题", "", None, &["cargo 镜像用 rsproxy".to_string()]);
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
