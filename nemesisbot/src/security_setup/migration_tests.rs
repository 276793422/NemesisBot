//! BUG-6（2026-11-25 远程 home 取证）回归：审计链存储独立化的一次性迁移
//! 语义在此钉死。链原落 `logs/security_logs/`（与普通日志同目录同权限
//! ——logs 级删除/清扫/故障波及会让审计链陪葬），迁 workspace 顶层
//! `security_chain/`。
//!
//! 钉死的语义：
//! 1. 新位置无主链 && 旧位置有主链 → 整族搬迁（主文件 + `_segNNNN` 段），
//!    链连续性保持（搬完在新位置 load + verify 全量通过）；
//! 2. 新位置已有主链（已迁移/已建新链）→ 无事可做，旧位置文件不动；
//! 3. 旧位置无主链（新装/旧链已被清）→ 无事可做，新目录都不创建。

use super::*;

/// 语义 1：整族搬迁 + 链连续性保持。旧链用真 AuditChain 写（两个哈希
/// 链接的事件），迁移后在新位置回载 + 全量 verify 通过。
#[test]
fn migrates_chain_family_with_continuity() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();
    let old_dir = nemesis_path::resolve_audit_log_dir_in_workspace(workspace);
    let new_dir = nemesis_path::resolve_security_chain_dir_in_workspace(workspace);
    let old_main = old_dir.join("audit_chain.jsonl");
    std::fs::create_dir_all(&old_dir).expect("old dir");

    // 旧位置写真链：两个链事件（链式 prev/hash）。
    let old_chain = nemesis_security::integrity::AuditChain::new(
        nemesis_security::integrity::AuditChainConfig {
            enabled: true,
            storage_path: old_main.clone(),
            max_file_size: 50 * 1024 * 1024,
            verify_on_load: false,
            max_events_per_segment: 100_000,
            signing_key: None,
        },
    );
    old_chain
        .append("exec", "shell", "u", "web", "t1", "deny", "r1")
        .expect("old append 1");
    old_chain
        .append("exec", "shell", "u", "web", "t2", "allow", "r2")
        .expect("old append 2");
    // 轮转段形态的伴生文件（空段——只验证「整族」搬迁面）。
    std::fs::write(old_dir.join("audit_chain_seg0002.jsonl"), "").expect("seg file");

    migrate_audit_chain_to_security_chain(workspace);

    // 整族到位：旧位置清空，新位置齐了。
    assert!(!old_main.exists(), "旧主链文件必须已搬走");
    assert!(
        !old_dir.join("audit_chain_seg0002.jsonl").exists(),
        "旧段文件必须已搬走"
    );
    let new_main = new_dir.join("audit_chain.jsonl");
    assert!(new_main.exists(), "新主链文件必须在位");
    assert!(
        new_dir.join("audit_chain_seg0002.jsonl").exists(),
        "新段文件必须在位"
    );

    // 链连续性：新位置回载 2 事件 + 全量 verify 通过。
    let new_chain = nemesis_security::integrity::AuditChain::new(
        nemesis_security::integrity::AuditChainConfig {
            enabled: true,
            storage_path: new_main.clone(),
            max_file_size: 50 * 1024 * 1024,
            verify_on_load: false,
            max_events_per_segment: 100_000,
            signing_key: None,
        },
    );
    assert_eq!(
        new_chain.load_segments().expect("load"),
        2,
        "迁移后链事件数不变"
    );
    assert!(
        nemesis_security::integrity::AuditChain::verify_chain(&read_all_events(&new_main)),
        "迁移后链全量校验通过（连续性保持）"
    );
}

/// 语义 2：新位置已有主链 → 无事可做（旧文件不动，新文件不动）。
#[test]
fn no_op_when_new_chain_already_present() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();
    let old_dir = nemesis_path::resolve_audit_log_dir_in_workspace(workspace);
    let new_dir = nemesis_path::resolve_security_chain_dir_in_workspace(workspace);
    std::fs::create_dir_all(&old_dir).expect("old dir");
    std::fs::create_dir_all(&new_dir).expect("new dir");
    let old_main = old_dir.join("audit_chain.jsonl");
    let new_main = new_dir.join("audit_chain.jsonl");
    std::fs::write(&old_main, "{\"old\":true}\n").expect("old chain");
    std::fs::write(&new_main, "{\"new\":true}\n").expect("new chain");

    migrate_audit_chain_to_security_chain(workspace);

    assert_eq!(
        std::fs::read_to_string(&old_main).expect("old readable"),
        "{\"old\":true}\n",
        "新链已在位时旧文件必须原样保留（不覆盖不删除）"
    );
    assert_eq!(
        std::fs::read_to_string(&new_main).expect("new readable"),
        "{\"new\":true}\n",
        "新链在位时必须不被旧链覆盖"
    );
}

/// 语义 3：旧位置无主链（新装）→ 无事可做，新目录都不创建。
#[test]
fn no_op_when_old_chain_absent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();

    migrate_audit_chain_to_security_chain(workspace);

    assert!(
        !nemesis_path::resolve_security_chain_dir_in_workspace(workspace).exists(),
        "新装形态不得创建新目录"
    );
}

/// 读整族链事件（主文件 + `_segNNNN` 段，名字序——与 web 读方
/// read_all_audit_events 同款语义的最小实现，只为 verify 断言）。
fn read_all_events(main_path: &std::path::Path) -> Vec<nemesis_security::integrity::AuditEvent> {
    let stem = "audit_chain";
    let parent = main_path.parent().expect("parent");
    let mut files = vec![main_path.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(parent) {
        let mut segs: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with(stem) && name.contains("_seg")
            })
            .collect();
        segs.sort();
        files.extend(segs);
    }
    let mut events = Vec::new();
    for f in files {
        if let Ok(content) = std::fs::read_to_string(&f) {
            for line in content.lines() {
                if let Ok(ev) = serde_json::from_str(line) {
                    events.push(ev);
                }
            }
        }
    }
    events
}
