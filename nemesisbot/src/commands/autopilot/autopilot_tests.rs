//! autopilot CLI 测试（W2 P4 看板定时自动化离线臂）。
//!
//! 全部 `run()` 分发臂走 `NEMESISBOT_HOME` 临时根 + 共享 SQLite
//! （`{workspace}/board/board.db`），无端口无网络：`Run` 子命令经
//! `fire_autopilot`（cluster=None / CLI 语义）直接建单。env set_var 是
//! 进程级 → 持 `crate::GLOBAL_STATE_LOCK` 串行 + Windows-form 惯例
//! （CI Linux nightly 跳过，同 2026-09-02 sweep）。

use super::*;

/// RAII 守卫：NEMESISBOT_HOME → 临时根（drop 撤销）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
struct TempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
impl TempHomeEnv {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        Self { _tmp: tmp, home }
    }
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

/// 重新打开 run() 刚写过的共享库，断言持久化副作用。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn reopen_store(home: &std::path::Path) -> std::sync::Arc<BoardStore> {
    let db = crate::common::workspace_path(home)
        .join("board")
        .join("board.db");
    std::sync::Arc::new(
        BoardStore::open(&db, "NB")
            .map_err(anyhow::Error::msg)
            .unwrap(),
    )
}

/// 经 store 直接预置一条规则（绕开 CLI，隔离被测臂）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn seed_rule(store: &BoardStore, name: &str, target: &str, enabled: bool) -> i64 {
    store
        .create_autopilot(&NewAutopilot {
            name: name.into(),
            cron: "0 9 * * *".into(),
            title: "daily {date}".into(),
            description: String::new(),
            priority: 1,
            project_id: None,
            target: target.into(),
            enabled,
            auto_plan: false,
            acceptance_criteria: None,
        })
        .map_err(anyhow::Error::msg)
        .unwrap()
        .id
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_list_empty_then_create_then_list_nonempty() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    // 空库 → 早退臂（autopilot.rs:126-128）。
    run(AutopilotAction::List, false).unwrap();
    // 合法 cron 建 1 条（Create 臂全路径）。
    run(
        AutopilotAction::Create {
            name: "daily".into(),
            cron: "0 9 * * *".into(),
            title: "report {date}".into(),
            description: "daily report".into(),
            priority: 2,
            project_id: None,
            target: String::new(),
            acceptance_criteria: "has summary".into(),
            disabled: false,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let rules = store.list_autopilots().map_err(anyhow::Error::msg).unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].name, "daily");
    assert_eq!(rules[0].cron, "0 9 * * *");
    assert!(rules[0].enabled, "未传 --disabled 默认启用");
    assert_eq!(rules[0].acceptance_criteria.as_deref(), Some("has summary"));
    drop(store);
    // 非空列表臂。
    run(AutopilotAction::List, false).unwrap();
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_create_rejects_invalid_cron() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let err = run(
        AutopilotAction::Create {
            name: "broken".into(),
            cron: "definitely not a cron".into(),
            title: "t".into(),
            description: String::new(),
            priority: 1,
            project_id: None,
            target: String::new(),
            acceptance_criteria: String::new(),
            disabled: false,
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("cron 表达式无效"), "{err}");
    let store = reopen_store(&th.home);
    assert!(
        store
            .list_autopilots()
            .map_err(anyhow::Error::msg)
            .unwrap()
            .is_empty(),
        "校验失败不得落库"
    );
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_update_patches_fields_and_rejects_invalid_cron() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "u", "", true);
    drop(store);
    // 合法 patch：title + priority + target。
    run(
        AutopilotAction::Update {
            id,
            name: None,
            cron: None,
            title: Some("patched {date}".into()),
            description: None,
            priority: Some(3),
            project_id: None,
            target: Some("node-b".into()),
            acceptance_criteria: None,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let ap = store.get_autopilot(id).map_err(anyhow::Error::msg).unwrap();
    assert_eq!(ap.title, "patched {date}");
    assert_eq!(ap.priority, 3);
    assert_eq!(ap.target, "node-b");
    drop(store);
    // Update 带非法 cron → Err（autopilot.rs:180-183）。
    let err = run(
        AutopilotAction::Update {
            id,
            name: None,
            cron: Some("nope".into()),
            title: None,
            description: None,
            priority: None,
            project_id: None,
            target: None,
            acceptance_criteria: None,
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("cron 表达式无效"), "{err}");
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_enable_disable_remove_and_remove_missing() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "toggle", "", false);
    drop(store);
    run(AutopilotAction::Enable { id }, false).unwrap();
    let store = reopen_store(&th.home);
    assert!(
        store
            .get_autopilot(id)
            .map_err(anyhow::Error::msg)
            .unwrap()
            .enabled
    );
    drop(store);
    run(AutopilotAction::Disable { id }, false).unwrap();
    let store = reopen_store(&th.home);
    assert!(
        !store
            .get_autopilot(id)
            .map_err(anyhow::Error::msg)
            .unwrap()
            .enabled
    );
    drop(store);
    // Remove 存在 → 删除；再删 → 不存在分支同样 Ok（autopilot.rs:234-239）。
    run(AutopilotAction::Remove { id }, false).unwrap();
    run(AutopilotAction::Remove { id }, false).unwrap();
    let store = reopen_store(&th.home);
    assert!(store.get_autopilot(id).is_err(), "删除后应不可再取");
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_fire_creates_issue_and_runs_lists_history() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    // 空库 Runs → 「从未运行」早退臂（autopilot.rs:271-273）。
    let id = seed_rule(&store, "fired", "", true);
    run(AutopilotAction::Runs { id }, false).unwrap();
    drop(store);
    // Run：target 空 → 仅建单（CLI 无集群连接的诚实路径）。
    run(AutopilotAction::Run { id }, false).unwrap();
    let store = reopen_store(&th.home);
    let history = store
        .list_issues_by_origin("autopilot", &id.to_string(), 20)
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert_eq!(history.len(), 1, "触发一次应按 origin 落 1 条历史");
    assert!(
        history[0].title.contains("daily"),
        "标题模板应渲染：{}",
        history[0].title
    );
    drop(store);
    // Runs 非空臂。
    run(AutopilotAction::Runs { id }, false).unwrap();
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_fire_targeted_rule_rejected_without_cluster() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "targeted", "node-b", true);
    drop(store);
    // 配了派发目标 + CLI 进程无集群连接 → 明确拒绝（Err），不建单。
    let err = run(AutopilotAction::Run { id }, false).unwrap_err();
    assert!(!err.to_string().is_empty());
    let store = reopen_store(&th.home);
    let history = store
        .list_issues_by_origin("autopilot", &id.to_string(), 20)
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert!(history.is_empty(), "拒绝路径不得留下半成品单");
}
