//! P6-R2 迁移模块单测（纯函数 + 真实临时目录面；零内联纪律独立文件）。

use std::path::PathBuf;

use super::{
    MigrateCommitDeps, handle_migrate_commit, landing_files_dir, migrate_staging_root,
    migrate_task_id, rewrite_record_for_target, sweep_migrating, tombstone_exists, tombstone_path,
    validate_migrate_agent_id,
};
use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};
use nemesis_taskstore::{AgentRecord, SqliteAgentsStore, TaskDb};

const AGENT_ID: &str = "abc123def4567890abcdef1234567890"; // uuid simple 形态

fn setup_ws(dir: &std::path::Path) -> (PathBuf, SqliteAgentsStore) {
    let ws = dir.join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(&ws)).unwrap();
    (ws, SqliteAgentsStore::from_db(db))
}

fn selfbuilt_record(agent_id: &str, home_node: &str) -> AgentRecord {
    let mut rec = AgentRecord::new_provisioning(agent_id, "worker-a", agent_kind::SELFBUILT);
    rec.status = agent_status::ACTIVE.to_string();
    rec.exec_root = format!("C:\\other\\workspace\\agents\\{agent_id}\\home");
    rec.home_path = rec.exec_root.clone();
    rec.home_node = home_node.to_string();
    rec
}

// ---------------------------------------------------------------------------
// validate_migrate_agent_id
// ---------------------------------------------------------------------------

#[test]
fn agent_id_validation_accepts_uuid_simple() {
    validate_migrate_agent_id(AGENT_ID).unwrap();
    // 值域只放行字母数字与连字符（uuid simple 恒过）。
    validate_migrate_agent_id("My-Agent-01").unwrap();
}

#[test]
fn agent_id_validation_rejects_bad_shapes() {
    validate_migrate_agent_id("").unwrap_err();
    validate_migrate_agent_id("a/b").unwrap_err(); // 路径分隔
    validate_migrate_agent_id("..").unwrap_err();
    validate_migrate_agent_id("a b").unwrap_err(); // 空格
    validate_migrate_agent_id(&"x".repeat(129)).unwrap_err();
}

#[test]
fn migrate_task_id_is_deterministic_and_pathsafe() {
    let tid = migrate_task_id(AGENT_ID);
    assert_eq!(tid, format!("agent-migrate-{AGENT_ID}"));
    // B 端用同串算 landing 路径：sanitize 恒等是硬前提。
    assert_eq!(nemesis_utils::sanitize::sanitize_path_segment(&tid), tid);
}

// ---------------------------------------------------------------------------
// rewrite_record_for_target
// ---------------------------------------------------------------------------

#[test]
fn rewrite_record_rewires_node_and_paths() {
    let ws = PathBuf::from("C:\\target\\workspace");
    let mut rec = selfbuilt_record(AGENT_ID, "node-a");
    rec.memory_namespace = format!("agents/{AGENT_ID}");
    // B 端真实收到的形态：A 端步骤③已置 migrating——改写应翻回 active。
    rec.status = agent_status::MIGRATING.to_string();

    let changed = rewrite_record_for_target(&mut rec, "node-b", &ws);
    assert_eq!(rec.home_node, "node-b");
    assert_eq!(rec.status, "active");
    let expected_home = nemesis_path::resolve_agent_home_in_workspace(&ws, AGENT_ID)
        .to_string_lossy()
        .to_string();
    assert_eq!(rec.exec_root, expected_home);
    assert_eq!(rec.home_path, expected_home);
    // 记忆命名空间与身份不变。
    assert_eq!(rec.memory_namespace, format!("agents/{AGENT_ID}"));
    assert_eq!(rec.agent_id, AGENT_ID);
    // 四字段全部真实变化（migrating→active 也算）。
    assert!(changed.contains(&"home_node"));
    assert!(changed.contains(&"status"));
    assert!(changed.contains(&"exec_root"));
    assert!(changed.contains(&"home_path"));
}

#[test]
fn rewrite_record_is_idempotent() {
    let ws = PathBuf::from("C:\\target\\workspace");
    let mut rec = selfbuilt_record(AGENT_ID, "node-a");
    rewrite_record_for_target(&mut rec, "node-b", &ws);
    let changed = rewrite_record_for_target(&mut rec, "node-b", &ws);
    assert!(changed.is_empty(), "二次改写应零改动: {changed:?}");
}

// ---------------------------------------------------------------------------
// 墓碑 + 目录布局
// ---------------------------------------------------------------------------

#[test]
fn tombstone_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    assert!(!tombstone_exists(&ws, AGENT_ID));

    let p = tombstone_path(&ws, AGENT_ID);
    assert!(p.to_string_lossy().contains("agents"));
    assert!(
        p.to_string_lossy()
            .ends_with(&format!("{AGENT_ID}.migrated.json"))
    );

    // 经 write_tombstone 写入后 exists 命中（write_tombstone 私有——经
    // sweep 路径间接验证；这里直接手写一份等价 JSON 验 exists 判定）。
    std::fs::create_dir_all(ws.join("agents")).unwrap();
    std::fs::write(
        &p,
        serde_json::json!({
            "agent_id": AGENT_ID,
            "target_node": "node-b",
            "migrated_at": "2026-10-21T00:00:00+00:00",
        })
        .to_string(),
    )
    .unwrap();
    assert!(tombstone_exists(&ws, AGENT_ID));
}

#[test]
fn layout_paths_are_distinct() {
    let ws = PathBuf::from("C:\\ws");
    let staging = migrate_staging_root(&ws, AGENT_ID);
    let landing = landing_files_dir(&ws, AGENT_ID);
    assert!(staging.to_string_lossy().contains("migrate_out"));
    assert!(landing.to_string_lossy().contains("inbox"));
    assert!(
        landing
            .to_string_lossy()
            .contains(&migrate_task_id(AGENT_ID))
    );
    assert!(landing.ends_with("files"));
    assert_ne!(staging, landing);
}

// ---------------------------------------------------------------------------
// handle_migrate_commit（B 端落地面，真实临时 workspace）
// ---------------------------------------------------------------------------

fn make_landing(ws: &std::path::Path, agent_id: &str, home_files: &[&str]) {
    let files = landing_files_dir(ws, agent_id);
    std::fs::create_dir_all(files.join("home")).unwrap();
    for f in home_files {
        let p = files.join("home").join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "content").unwrap();
    }
    std::fs::create_dir_all(files.join("memory")).unwrap();
}

fn commit_payload(agent_id: &str, home_node: &str) -> serde_json::Value {
    serde_json::json!({
        "agent_id": agent_id,
        "record": selfbuilt_record(agent_id, home_node),
    })
}

#[test]
fn commit_lands_agent_and_rewrites_record() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, store) = setup_ws(dir.path());
    make_landing(&ws, AGENT_ID, &["notes.md", "sub/data.txt"]);

    let deps = MigrateCommitDeps {
        workspace: ws.clone(),
        self_node_id: "node-b".to_string(),
    };
    let resp = handle_migrate_commit(&deps, commit_payload(AGENT_ID, "node-a")).unwrap();
    assert_eq!(resp.get("ok").and_then(|v| v.as_bool()), Some(true));

    // 记录落库 + 改写到位。
    let rec = store.get(AGENT_ID).unwrap().expect("record upserted");
    assert_eq!(rec.home_node, "node-b");
    assert_eq!(rec.status, agent_status::ACTIVE);
    let expected_home = nemesis_path::resolve_agent_home_in_workspace(&ws, AGENT_ID);
    assert_eq!(PathBuf::from(&rec.exec_root), expected_home);

    // home 文件真实落位 + landing 清空。
    assert!(expected_home.join("notes.md").is_file());
    assert!(expected_home.join("sub").join("data.txt").is_file());
    assert!(!landing_files_dir(&ws, AGENT_ID).exists());
}

#[test]
fn commit_rejects_missing_landing_and_duplicate_and_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, store) = setup_ws(dir.path());
    let deps = MigrateCommitDeps {
        workspace: ws.clone(),
        self_node_id: "node-b".to_string(),
    };

    // landing 未到场。
    let err = handle_migrate_commit(&deps, commit_payload(AGENT_ID, "node-a")).unwrap_err();
    assert!(err.contains("未到场"), "实际: {err}");

    // 落地后首次成功。
    make_landing(&ws, AGENT_ID, &["a.md"]);
    handle_migrate_commit(&deps, commit_payload(AGENT_ID, "node-a")).unwrap();

    // 重复投递拒绝。
    make_landing(&ws, AGENT_ID, &["a.md"]);
    let err = handle_migrate_commit(&deps, commit_payload(AGENT_ID, "node-a")).unwrap_err();
    assert!(err.contains("已存在同 id"), "实际: {err}");

    // record.agent_id 与载荷不一致拒绝。
    let mut bad = commit_payload(AGENT_ID, "node-a");
    bad["record"]["agent_id"] = serde_json::json!("ffffffffffffffffffffffffffffffff");
    make_landing(&ws, "ffffffffffffffffffffffffffffffff", &[]);
    let err = handle_migrate_commit(&deps, bad).unwrap_err();
    assert!(err.contains("不一致"), "实际: {err}");

    // 非法 agent_id（路径分隔）拒绝。
    let err = handle_migrate_commit(&deps, commit_payload("../evil", "node-a")).unwrap_err();
    assert!(
        err.contains("非法字符") || err.contains("归一"),
        "实际: {err}"
    );

    let _ = store; // store 仅供上面断言用
}

#[test]
fn commit_clears_stale_destination() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, _store) = setup_ws(dir.path());
    // 陈旧落位残留（此前半途失败）：旧 home 文件应在 rename 后消失。
    let stale_home = nemesis_path::resolve_agent_home_in_workspace(&ws, AGENT_ID);
    std::fs::create_dir_all(&stale_home).unwrap();
    std::fs::write(stale_home.join("stale.txt"), "old").unwrap();

    make_landing(&ws, AGENT_ID, &["fresh.md"]);
    let deps = MigrateCommitDeps {
        workspace: ws,
        self_node_id: "node-b".to_string(),
    };
    handle_migrate_commit(&deps, commit_payload(AGENT_ID, "node-a")).unwrap();
    assert!(!stale_home.join("stale.txt").exists());
    assert!(stale_home.join("fresh.md").is_file());
}

// ---------------------------------------------------------------------------
// sweep_migrating（收敛语义）
// ---------------------------------------------------------------------------

#[test]
fn sweep_migrating_converges_both_ways() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, store) = setup_ws(dir.path());
    std::fs::create_dir_all(ws.join("agents")).unwrap();

    // 案例一：migrating + 墓碑在场 = 迁移成功终局 → 删记录。
    let done_id = "dddddddddddddddddddddddddddddddd";
    let mut rec = selfbuilt_record(done_id, "node-a");
    rec.status = agent_status::MIGRATING.to_string();
    store.upsert(&rec).unwrap();
    std::fs::write(
        tombstone_path(&ws, done_id),
        serde_json::json!({"agent_id": done_id, "target_node": "node-b", "migrated_at": "t"})
            .to_string(),
    )
    .unwrap();
    // staging 残留也一并清扫。
    let staging = migrate_staging_root(&ws, done_id);
    std::fs::create_dir_all(&staging).unwrap();

    // 案例二：migrating + 无墓碑 = commit 未达 → 回滚 active。
    let stuck_id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let mut rec2 = selfbuilt_record(stuck_id, "node-a");
    rec2.status = agent_status::MIGRATING.to_string();
    store.upsert(&rec2).unwrap();

    let mut report = crate::agents::SweepReport {
        activated: vec![],
        failed: vec![],
        migrated_removed: vec![],
        reactivated: vec![],
    };
    sweep_migrating(&store, &ws, &mut report).unwrap();

    assert_eq!(report.migrated_removed, vec![done_id.to_string()]);
    assert_eq!(report.reactivated, vec![stuck_id.to_string()]);
    assert!(store.get(done_id).unwrap().is_none(), "成功终局记录应已删");
    assert!(!tombstone_exists(&ws, done_id));
    assert!(!staging.exists(), "staging 残留应被清扫");
    assert_eq!(
        store.get(stuck_id).unwrap().unwrap().status,
        agent_status::ACTIVE,
        "commit 未达应回滚 active"
    );
}

// ---------------------------------------------------------------------------
// credential_alias_list（经 handle_migrate_commit 的 vault 臂间接覆盖在
// 集成层；此处钉裸别名映射纯函数）
// ---------------------------------------------------------------------------

#[test]
fn vault_alias_mapping_strips_prefix() {
    use super::credential_alias_list;
    let mut rec = selfbuilt_record(AGENT_ID, "node-a");
    rec.credential_refs = serde_json::json!([
        format!("vault:agent/{AGENT_ID}/api_key"),
        format!("vault:agent/{AGENT_ID}/token"),
    ]);
    let aliases = credential_alias_list(&rec);
    assert_eq!(
        aliases,
        vec![
            format!("agent/{AGENT_ID}/api_key"),
            format!("agent/{AGENT_ID}/token")
        ]
    );

    // 裸形态（无前缀）原样保留。
    rec.credential_refs = serde_json::json!(["plain-alias"]);
    assert_eq!(credential_alias_list(&rec), vec!["plain-alias".to_string()]);

    // 非数组/空 = 空清单。
    rec.credential_refs = serde_json::json!(null);
    assert!(credential_alias_list(&rec).is_empty());
}
