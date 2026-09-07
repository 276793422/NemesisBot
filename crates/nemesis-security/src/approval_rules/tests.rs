//! approval_rules 单测（F3）：pattern 生成 / 匹配语义 / 层级安全门 /
//! upsert 去重 / 磁盘加载与容错。

use super::*;
use std::path::Path;

// ---------- pattern_for ----------

#[test]
fn pattern_for_exec_uses_b5_reduction() {
    assert_eq!(
        pattern_for("process_exec", "cargo test --release"),
        "cargo test *"
    );
    assert_eq!(
        pattern_for("process_exec", "cargo publish"),
        "cargo publish *"
    );
    assert_eq!(
        pattern_for("process_spawn", "npm run dev -- --port"),
        "npm run dev *"
    );
}

#[test]
fn pattern_for_nonexec_is_exact_target() {
    assert_eq!(pattern_for("file_write", "/tmp/a.txt"), "/tmp/a.txt");
}

#[test]
fn pattern_for_empty_target_is_empty() {
    assert_eq!(pattern_for("process_exec", ""), "");
    assert_eq!(pattern_for("process_exec", "   "), "");
}

#[test]
fn pattern_for_env_only_target_is_empty_not_blanket_star() {
    // 全 env 前缀（无真实命令）→ 空 pattern（不构成规则）。若在这里产出
    // ` *`，空 token 前缀会匹配一切 = 全放行洞。
    assert_eq!(pattern_for("process_exec", "FOO=1"), "");
}

// ---------- matches_rule ----------

#[test]
fn matches_starred_prefix() {
    let m = |pat, t| matches_rule("process_exec", pat, "process_exec", t);
    assert!(m("cargo test *", "cargo test --release"));
    assert!(m("cargo test *", "cargo test"), "bare prefix also matches");
    assert!(!m("cargo test *", "cargo publish"));
    assert!(!m("cargo test *", "cargo"));
}

#[test]
fn matches_exact_nonexec() {
    assert!(matches_rule(
        "file_write",
        "/tmp/a.txt",
        "file_write",
        "/tmp/a.txt"
    ));
    assert!(!matches_rule(
        "file_write",
        "/tmp/a.txt",
        "file_write",
        "/tmp/b.txt"
    ));
}

#[test]
fn matches_requires_same_op() {
    assert!(!matches_rule(
        "process_exec",
        "cargo test *",
        "file_write",
        "cargo test --release"
    ));
}

#[test]
fn matches_rejects_empty_inputs() {
    assert!(!matches_rule("process_exec", "", "process_exec", "x"));
    assert!(!matches_rule(
        "process_exec",
        "cargo test *",
        "process_exec",
        ""
    ));
}

#[test]
fn matches_rejects_handwritten_catch_all_star() {
    // 手写裸 `*` 永不命中：防规则文件被改成全放行。
    assert!(!matches_rule(
        "process_exec",
        "*",
        "process_exec",
        "anything"
    ));
}

#[test]
fn matches_rejects_blanket_star_suffix_pattern() {
    // 手写退化 pattern ` *`（空 token 前缀）同样永不命中：与裸 `*` 同罪。
    assert!(!matches_rule(
        "process_exec",
        " *",
        "process_exec",
        "anything"
    ));
}

#[test]
fn matches_tokenizes_quoted_targets() {
    // 引号内空格照 B5 分词语义匹配。
    assert!(matches_rule(
        "process_exec",
        "node server.js *",
        "process_exec",
        "node server.js --port 3000"
    ));
}

// ---------- 层级安全门 ----------

#[test]
fn rule_permitted_non_critical_always() {
    assert!(rule_permitted_for("file_write", "HIGH"));
    assert!(rule_permitted_for("network_request", "MEDIUM"));
    assert!(rule_permitted_for("file_read", "LOW"));
}

#[test]
fn rule_permitted_critical_only_exec() {
    assert!(rule_permitted_for("process_exec", "CRITICAL"));
    assert!(!rule_permitted_for("file_write", "CRITICAL"));
    assert!(!rule_permitted_for("process_kill", "CRITICAL"));
    assert!(!rule_permitted_for("system_shutdown", "CRITICAL"));
}

#[test]
fn find_auto_allow_rule_hits_exec_prefix() {
    let rules = vec![ApprovalRule {
        op: "process_exec".to_string(),
        pattern: "cargo test *".to_string(),
        action: "allow".to_string(),
        created_at: "t".to_string(),
    }];
    assert!(
        find_auto_allow_rule(&rules, "process_exec", "cargo test --release", "CRITICAL").is_some()
    );
    assert!(find_auto_allow_rule(&rules, "process_exec", "cargo publish", "CRITICAL").is_none());
}

#[test]
fn find_auto_allow_rule_critical_nonexec_blocked() {
    let rules = vec![ApprovalRule {
        op: "file_write".to_string(),
        pattern: "/tmp/a.txt".to_string(),
        action: "allow".to_string(),
        created_at: "t".to_string(),
    }];
    assert!(find_auto_allow_rule(&rules, "file_write", "/tmp/a.txt", "CRITICAL").is_none());
    assert!(find_auto_allow_rule(&rules, "file_write", "/tmp/a.txt", "HIGH").is_some());
}

#[test]
fn find_auto_allow_rule_ignores_non_allow_action() {
    let rules = vec![ApprovalRule {
        op: "process_exec".to_string(),
        pattern: "cargo test *".to_string(),
        action: "deny".to_string(),
        created_at: "t".to_string(),
    }];
    assert!(find_auto_allow_rule(&rules, "process_exec", "cargo test", "CRITICAL").is_none());
}

// ---------- upsert ----------

#[test]
fn upsert_dedupes_same_op_pattern() {
    let mut rules = Vec::new();
    assert!(upsert_rule(&mut rules, "process_exec", "cargo test *"));
    assert!(upsert_rule(&mut rules, "process_exec", "cargo test *"));
    assert_eq!(rules.len(), 1);
}

#[test]
fn upsert_rejects_empty_pattern() {
    let mut rules = Vec::new();
    assert!(!upsert_rule(&mut rules, "process_exec", ""));
    assert!(rules.is_empty());
}

#[test]
fn upsert_appends_distinct_patterns() {
    let mut rules = Vec::new();
    upsert_rule(&mut rules, "process_exec", "cargo test *");
    upsert_rule(&mut rules, "process_exec", "cargo build *");
    assert_eq!(rules.len(), 2);
}

// ---------- 磁盘 ----------

#[test]
fn load_missing_file_is_empty() {
    assert!(load_rules(Path::new("/nonexistent/approval_rules.json")).is_empty());
}

#[test]
fn load_malformed_file_is_empty_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad.json");
    std::fs::write(&path, "{not json").unwrap();
    assert!(load_rules(&path).is_empty());
}

#[test]
fn save_and_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("approval_rules.json");
    let rules = vec![ApprovalRule {
        op: "process_exec".to_string(),
        pattern: "cargo test *".to_string(),
        action: "allow".to_string(),
        created_at: "2026-09-06T00:00:00+08:00".to_string(),
    }];
    save_rules(&path, &rules).unwrap();
    let loaded = load_rules(&path);
    assert_eq!(loaded, rules);
}

#[test]
fn resolve_rules_path_lands_in_workspace_config() {
    let p = resolve_rules_path_in_workspace(Path::new("/ws"));
    assert!(
        p.to_string_lossy()
            .replace('\\', "/")
            .ends_with("/ws/config/approval_rules.json")
    );
}
