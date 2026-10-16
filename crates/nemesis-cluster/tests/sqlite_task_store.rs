//! P1 统一任务层：SqliteTaskStore 过 TaskStore trait 的集群侧集成回归。
//!
//! 覆盖三条链路：
//! 1. trait 面逐方法语义（create/get/update/条件更新/delete/list）——
//!    与 InMemoryTaskStore 语义逐条对齐（错误文案/盖章规则/先到先得）；
//! 2. `TaskManager::with_store(SqliteTaskStore)` 生产换装链路——
//!    create→assign→complete 全走 tasks.db，重启（重开库）后任务可读；
//! 3. gateway 杀进程重启验收的最小等价：重开 TaskDb 后 pending/在途
//!    状态原样恢复（P1 验收门「在途任务状态可恢复」的单元级锚点）。

use nemesis_cluster::{InMemoryTaskStore, TaskManager, TaskStore};
use nemesis_types::cluster::{Task, TaskStatus};

use nemesis_taskstore::TaskDb;

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tasks.db");
    (dir, path)
}

fn sample_task(id: &str, action: &str) -> Task {
    Task {
        id: id.to_string(),
        status: TaskStatus::Pending,
        action: action.to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({"prompt": "hello"}),
        result: None,
        original_channel: "web".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    }
}

/// trait 面：create→get 往返 + 错误语义与 InMemory 对齐。
#[test]
fn trait_round_trip_and_error_parity() {
    let (_dir, path) = temp_db();
    let db = TaskDb::open(&path).expect("open");
    let store = nemesis_taskstore::SqliteTaskStore::from_db(db);

    let t = sample_task("t-1", "review code");
    store.create(t.clone()).expect("create");
    // 重复 create 诚实报错（与 InMemory 同文案形态）。
    assert!(
        store.create(t.clone()).is_err(),
        "duplicate create must Err"
    );

    let got = store.get("t-1").expect("get");
    assert_eq!(got.action, "review code");
    assert_eq!(got.original_channel, "web");
    assert_eq!(got.status, TaskStatus::Pending);

    // 缺失任务 get/delete/update 与 InMemory 同为 Err。
    assert!(store.get("nope").is_err());
    assert!(store.delete("nope").is_err());
    assert!(
        store
            .update_result("nope", TaskStatus::Completed, None)
            .is_err()
    );

    // 非终态 update_result 不盖 completed_at；终态盖。
    store
        .update_result("t-1", TaskStatus::Running, None)
        .expect("to running");
    assert!(store.get("t-1").expect("get").completed_at.is_none());
    store
        .update_result(
            "t-1",
            TaskStatus::Completed,
            Some(serde_json::json!("done")),
        )
        .expect("to completed");
    let fin = store.get("t-1").expect("get");
    assert_eq!(fin.status, TaskStatus::Completed);
    assert!(fin.completed_at.is_some());
    assert_eq!(fin.result, Some(serde_json::json!("done")));
}

/// trait 面：update_result_if_status 先到先得（审批幂等/防重复续行的原子闸）。
#[test]
fn trait_conditional_update_first_wins() {
    let (_dir, path) = temp_db();
    let db = TaskDb::open(&path).expect("open");
    let store = nemesis_taskstore::SqliteTaskStore::from_db(db);

    store
        .create(sample_task("t-2", "dispatch"))
        .expect("create");

    // 第一次：Pending→Running 命中。
    let first = store
        .update_result_if_status("t-2", &[TaskStatus::Pending], TaskStatus::Running, None)
        .expect("cond update");
    assert!(first, "first transition must win");

    // 第二次同条件：状态已 Running，不命中 → Ok(false)。
    let second = store
        .update_result_if_status("t-2", &[TaskStatus::Pending], TaskStatus::Running, None)
        .expect("cond update 2");
    assert!(!second, "second transition must lose");

    // 多期望状态任一命中即可。
    let third = store
        .update_result_if_status(
            "t-2",
            &[TaskStatus::Running, TaskStatus::Pending],
            TaskStatus::Completed,
            Some(serde_json::json!({"ok": true})),
        )
        .expect("cond update 3");
    assert!(third);
    assert_eq!(store.get("t-2").expect("get").status, TaskStatus::Completed);
}

/// 生产换装链路：TaskManager::with_store(SqliteTaskStore) 全生命周期走
/// tasks.db，重开库（模拟 gateway 重启）后终态任务可查。
#[test]
fn task_manager_over_sqlite_persists_across_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tasks.db");

    {
        let db = TaskDb::open(&path).expect("open");
        let store = nemesis_taskstore::SqliteTaskStore::from_db(db);
        let tm = TaskManager::with_store(std::sync::Arc::new(store));
        // 不 start（不起清扫循环），纯状态机驱动。
        let t = tm.create_task_with_peer(
            "run tests",
            serde_json::json!({"x": 1}),
            "web",
            "chat-9",
            "node-b",
        );
        assert!(tm.assign_task(&t.id, "node-b"), "assign Pending→Running");
        assert!(tm.complete_task(&t.id, serde_json::json!("all green")));
        let done = tm.get_task(&t.id).expect("task present");
        assert_eq!(done.status, TaskStatus::Completed);
        assert!(done.completed_at.is_some());
    }

    // 模拟进程重启：重开 tasks.db，任务与结果原样可读。
    let db = TaskDb::open(&path).expect("reopen");
    let store = nemesis_taskstore::SqliteTaskStore::from_db(db);
    let tm = TaskManager::with_store(std::sync::Arc::new(store));
    let listed = tm.list_tasks();
    assert_eq!(listed.len(), 1, "task must survive restart");
    assert_eq!(listed[0].status, TaskStatus::Completed);
    assert_eq!(listed[0].result, Some(serde_json::json!("all green")));
    assert_eq!(listed[0].peer_id, "node-b");
}

/// 在途恢复：assign 到 Running 后进程死亡，重开库仍见 Running
/// （P1 验收门「在途任务状态可恢复」——恢复轮询向 B 端 query 的前提）。
#[test]
fn in_flight_running_state_survives_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("tasks.db");

    {
        let db = TaskDb::open(&path).expect("open");
        let store = nemesis_taskstore::SqliteTaskStore::from_db(db);
        let tm = TaskManager::with_store(std::sync::Arc::new(store));
        let t = tm.create_task("review pr", serde_json::json!({}), "web", "chat-2");
        assert!(tm.assign_task(&t.id, "node-b"));
        // 进程在此死亡：无 complete/fail。
    }

    let db = TaskDb::open(&path).expect("reopen");
    let store = nemesis_taskstore::SqliteTaskStore::from_db(db);
    let in_flight = store
        .list_by_status(TaskStatus::Running)
        .expect("list running");
    assert_eq!(
        in_flight.len(),
        1,
        "running task must be visible after restart"
    );
    assert_eq!(in_flight[0].action, "review pr");
}

/// 语义护栏：SqliteTaskStore 与 InMemoryTaskStore 对同一操作序列的
/// 可观测行为一致（P1 计划要求「语义逐条镜像」的回归锁）。
#[test]
fn sqlite_and_inmemory_behavioral_parity() {
    let (_dir, path) = temp_db();
    let db = TaskDb::open(&path).expect("open");
    let sqlite = nemesis_taskstore::SqliteTaskStore::from_db(db);
    let memory = InMemoryTaskStore::default();

    for (name, store) in [("sqlite", &sqlite as &dyn TaskStore), ("memory", &memory)] {
        let _ = name;
        let t = Task {
            id: "parity-1".into(),
            status: TaskStatus::Pending,
            action: "act".into(),
            peer_id: String::new(),
            payload: serde_json::json!(null),
            result: None,
            original_channel: String::new(),
            original_chat_id: String::new(),
            created_at: "2026-01-01T00:00:00+08:00".into(),
            completed_at: None,
        };
        store.create(t).expect("create");
        assert!(
            store
                .create(Task {
                    id: "parity-1".into(),
                    status: TaskStatus::Pending,
                    action: "act".into(),
                    peer_id: String::new(),
                    payload: serde_json::json!(null),
                    result: None,
                    original_channel: String::new(),
                    original_chat_id: String::new(),
                    created_at: "2026-01-01T00:00:00+08:00".into(),
                    completed_at: None,
                })
                .is_err(),
            "duplicate must Err on both stores"
        );

        // 缺失更新 Err、条件更新 miss=Ok(false)。
        assert!(
            store
                .update_result("ghost", TaskStatus::Failed, None)
                .is_err()
        );
        let missed = store
            .update_result_if_status("parity-1", &[TaskStatus::Running], TaskStatus::Failed, None)
            .expect("cond");
        assert!(!missed);

        // list 两面。
        assert_eq!(store.list_by_status(TaskStatus::Pending).len(), 1);
        assert_eq!(store.list_all().len(), 1);

        // delete 后消失。
        store.delete("parity-1").expect("delete");
        assert!(store.get("parity-1").is_err());
        assert!(store.list_all().is_empty());
    }
}

// ===========================================================================
// 4. `Cluster::with_workspace` 生产换装链路（P1 验收门「fail-closed」锚点）
// ===========================================================================

mod with_workspace_wiring {
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;
    use nemesis_types::cluster::{Task, TaskStatus};

    use super::sample_task;

    /// 正常路径：with_workspace 装 SqliteTaskStore，任务经 cluster API 落
    /// tasks.db；重开一个新 Cluster（等价重启）后任务原样可读。
    #[test]
    fn with_workspace_persists_tasks_across_cluster_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().to_path_buf();

        let task: Task = sample_task("ws-1", "echo");
        // create_task_with_peer 生成 uuid（不采信调用方 id），带出块外供断言。
        let task_id;
        {
            let cluster = Cluster::with_workspace(ClusterConfig::default(), ws.clone());
            let created = cluster.task_manager().create_task_with_peer(
                &task.action,
                task.payload.clone(),
                &task.original_channel,
                &task.original_chat_id,
                "node-b",
            );
            task_id = created.id.clone();
            assert!(
                cluster.task_manager().assign_task(&created.id, "node-b"),
                "assign must land"
            );
        }
        // 「重启」：新 Cluster 重开同一 workspace 的 tasks.db。
        let cluster2 = Cluster::with_workspace(ClusterConfig::default(), ws);
        let t = cluster2
            .task_manager()
            .get_task(&task_id)
            .expect("task back after reopen");
        assert_eq!(t.status, TaskStatus::Running, "in-flight state survives");
        assert_eq!(t.peer_id, "node-b");
    }

    /// 损坏路径：tasks.db 被覆写为垃圾 → with_workspace 挂 PoisonedTaskStore
    /// ——写面诚实失败（日志留痕），读面诚实降级为空；绝不静默退回内存
    /// 假装持久化（fail-closed 语义）。
    #[test]
    fn with_workspace_fail_closed_on_corrupt_tasks_db() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().to_path_buf();
        let tasks_db = nemesis_path::resolve_tasks_db_path_in_workspace(&ws);
        std::fs::create_dir_all(tasks_db.parent().unwrap()).expect("mkdir");
        std::fs::write(&tasks_db, vec![b'x'; 8192]).expect("corrupt");

        let cluster = Cluster::with_workspace(ClusterConfig::default(), ws);
        // PoisonedTaskStore：store.create 失败只 error 留痕（create_task 返回
        // 任务本体不 panic），任务实际没落库——get 读面诚实为空。
        let task: Task = sample_task("ws-dead", "echo");
        let created = cluster.task_manager().create_task_with_peer(
            &task.action,
            task.payload.clone(),
            &task.original_channel,
            &task.original_chat_id,
            "node-b",
        );
        assert!(cluster.task_manager().get_task(&created.id).is_none());
        // 写面（assign 走 update_result_if_status）诚实失败。
        assert!(!cluster.task_manager().assign_task(&created.id, "node-b"));
    }
}
