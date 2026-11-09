//! approval_rules 单测：pattern 生成 / 匹配语义 / 层级安全门 /
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
    // 引号内空格照 分词语义匹配。
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
fn rule_permitted_install_ops_never() {
    // 复查 #12：安装类永不入规则（任何风险级）——目录记忆 ≠
    // 载荷记忆，装代码的每次裁决都保持人工。
    assert!(!rule_permitted_for("plugins.install", "HIGH"));
    assert!(!rule_permitted_for("skills.install", "MEDIUM"));
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

// ---------- CMD-12：复合命令盲区 ----------

#[test]
fn pattern_for_compound_command_refuses_rule() {
    // 复合命令不构成前缀规则：单一 pattern 表达不了「全段命中」语义
    assert_eq!(pattern_for("process_exec", "cd /tmp && rm -rf y"), "");
    assert_eq!(pattern_for("process_spawn", "echo hi; kill -9 1"), "");
    assert_eq!(pattern_for("process_exec", "a | tee x"), "");
}

#[test]
fn pattern_for_single_segment_with_operators_inside_quotes_ok() {
    // 引号内的 `&&` 是字面量 = 单段，照常归约
    assert_eq!(
        pattern_for("process_exec", "echo \"a && b\" --flag"),
        "echo *"
    );
}

#[test]
fn matches_rule_prefix_does_not_cover_compound_tail() {
    // 核心：`cd *` 不得罩住 `cd x && rm -rf y` 的删除段
    assert!(!matches_rule(
        "process_exec",
        "cd *",
        "process_exec",
        "cd /tmp && rm -rf y"
    ));
    // 单段行为不变
    assert!(matches_rule(
        "process_exec",
        "cd *",
        "process_exec",
        "cd /tmp"
    ));
    assert!(matches_rule(
        "process_exec",
        "cd *",
        "process_exec",
        "cd /tmp --flag"
    ));
    // 三段复合，首段命中、尾段不命中 → 不放
    assert!(!matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "cargo build && cargo test && curl evil.example"
    ));
}

#[test]
fn matches_rule_all_segments_hit_is_allowed() {
    // 全段命中（同族前缀规则）→ 放行语义成立
    assert!(matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "cargo build && cargo test"
    ));
    // 重定向 `2>&1` 不切段：`cargo *` 照常罩住
    assert!(matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "cargo build > log.txt 2>&1"
    ));
}

#[test]
fn matches_rule_nonexec_keeps_exact_semantics() {
    // 非 exec 类不受分段影响：无 " *" 后缀走精确匹配（旧语义原样）
    assert!(matches_rule(
        "file_write",
        "/tmp/a.txt",
        "file_write",
        "/tmp/a.txt"
    ));
    assert!(!matches_rule(
        "file_write",
        "/tmp/*",
        "file_write",
        "/tmp/a.txt"
    ));
}

#[test]
fn persist_compound_command_returns_honest_error() {
    let dir = std::env::temp_dir().join(format!("approval_rules_cmd12_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("approval_rules.json");
    let err =
        persist_auto_allow_rule(&path, "process_exec", "cd /tmp && rm -rf y", "HIGH").unwrap_err();
    assert!(err.contains("compound command"), "honest error, got: {err}");
    // 规则文件未落任何东西
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- env 前缀（闭环-13）：读侧剥离与写侧归约同语义 ----------

#[test]
fn matches_rule_strips_env_prefix_per_segment() {
    // 写侧 pattern 永无 env 头（reduce_command 剥）；读侧不剥则
    // `FOO=1 cargo test` 永远弹卡——ABAC 放行、审批记忆打扰的分叉。
    assert!(matches_rule(
        "process_exec",
        "cargo test *",
        "process_exec",
        "FOO=1 cargo test --release"
    ));
    // 复合命令逐段剥：`RUST_LOG=x` 只属于左段
    assert!(matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "RUST_LOG=debug cargo build && cargo test"
    ));
    // env 头不构成通配逃生：`FOO=1 rm x` 不得命中 `cargo *`
    assert!(!matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "FOO=1 rm x"
    ));
    // 纯 env 无命令：剥后为空 → 不命中（不得借 env 头全放行）
    assert!(!matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "FOO=1"
    ));
}

#[test]
fn matches_rule_exact_branch_strips_env_for_command_ops() {
    // 手写裸 pattern（无 " *" 后缀）的 exec 防御性对齐：剥 env 头再比
    assert!(matches_rule(
        "process_exec",
        "cargo build",
        "process_exec",
        "FOO=1 cargo build"
    ));
    // 非 exec 类 target 是路径/URL：env 剥离不适用（含 `=` 的路径不变形）
    assert!(!matches_rule(
        "file_write",
        "/tmp/a=b.txt",
        "file_write",
        "/tmp/other.txt"
    ));
    assert!(matches_rule(
        "file_write",
        "/tmp/a=b.txt",
        "file_write",
        "/tmp/a=b.txt"
    ));
}

// ---------- （第 6 轮审计）：引号形 env 头两侧同源 ----------

#[test]
fn quoted_env_prefix_hits_reduced_rule() {
    // 写侧 reduce_command 在 tokenize 后剥 env（引号已掉，`"FOO=1"` 判 env）；
    // 读侧若在原串上剥，首 token 带引号判非 env → 剥不掉 → 规则永远弹卡。
    assert_eq!(
        pattern_for("process_exec", "\"FOO=1\" cargo test"),
        "cargo test *"
    );
    assert!(matches_rule(
        "process_exec",
        "cargo test *",
        "process_exec",
        "\"FOO=1\" cargo test --release"
    ));
    // 精确臂同源：手写 pattern 与引号 env 头 target
    assert!(matches_rule(
        "process_exec",
        "cargo build",
        "process_exec",
        "\"FOO=1\" cargo build"
    ));
    // 引号形 env 头不构成通配逃生
    assert!(!matches_rule(
        "process_exec",
        "cargo *",
        "process_exec",
        "\"FOO=1\" rm x"
    ));
    // 手写 pattern 的引号/多余空白与 target 归一（token 向量比较）
    assert!(matches_rule(
        "process_exec",
        "cargo   build",
        "process_exec",
        "cargo build"
    ));
}

// ---------- 人工规则管理（upsert_manual_rule / remove_manual_rule） ----------

#[test]
fn manual_rule_happy_exec_star_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let stored = upsert_manual_rule(&path, "process_exec", " cargo test * ", "allow").unwrap();
    assert_eq!(stored, "cargo test *", "pattern must be trimmed");
    // 落盘可回读，且匹配侧真放行（非死条目的实证）
    let rules = load_rules(&path);
    assert_eq!(rules.len(), 1);
    assert!(matches_rule(
        "process_exec",
        &rules[0].pattern,
        "process_exec",
        "cargo test --release"
    ));
    // 幂等 upsert：同 (op, pattern) 不增行
    upsert_manual_rule(&path, "process_exec", "cargo test *", "allow").unwrap();
    assert_eq!(load_rules(&path).len(), 1);
}

#[test]
fn manual_rule_rejects_empty_and_non_allow() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    assert!(
        upsert_manual_rule(&path, "  ", "x", "allow").is_err(),
        "blank op"
    );
    assert!(
        upsert_manual_rule(&path, "file_write", "   ", "allow").is_err(),
        "blank pattern"
    );
    let err = upsert_manual_rule(&path, "file_write", "/tmp/a", "deny").unwrap_err();
    assert!(
        err.contains("allow-only"),
        "v1 deny has no memory meaning, got: {err}"
    );
    assert!(!path.exists(), "rejected entries must not write the file");
}

#[test]
fn manual_rule_rejects_install_op() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    let err = upsert_manual_rule(&path, "plugins.install", "/tmp/src", "allow").unwrap_err();
    assert!(err.contains("never enters the rule table"), "got: {err}");
    assert!(!path.exists());
}

#[test]
fn manual_rule_rejects_critical_nonexec_and_unknown_op() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    // 静态 CRITICAL 非 exec：匹配侧永不放行 = 死条目，写入侧 loud 拒
    let err = upsert_manual_rule(&path, "system_shutdown", "now", "allow").unwrap_err();
    assert!(err.contains("always stay manual"), "got: {err}");
    // 未知 op：danger_level_of_slug fail-closed CRITICAL → 同拒
    let err = upsert_manual_rule(&path, "file_transmogrify", "x", "allow").unwrap_err();
    assert!(
        err.contains("CRITICAL"),
        "fail-closed on unknown op, got: {err}"
    );
    assert!(!path.exists());
}

#[test]
fn manual_rule_rejects_read_side_dead_patterns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    // 裸 `*` 与空 token 前缀 ` *`：matches_rule 永不命中
    for pat in ["*", " *"] {
        let err = upsert_manual_rule(&path, "process_exec", pat, "allow").unwrap_err();
        assert!(err.contains("never match"), "pattern {pat:?}, got: {err}");
    }
    // exec 复合 pattern：分段匹配作用在 target 侧，pattern 侧无意义
    let err = upsert_manual_rule(&path, "process_exec", "cd x && rm -rf y *", "allow").unwrap_err();
    assert!(err.contains("compound"), "got: {err}");
    assert!(!path.exists());
}

#[test]
fn manual_rule_exact_exec_pattern_allowed() {
    // 手写精确（无 ` *` 后缀）= 比前缀更窄的作用域，合法
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    upsert_manual_rule(&path, "process_exec", "cargo publish", "allow").unwrap();
    let rules = load_rules(&path);
    assert!(matches_rule(
        "process_exec",
        &rules[0].pattern,
        "process_exec",
        "cargo publish"
    ));
    assert!(!matches_rule(
        "process_exec",
        &rules[0].pattern,
        "process_exec",
        "cargo publish --dry-run"
    ));
}

#[test]
fn remove_manual_rule_counts_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("approval_rules.json");
    upsert_manual_rule(&path, "process_exec", "cargo test *", "allow").unwrap();
    upsert_manual_rule(&path, "file_write", "/tmp/a.txt", "allow").unwrap();
    assert_eq!(
        remove_manual_rule(&path, "process_exec", "cargo test *").unwrap(),
        1
    );
    let rules = load_rules(&path);
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].op, "file_write");
    // 再删同一条 = 0，诚实上报不报错
    assert_eq!(
        remove_manual_rule(&path, "process_exec", "cargo test *").unwrap(),
        0
    );
    // 不存在的文件上删除 = Ok(0) 且不 seed 落盘
    let absent = dir.path().join("absent.json");
    assert_eq!(remove_manual_rule(&absent, "file_write", "x").unwrap(), 0);
    assert!(!absent.exists());
}
