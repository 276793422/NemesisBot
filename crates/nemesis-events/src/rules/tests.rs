use super::*;
use std::path::PathBuf;

fn fs_rule(id: &str, root: &str, glob: Option<&str>) -> EventRule {
    EventRule {
        id: id.to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: PathBuf::from(root),
            glob: glob.map(String::from),
            debounce_ms: None,
        },
        deliver: DeliverSpec {
            session_key: "agent:main:session:t".to_string(),
            prompt: "变化 {paths} 共 {count}".to_string(),
            cooldown_secs: None,
            merge_window_secs: None,
            max_per_hour: None,
            enabled: None,
        },
    }
}

#[test]
fn rule_matches_requires_fs_kind_and_root_prefix() {
    let r = fs_rule("a", "C:/logs/build", None);
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/build/x.txt")));
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/build/sub/y.txt")));
    // component 级前缀：同前缀字符串但不同组件不算命中
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/build2/x.txt")));
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/other/x.txt")));
    let mut non_fs = fs_rule("b", "C:/x", None);
    non_fs.watch.kind = "webhook".to_string();
    assert!(!rule_matches(&non_fs, &PathBuf::from("C:/x/y")));
}

#[test]
fn rule_matches_glob_on_file_name_only() {
    let r = fs_rule("a", "C:/logs", Some("*.log"));
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/app.log")));
    assert!(rule_matches(&r, &PathBuf::from("C:/logs/sub/app.log")));
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/app.txt")));
    // glob 不对路径中段匹配：目录名 app.log 下的 x.txt 不命中
    assert!(!rule_matches(&r, &PathBuf::from("C:/logs/app.log/x.txt")));
}

#[test]
fn validate_keeps_good_rules_and_collects_warnings() {
    let good = fs_rule("ok", "C:/logs", None);
    let out = validate(vec![
        good,
        fs_rule("", "C:/x", None),    // 空 id
        fs_rule("dup", "C:/x", None), // id 重复（首条在前）
        fs_rule("dup", "C:/y", None), // id 重复（本条剔除）
        {
            let mut r = fs_rule("kind", "C:/x", None);
            r.watch.kind = "webhook".to_string();
            r
        },
        fs_rule("nopath", "", None), // 空 path
        {
            let mut r = fs_rule("nosession", "C:/x", None);
            r.deliver.session_key = "  ".to_string();
            r
        },
        {
            let mut r = fs_rule("badsession", "C:/x", None);
            r.deliver.session_key = "bare-session-id".to_string();
            r
        },
        {
            let mut r = fs_rule("noprompt", "C:/x", None);
            r.deliver.prompt = "".to_string();
            r
        },
    ]);
    assert_eq!(out.rules.len(), 2);
    assert_eq!(out.rules[0].id, "ok");
    assert_eq!(out.rules[1].id, "dup"); // 重复 id 保留首条
    assert_eq!(out.warnings.len(), 7);
    // bare id 警告点名错路由原因
    assert!(out.warnings.iter().any(|w| w.contains("preset 形态")));
}

#[test]
fn defaults_resolution() {
    let d = DeliverSpec {
        session_key: "agent:main:session:t".to_string(),
        prompt: "p".to_string(),
        cooldown_secs: None,
        merge_window_secs: None,
        max_per_hour: None,
        enabled: None,
    };
    assert_eq!(d.cooldown_secs_or_default(), 60);
    assert_eq!(d.merge_window_secs_or_default(), 5);
    assert_eq!(d.max_per_hour_or_default(), 10);
    assert!(d.enabled_or_default());
    let zero = DeliverSpec {
        cooldown_secs: Some(0),
        merge_window_secs: Some(0),
        max_per_hour: Some(0),
        enabled: Some(false),
        ..d
    };
    assert_eq!(zero.cooldown_secs_or_default(), 0);
    assert_eq!(zero.max_per_hour_or_default(), 0);
    assert!(!zero.enabled_or_default());
}

#[test]
fn load_missing_file_is_empty_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config").join("events.json");
    let out = load_rules(&path).unwrap();
    assert!(out.rules.is_empty());
    assert!(out.warnings.is_empty());
}

#[test]
fn load_roundtrip_preserves_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    let rules = vec![EventRule {
        id: "r1".to_string(),
        watch: WatchSpec {
            kind: "fs".to_string(),
            path: PathBuf::from("C:/logs"),
            glob: Some("*.log".to_string()),
            debounce_ms: Some(300),
        },
        deliver: DeliverSpec {
            session_key: "agents:main:session:x".to_string(),
            prompt: "文件 {path} 更新".to_string(),
            cooldown_secs: Some(0),
            merge_window_secs: Some(1),
            max_per_hour: Some(0),
            enabled: Some(false),
        },
    }];
    save_rules(&path, &rules).unwrap();
    let out = load_rules(&path).unwrap();
    assert_eq!(out.rules.len(), 1);
    assert_eq!(out.rules[0].id, "r1");
    assert_eq!(out.rules[0].watch.glob.as_deref(), Some("*.log"));
    assert_eq!(out.rules[0].watch.debounce_ms, Some(300));
    assert_eq!(out.rules[0].deliver.cooldown_secs, Some(0));
    assert_eq!(out.rules[0].deliver.merge_window_secs, Some(1));
    assert_eq!(out.rules[0].deliver.max_per_hour, Some(0));
    assert_eq!(out.rules[0].deliver.enabled, Some(false));
}

#[test]
fn load_parse_error_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.json");
    std::fs::write(&path, "[{broken").unwrap();
    assert!(load_rules(&path).is_err());
}
