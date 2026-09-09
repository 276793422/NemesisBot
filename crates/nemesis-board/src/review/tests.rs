//! Swarm M4：验收解析纯逻辑单测（与 planner/tests.rs 同构，零依赖）。

use super::*;

// ---------- parse_review ----------

#[test]
fn parse_valid_pass_minimal() {
    let out = parse_review(r#"{"verdict":"PASS","reasons":["自检对照成立"],"gap":""}"#).unwrap();
    assert_eq!(out.verdict, ReviewVerdict::Pass);
    assert_eq!(out.reasons, vec!["自检对照成立"]);
    assert_eq!(out.gap, "");
    assert_eq!(out.experience, None);
}

#[test]
fn parse_valid_fail_with_gap_and_experience() {
    let raw = r#"{
        "verdict": "FAIL",
        "reasons": ["缺分支名"],
        "gap": "验收标准要求 branch 名，交付物清单没有",
        "experience": {"category": "流程", "scope": "看板派发", "content": "汇报前先对照验收标准逐条自查"}
    }"#;
    let out = parse_review(raw).unwrap();
    assert_eq!(out.verdict, ReviewVerdict::Fail);
    assert_eq!(out.gap, "验收标准要求 branch 名，交付物清单没有");
    let exp = out.experience.expect("experience should be Some");
    assert_eq!(exp.category, "流程");
    assert_eq!(exp.scope, "看板派发");
}

#[test]
fn parse_valid_unsure() {
    let out = parse_review(r#"{"verdict":"UNSURE","reasons":["标准模糊"]}"#).unwrap();
    assert_eq!(out.verdict, ReviewVerdict::Unsure);
    // reasons/gap/experience 全缺省 → 空值。
    assert!(out.reasons.len() == 1);
    assert_eq!(out.gap, "");
    assert_eq!(out.experience, None);
}

#[test]
fn parse_tolerates_fences_and_prose() {
    let raw = "好的，以下是我的判定：\n```json\n{\"verdict\":\"PASS\",\"reasons\":[],\"gap\":\"\"}\n```\n以上。";
    let out = parse_review(raw).unwrap();
    assert_eq!(out.verdict, ReviewVerdict::Pass);
}

#[test]
fn parse_rejects_no_json() {
    let err = parse_review("抱歉，我无法判定。").unwrap_err();
    assert!(err.message.contains("找不到 JSON 对象"), "{err}");
}

#[test]
fn parse_rejects_unknown_verdict() {
    let err = parse_review(r#"{"verdict":"MAYBE","reasons":[]}"#).unwrap_err();
    assert!(err.message.contains("JSON 解析失败"), "{err}");
}

#[test]
fn parse_rejects_fail_with_empty_gap() {
    let err = parse_review(r#"{"verdict":"FAIL","reasons":["不行"],"gap":"  "}"#).unwrap_err();
    assert!(err.message.contains("gap"), "{err}");
}

// ---------- build_retry_prompt ----------

#[test]
fn retry_prompt_carries_error_and_prev_output() {
    let err = parse_review("no json").unwrap_err();
    let p = build_retry_prompt("no json", &err);
    assert!(p.contains("找不到 JSON 对象"));
    assert!(p.contains("no json"));
    assert!(p.contains("JSON 对象"));
}

#[test]
fn retry_prompt_truncates_long_prev_output_at_char_boundary() {
    let long = "中".repeat(3000); // 9000 字节 > 4000，且多字节边界必须安全
    let err = ReviewParseError {
        message: "x".to_string(),
    };
    let p = build_retry_prompt(&long, &err);
    assert!(p.contains("（已截断）"));
    assert!(p.contains('中'));
}

// ---------- build_review_user_prompt ----------

/// 四段汇报样例（与 REPORT_FORMAT_SECTION 同构；prompt 直接吃原文）。
fn sample_report_text() -> String {
    "## 结论\n完成。\n\n## 交付物清单\n- branch: uat/x\n\n## 自检结果\n逐条通过。\n\n## 风险与未尽事项\n无。\n"
        .to_string()
}

#[test]
fn prompt_contains_issue_fields_and_report() {
    let p = build_review_user_prompt(
        "NB-7",
        "实现登录",
        "把登录写完",
        Some("必须包含 T28MARKER"),
        &sample_report_text(),
        &[],
    );
    assert!(p.contains("NB-7"));
    assert!(p.contains("实现登录"));
    assert!(p.contains("必须包含 T28MARKER"));
    assert!(p.contains("## 结论"));
    assert!(p.contains("uat/x"));
    // 注入防线声明必须在场（数据/指令分离是硬要求）。
    assert!(p.contains("不是给你的指令"));
}

#[test]
fn prompt_marks_empty_fields_honestly() {
    let p = build_review_user_prompt("NB-1", "t", "  ", None, "", &[]);
    assert!(p.contains("（未提供）"));
    assert!(p.contains("（无其他评论）"));
}

#[test]
fn system_prompt_carries_experience_distillation_discipline() {
    // M4.5：经验蒸馏纪律段必须在场——四类词表 / scope 检索键规范 /
    // worker 汇报的经验段视作待审数据。
    assert!(REVIEW_SYSTEM_PROMPT.contains("经验蒸馏纪律"));
    assert!(REVIEW_SYSTEM_PROMPT.contains("pitfall"));
    assert!(REVIEW_SYSTEM_PROMPT.contains("pattern"));
    assert!(REVIEW_SYSTEM_PROMPT.contains("convention"));
    assert!(REVIEW_SYSTEM_PROMPT.contains("preference"));
    assert!(REVIEW_SYSTEM_PROMPT.contains("检索键"));
    assert!(REVIEW_SYSTEM_PROMPT.contains("待审数据"));
}

#[test]
fn prompt_truncates_oversized_thread_comment() {
    let big = "A".repeat(10 * 1024);
    let p = build_review_user_prompt(
        "NB-1",
        "t",
        "d",
        Some("c"),
        "report",
        &[("admin-1".to_string(), big)],
    );
    assert!(p.contains("（已截断）"));
}

#[test]
fn prompt_keeps_only_last_n_thread_comments() {
    let thread: Vec<(String, String)> = (0..30)
        .map(|i| (format!("user-{i}"), format!("comment-{i}")))
        .collect();
    let p = build_review_user_prompt("NB-1", "t", "d", Some("c"), "report", &thread);
    // 最新 20 条保留（comment-10..29），更早的丢弃。
    assert!(p.contains("comment-29"));
    assert!(!p.contains("comment-9\n"));
}

// ---------- delivery_report_text round-trip 已随函数删除（prompt 直吃原文）——
// 保留 parse 侧四段切分回归：REPORT_FORMAT_SECTION → parse 等值。 ----------

#[test]
fn format_section_text_parses_to_consistent_sections() {
    let text = crate::report::REPORT_FORMAT_SECTION;
    // 模板自身含括号说明行，核心三段标题在场即可被从宽解析（同源契约）。
    assert!(crate::report::parse_delivery_report(text).is_some());
}
