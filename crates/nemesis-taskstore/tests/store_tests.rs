//! nemesis-taskstore 集成测试：open/迁移幂等/损坏熔断/任务语义对齐
//! InMemory/审批条件更新幂等/交付幂等/runs 状态机。

use std::thread;

use nemesis_taskstore::{SqliteApprovalJournal, SqliteTaskStore, TaskDb};
use nemesis_types::approval_journal::{ApprovalDecision, ApprovalJournal, ApprovalJournalEntry};
use nemesis_types::cluster::{Task, TaskStatus};

fn temp_db(tag: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(format!("{tag}.db"));
    (dir, path)
}

fn sample_task(id: &str, status: TaskStatus) -> Task {
    Task {
        id: id.to_string(),
        status,
        action: "peer_chat".to_string(),
        peer_id: "Node-B".to_string(),
        payload: serde_json::json!({"prompt": "hello"}),
        result: None,
        original_channel: "web".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: "2026-10-16T10:00:00+08:00".to_string(),
        completed_at: None,
    }
}

fn sample_approval(id: &str, short: Option<&str>) -> ApprovalJournalEntry {
    ApprovalJournalEntry {
        id: id.to_string(),
        broker: "channel".to_string(),
        short_id: short.map(|s| s.to_string()),
        operation: "process_exec".to_string(),
        target: "cargo build".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        payload: serde_json::json!({"channel": "telegram", "chat_id": "42"}),
        status: "pending".to_string(),
        created_at: "2026-10-16T10:00:00+08:00".to_string(),
        decided_at: None,
        decision_note: None,
        decided_by: None,
    }
}

// --- open / 迁移 / 熔断 ---

#[test]
fn open_creates_schema_and_reopen_is_idempotent() {
    let (_dir, path) = temp_db("open_reopen");
    let first = TaskDb::open(&path).expect("first open");
    drop(first);
    // 二次 open（版本已推进）= 幂等成功；且数据存活。
    let second = TaskDb::open(&path).expect("second open");
    assert!(second.delivery_by_key("nope").expect("read").is_none());
}

#[test]
fn concurrent_first_open_is_safe() {
    let (_dir, path) = temp_db("race");
    // 双连接同时首次 open（BEGIN IMMEDIATE 串行化迁移）。
    let p1 = path.clone();
    let h1 = thread::spawn(move || TaskDb::open(&p1).map(|_| ()));
    let h2 = thread::spawn(move || TaskDb::open(&path).map(|_| ()));
    h1.join().expect("h1").expect("open 1");
    h2.join().expect("h2").expect("open 2");
}

#[test]
fn corrupted_file_fails_closed() {
    let (_dir, path) = temp_db("corrupt");
    // 先造一个合法库再覆写为垃圾字节（SQLite 头损坏，quick_check 必炸）。
    drop(TaskDb::open(&path).expect("seed"));
    std::fs::write(&path, vec![b'x'; 8192]).expect("overwrite");
    let err = TaskDb::open(&path).expect_err("must fail closed");
    assert!(err.contains("integrity"), "unexpected error: {err}");
}

#[test]
fn newer_user_version_refuses_to_open() {
    let (_dir, path) = temp_db("newer");
    drop(TaskDb::open(&path).expect("seed"));
    let conn = rusqlite_conn(&path);
    conn.pragma_update(None, "user_version", 999)
        .expect("stamp");
    let err = TaskDb::open(&path).expect_err("must refuse downgrade-open");
    assert!(err.contains("newer version"), "unexpected error: {err}");
}

fn rusqlite_conn(path: &std::path::Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).expect("raw conn")
}

// --- TaskStore 语义（对齐 InMemory） ---

#[test]
fn task_round_trip_and_error_parity() {
    let (_dir, path) = temp_db("tasks");
    let store = SqliteTaskStore::open(&path).expect("open");

    store
        .create(sample_task("t1", TaskStatus::Pending))
        .expect("create");
    // 重复创建 → 与 InMemory 同文案。
    let dup = store
        .create(sample_task("t1", TaskStatus::Pending))
        .unwrap_err();
    assert!(dup.contains("task already exists: t1"), "{dup}");

    let got = store.get("t1").expect("get");
    assert_eq!(got.status, TaskStatus::Pending);
    assert_eq!(got.payload, serde_json::json!({"prompt": "hello"}));
    assert_eq!(got.original_channel, "web");
    assert!(got.completed_at.is_none());

    let missing = store.get("nope").unwrap_err();
    assert!(missing.contains("task not found: nope"), "{missing}");
}

#[test]
fn terminal_stamp_follows_inmemory_rules() {
    let (_dir, path) = temp_db("stamp");
    let store = SqliteTaskStore::open(&path).expect("open");
    store
        .create(sample_task("t1", TaskStatus::Running))
        .expect("create");

    // 非终态更新不盖 completed_at。
    store
        .update_result(
            "t1",
            TaskStatus::Running,
            Some(serde_json::json!("partial")),
        )
        .expect("update running");
    assert!(store.get("t1").expect("get").completed_at.is_none());

    // 终态盖章。
    store
        .update_result("t1", TaskStatus::Completed, Some(serde_json::json!("done")))
        .expect("complete");
    let done = store.get("t1").expect("get");
    assert_eq!(done.status, TaskStatus::Completed);
    assert!(
        done.completed_at.is_some(),
        "terminal must stamp completed_at"
    );
    assert_eq!(done.result, Some(serde_json::json!("done")));
}

#[test]
fn conditional_update_first_wins_and_parity() {
    let (_dir, path) = temp_db("cond");
    let store = SqliteTaskStore::open(&path).expect("open");
    store
        .create(sample_task("t1", TaskStatus::Pending))
        .expect("create");

    // 命中期望集 → Ok(true)。
    let won = store
        .update_result_if_status(
            "t1",
            &[TaskStatus::Pending, TaskStatus::Running],
            TaskStatus::Running,
            None,
        )
        .expect("first");
    assert!(won);

    // 同条件二投 → Ok(false)（状态已不匹配）。
    let lost = store
        .update_result_if_status("t1", &[TaskStatus::Pending], TaskStatus::Running, None)
        .expect("second");
    assert!(!lost);

    // 任务不存在 → Err。
    let err = store
        .update_result_if_status("ghost", &[TaskStatus::Pending], TaskStatus::Running, None)
        .unwrap_err();
    assert!(err.contains("task not found: ghost"), "{err}");
}

#[test]
fn list_and_delete_and_persistence_across_reopen() {
    let (_dir, path) = temp_db("persist");
    {
        let store = SqliteTaskStore::open(&path).expect("open");
        store
            .create(sample_task("a", TaskStatus::Pending))
            .expect("a");
        store
            .create(sample_task("b", TaskStatus::Running))
            .expect("b");
        assert_eq!(
            store
                .list_by_status(TaskStatus::Pending)
                .expect("list")
                .len(),
            1
        );
        assert_eq!(store.list_all().expect("all").len(), 2);
        store.delete("a").expect("delete");
        assert!(store.delete("a").is_err(), "second delete must Err");
    }
    // 重开（新连接）后数据仍在 = 持久化成立。
    let store = SqliteTaskStore::open(&path).expect("reopen");
    assert_eq!(store.list_all().expect("all").len(), 1);
    assert_eq!(store.get("b").expect("b").status, TaskStatus::Running);
}

// --- 审批台账 ---

#[test]
fn approval_decision_is_exactly_once() {
    let (_dir, path) = temp_db("approvals");
    let journal = SqliteApprovalJournal::open(&path).expect("open");
    journal
        .record_pending(&sample_approval("req-1", Some("a1b2c3d4")))
        .expect("pending");
    // 幂等：重复 pending 不覆盖不报错。
    journal
        .record_pending(&sample_approval("req-1", Some("a1b2c3d4")))
        .expect("dup pending");

    assert_eq!(journal.pending_entries().len(), 1);

    let won = journal
        .record_decision("req-1", ApprovalDecision::Approved, Some("ok"), "web-user")
        .expect("decide");
    assert!(won);
    // 二投 = Ok(false)（先到先得）。
    let lost = journal
        .record_decision("req-1", ApprovalDecision::Denied, None, "im-user")
        .expect("second");
    assert!(!lost);

    let entry = journal.entry("req-1").expect("entry");
    assert_eq!(entry.status, "approved");
    assert_eq!(entry.decided_by.as_deref(), Some("web-user"));
    assert!(entry.decided_at.is_some());
    assert!(journal.pending_entries().is_empty());
}

#[test]
fn approval_pending_survives_restart_and_stale_respondable() {
    let (_dir, path) = temp_db("stale");
    {
        let journal = SqliteApprovalJournal::open(&path).expect("open");
        journal
            .record_pending(&sample_approval("req-old", Some("deadbeef")))
            .expect("pending");
    }
    // 「重启」（新连接）后仍可按短编号补裁，且只生效一次。
    let journal = SqliteApprovalJournal::open(&path).expect("reopen");
    let found = journal.find_pending("deadbeef").expect("short id hit");
    assert_eq!(found.id, "req-old");
    assert!(
        journal.find_pending("req-old").is_some(),
        "full id also hits"
    );
    assert!(journal.find_pending("no-such").is_none());

    let won = journal
        .record_decision(
            "req-old",
            ApprovalDecision::Expired,
            Some("超时"),
            "timeout",
        )
        .expect("decide");
    assert!(won);
    assert!(
        journal.find_pending("deadbeef").is_none(),
        "decided = no longer pending"
    );
}

// --- 交付账本 ---

#[test]
fn delivery_idempotency_and_confirm() {
    let (_dir, path) = temp_db("deliveries");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({"result": "42"});

    assert!(
        db.record_delivery("cluster_result", Some("t-9"), "t-9", &payload)
            .expect("first")
    );
    assert!(
        !db.record_delivery("cluster_result", Some("t-9"), "t-9", &payload)
            .expect("dup"),
        "duplicate key must not re-record"
    );

    let rec = db.delivery_by_key("t-9").expect("read").expect("present");
    assert_eq!(rec.status, "delivered");
    assert!(rec.confirmed_at.is_none());

    assert!(db.confirm_delivery("t-9").expect("confirm"));
    assert!(
        !db.confirm_delivery("t-9").expect("re-confirm"),
        "already confirmed"
    );
    assert_eq!(
        db.delivery_by_key("t-9")
            .expect("read")
            .expect("present")
            .status,
        "confirmed"
    );
    assert!(db.delivery_by_key("ghost").expect("read").is_none());
}

// --- 交付账本（P4 交付门：拒绝语义 + 查询面 + v3 迁移） ---

#[test]
fn delivery_reject_semantics() {
    let (_dir, path) = temp_db("deliveries_reject");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({"branch": "nemesis/run-1"});

    assert!(
        db.record_delivery("branch", Some("run-1"), "run-1:branch", &payload)
            .expect("record")
    );

    // 拒绝生效一次：delivered → rejected，原因留痕。
    assert!(
        db.reject_delivery("run-1:branch", "锚点双检 FAIL")
            .expect("reject")
    );
    let rec = db
        .delivery_by_key("run-1:branch")
        .expect("read")
        .expect("present");
    assert_eq!(rec.status, "rejected");
    assert_eq!(rec.reject_reason.as_deref(), Some("锚点双检 FAIL"));
    assert!(rec.rejected_at.is_some());
    assert!(rec.confirmed_at.is_none(), "拒绝路径不产生 confirmed_at");

    // 幂等：已 rejected 再拒 = Ok(false)，原因不被覆盖。
    assert!(
        !db.reject_delivery("run-1:branch", "二次理由")
            .expect("re-reject")
    );
    let rec = db
        .delivery_by_key("run-1:branch")
        .expect("read")
        .expect("present");
    assert_eq!(rec.reject_reason.as_deref(), Some("锚点双检 FAIL"));

    // 对不存在记录拒绝 = Ok(false)（条件更新零命中）。
    assert!(!db.reject_delivery("ghost", "x").expect("ghost"));
}

#[test]
fn delivery_confirm_then_reject_overrides() {
    let (_dir, path) = temp_db("deliveries_conf_rej");
    let db = TaskDb::open(&path).expect("open");
    let payload = serde_json::json!({});

    assert!(
        db.record_delivery("review", Some("run-2"), "run-2:review", &payload)
            .expect("record")
    );
    assert!(db.confirm_delivery("run-2:review").expect("confirm"));
    // 人工复核推翻自动确认：confirmed → rejected 合法。
    assert!(
        db.reject_delivery("run-2:review", "checkers 多数决 FAIL")
            .expect("reject")
    );
    let rec = db
        .delivery_by_key("run-2:review")
        .expect("read")
        .expect("present");
    assert_eq!(rec.status, "rejected");
    assert!(
        rec.confirmed_at.is_some(),
        "历史确认时间戳保留（审计链完整）"
    );
    assert!(rec.rejected_at.is_some());
}

#[test]
fn delivery_by_run_and_recent_listing() {
    let (_dir, path) = temp_db("deliveries_list");
    let db = TaskDb::open(&path).expect("open");

    for (run, key) in [
        ("run-a", "run-a:branch"),
        ("run-a", "run-a:review"),
        ("run-b", "run-b:branch"),
        ("run-b", "run-b:review"),
        ("run-b", "run-b:dossier"),
    ] {
        assert!(
            db.record_delivery("branch", Some(run), key, &serde_json::json!({}))
                .expect("record")
        );
    }

    // run 维过滤：只含本 run 的记录，created_at + id 稳定序。
    let run_b = db.deliveries_by_run("run-b").expect("by run");
    assert_eq!(run_b.len(), 3);
    assert!(run_b.iter().all(|r| r.run_id.as_deref() == Some("run-b")));
    let mut keys: Vec<&str> = run_b.iter().map(|r| r.idempotency_key.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["run-b:branch", "run-b:dossier", "run-b:review"]);
    assert!(db.deliveries_by_run("run-ghost").expect("empty").is_empty());

    // recent 截断：limit 0 = 空列表（不做全量拉取）；超量不炸。
    let recent = db.deliveries_recent(3).expect("recent");
    assert_eq!(recent.len(), 3);
    assert!(db.deliveries_recent(0).expect("zero").is_empty());
    assert_eq!(db.deliveries_recent(10).expect("over").len(), 5);
}

#[test]
fn schema_v2_database_migrates_to_v3_keeping_rows() {
    let (_dir, path) = temp_db("v2_upgrade");
    // 手工构造 v2 形态库（deliveries 无 rejected_at/reject_reason 列，
    // user_version=2）——锁定「存量 v2 库被新代码打开」的迁移路径。
    let conn = rusqlite_conn(&path);
    conn.execute_batch(
        "CREATE TABLE runs (id TEXT PRIMARY KEY, kind TEXT NOT NULL DEFAULT 'cluster_task', \
         title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'pending', \
         action TEXT NOT NULL DEFAULT '', peer_id TEXT NOT NULL DEFAULT '', \
         payload TEXT NOT NULL DEFAULT '{}', result TEXT, original_channel TEXT NOT NULL DEFAULT '', \
         original_chat_id TEXT NOT NULL DEFAULT '', created_at TEXT NOT NULL, completed_at TEXT, \
         updated_at TEXT NOT NULL DEFAULT '');
         CREATE TABLE deliveries (id TEXT PRIMARY KEY, run_id TEXT, kind TEXT NOT NULL DEFAULT 'result', \
         idempotency_key TEXT NOT NULL UNIQUE, status TEXT NOT NULL DEFAULT 'recorded', \
         payload TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL, delivered_at TEXT, \
         confirmed_at TEXT);
         INSERT INTO deliveries (id, run_id, kind, idempotency_key, status, payload, created_at, delivered_at)
         VALUES ('d-1', 'old-run', 'cluster_result', 'old-run', 'delivered', '{}', \
         '2026-10-16T10:00:00+08:00', '2026-10-16T10:00:01+08:00');",
    )
    .expect("v2 schema");
    conn.pragma_update(None, "user_version", 2)
        .expect("stamp v2");
    drop(conn);

    // 新代码打开 → 静默迁到 v3，旧数据完好且新列可用。
    let db = TaskDb::open(&path).expect("open after migration");
    let rec = db
        .delivery_by_key("old-run")
        .expect("read")
        .expect("legacy row survives");
    assert_eq!(rec.status, "delivered");
    assert!(rec.rejected_at.is_none());
    assert!(rec.reject_reason.is_none());
    // 新列可用：对迁移过来的旧记录直接拒绝。
    assert!(db.reject_delivery("old-run", "补拒").expect("reject"));
    assert_eq!(
        db.delivery_by_key("old-run")
            .expect("read")
            .expect("present")
            .status,
        "rejected"
    );
}

// --- runs / attempts 状态机 ---

#[test]
fn run_state_machine_and_attempts() {
    let (_dir, path) = temp_db("runs");
    let db = TaskDb::open(&path).expect("open");

    let run_id = db
        .create_run(&nemesis_taskstore::NewRun {
            kind: "agent_run".into(),
            title: "测试运行".into(),
            action: "exec".into(),
            peer_id: "local".into(),
            payload: serde_json::json!({}),
        })
        .expect("create run");
    assert_eq!(db.get_run(&run_id).expect("get").status, "pending");

    db.set_run_status(&run_id, "running").expect("running");
    db.set_run_status(&run_id, "succeeded").expect("succeeded");
    // 终态不可再转移；同终态幂等放行。
    assert!(
        db.set_run_status(&run_id, "running").is_err(),
        "terminal is immutable"
    );
    db.set_run_status(&run_id, "succeeded")
        .expect("idempotent re-entry");
    assert!(
        db.set_run_status(&run_id, "bogus").is_err(),
        "invalid status loud"
    );
    assert!(db.get_run(&run_id).expect("get").completed_at.is_some());

    // attempt 生命周期。
    let a1 = db.begin_attempt(&run_id, "node-a").expect("attempt 1");
    let a2 = db
        .begin_attempt(&run_id, "node-b")
        .expect("attempt 2 (重派)");
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].seq, 1);
    assert_eq!(attempts[1].seq, 2);
    db.finish_attempt(a1, "failed", Some("worker crashed"), None)
        .expect("finish 1");
    db.finish_attempt(
        a2,
        "succeeded",
        None,
        Some(&serde_json::json!({"tokens": 42})),
    )
    .expect("finish 2");
    assert!(
        db.finish_attempt(a1, "succeeded", None, None).is_err(),
        "double finish rejected"
    );
    let attempts = db.attempts_of(&run_id).expect("attempts");
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].error.as_deref(), Some("worker crashed"));
    assert_eq!(attempts[1].usage, Some(serde_json::json!({"tokens": 42})));
}

#[test]
fn wal_allows_reader_while_writer_active() {
    let (_dir, path) = temp_db("wal");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteTaskStore::from_db(db.clone());
    store
        .create(sample_task("w", TaskStatus::Pending))
        .expect("write");
    // 第二连接（journal 面）并发读——WAL 下读不阻塞。
    let journal = SqliteApprovalJournal::from_db(db);
    assert!(journal.pending_entries().is_empty());
}

// --- agents 表（P2：Agent 持久实体） ---

use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};
use nemesis_taskstore::{AgentRecord, SqliteAgentsStore};

fn sample_agent(id: &str, name: &str, record_kind: &str) -> AgentRecord {
    let mut rec = AgentRecord::new_provisioning(id, name, record_kind);
    rec.exec_root = format!(r"C:\ws\agents\{id}\home");
    rec.home_path = format!(r"C:\ws\agents\{id}\home");
    rec
}

#[test]
fn agent_two_phase_create_and_persistence() {
    let (_dir, path) = temp_db("agents_two_phase");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);

    // 阶段一：provisioning 落库（记忆命名空间自动派生）。
    let rec = sample_agent("a-1", "builder", agent_kind::SELFBUILT);
    assert_eq!(rec.status, agent_status::PROVISIONING);
    assert_eq!(rec.memory_namespace, "agents/a-1");
    store.upsert(&rec).expect("upsert");

    // 半创建态可查（启动 sweep 的输入面）。
    let got = store.get("a-1").expect("get").expect("exists");
    assert_eq!(got.status, agent_status::PROVISIONING);

    // 阶段二：home 物化完成后转 active——条件转移恰好生效一次。
    assert!(
        store
            .transition_status("a-1", agent_status::PROVISIONING, agent_status::ACTIVE)
            .expect("transition")
    );
    // 重复转移（并发二次收尾）不再生效。
    assert!(
        !store
            .transition_status("a-1", agent_status::PROVISIONING, agent_status::ACTIVE)
            .expect("re-transition")
    );

    // 重启（重开库）后身份/路径/命名空间完好。
    drop(store);
    let db2 = TaskDb::open(&path).expect("reopen");
    let store2 = SqliteAgentsStore::from_db(db2);
    let got = store2.get("a-1").expect("get").expect("exists");
    assert_eq!(got.status, agent_status::ACTIVE);
    assert_eq!(got.memory_namespace, "agents/a-1");
    assert_eq!(got.home_path, r"C:\ws\agents\a-1\home");
    assert_eq!(got.kind, agent_kind::SELFBUILT);
}

#[test]
fn agent_transition_only_from_matching_state() {
    let (_dir, path) = temp_db("agents_transition");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    store
        .upsert(&sample_agent("a-2", "x", agent_kind::MAIN))
        .expect("upsert");
    // 错误的 from 态：不生效。
    assert!(
        !store
            .transition_status("a-2", agent_status::ACTIVE, agent_status::FAILED)
            .expect("wrong-from")
    );
    // 正确链路：provisioning → active →（测试性）failed 只允许从 active。
    assert!(
        store
            .transition_status("a-2", agent_status::PROVISIONING, agent_status::ACTIVE)
            .expect("to-active")
    );
    assert!(
        !store
            .transition_status("a-2", agent_status::PROVISIONING, agent_status::FAILED)
            .expect("wrong-from-2")
    );
    assert!(
        store
            .transition_status("a-2", agent_status::ACTIVE, agent_status::FAILED)
            .expect("to-failed")
    );
    assert_eq!(
        store.get("a-2").expect("get").expect("exists").status,
        agent_status::FAILED
    );
}

#[test]
fn agent_sweep_finds_stuck_provisioning_records() {
    let (_dir, path) = temp_db("agents_sweep");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    store
        .upsert(&sample_agent("stuck-1", "half", agent_kind::SELFBUILT))
        .expect("upsert 1");
    store
        .upsert(&sample_agent("ok-1", "fine", agent_kind::PROJECT))
        .expect("upsert 2");
    store
        .transition_status("ok-1", agent_status::PROVISIONING, agent_status::ACTIVE)
        .expect("activate");
    // 启动 sweep 输入面：list_all + 状态过滤（store 侧稳定序）。
    let all = store.list_all().expect("list");
    assert_eq!(all.len(), 2);
    let stuck: Vec<_> = all
        .iter()
        .filter(|r| r.status == agent_status::PROVISIONING)
        .collect();
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0].agent_id, "stuck-1");
    // 收敛臂：可续建的续建；无法续建的标 failed。
    assert!(
        store
            .transition_status("stuck-1", agent_status::PROVISIONING, agent_status::FAILED)
            .expect("fail")
    );
}

#[test]
fn agent_upsert_keeps_created_at_and_updates_metadata() {
    let (_dir, path) = temp_db("agents_upsert");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    let mut rec = sample_agent("a-3", "before", agent_kind::SELFBUILT);
    store.upsert(&rec).expect("upsert 1");
    let created = store.get("a-3").expect("get").expect("exists").created_at;

    rec.name = "after".to_string();
    rec.tier = "big".to_string();
    rec.credential_refs = serde_json::json!(["vault:agent/a-3/github"]);
    store.upsert(&rec).expect("upsert 2");

    let got = store.get("a-3").expect("get").expect("exists");
    assert_eq!(got.name, "after");
    assert_eq!(got.tier, "big");
    assert_eq!(got.created_at, created, "created_at 不被 upsert 覆盖");
    assert_eq!(
        got.credential_refs,
        serde_json::json!(["vault:agent/a-3/github"])
    );
}

#[test]
fn agent_remove_and_list_order_stable() {
    let (_dir, path) = temp_db("agents_remove");
    let db = TaskDb::open(&path).expect("open");
    let store = SqliteAgentsStore::from_db(db);
    // created_at 人为对齐到同值，专门打 tie-break（agent_id 稳定序）。
    for id in ["z-1", "a-1", "m-1"] {
        let mut rec = sample_agent(id, id, agent_kind::SELFBUILT);
        rec.created_at = "2026-10-16T10:00:00+08:00".to_string();
        store.upsert(&rec).expect("upsert");
    }
    let ids: Vec<_> = store
        .list_all()
        .expect("list")
        .iter()
        .map(|r| r.agent_id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["a-1", "m-1", "z-1"],
        "created_at 同值时按 agent_id 稳定序"
    );
    store.remove("m-1").expect("remove");
    assert!(store.get("m-1").expect("get").is_none());
    assert_eq!(store.list_all().expect("list").len(), 2);
}
