//! 信封解析单测。

use super::*;

#[test]
fn test_parse_envelope_roundtrip_and_defaults() {
    let raw = serde_json::json!({
        "ns": "board",
        "op": "comment.post",
        "corr_id": "c-91",
        "body": {"content": "hi"}
    });
    let env = parse_envelope(&raw).unwrap();
    assert_eq!(env.v, ENVELOPE_VERSION, "缺 v → serde default 补当前版本");
    assert_eq!(env.ns, "board");
    assert_eq!(env.op, "comment.post");
    assert_eq!(env.corr_id, "c-91");

    let resp = EnvelopeResponse::success(&env, serde_json::json!({"comment_id": 456}));
    assert!(resp.ok);
    let back: serde_json::Value = serde_json::from_value(resp.to_json()).unwrap();
    assert_eq!(back["corr_id"], "c-91");
    assert_eq!(back["op"], "comment.post");
    assert_eq!(back["body"]["comment_id"], 456);
}

#[test]
fn test_parse_envelope_rejects_version_and_missing_fields() {
    let bad_version =
        serde_json::json!({"v": 99, "ns": "board", "op": "x", "corr_id": "", "body": {}});
    let err = parse_envelope(&bad_version).unwrap_err();
    assert_eq!(err.code, error_code::BAD_ENVELOPE);

    let missing = serde_json::json!({"body": {}});
    let err = parse_envelope(&missing).unwrap_err();
    assert_eq!(err.code, error_code::BAD_ENVELOPE);

    // 非 JSON 对象同样拒绝。
    let err = parse_envelope(&serde_json::json!("not-an-object")).unwrap_err();
    assert_eq!(err.code, error_code::BAD_ENVELOPE);
}

#[test]
fn test_response_failure_carries_error_and_corrid() {
    let env = Envelope {
        ns: "board".into(),
        op: "comment.post".into(),
        corr_id: "c-7".into(),
        ..Default::default()
    };
    let resp = EnvelopeResponse::failure(&env, EnvelopeError::new(error_code::UNKNOWN_OP, "nope"));
    let json = resp.to_json();
    assert!(!json["ok"].as_bool().unwrap());
    assert_eq!(json["error"]["code"], error_code::UNKNOWN_OP);
    assert_eq!(json["corr_id"], "c-7");
}

#[test]
fn test_client_msg_id_extraction() {
    let body = serde_json::json!({"client_msg_id": "u-1", "content": "x"});
    assert_eq!(client_msg_id(&body), Some("u-1"));
    let empty = serde_json::json!({"client_msg_id": ""});
    assert_eq!(client_msg_id(&empty), None, "空串视为无幂等键");
    let none = serde_json::json!({"content": "x"});
    assert_eq!(client_msg_id(&none), None);
}
