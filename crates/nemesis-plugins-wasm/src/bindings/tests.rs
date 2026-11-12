use super::*;

// -- HostErr 展示 -------------------------------------------------------

#[test]
fn host_err_display_all_variants() {
    assert_eq!(HostErr::NoPermission.to_string(), "no-permission");
    assert_eq!(
        HostErr::PolicyDenied("x".into()).to_string(),
        "policy-denied(x)"
    );
    assert_eq!(HostErr::NotFound("y".into()).to_string(), "not-found(y)");
    assert_eq!(
        HostErr::BudgetExceeded("z".into()).to_string(),
        "budget-exceeded(z)"
    );
    assert_eq!(
        HostErr::Unavailable("w".into()).to_string(),
        "unavailable(w)"
    );
}

// -- 结果 JSON 包络 -----------------------------------------------------

#[test]
fn result_json_err_tags() {
    let mk =
        |tag: &str, detail: &str| format!(r#"{{"err":{{"tag":"{tag}","detail":"{detail}"}}}}"#);
    assert_eq!(
        parse_result_json(&mk("no-permission", "")),
        Err(HostErr::NoPermission)
    );
    assert_eq!(
        parse_result_json(&mk("policy-denied", "d1")),
        Err(HostErr::PolicyDenied("d1".into()))
    );
    assert_eq!(
        parse_result_json(&mk("not-found", "d2")),
        Err(HostErr::NotFound("d2".into()))
    );
    assert_eq!(
        parse_result_json(&mk("budget-exceeded", "d3")),
        Err(HostErr::BudgetExceeded("d3".into()))
    );
    assert_eq!(
        parse_result_json(&mk("weird-tag", "d4")),
        Err(HostErr::Unavailable("d4".into()))
    );
    // tag 缺席 → 兜底 unavailable；detail 缺席 → 空。
    assert_eq!(
        parse_result_json(r#"{"err":{}}"#),
        Err(HostErr::Unavailable("".into()))
    );
    // 非 JSON。
    let err = parse_result_json("not-json")
        .err()
        .unwrap_or_else(|| panic!("decode failure"));
    assert!(matches!(err, HostErr::Unavailable(ref m) if m.contains("decode failed")));
    // ok 臂（payload 原样带出）。
    assert_eq!(
        parse_result_json(r#"{"ok":{"k":1}}"#).expect("ok"),
        serde_json::json!({"k": 1})
    );
    // 无 ok 无 err → Null。
    assert_eq!(
        parse_result_json("{}").expect("null"),
        serde_json::Value::Null
    );
}

// -- get-metadata -------------------------------------------------------

#[test]
fn parse_get_metadata_full_and_defaults() {
    let out = r#"{"ok":{"name":"translate","title":"翻译","description":"d","parameters-json":"{}","operation-type":"read"}}"#;
    let m = parse_get_metadata(out).expect("ok");
    assert_eq!(m.name, "translate");
    assert_eq!(m.title, "翻译");
    assert_eq!(m.description, "d");
    assert_eq!(m.parameters_json, "{}");
    assert_eq!(m.operation_type, "read");
    // 字段缺席 = 空串兜底。
    let m = parse_get_metadata(r#"{"ok":{}}"#).expect("ok");
    assert!(m.name.is_empty() && m.title.is_empty());
    // err 臂透传。
    assert!(
        matches!(
            parse_get_metadata(r#"{"err":{"tag":"no-permission"}}"#),
            Err(HostErr::NoPermission)
        ),
        "err 臂透传"
    );
}

// -- execute ------------------------------------------------------------

#[test]
fn parse_execute_shapes() {
    let out = parse_execute(r#"{"ok":{"content":"hi","is-error":false}}"#).expect("env");
    assert_eq!(out.expect("payload"), ("hi".into(), false));
    let out = parse_execute(r#"{"ok":{"content":"bad","is-error":true}}"#).expect("env");
    assert_eq!(out.expect("payload"), ("bad".into(), true));
    // is-error 缺席 = false。
    let out = parse_execute(r#"{"ok":{"content":"x"}}"#).expect("env");
    assert_eq!(out.expect("payload"), ("x".into(), false));
    // content 缺席 = guest 显式 host-error。
    let out = parse_execute(r#"{"ok":{"other":1}}"#).expect("env");
    let he = out.err().unwrap_or_else(|| panic!("unexpected shape"));
    assert!(
        matches!(he, HostErr::Unavailable(ref m) if m.contains("unexpected tool-output shape"))
    );
    // env 层 err。
    assert_eq!(
        parse_execute(r#"{"err":{"tag":"budget-exceeded","detail":"cap"}}"#),
        Err(HostErr::BudgetExceeded("cap".into()))
    );
}

// -- observe ------------------------------------------------------------

#[test]
fn parse_observe_null_and_drift_tolerant() {
    assert_eq!(parse_observe(r#"{"ok":null}"#), Ok(Ok(())));
    // 非 null payload 也按成功（宽和面：warn 由调用方落）。
    assert_eq!(parse_observe(r#"{"ok":{"weird":true}}"#), Ok(Ok(())));
    assert_eq!(
        parse_observe(r#"{"err":{"tag":"no-permission"}}"#),
        Err(HostErr::NoPermission)
    );
}

// -- policy metadata + json-string 结果 ---------------------------------

#[test]
fn parse_policy_metadata_fields() {
    let m =
        parse_policy_metadata(r#"{"ok":{"compact":true,"final-judge":false,"validation":true}}"#)
            .expect("ok");
    assert!(m.compact);
    assert!(!m.final_judge);
    assert!(m.validation);
    // 全缺席 = false 三连。
    let m = parse_policy_metadata(r#"{"ok":{}}"#).expect("ok");
    assert!(!m.compact && !m.final_judge && !m.validation);
    assert!(
        matches!(
            parse_policy_metadata(r#"{"err":{"tag":"not-found"}}"#),
            Err(HostErr::NotFound(_))
        ),
        "err 臂透传"
    );
}

#[test]
fn parse_json_string_result_shapes() {
    let s = parse_json_string_result(r#"{"ok":"{\"verdict\":\"ok\"}"}"#)
        .expect("env")
        .expect("payload");
    assert_eq!(s, r#"{"verdict":"ok"}"#);
    // 非 string payload = guest 形态漂移。
    let out = parse_json_string_result(r#"{"ok":{"x":1}}"#).expect("env");
    let he = out.err().unwrap_or_else(|| panic!("unexpected shape"));
    assert!(
        matches!(he, HostErr::Unavailable(ref m) if m.contains("unexpected json-string result shape"))
    );
    assert_eq!(
        parse_json_string_result(r#"{"err":{"tag":"policy-denied","detail":"no"}}"#),
        Err(HostErr::PolicyDenied("no".into()))
    );
}

// -- step metadata ------------------------------------------------------

#[test]
fn parse_step_metadata_kinds() {
    let m = parse_step_metadata(r#"{"ok":{"kinds":["echo","jira:fetch"]}}"#).expect("ok");
    assert_eq!(m.kinds, vec!["echo".to_string(), "jira:fetch".to_string()]);
    // kinds 缺席 = 空表。
    let m = parse_step_metadata(r#"{"ok":{}}"#).expect("ok");
    assert!(m.kinds.is_empty());
    // 非字符串成员被滤掉。
    let m = parse_step_metadata(r#"{"ok":{"kinds":["a",42,"b"]}}"#).expect("ok");
    assert_eq!(m.kinds, vec!["a".to_string(), "b".to_string()]);
    assert!(
        matches!(
            parse_step_metadata(r#"{"err":{"tag":"unavailable"}}"#),
            Err(HostErr::Unavailable(_))
        ),
        "err 臂透传"
    );
}
