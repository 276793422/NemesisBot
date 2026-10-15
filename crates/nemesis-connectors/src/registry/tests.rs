use super::*;
use crate::manifest::RiskLevel;

fn tmpws() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[test]
fn scan_sorted_and_missing_dir_is_empty() {
    let ws = tmpws();
    assert!(scan_connectors(ws.path()).is_empty(), "缺目录 = 空注册表");

    for id in ["c-b", "c-a", "c-c"] {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#
            ),
        )
        .expect("write");
    }
    // 混入散文件与空目录，确认都被处理（散文件跳过、空目录诚实入列）
    std::fs::write(ws.path().join(CONNECTORS_DIR).join("stray.toml"), "x = 1").expect("stray");

    let loaded = scan_connectors(ws.path());
    let ids: Vec<String> = loaded
        .iter()
        .map(|l| {
            l.connector
                .as_ref()
                .map(|c| c.id.clone())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(ids, vec!["c-a", "c-b", "c-c"], "目录名升序，确定性");
}

#[test]
fn scan_corrupt_manifest_honest_entry() {
    let ws = tmpws();
    let dir = ws.path().join(CONNECTORS_DIR).join("broken");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join(MANIFEST_FILE), "this is not toml [").expect("write");

    let loaded = scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].connector.is_none());
    let err = loaded[0].error.as_deref().expect("honest error");
    assert!(!err.is_empty());
}

#[test]
fn scan_dir_without_manifest_honest_entry() {
    let ws = tmpws();
    std::fs::create_dir_all(ws.path().join(CONNECTORS_DIR).join("empty-c")).expect("mkdir");
    let loaded = scan_connectors(ws.path());
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].error.as_deref().expect("err").contains("缺少"));
}

#[test]
fn find_connector_three_states() {
    let ws = tmpws();
    // 不存在
    let err = find_connector(ws.path(), "nope");
    assert!(matches!(err, Err(ConnectorLookupError::NotFound { .. })));

    // 损坏（目录存在但缺 manifest）
    std::fs::create_dir_all(ws.path().join(CONNECTORS_DIR).join("half")).expect("mkdir");
    let err = find_connector(ws.path(), "half");
    assert!(matches!(err, Err(ConnectorLookupError::Invalid { .. })));

    // 停用
    let dir = ws.path().join(CONNECTORS_DIR).join("off");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "off"
name = "n"
version = "0.1.0"
kind = "local"
enabled = false

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    )
    .expect("write");
    let err = find_connector(ws.path(), "off");
    assert!(matches!(err, Err(ConnectorLookupError::Disabled { .. })));

    // 正常
    let dir = ws.path().join(CONNECTORS_DIR).join("ok");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "ok"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "noop"
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    )
    .expect("write");
    let c = find_connector(ws.path(), "ok").expect("ok connector");
    assert_eq!(c.id, "ok");
}

#[test]
fn find_op_by_id() {
    let ws = tmpws();
    let dir = ws.path().join(CONNECTORS_DIR).join("c");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "c"
name = "n"
version = "0.1.0"
kind = "local"

[[op]]
id = "alpha"
risk = "low"
class = "read"
action = "builtin.a"

[[op]]
id = "beta"
risk = "high"
class = "write"
action = "builtin.b"
"#,
    )
    .expect("write");
    let c = find_connector(ws.path(), "c").expect("c");
    assert_eq!(find_op(&c, "beta").expect("beta op").risk, RiskLevel::High);
    assert!(find_op(&c, "gamma").is_none());
}

#[test]
fn search_intent_hit_and_empty_query_lists_all() {
    let ws = tmpws();
    for (id, intent) in [
        ("monitor", vec!["监控网页", "检查更新"]),
        ("paper-broker", vec!["买入", "卖出"]),
    ] {
        let dir = ws.path().join(CONNECTORS_DIR).join(id);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let intents = intent
            .iter()
            .map(|i| format!("\"{i}\""))
            .collect::<Vec<_>>()
            .join(", ");
        std::fs::write(
            dir.join(MANIFEST_FILE),
            format!(
                r#"[connector]
id = "{id}"
name = "{id}"
version = "0.1.0"
kind = "local"

[[op]]
id = "doit"
intent = [{intents}]
risk = "low"
class = "read"
action = "builtin.noop"
"#
            ),
        )
        .expect("write");
    }

    // op 级中文意图命中
    let hits = search_connectors(ws.path(), "监控");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].connector.id, "monitor");
    assert_eq!(hits[0].matched_ops, vec!["doit".to_string()]);

    // connector 级命中（名称包含）→ 全部 ops
    let hits = search_connectors(ws.path(), "broker");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].connector.id, "paper-broker");
    assert_eq!(hits[0].matched_ops, vec!["doit".to_string()]);

    // 空 query = 列全部 enabled
    let hits = search_connectors(ws.path(), "");
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|h| h.matched_ops.len() == 1));

    // 无命中
    assert!(search_connectors(ws.path(), "不存在的意图词").is_empty());

    // disabled 不出现在检索面
    let dir = ws.path().join(CONNECTORS_DIR).join("off");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join(MANIFEST_FILE),
        r#"[connector]
id = "off"
name = "监控隐藏版"
version = "0.1.0"
kind = "local"
enabled = false

[[op]]
id = "doit"
intent = ["监控"]
risk = "low"
class = "read"
action = "builtin.noop"
"#,
    )
    .expect("write");
    let hits = search_connectors(ws.path(), "监控");
    assert_eq!(hits.len(), 1, "disabled 必须从检索面消失");
    assert_eq!(hits[0].connector.id, "monitor");
}

#[test]
fn lookup_error_display_has_id() {
    let e = ConnectorLookupError::NotFound { id: "x".into() };
    assert!(e.to_string().contains("x"));
}
