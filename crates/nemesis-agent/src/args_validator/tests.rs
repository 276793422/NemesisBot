use super::*;
use serde_json::json;

fn sample_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "content": {"type": "string"},
            "timeout": {"type": "integer"},
            "action": {"type": "string", "enum": ["create", "delete", "list"]}
        },
        "required": ["path"]
    })
}

#[test]
fn valid_args_pass() {
    let s = sample_schema();
    assert!(matches!(check(&s, r#"{"path":"a.txt"}"#), Outcome::Valid));
    assert!(matches!(
        check(&s, r#"{"path":"a.txt","timeout":30}"#),
        Outcome::Valid
    ));
}

#[test]
fn missing_required_is_invalid() {
    let s = sample_schema();
    match check(&s, r#"{"content":"hi"}"#) {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "B");
            assert!(message.contains("required"), "{}", message);
            assert!(message.contains("path"), "{}", message);
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

#[test]
fn wrong_type_is_invalid() {
    let s = sample_schema();
    match check(&s, r#"{"path":123}"#) {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "B");
            assert!(message.contains("path"), "{}", message);
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

#[test]
fn bad_enum_is_invalid() {
    let s = sample_schema();
    match check(&s, r#"{"path":"a","action":"nope"}"#) {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "B");
            assert!(message.contains("one of"), "{}", message);
            assert!(message.contains("create"), "{}", message);
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

#[test]
fn unknown_field_typo_is_autofixed() {
    // "patch" is edit-distance 1 from "path" — should autofix.
    let s = sample_schema();
    match check(&s, r#"{"patch":"a.txt"}"#) {
        Outcome::Fixed(fixed) => {
            let v: Value = serde_json::from_str(&fixed).unwrap();
            assert_eq!(v["path"], "a.txt");
            assert!(v.get("patch").is_none());
        }
        other => panic!("expected Fixed, got {:?}", other),
    }
}

#[test]
fn extra_field_with_no_close_neighbor_is_ignored() {
    // A clearly-extra field (no near-miss with a real field) is IGNORED —
    // tools skip undeclared keys, and bouncing would false-positive on
    // helpful extras that strong models sometimes add (e.g. "encoding",
    // "verbose"). The valid field still executes normally. The validator is
    // now lenient about extras (JSON Schema default), while still catching
    // typos (unknown_field_typo_is_autofixed) and unambiguous errors
    // (missing required / wrong type / bad enum).
    let s = sample_schema();
    assert!(matches!(
        check(&s, r#"{"path":"a","zzzzzz":"x"}"#),
        Outcome::Valid
    ));
    assert!(matches!(
        check(&s, r#"{"path":"a","encoding":"utf-8"}"#),
        Outcome::Valid
    ));
}

#[test]
fn invalid_json_is_class_a() {
    let s = sample_schema();
    match check(&s, r#"{"path":"a", broken}"#) {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "A");
            assert!(message.contains("not valid JSON"), "{}", message);
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

#[test]
fn ambiguous_typo_not_autofixed() {
    // Schema with two fields equally close to the typo. "xat" is distance 1
    // from both "bat" and "cat" → ambiguous → must bounce, not guess.
    let s = json!({
        "type": "object",
        "properties": {
            "bat": {"type": "string"},
            "cat": {"type": "string"},
            "content": {"type": "string"}
        },
        "required": []
    });
    match check(&s, r#"{"xat":"a"}"#) {
        Outcome::Invalid { .. } => {}
        other => panic!("expected Invalid (ambiguous), got {:?}", other),
    }
}

#[test]
fn no_schema_fails_open() {
    let s = json!({"type": "object"});
    assert!(matches!(
        check(&s, r#"{"anything": 1, "else": "x"}"#),
        Outcome::Valid
    ));
}

#[test]
fn non_object_args_with_object_schema() {
    let s = sample_schema();
    match check(&s, r#"[1,2,3]"#) {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "B");
            assert!(message.contains("object"), "{}", message);
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

#[test]
fn edit_distance_basic() {
    assert_eq!(edit_distance("", ""), 0);
    assert_eq!(edit_distance("abc", "abc"), 0);
    assert_eq!(edit_distance("path", "patch"), 1); // insert
    assert_eq!(edit_distance("patch", "path"), 1); // delete
    assert_eq!(edit_distance("cat", "cut"), 1); // substitute
    assert_eq!(edit_distance("path", "content"), 6);
}

#[test]
fn multiple_violations_all_reported() {
    let s = sample_schema();
    // missing path (required) + bad action enum + unknown field
    match check(&s, r#"{"action":"foo","wat":"x"}"#) {
        Outcome::Invalid { message, .. } => {
            assert!(message.contains("required"), "{}", message);
            assert!(message.contains("one of"), "{}", message);
            assert!(message.contains("unknown"), "{}", message);
        }
        other => panic!("expected Invalid, got {:?}", other),
    }
}

// ----- W3a: direct helper-arm coverage -----

/// `Violation::message()` renders every variant precisely, including the
/// `UnknownField` WITHOUT a suggestion.
#[test]
fn violation_message_all_variants() {
    assert_eq!(
        Violation::UnknownField {
            field: "pth".into(),
            suggestion: None,
        }
        .message(),
        "unknown field 'pth'"
    );
    assert_eq!(
        Violation::UnknownField {
            field: "pth".into(),
            suggestion: Some("path".into()),
        }
        .message(),
        "unknown field 'pth' (did you mean 'path'?)"
    );
    assert_eq!(
        Violation::MissingRequired {
            field: "path".into()
        }
        .message(),
        "missing required field 'path'"
    );
    assert_eq!(
        Violation::WrongType {
            field: "n".into(),
            expected: "integer".into(),
            got: "string".into(),
        }
        .message(),
        "field 'n' should be integer, got string"
    );
    assert_eq!(
        Violation::NotInEnum {
            field: "mode".into(),
            allowed: vec!["a".into(), "b".into()],
        }
        .message(),
        "field 'mode' must be one of: a | b"
    );
}

/// `type_matches` every spec arm — including unknown specs failing open.
#[test]
fn type_matches_all_arms() {
    let v_str = serde_json::json!("x");
    assert!(type_matches("string", &v_str));
    assert!(!type_matches("integer", &v_str));
    assert!(type_matches("integer", &serde_json::json!(3)));
    assert!(type_matches("integer", &serde_json::json!(3u64)));
    assert!(!type_matches("integer", &serde_json::json!(3.5)));
    assert!(type_matches("number", &serde_json::json!(3.5)));
    assert!(type_matches("number", &serde_json::json!(7)));
    assert!(type_matches("boolean", &serde_json::json!(true)));
    assert!(type_matches("array", &serde_json::json!([1, 2])));
    assert!(type_matches("object", &serde_json::json!({"a": 1})));
    assert!(type_matches("null", &serde_json::Value::Null));
    // Unknown type spec fails open.
    assert!(type_matches("weird", &v_str));
}

/// `type_name` for every JSON value kind.
#[test]
fn type_name_all_kinds() {
    assert_eq!(type_name(&serde_json::Value::Null), "null");
    assert_eq!(type_name(&serde_json::json!(true)), "boolean");
    assert_eq!(type_name(&serde_json::json!(1)), "number");
    assert_eq!(type_name(&serde_json::json!(1.5)), "number");
    assert_eq!(type_name(&serde_json::json!("s")), "string");
    assert_eq!(type_name(&serde_json::json!([1])), "array");
    assert_eq!(type_name(&serde_json::json!({})), "object");
}

/// `edit_distance` with an EMPTY second operand returns the first's length
/// (early return arm).
#[test]
fn edit_distance_empty_second() {
    assert_eq!(edit_distance("abc", ""), 3);
    assert_eq!(edit_distance("", "abc"), 3);
    assert_eq!(edit_distance("", ""), 0);
}

/// `try_autofix` collision: two distinct unknown fields that both map to the
/// SAME schema property → ambiguous rewrite is refused (None).
#[test]
fn try_autofix_collision_on_same_target_returns_none() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "path": { "type": "string" }, "mode": { "type": "string" } },
        "required": ["path"]
    });
    // "patj" → path (d=1), "patk" → path (d=1): both would collide on "path".
    let args = serde_json::json!({ "patj": "a", "patk": "b" });
    assert!(try_autofix(&schema, &args).is_none());
}

/// `try_autofix` with all-valid keys → nothing to rename → None.
#[test]
fn try_autofix_no_unknowns_returns_none() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"]
    });
    let args = serde_json::json!({ "path": "ok" });
    assert!(try_autofix(&schema, &args).is_none());
    // Non-object args also bail out with None.
    assert!(try_autofix(&schema, &serde_json::json!([1, 2])).is_none());
}

/// `format_violations` appends the accepted-fields list when at least one
/// UnknownField is present (so the model sees the legal keys).
#[test]
fn format_violations_appends_accepted_fields() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "alpha": {}, "beta": {} },
    });
    let vs = vec![
        Violation::UnknownField {
            field: "alpga".into(),
            suggestion: Some("alpha".into()),
        },
        Violation::MissingRequired {
            field: "beta".into(),
        },
    ];
    let msg = format_violations(&schema, &vs);
    assert!(msg.starts_with("Tool argument validation failed:"));
    assert!(msg.contains("Accepted fields: alpha, beta."));
}

/// `format_violations` WITHOUT any UnknownField omits the accepted-fields list.
#[test]
fn format_violations_without_unknown_field_has_no_field_list() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "alpha": {} },
    });
    let vs = vec![Violation::MissingRequired {
        field: "alpha".into(),
    }];
    let msg = format_violations(&schema, &vs);
    assert!(!msg.contains("Accepted fields"));
}

/// Root-level WrongType reports the actual JSON kind of the root value.
#[test]
fn non_object_root_reports_actual_type_name() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": { "path": { "type": "string" } },
    });
    let out = check(&schema, "[1, 2, 3]");
    match out {
        Outcome::Invalid { message, class } => {
            assert_eq!(class, "B");
            assert!(message.contains("array"), "root kind reported: {message}");
        }
        _ => panic!("expected Invalid"),
    }
}
