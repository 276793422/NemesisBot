//! [`crate::history`] 共享组装件测试：字段 golden + 解析行为。

use crate::history::{HistoryRequest, history_response_json};

// ---------------------------------------------------------------------
// HistoryRequest::parse
// ---------------------------------------------------------------------

#[test]
fn parse_full_payload() {
    let req = HistoryRequest::parse(r#"{"request_id":"r1","limit":50,"before_index":7}"#)
        .expect("valid payload");
    assert_eq!(req.request_id, "r1");
    assert_eq!(req.limit, Some(50));
    assert_eq!(req.before_index, Some(7));
    assert_eq!(req.effective_limit(), 50);
}

#[test]
fn parse_defaults_missing_fields() {
    let req = HistoryRequest::parse(r#"{"request_id":"r2"}"#).expect("valid payload");
    assert_eq!(req.request_id, "r2");
    assert_eq!(req.limit, None);
    assert_eq!(req.before_index, None);
    // 缺省条数 = 20（与 loop 路径历史行为一致）。
    assert_eq!(req.effective_limit(), 20);
}

#[test]
fn parse_rejects_garbage() {
    assert!(HistoryRequest::parse("not json").is_err());
    assert!(HistoryRequest::parse(r#"{"limit":"fifty"}"#).is_err());
}

// ---------------------------------------------------------------------
// history_response_json golden（字段集与省略规则是跨端契约）
// ---------------------------------------------------------------------

fn sample_messages() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({"role":"user","content":"你好","seq":1}),
        serde_json::json!({"role":"assistant","content":"回复","seq":2}),
    ]
}

#[test]
fn golden_full_fields() {
    let json = history_response_json(
        "req-9",
        &sample_messages(),
        true,
        3,
        42,
        Some("sid-abc"),
        17,
    )
    .expect("serializable");
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(v["request_id"], "req-9");
    assert_eq!(v["messages"], serde_json::json!(sample_messages()));
    assert_eq!(v["has_more"], serde_json::json!(true));
    assert_eq!(v["oldest_index"], serde_json::json!(3));
    assert_eq!(v["total_count"], serde_json::json!(42));
    // HD：会话归属回显。
    assert_eq!(v["session_id"], "sid-abc");
    // A1：last_seq>0 必须下发。
    assert_eq!(v["last_seq"], serde_json::json!(17));
    // 字段集封闭：不允许契约外字段悄悄进出入（serde_json Map 键序为字典序，
    // 断言按键集合比较）。
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "has_more",
            "last_seq",
            "messages",
            "oldest_index",
            "request_id",
            "session_id",
            "total_count"
        ]
    );
}

#[test]
fn last_seq_zero_is_omitted() {
    let json = history_response_json("r", &sample_messages(), false, 0, 2, Some("s"), 0)
        .expect("serializable");
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    // A1：0 = 未注入回调/环空，序列化时省略（前端走兜底，旧前端零影响）。
    assert!(v.get("last_seq").is_none());
}

#[test]
fn session_id_none_serializes_empty_string() {
    let json = history_response_json("r", &[], false, 0, 0, None, 0).expect("serializable");
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    // 解析失败路径：空串（前端按无归属放行），键仍存在。
    assert_eq!(v["session_id"], "");
}
