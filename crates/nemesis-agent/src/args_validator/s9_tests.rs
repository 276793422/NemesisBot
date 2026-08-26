//! S9 覆盖率批次：args_validator.rs 剩余未覆盖行。
//! - 338：format_violations 的 "Accepted fields" 追加臂——需要
//!   violations 里含 UnknownField 且 schema 有非空 properties。
//!   经由 `check()` 走歧义 typo 碰撞（两个未知字段最近邻同一目标 →
//!   try_autofix 返 None → Invalid 路径）触达，最贴近生产调用链。
//! - 105：`serde_json::to_string(&fixed)` 失败分支为结构性不可达
//!   （合法 Value 序列化永不失败，见报告豁免组）。

use super::*;

#[test]
fn check_with_ambiguous_typos_reports_accepted_fields() {
    // "xat" 到 "bat" 和 "cat" 编辑距离都是 1 → 歧义（tied=2）：
    // try_autofix 拒猜返 None，validate 产出带 suggestion 的 UnknownField，
    // format_violations 因此追加合法字段清单（338）。
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "bat": {"type": "string"},
            "cat": {"type": "string"}
        }
    });
    let out = check(&schema, r#"{"xat": "a"}"#);
    match out {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "B");
            assert!(
                message.contains("Accepted fields:"),
                "must list accepted fields, got: {}",
                message
            );
            assert!(message.contains("unknown field 'xat'"));
            assert!(message.contains("did you mean"));
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

/// 直接驱动 format_violations：无 suggestion 的 UnknownField（纯多余字段
/// 不经 validate 产出，这里手工构造）+ 空 properties schema 不追加清单。
#[test]
fn format_violations_without_properties_skips_accepted_fields() {
    let vs = vec![Violation::UnknownField {
        field: "encoding".to_string(),
        suggestion: None,
    }];
    let empty_schema = serde_json::json!({"type": "object", "properties": {}});
    let msg = format_violations(&empty_schema, &vs);
    assert!(msg.contains("unknown field 'encoding'"));
    assert!(
        !msg.contains("Accepted fields:"),
        "no properties → no field list appended"
    );
}
