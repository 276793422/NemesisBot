// types.rs 覆盖率补充测试（MessageContent 的 Default 128-130、Parts 数组
// 序列化 147-149、反序列化宽容臂 null/unit 166-168）。

use super::*;

/// Default = 空纯文本（128-130）。
#[test]
fn message_content_default_is_empty_text() {
    assert_eq!(
        MessageContent::default(),
        MessageContent::Text(String::new())
    );
}

/// 序列化契约：Text → JSON 字符串；Parts → JSON 数组；数组/null 反序列化
/// 回读（null → 空 Text，visit_unit 宽容臂）。
#[test]
fn message_content_serde_roundtrips_all_forms() {
    assert_eq!(
        serde_json::to_string(&MessageContent::Text("你好".to_string())).unwrap(),
        "\"你好\""
    );

    let parts = MessageContent::Parts(vec![ContentPart::Text {
        text: "a".to_string(),
    }]);
    let json = serde_json::to_string(&parts).unwrap();
    assert!(json.starts_with('['), "{json}");

    let back: MessageContent = serde_json::from_str(&json).unwrap();
    assert_eq!(back, parts);

    let null_back: MessageContent = serde_json::from_str("null").unwrap();
    assert_eq!(null_back, MessageContent::Text(String::new()));
}

/// 非 string/array 的 JSON（数字）→ 反序列化错误走 expecting 文案
/// （147-149）。
#[test]
fn message_content_rejects_scalar_with_expecting_message() {
    let err = serde_json::from_str::<MessageContent>("42").unwrap_err();
    assert!(
        err.to_string()
            .contains("a string or an array of content parts"),
        "{err}"
    );
}
