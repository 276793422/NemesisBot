//! optimize 算子单测：四算子 + 预算 + 无变化 + 确定性。

use super::*;

const BODY: &str =
    "任务开头说明。\n\n## 步骤\n\n第一步做事。\n\n## 注意\n\n- 不要越权\n\n## 输出\n\n输出结果。\n";

#[test]
fn reorder_moves_constraints_first() {
    let c = apply(
        Operator::ReorderConstraints,
        BODY,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    let cons_pos = c.content.find("## 注意").unwrap();
    let steps_pos = c.content.find("## 步骤").unwrap();
    assert!(cons_pos < steps_pos);
    assert!(c.content.contains("不要越权"));
}

#[test]
fn reorder_keeps_intro_before_constraint_section() {
    // 引导文本必须在「## 约束」段之前——插到约束段之后且无标题分隔会被
    // 渲染并进该节（文档结构损坏，二轮复查根修回归锁）。
    let c = apply(
        Operator::ReorderConstraints,
        BODY,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    let intro_pos = c.content.find("任务开头说明").unwrap();
    let cons_pos = c.content.find("## 注意").unwrap();
    assert!(
        intro_pos < cons_pos,
        "引导文本必须先于约束段：{}",
        c.content
    );
    // 引导文本与约束标题之间保持段落分隔（空行），不被并进节正文。
    let between = &c.content[intro_pos..cons_pos];
    assert!(between.contains("\n\n"), "intro 与约束标题间应有空行");
}

#[test]
fn reorder_without_constraints_is_err() {
    let body = "## 步骤\n\n做事。\n";
    assert!(
        apply(
            Operator::ReorderConstraints,
            body,
            None,
            OPTIMIZE_BUDGET_TOKENS
        )
        .is_err()
    );
}

#[test]
fn add_example_appends_section() {
    let c = apply(
        Operator::AddExample,
        BODY,
        Some("输入：重构登录 → 输出：方案"),
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(c.content.contains("## 示例"));
    assert!(c.content.contains("重构登录"));
}

#[test]
fn add_example_requires_param() {
    assert!(apply(Operator::AddExample, BODY, None, OPTIMIZE_BUDGET_TOKENS).is_err());
}

#[test]
fn clarify_collects_inline_constraints() {
    let body = "说明。\n- 不要直接改生产库\n- 必须先跑测试\n\n## 输出\n\n结果。\n";
    let c = apply(
        Operator::ClarifyConstraints,
        body,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(c.content.contains("## 约束"));
    assert!(c.content.contains("不要直接改生产库"));
    assert!(c.content.contains("必须先跑测试"));
    assert_eq!(c.summary, "收拢 2 条散点约束为显式段");
}

#[test]
fn clarify_without_markers_is_err() {
    assert!(
        apply(
            Operator::ClarifyConstraints,
            "## 输出\n\n结果。\n",
            None,
            OPTIMIZE_BUDGET_TOKENS
        )
        .is_err()
    );
}

#[test]
fn inject_failure_case_appends() {
    let c = apply(
        Operator::InjectFailureCase,
        BODY,
        Some("曾直接删库导致数据丢失"),
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(c.content.contains("## 已知失败案例"));
    assert!(c.content.contains("删库"));
}

#[test]
fn budget_overflow_is_err() {
    let big = "x".repeat(OPTIMIZE_BUDGET_TOKENS as usize * 2 + 1);
    let r = apply(
        Operator::AddExample,
        &big,
        Some("示例"),
        OPTIMIZE_BUDGET_TOKENS,
    );
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("exceeds budget"));
}

#[test]
fn custom_budget_is_respected() {
    // config 可调预算生效：小预算拦正常长度候选。
    let r = apply(Operator::AddExample, BODY, Some("示例"), 8);
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("exceeds budget"));
}

#[test]
fn single_section_reorder_is_err() {
    let body = "## 示例\n\n已有示例。\n";
    assert!(
        apply(
            Operator::ReorderConstraints,
            body,
            None,
            OPTIMIZE_BUDGET_TOKENS
        )
        .is_err()
    );
}

#[test]
fn deterministic_output() {
    let a = apply(
        Operator::ReorderConstraints,
        BODY,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    let b = apply(
        Operator::ReorderConstraints,
        BODY,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert_eq!(a.content, b.content);
}

#[test]
fn frontmatter_preserved() {
    let content =
        "---\nname: demo\ndescription: d\n---\n## 步骤\n\n做事。\n\n## 注意\n\n- 不要越权\n";
    let c = apply(
        Operator::ReorderConstraints,
        content,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(c.content.starts_with("---\nname: demo"));
    assert!(c.content.contains("description: d"));
}

#[test]
fn frontmatter_preserved_with_crlf() {
    // Windows 编辑器 CRLF 形态：frontmatter 识别失败 = 候选丢元数据
    // （数据丢失，二轮复查根修回归锁）。
    let content = "---\r\nname: demo\r\ndescription: d\r\n---\r\n## 步骤\r\n\r\n做事。\r\n\r\n## 注意\r\n\r\n- 不要越权\r\n";
    let c = apply(
        Operator::ReorderConstraints,
        content,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(c.content.starts_with("---\nname: demo"), "{}", c.content);
    assert!(c.content.contains("description: d"), "{}", c.content);
    assert!(c.content.contains("## 注意"), "{}", c.content);
}

#[test]
fn no_frontmatter_no_leading_blank() {
    // 无 frontmatter 的候选不得带前导空行——fm 为空串时旧实现
    // format!("{fm}\n…") 注入 "\n"，人工应用后逐轮累积（R72 根修回归锁）。
    let body = "## 步骤\n\n做事。\n\n## 注意\n\n- 不要越权\n";
    let c = apply(
        Operator::ReorderConstraints,
        body,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(
        !c.content.starts_with('\n'),
        "候选头部不得有空行: {:?}",
        c.content
    );
    assert!(c.content.starts_with("## "));
}

#[test]
fn empty_frontmatter_no_leading_blank() {
    // 空 fence（---\n---）按无 frontmatter 处理（C-F5 语义，fence 行作为
    // 引导文本原样保留在头部）——候选同样不得带前导空行。
    let content = "---\n---\n## 步骤\n\n做事。\n\n## 注意\n\n- 不要越权\n";
    let c = apply(
        Operator::ReorderConstraints,
        content,
        None,
        OPTIMIZE_BUDGET_TOKENS,
    )
    .unwrap();
    assert!(
        !c.content.starts_with('\n'),
        "候选头部不得有空行: {:?}",
        c.content
    );
    assert!(c.content.starts_with("---"), "{}", c.content);
}

#[test]
fn operator_parse_roundtrip() {
    for op in [
        Operator::ReorderConstraints,
        Operator::AddExample,
        Operator::ClarifyConstraints,
        Operator::InjectFailureCase,
    ] {
        assert_eq!(Operator::parse(op.as_str()), Some(op));
    }
    assert!(Operator::parse("nope").is_none());
}
