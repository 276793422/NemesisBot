//! flow 原语层单测（自 flow_engine/tests.rs 随搬迁收编）：token 渲染 /
//! 外链 scheme 闸 / JSON 标量化 / JSON path 子集 / plugin 步出参四态。

use super::*;
use serde_json::json;

#[test]
fn render_tokens_lookup_and_unknown_passthrough() {
    // 命中键替换；未知 token 原样保留（诚实不猜）。
    assert_eq!(
        render_tokens("a-{{x}}-b", |k| if k == "x" {
            Some("1".into())
        } else {
            None
        }),
        "a-1-b"
    );
    assert_eq!(
        render_tokens("{{ghost}}", |_| None),
        "{{ghost}}",
        "未知 token 原样保留"
    );
    // 未闭合 `{{` 原样透传（非 ASCII 安全）。
    assert_eq!(
        render_tokens("x{{ unclosed", |_| Some("y".into())),
        "x{{ unclosed"
    );
    assert_eq!(
        render_tokens("标题{{key}}", |k| if k == "key" {
            Some("值".into())
        } else {
            None
        }),
        "标题值"
    );
}

#[test]
fn safe_external_url_scheme_gate() {
    // http(s) 放行且保原形；大小写不敏感；其余 scheme / 空 = None。
    assert_eq!(
        safe_external_url("https://example.com/p/1").as_deref(),
        Some("https://example.com/p/1")
    );
    assert_eq!(
        safe_external_url("HTTP://Example.com").as_deref(),
        Some("HTTP://Example.com")
    );
    assert_eq!(
        safe_external_url("  https://x.y  ").as_deref(),
        Some("https://x.y")
    );
    assert_eq!(safe_external_url("javascript:alert(1)"), None);
    assert_eq!(safe_external_url("ftp://x"), None);
    assert_eq!(safe_external_url(""), None);
}

#[test]
fn split_compare_expr_forms() {
    // 同位取最长 op（>= 不被 > 截断）；两侧 trim + 成对引号剥离。
    let (l, op, r) = split_compare_expr("$.a >= done").unwrap();
    assert_eq!((l, op, r), ("$.a", ">=", "done"));
    let (l, op, r) = split_compare_expr("'x' == \"x\"").unwrap();
    assert_eq!((l, op, r), ("x", "==", "x"));
    // 任一侧空 = None（畸形）。
    assert!(split_compare_expr("== done").is_none());
    assert!(split_compare_expr("a ==").is_none());
    assert!(split_compare_expr("no-op").is_none());
}

#[test]
fn compare_str_numeric_and_lexical_arms() {
    // 数值可比走数值面。
    assert!(compare_str(">=", "3", "3"));
    assert!(compare_str("<", "2", "10"));
    assert!(!compare_str("<", "10", "2"));
    // 不可比走字典序。
    assert!(compare_str("<", "abc", "done"));
    assert!(!compare_str("<", "done", "abc"));
    assert!(compare_str("==", "x", "x"));
    assert!(compare_str("!=", "x", "y"));
}

#[test]
fn json_scalar_and_json_path_faces() {
    assert_eq!(json_scalar(Some(&json!(true))), "true");
    assert_eq!(json_scalar(Some(&json!([1, 2]))), "array");
    assert_eq!(json_scalar(Some(&json!({"a": 1}))), "object");
    assert_eq!(json_scalar(Some(&json!(null))), "null");
    assert_eq!(json_scalar(None), "null");
    assert_eq!(json_scalar(Some(&json!(3.5))), "3.5");

    // 词法：通配段与畸形路径。
    assert_eq!(
        json_path_segments("$.a[*].b").unwrap(),
        vec!["a".to_string(), "*".to_string(), "b".to_string()]
    );
    assert!(json_path_segments("$.").is_none(), "尾随空段");
    assert!(json_path_segments("$.a..b").is_none(), "连续点空段");
    assert!(json_path_segments("$[0]").is_none(), "索引语法不支持");
    assert!(json_path_segments("a.b").is_none(), "缺 $ 前缀");

    // 求值：通配展开、字段缺席、畸形路径。
    let v = json!({"items": [{"name": "x"}, {"name": "y"}]});
    let hits = json_path_eval(&v, "$.items[*].name");
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0], &json!("x"));
    assert!(json_path_eval(&v, "$.nope[*]").is_empty(), "中途落空 = 空");
    assert!(json_path_eval(&v, "$bad").is_empty(), "畸形路径 = 空");

    // 首值投影。
    assert_eq!(json_path_first(&v, "$.items[*].name"), Some(&json!("x")));
}

#[test]
fn plugin_step_outcome_four_states() {
    // done：status 剥离，其余负载 = artifacts。
    let o = plugin_step_outcome(r#"{"status":"done","report_path":"/x/r.md","n":3}"#);
    assert_eq!(
        o,
        PluginOutcome::Done(json!({"report_path":"/x/r.md","n":3}))
    );
    assert_eq!(
        plugin_step_outcome(r#"{"status":"done"}"#),
        PluginOutcome::Done(json!({}))
    );
    assert_eq!(
        plugin_step_outcome(r#"{"status":"running"}"#),
        PluginOutcome::Running
    );
    let o = plugin_step_outcome(r#"{"status":"failed","error":"上游 502"}"#);
    assert_eq!(o, PluginOutcome::Failed("上游 502".to_string()));
    let o = plugin_step_outcome(r#"{"status":"failed"}"#);
    assert!(
        matches!(o, PluginOutcome::Failed(ref m) if m.contains("guest 自报 failed")),
        "o: {o:?}"
    );
    let o = plugin_step_outcome(r#"{"status":"skipped","why":"已存在"}"#);
    assert_eq!(o, PluginOutcome::Skipped(json!({"why":"已存在"})));
}

#[test]
fn plugin_step_outcome_malformed_faces() {
    let o = plugin_step_outcome("not-json-at-all");
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.contains("非 JSON")),
        "o: {o:?}"
    );
    let o = plugin_step_outcome(r#"{"payload":1}"#);
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.contains("缺 status")),
        "o: {o:?}"
    );
    let o = plugin_step_outcome(r#"{"status":3}"#);
    assert!(matches!(o, PluginOutcome::Malformed(_)), "o: {o:?}");
    let o = plugin_step_outcome(r#"{"status":"wip"}"#);
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.contains("未知 status: wip")),
        "o: {o:?}"
    );
    // 摘要截断预算（ERR_SNIPPET + 少量包装文本）。
    let long = format!("\"{}\"", "x".repeat(ERR_SNIPPET * 3));
    let o = plugin_step_outcome(&long);
    assert!(
        matches!(o, PluginOutcome::Malformed(ref m) if m.chars().count() < ERR_SNIPPET + 60),
        "o: {o:?}"
    );
}
