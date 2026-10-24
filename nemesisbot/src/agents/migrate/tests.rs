//! P6-R2 迁移模块单测（纯函数 + 真实临时目录面；零内联纪律独立文件）。

use std::path::PathBuf;

use super::{
    MigrateCommitDeps, handle_migrate_commit, journal_exists, journal_path, landing_files_dir,
    migrate_staging_root, migrate_task_id, rewrite_record_for_target, sweep_migrating,
    tombstone_exists, tombstone_path, validate_migrate_agent_id,
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

    // 案例三（R7）：migrating + 意图日志在场、无墓碑 = commit 已发出
    // （B 端可能已激活）→ 冻结转人工，绝不自动回滚。
    let frozen_id = "ffffffffffffffffffffffffffffffff";
    let mut rec3 = selfbuilt_record(frozen_id, "node-a");
    rec3.status = agent_status::MIGRATING.to_string();
    store.upsert(&rec3).unwrap();
    std::fs::write(
        journal_path(&ws, frozen_id),
        serde_json::json!({"agent_id": frozen_id, "target_node": "node-b", "started_at": "t"})
            .to_string(),
    )
    .unwrap();

    let mut report = crate::agents::SweepReport {
        activated: vec![],
        failed: vec![],
        migrated_removed: vec![],
        reactivated: vec![],
        frozen_migrating: vec![],
    };
    sweep_migrating(&store, &ws, &mut report).unwrap();

    assert_eq!(report.migrated_removed, vec![done_id.to_string()]);
    assert_eq!(report.reactivated, vec![stuck_id.to_string()]);
    assert_eq!(report.frozen_migrating, vec![frozen_id.to_string()]);
    assert!(store.get(done_id).unwrap().is_none(), "成功终局记录应已删");
    assert!(!tombstone_exists(&ws, done_id));
    assert!(!staging.exists(), "staging 残留应被清扫");
    assert_eq!(
        store.get(stuck_id).unwrap().unwrap().status,
        agent_status::ACTIVE,
        "commit 未达应回滚 active"
    );
    assert_eq!(
        store.get(frozen_id).unwrap().unwrap().status,
        agent_status::MIGRATING,
        "意图日志在场绝不自动回滚（B 端可能已激活）"
    );

    // 人工裁决路径 A：确认 B 已激活 → 落墓碑 → 下轮 sweep 收敛删记录。
    std::fs::write(
        tombstone_path(&ws, frozen_id),
        serde_json::json!({"agent_id": frozen_id, "target_node": "node-b", "migrated_at": "t"})
            .to_string(),
    )
    .unwrap();
    let mut report2 = crate::agents::SweepReport {
        activated: vec![],
        failed: vec![],
        migrated_removed: vec![],
        reactivated: vec![],
        frozen_migrating: vec![],
    };
    sweep_migrating(&store, &ws, &mut report2).unwrap();
    assert!(report2.migrated_removed.contains(&frozen_id.to_string()));
    assert!(
        store.get(frozen_id).unwrap().is_none(),
        "人工落墓碑后应收敛删记录"
    );
    assert!(!journal_exists(&ws, frozen_id), "收敛时应一并清意图日志");
}

// ---------------------------------------------------------------------------
// A 端编排失败分轨（2026-10-24 N2 根修回归锁）：post-ACK 失败保持
// migrating 等墓碑收敛；pre-commit 失败照旧回滚 active。transport 用本地
// 假实现——push_dir 走 copy_dir_recursive、call 直调真 commit handler，
// 除网络层外全链路真实（B 端落位/改写/落库都是真代码）。
// ---------------------------------------------------------------------------

/// 本地 transport：call = 直调 B 端 [`handle_migrate_commit`]（真链路），
/// push_dir = 拷贝 staging 到 B 端 landing（真实落位前置态）；`fail_push`
/// 注入 pre-commit 失败；`fail_call` 非空 = call 注入该错误串（模拟模糊
/// 类/明确拒绝类传输错误——outbox 层拍平形态），None = 直调真 handler。
struct LocalTransport {
    target: MigrateCommitDeps,
    fail_push: bool,
    fail_call: Option<String>,
}

#[async_trait::async_trait]
impl super::MigrateTransport for LocalTransport {
    fn self_node_id(&self) -> String {
        "node-a".into()
    }
    fn canonical_peer(&self, name: &str) -> Option<String> {
        Some(name.into())
    }
    fn is_peer_online(&self, _: &str) -> bool {
        true
    }
    async fn push_dir(
        &self,
        _: &str,
        _: &str,
        dir: &std::path::Path,
    ) -> Result<nemesis_cluster::outbox::PushDirOutcome, String> {
        if self.fail_push {
            return Err("push 注入失败（pre-commit）".to_string());
        }
        nemesis_cluster::transfer::copy_dir_recursive(
            dir,
            &landing_files_dir(&self.target.workspace, AGENT_ID),
        )?;
        Ok(nemesis_cluster::outbox::PushDirOutcome::Delivered)
    }
    async fn call(
        &self,
        _: &str,
        _: &str,
        payload: serde_json::Value,
        _: std::time::Duration,
    ) -> Result<serde_json::Value, String> {
        if let Some(e) = &self.fail_call {
            return Err(e.clone());
        }
        handle_migrate_commit(&self.target, payload)
    }
}

/// N2 回归锁（外评 2026-10-23 探针反演）：commit ACK 已达（B 端真活）后
/// 本地 store.remove 失败（SQLite trigger 注入 RAISE(ABORT)）——修复前外
/// 层一视同仁回滚 active = 同 uuid 双活且 sweep 失明（ACTIVE 不在收敛射
/// 程）；修复后错误带 post-ACK 标记、源端**保持 migrating** + 墓碑在场，
/// 解除注入后 sweep 按迁移成功收敛删记录，双活窗口收敛为无。
#[tokio::test]
async fn migrate_post_ack_delete_failure_stays_migrating_and_sweep_converges() {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let (source_ws, source_store) = setup_ws(source_dir.path());
    let (target_ws, target_store) = setup_ws(target_dir.path());
    let source_db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &source_ws,
    ))
    .unwrap();
    let mgr = super::super::AgentManager::from_db(source_ws.clone(), source_db);
    let mut record = selfbuilt_record(AGENT_ID, "node-a");
    record.credential_refs = serde_json::json!([]);
    source_store.upsert(&record).unwrap();
    let source_home = nemesis_path::resolve_agent_home_in_workspace(&source_ws, AGENT_ID);
    std::fs::create_dir_all(&source_home).unwrap();
    std::fs::write(source_home.join("data.txt"), "original source content").unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_memory_dir_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    mgr.set_migrate_transport(std::sync::Arc::new(LocalTransport {
        target: MigrateCommitDeps {
            workspace: target_ws.clone(),
            self_node_id: "node-b".into(),
        },
        fail_push: false,
        fail_call: None,
    }));
    // 失败注入：源库 agents 表 DELETE 全拒（复用真 store，无 mock）。
    let conn =
        rusqlite::Connection::open(nemesis_path::resolve_tasks_db_path_in_workspace(&source_ws))
            .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER review_oct23_fail_delete BEFORE DELETE ON agents \
         BEGIN SELECT RAISE(ABORT, 'review source delete failure'); END;",
    )
    .unwrap();

    let error = super::migrate_agent(&mgr, AGENT_ID, "node-b")
        .await
        .unwrap_err();

    // 修复后语义：B 端已激活，源端保持 migrating + 墓碑在场（绝不回滚）。
    assert!(
        error.contains("review source delete failure"),
        "根因错误保留在链上: {error}"
    );
    assert!(
        error.contains("保持 migrating"),
        "post-ACK 失败必须显式声明不回滚语义: {error}"
    );
    let source_status = source_store.get(AGENT_ID).unwrap().unwrap().status;
    let target_status = target_store.get(AGENT_ID).unwrap().unwrap().status;
    assert_eq!(
        source_status,
        agent_status::MIGRATING,
        "post-ACK 删除失败绝不回滚 active（回滚 = 永久双活）"
    );
    assert_eq!(target_status, agent_status::ACTIVE, "B 端已激活");
    assert!(
        tombstone_exists(&source_ws, AGENT_ID),
        "墓碑在场 = sweep 收敛锚"
    );

    // 解除注入 → sweep 按墓碑收敛：记录删 + 墓碑清，双活窗口归零。
    conn.execute_batch("DROP TRIGGER review_oct23_fail_delete;")
        .unwrap();
    let report = mgr.sweep().unwrap();
    assert!(
        report.migrated_removed.contains(&AGENT_ID.to_string()),
        "sweep 按迁移成功收敛: {:?}",
        report
    );
    assert!(source_store.get(AGENT_ID).unwrap().is_none(), "记录已删");
    assert!(!tombstone_exists(&source_ws, AGENT_ID), "墓碑已清");
}

/// pre-commit 失败（push_dir 报错）照旧回滚 active——N2 分轨只作用于
/// post-ACK，此钉子防「分轨扩大化」把可安全回滚的失败也卡在 migrating。
#[tokio::test]
async fn migrate_pre_commit_failure_rolls_back_active() {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let (source_ws, source_store) = setup_ws(source_dir.path());
    let (target_ws, _target_store) = setup_ws(target_dir.path());
    let source_db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &source_ws,
    ))
    .unwrap();
    let mgr = super::super::AgentManager::from_db(source_ws.clone(), source_db);
    let mut record = selfbuilt_record(AGENT_ID, "node-a");
    record.credential_refs = serde_json::json!([]);
    source_store.upsert(&record).unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_home_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_memory_dir_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    mgr.set_migrate_transport(std::sync::Arc::new(LocalTransport {
        target: MigrateCommitDeps {
            workspace: target_ws,
            self_node_id: "node-b".into(),
        },
        fail_push: true,
        fail_call: None,
    }));

    let error = super::migrate_agent(&mgr, AGENT_ID, "node-b")
        .await
        .unwrap_err();
    assert!(error.contains("push 注入失败"), "{error}");
    assert!(
        error.contains("已回滚 active"),
        "pre-commit 失败照旧回滚: {error}"
    );
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::ACTIVE,
        "pre-commit 失败回滚 active（B 端从未激活，回滚安全）"
    );
    assert!(
        !tombstone_exists(&source_ws, AGENT_ID),
        "pre-commit 路径无墓碑"
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

// ---------------------------------------------------------------------------
// R7（2026-10-24 复核，外评探针反演）：post-ACK「墓碑全败 + 记录删除全败」
// 复合失败——修复前 sweep 按无墓碑回滚 active = 同 uuid 双活且永久失明；
// 修复后 sweep 冻结 migrating 转人工，绝不自动回滚。
// ---------------------------------------------------------------------------

/// R7 回归锁：墓碑路径占位成目录（全部写墓碑失败）+ 源库 DELETE trigger
/// （store.remove 失败）→ post-ACK 复合失败；解除注入后 sweep 必须**冻结**
/// （migrating 保持 + frozen_migrating 上报），绝不回滚 active；人工落墓碑
/// 后 sweep 收敛删记录。
#[tokio::test]
async fn migrate_post_ack_marker_and_delete_failure_sweep_freezes_not_reactivates() {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let (source_ws, source_store) = setup_ws(source_dir.path());
    let (target_ws, target_store) = setup_ws(target_dir.path());
    let source_db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &source_ws,
    ))
    .unwrap();
    let mgr = super::super::AgentManager::from_db(source_ws.clone(), source_db);
    let mut record = selfbuilt_record(AGENT_ID, "node-a");
    record.credential_refs = serde_json::json!([]);
    source_store.upsert(&record).unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_home_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_memory_dir_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    // 失败注入①：墓碑路径占位成目录 → 三试写墓碑全败。
    let marker = tombstone_path(&source_ws, AGENT_ID);
    std::fs::create_dir_all(&marker).unwrap();
    mgr.set_migrate_transport(std::sync::Arc::new(LocalTransport {
        target: MigrateCommitDeps {
            workspace: target_ws.clone(),
            self_node_id: "node-b".into(),
        },
        fail_push: false,
        fail_call: None,
    }));
    // 失败注入②：源库 DELETE 全拒（真 store，无 mock）。
    let conn =
        rusqlite::Connection::open(nemesis_path::resolve_tasks_db_path_in_workspace(&source_ws))
            .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER review_oct24_delete BEFORE DELETE ON agents \
         BEGIN SELECT RAISE(ABORT, 'review delete failure'); END;",
    )
    .unwrap();

    let error = super::migrate_agent(&mgr, AGENT_ID, "node-b")
        .await
        .unwrap_err();

    // B 端已激活、A 端保持 migrating（N2 语义），墓碑缺席 + 意图日志在场。
    assert!(error.starts_with(super::POST_ACK_ERR_MARK), "{error}");
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::MIGRATING
    );
    assert_eq!(
        target_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::ACTIVE,
        "commit ACK 已达 = B 端已激活"
    );
    assert!(!tombstone_exists(&source_ws, AGENT_ID), "墓碑三试全败");
    assert!(
        journal_exists(&source_ws, AGENT_ID),
        "意图日志在场（commit 已发出）= sweep 冻结锚"
    );

    // 解除注入② → sweep：冻结（绝不回滚 active——B 已激活，回滚 = 双活）。
    conn.execute_batch("DROP TRIGGER review_oct24_delete;")
        .unwrap();
    let report = mgr.sweep().unwrap();
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::MIGRATING,
        "复合失败后 sweep 必须冻结，绝不自动回滚 active"
    );
    assert!(
        report.frozen_migrating.contains(&AGENT_ID.to_string()),
        "冻结必须上报转人工: {:?}",
        report
    );
    assert!(
        !report.reactivated.contains(&AGENT_ID.to_string()),
        "此场景回滚 = 同 uuid 双活: {:?}",
        report
    );

    // 人工裁决（确认 B 已激活）→ 落墓碑 → sweep 收敛删记录，双活归零。
    // （先清注入①的占位目录——人工到场修复磁盘问题后墓碑路径恢复可写。）
    std::fs::remove_dir(&marker).unwrap();
    std::fs::write(
        tombstone_path(&source_ws, AGENT_ID),
        serde_json::json!({"agent_id": AGENT_ID, "target_node": "node-b", "migrated_at": "t"})
            .to_string(),
    )
    .unwrap();
    let report = mgr.sweep().unwrap();
    assert!(report.migrated_removed.contains(&AGENT_ID.to_string()));
    assert!(
        source_store.get(AGENT_ID).unwrap().is_none(),
        "人工落墓碑后收敛删记录"
    );
    assert!(!journal_exists(&source_ws, AGENT_ID), "收敛时清意图日志");
}

/// R7 对照臂：意图日志在场、人工确认 B 未收到 commit → 删日志 → sweep
/// 回滚 active 照常服务（三分支的人工路径 B）。
#[test]
fn sweep_journal_removed_by_human_then_reactivates() {
    let dir = tempfile::tempdir().unwrap();
    let (ws, store) = setup_ws(dir.path());
    std::fs::create_dir_all(ws.join("agents")).unwrap();
    let mut rec = selfbuilt_record(AGENT_ID, "node-a");
    rec.status = agent_status::MIGRATING.to_string();
    store.upsert(&rec).unwrap();
    std::fs::write(
        journal_path(&ws, AGENT_ID),
        serde_json::json!({"agent_id": AGENT_ID, "target_node": "node-b", "started_at": "t"})
            .to_string(),
    )
    .unwrap();

    // 冻结在先（日志在场不动手）。
    let mut report = crate::agents::SweepReport {
        activated: vec![],
        failed: vec![],
        migrated_removed: vec![],
        reactivated: vec![],
        frozen_migrating: vec![],
    };
    sweep_migrating(&store, &ws, &mut report).unwrap();
    assert_eq!(report.frozen_migrating, vec![AGENT_ID.to_string()]);

    // 人工裁决（确认 B 未收到）→ 删日志 → 下轮 sweep 回滚 active。
    std::fs::remove_file(journal_path(&ws, AGENT_ID)).unwrap();
    let mut report = crate::agents::SweepReport {
        activated: vec![],
        failed: vec![],
        migrated_removed: vec![],
        reactivated: vec![],
        frozen_migrating: vec![],
    };
    sweep_migrating(&store, &ws, &mut report).unwrap();
    assert_eq!(report.reactivated, vec![AGENT_ID.to_string()]);
    assert_eq!(
        store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::ACTIVE
    );
}

// ---------------------------------------------------------------------------
// R79 残余面（2026-10-24 台账复核）：commit call **模糊类**失败（超时/取消/
// 连接中断——请求可能已达 B 且被处理）——修复前 ⑫ 删意图日志 + 回滚 active，
// B 端恰好已激活时 = 同 uuid 双活且 sweep 永久失明（v1 诚实窗口）；修复后
// 标记 COMMIT_ACK_LOST_MARK 走冻结轨（与 R7 同机制）。明确拒绝/确定未发出
// 类照旧回滚（防冻结轨扩大化）。
// ---------------------------------------------------------------------------

/// 模糊类（Timeout 锚串，outbox 层拍平真形态）→ 错误带 COMMIT_ACK_LOST_MARK
/// + 保持 migrating + 意图日志在场（冻结锚）；sweep 冻结转人工（绝不进
/// reactivated）；人工裁决「B 未收到」删日志后 sweep 回滚 active 复原。
#[tokio::test]
async fn migrate_commit_ack_lost_freezes_instead_of_rollback() {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let (source_ws, source_store) = setup_ws(source_dir.path());
    let (target_ws, target_store) = setup_ws(target_dir.path());
    let source_db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &source_ws,
    ))
    .unwrap();
    let mgr = super::super::AgentManager::from_db(source_ws.clone(), source_db);
    let mut record = selfbuilt_record(AGENT_ID, "node-a");
    record.credential_refs = serde_json::json!([]);
    source_store.upsert(&record).unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_home_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_memory_dir_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    mgr.set_migrate_transport(std::sync::Arc::new(LocalTransport {
        target: MigrateCommitDeps {
            workspace: target_ws,
            self_node_id: "node-b".into(),
        },
        fail_push: false,
        fail_call: Some(
            "[Transfer] RPC agent_migrate_commit → node-b 失败: Timeout waiting for response"
                .into(),
        ),
    }));

    let error = super::migrate_agent(&mgr, AGENT_ID, "node-b")
        .await
        .unwrap_err();

    // 冻结轨语义：标记 + 根因保留 + 显式不回滚声明。
    assert!(
        error.starts_with(super::COMMIT_ACK_LOST_MARK),
        "模糊类失败必须带结果未知标记: {error}"
    );
    assert!(
        error.contains("Timeout waiting for response"),
        "根因错误保留在链上: {error}"
    );
    assert!(error.contains("保持 migrating"), "{error}");
    // A 端保持 migrating + 日志在场（sweep 冻结锚）；绝不回滚。
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::MIGRATING,
        "commit 结果未知绝不回滚 active（B 可能已激活，回滚 = 双活）"
    );
    assert!(
        journal_exists(&source_ws, AGENT_ID),
        "意图日志必须保留（sweep 冻结锚）——修复前此臂删日志 + 回滚"
    );
    assert!(!tombstone_exists(&source_ws, AGENT_ID));
    // call 注入失败 = 请求从未到 B 端 handler：B 端无记录。
    assert!(
        target_store.get(AGENT_ID).unwrap().is_none(),
        "注入形态下 B 端未收到 commit"
    );

    // sweep 冻结转人工：migrating 保持 + frozen_migrating 上报 + 不回滚。
    let report = mgr.sweep().unwrap();
    assert!(
        report.frozen_migrating.contains(&AGENT_ID.to_string()),
        "模糊失败后 sweep 必须冻结转人工: {:?}",
        report
    );
    assert!(
        !report.reactivated.contains(&AGENT_ID.to_string()),
        "此场景自动回滚 = 同 uuid 双活: {:?}",
        report
    );
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::MIGRATING
    );

    // 人工裁决（确认 B 未收到）→ 删日志 → sweep 回滚 active 复原。
    std::fs::remove_file(journal_path(&source_ws, AGENT_ID)).unwrap();
    let report = mgr.sweep().unwrap();
    assert!(
        report.reactivated.contains(&AGENT_ID.to_string()),
        "人工确认未收到后回滚 active: {:?}",
        report
    );
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::ACTIVE
    );
}

/// 明确拒绝类（对端错误锚串，B 端从未激活）→ 照旧回滚 active + 意图日志
/// 清除——防冻结轨扩大化把可安全回滚的失败也卡在 migrating。
#[tokio::test]
async fn migrate_commit_definite_reject_still_rolls_back() {
    let source_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let (source_ws, source_store) = setup_ws(source_dir.path());
    let (target_ws, _target_store) = setup_ws(target_dir.path());
    let source_db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &source_ws,
    ))
    .unwrap();
    let mgr = super::super::AgentManager::from_db(source_ws.clone(), source_db);
    let mut record = selfbuilt_record(AGENT_ID, "node-a");
    record.credential_refs = serde_json::json!([]);
    source_store.upsert(&record).unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_home_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    std::fs::create_dir_all(nemesis_path::resolve_agent_memory_dir_in_workspace(
        &source_ws, AGENT_ID,
    ))
    .unwrap();
    mgr.set_migrate_transport(std::sync::Arc::new(LocalTransport {
        target: MigrateCommitDeps {
            workspace: target_ws,
            self_node_id: "node-b".into(),
        },
        fail_push: false,
        fail_call: Some(
            "[Transfer] agent_migrate_commit 对端错误: agent id 冲突（B 端已有同 id 记录）".into(),
        ),
    }));

    let error = super::migrate_agent(&mgr, AGENT_ID, "node-b")
        .await
        .unwrap_err();

    assert!(error.contains("对端错误"), "根因错误保留在链上: {error}");
    assert!(
        !error.starts_with(super::COMMIT_ACK_LOST_MARK),
        "明确拒绝类不走冻结轨: {error}"
    );
    assert!(
        error.contains("已回滚 active"),
        "明确拒绝 = B 端从未激活，回滚安全: {error}"
    );
    assert_eq!(
        source_store.get(AGENT_ID).unwrap().unwrap().status,
        agent_status::ACTIVE
    );
    assert!(
        !journal_exists(&source_ws, AGENT_ID),
        "回滚臂清意图日志（残留会让 sweep 误冻结）"
    );
    assert!(!tombstone_exists(&source_ws, AGENT_ID));
}

/// 分类器锚串表：四模糊串（含 outbox 拍平后的真形态包裹）true；确定未发出
/// （限流/序列化/客户端缺席）、对端明确拒绝、pre-commit 错误形态 false。
#[test]
fn commit_call_may_have_landed_anchors() {
    use super::commit_call_may_have_landed;

    // 模糊类（RpcClientError Display 经 outbox format! 包裹后的真形态）。
    assert!(commit_call_may_have_landed(
        "[Transfer] RPC agent_migrate_commit → node-b 失败: Timeout waiting for response"
    ));
    assert!(commit_call_may_have_landed(
        "[Transfer] RPC agent_migrate_commit → node-b 失败: Context cancelled"
    ));
    assert!(commit_call_may_have_landed(
        "[Transfer] RPC agent_migrate_commit → node-b 失败: Connection error: dial: no route"
    ));
    assert!(commit_call_may_have_landed(
        "[Transfer] RPC agent_migrate_commit → node-b 失败: IO error: broken pipe"
    ));

    // 确定未发出（请求未离开本节点）。
    assert!(!commit_call_may_have_landed(
        "[Transfer] RPC agent_migrate_commit → node-b 失败: Rate limited: bucket exhausted"
    ));
    assert!(!commit_call_may_have_landed(
        "[Transfer] RPC agent_migrate_commit → node-b 失败: Serialization error: bad payload"
    ));
    assert!(!commit_call_may_have_landed(
        "集群 RPC 客户端未装配（集群未启动？）"
    ));

    // 对端明确拒绝（resp.error 路径——B 端处理过且拒绝）。
    assert!(!commit_call_may_have_landed(
        "[Transfer] agent_migrate_commit 对端错误: agent id 冲突"
    ));

    // pre-commit 各步错误形态（不含传输锚）。
    assert!(!commit_call_may_have_landed(
        "staging 目录创建失败 (C:\\tmp\\x): Access is denied (os error 5)"
    ));
    assert!(!commit_call_may_have_landed("push 注入失败（pre-commit）"));
    assert!(!commit_call_may_have_landed(""));
}
