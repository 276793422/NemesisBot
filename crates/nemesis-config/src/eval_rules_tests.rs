//! eval_rules 模块基础覆盖（模型/校验/解析/IO/默认集）。
//!
//! 完整求值语义与 CLI 行为覆盖在 nemesisbot::eval_assessor/tests.rs
//! （经 re-export 同源覆盖本模块）；这里钉数据半边的契约面：
//! validate 各拒绝臂、版本漂移、id 唯一、lenient 双形态、种子与备份。

use crate::eval_rules::*;
use std::path::Path;

fn good_rule(id: &str) -> Rule {
    Rule {
        id: id.to_string(),
        description: "测试规则".to_string(),
        level: "high".to_string(),
        enabled: true,
        source: "driver_events".to_string(),
        conditions: vec![Condition {
            field: "event.type".to_string(),
            op: "equals".to_string(),
            value: serde_json::json!("file_delete"),
        }],
        min_count: 1,
    }
}

#[test]
fn validate_rule_accepts_minimal_rule() {
    validate_rule(&good_rule("r1")).expect("minimal rule must pass");
}

#[test]
fn validate_rule_rejects_empty_id() {
    let mut r = good_rule("");
    r.id = "  ".to_string();
    assert!(validate_rule(&r).is_err(), "blank id must be rejected");
}

#[test]
fn validate_rule_rejects_unknown_source() {
    let mut r = good_rule("r2");
    r.source = "nosuch".to_string();
    let err = validate_rule(&r).unwrap_err().to_string();
    assert!(err.contains("unknown source"), "got: {err}");
}

#[test]
fn validate_rule_rejects_empty_conditions() {
    let mut r = good_rule("r3");
    r.conditions.clear();
    assert!(
        validate_rule(&r).is_err(),
        "empty conditions must be rejected"
    );
}

#[test]
fn validate_rule_rejects_unknown_op() {
    let mut r = good_rule("r4");
    r.conditions[0].op = "startswith".to_string();
    let err = validate_rule(&r).unwrap_err().to_string();
    assert!(err.contains("unknown op"), "got: {err}");
}

#[test]
fn validate_rule_regex_requires_string_value() {
    let mut r = good_rule("r5");
    r.conditions[0].op = "regex".to_string();
    r.conditions[0].value = serde_json::json!(42);
    let err = validate_rule(&r).unwrap_err().to_string();
    assert!(err.contains("regex requires a string value"), "got: {err}");
}

#[test]
fn validate_rule_regex_rejects_uncompilable_pattern() {
    let mut r = good_rule("r6");
    r.conditions[0].op = "regex".to_string();
    r.conditions[0].value = serde_json::json!("([bad");
    let err = validate_rule(&r).unwrap_err().to_string();
    assert!(err.contains("invalid regex"), "got: {err}");
}

#[test]
fn validate_rule_contains_requires_string_value() {
    let mut r = good_rule("r7");
    r.conditions[0].op = "contains".to_string();
    r.conditions[0].value = serde_json::json!(7);
    assert!(validate_rule(&r).is_err());
}

#[test]
fn validate_rule_gt_requires_number_value() {
    let mut r = good_rule("r8");
    r.conditions[0].op = "gt".to_string();
    r.conditions[0].value = serde_json::json!("many");
    assert!(validate_rule(&r).is_err());
}

#[test]
fn validate_rule_rejects_unknown_level() {
    let mut r = good_rule("r9");
    r.level = "severe".to_string();
    let err = validate_rule(&r).unwrap_err().to_string();
    assert!(err.contains("unknown level"), "got: {err}");
}

#[test]
fn default_rules_json_parses_clean() {
    // 默认集是用户可编辑资产的种子：任何漂移在这里先红。
    let file = parse_rules(DEFAULT_RULES_JSON).expect("built-in default rules must parse");
    assert!(!file.rules.is_empty(), "default set must not be empty");
}

#[test]
fn parse_rules_rejects_newer_version() {
    let wrapped = format!(
        r#"{{"version": {}, "rules": []}}"#,
        CURRENT_RULES_VERSION + 1
    );
    let err = parse_rules(&wrapped).unwrap_err().to_string();
    assert!(err.contains("newer than supported"), "got: {err}");
}

#[test]
fn parse_rules_rejects_duplicate_id() {
    let a = good_rule("dup");
    let b = good_rule("dup");
    let content = serde_json::to_string(&RulesFile {
        version: CURRENT_RULES_VERSION,
        rules: vec![a, b],
    })
    .unwrap();
    let err = parse_rules(&content).unwrap_err().to_string();
    assert!(err.contains("duplicate rule id"), "got: {err}");
}

#[test]
fn parse_rules_lenient_accepts_both_shapes() {
    let single = serde_json::to_string(&good_rule("solo")).unwrap();
    assert_eq!(parse_rules_lenient(&single).unwrap().rules.len(), 1);
    let wrapped = serde_json::to_string(&RulesFile {
        version: CURRENT_RULES_VERSION,
        rules: vec![good_rule("solo"), good_rule("solo2")],
    })
    .unwrap();
    assert_eq!(parse_rules_lenient(&wrapped).unwrap().rules.len(), 2);
}

#[test]
fn load_rules_seeds_defaults_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config").join("eval_rules.json");
    assert!(!path.exists());
    let file = load_rules(&path).expect("load must seed then parse");
    assert!(!file.rules.is_empty());
    assert!(path.exists(), "seeded file must exist");
    // 二次加载 = 幂等（不重种子、不覆盖用户文件）。
    let again = load_rules(&path).unwrap();
    assert_eq!(again.rules.len(), file.rules.len());
}

#[test]
fn save_rules_roundtrips_and_backs_up() {
    let dir = tempfile::tempdir().unwrap();
    let path: std::path::PathBuf = dir.path().join("eval_rules.json");
    let v1 = RulesFile {
        version: CURRENT_RULES_VERSION,
        rules: vec![good_rule("v1")],
    };
    save_rules(&path, &v1).unwrap();
    let bak = path.with_extension("json.bak");
    assert!(!bak.exists(), "first save must not create backup");
    let v2 = RulesFile {
        version: CURRENT_RULES_VERSION,
        rules: vec![good_rule("v1"), good_rule("v2")],
    };
    save_rules(&path, &v2).unwrap();
    assert!(bak.exists(), "overwrite must rotate old file to .bak");
    let loaded = load_rules(Path::new(&path)).unwrap();
    assert_eq!(loaded.rules.len(), 2);
    assert_eq!(loaded.rules[1].id, "v2");
}

#[test]
fn load_rules_accepts_utf8_bom_file() {
    // BUG-20261109-036：rules 文件是用户可编辑资产（pretty JSON「便于手工
    // 编辑」）——Windows 记事本 utf8BOM 保存形态曾被裸 read_to_string 误报
    // 「rules file is damaged」。读侧走 BOM 剥离 helper 后正常解析。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval_rules.json");
    let body = serde_json::json!({
        "version": CURRENT_RULES_VERSION,
        "rules": [good_rule("bom1")]
    })
    .to_string();
    std::fs::write(&path, format!("\u{feff}{body}")).unwrap();
    let file = load_rules(Path::new(&path)).expect("BOM 前缀的合法规则文件必须可读");
    assert_eq!(file.rules.len(), 1);
    assert_eq!(file.rules[0].id, "bom1");
}
