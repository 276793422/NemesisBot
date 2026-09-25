// approval_rules.rs 覆盖率补充测试（load_rules 的损坏文件 warn 臂 174 /
// save_rules 的原子写入收尾 187）。

use super::*;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-ar-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cov_rule(op: &str, pattern: &str) -> ApprovalRule {
    ApprovalRule {
        op: op.to_string(),
        pattern: pattern.to_string(),
        action: "allow".to_string(),
        created_at: "2026-09-25T00:00:00Z".to_string(),
    }
}

/// save → load 往返（187 收尾），目录不存在自动创建。
#[test]
fn save_and_load_rules_roundtrip() {
    let dir = temp_dir("round");
    let path = dir
        .join("nested")
        .join("config")
        .join("approval_rules.json");

    let rules = vec![cov_rule("process_exec", "cargo test *")];
    save_rules(&path, &rules).unwrap();
    let back = load_rules(&path);
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].op, "process_exec");
    assert_eq!(back[0].pattern, "cargo test *");

    let _ = std::fs::remove_dir_all(&dir);
}

/// 损坏文件 → warn 并按空集处理（174）；缺失文件 → 空集常态。
#[test]
fn malformed_rules_file_treated_as_empty() {
    let dir = temp_dir("bad");
    let path = dir.join("rules.json");
    std::fs::write(&path, "{not valid json").unwrap();
    assert!(load_rules(&path).is_empty());

    let missing = dir.join("nope").join("rules.json");
    assert!(load_rules(&missing).is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}
