//! autopilot CLI 测试（看板定时自动化离线臂）。
//!
//! 全部 `run()` 分发臂走 `NEMESISBOT_HOME` 临时根 + 共享 SQLite
//! （`{workspace}/board/board.db`），无端口无网络：`Run` 子命令经
//! `fire_autopilot`（cluster=None / CLI 语义）直接建单。env set_var 是
//! 进程级 → 持 `crate::GLOBAL_STATE_LOCK` 串行 + Windows-form 惯例
//! （CI Linux nightly 跳过，同 2026-09-02 sweep）。
// 整文件 Windows 形态（全部分发臂走 env 重定向 + Windows 惯例）——文件级
// 门控，Linux 下 `use super:*` 不产生未用导入告警。
#![cfg(windows)]

use super::*;

/// RAII 守卫：NEMESISBOT_HOME → 临时根（drop 撤销）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, sweep)
struct TempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, sweep)
impl TempHomeEnv {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        Self { _tmp: tmp, home }
    }
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, sweep)
impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

/// 重新打开 run() 刚写过的共享库，断言持久化副作用。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, sweep)
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
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, sweep)
fn seed_rule(store: &BoardStore, name: &str, target: &str, enabled: bool, mode: &str) -> i64 {
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
            session_key: None,
            mode: mode.into(),
        })
        .map_err(anyhow::Error::msg)
        .unwrap()
        .id
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
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
            session_key: String::new(),
            live: false,
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
    // D3 拍板：未传 --live 默认影子模式。
    assert_eq!(rules[0].mode, AUTOPILOT_MODE_SHADOW);
    drop(store);
    // 非空列表臂。
    run(AutopilotAction::List, false).unwrap();
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
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
            session_key: String::new(),
            live: false,
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

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
#[test]
fn w_run_update_patches_fields_and_rejects_invalid_cron() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "u", "", true, AUTOPILOT_MODE_LIVE);
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
            session_key: None,
            live: false,
            shadow: false,
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
            session_key: None,
            live: false,
            shadow: false,
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("cron 表达式无效"), "{err}");
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
#[test]
fn w_run_enable_disable_remove_and_remove_missing() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "toggle", "", false, AUTOPILOT_MODE_LIVE);
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

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
#[test]
fn w_run_fire_creates_issue_and_runs_lists_history() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    // 空库 Runs → 「从未运行」早退臂（autopilot.rs:271-273）。
    let id = seed_rule(&store, "fired", "", true, AUTOPILOT_MODE_LIVE);
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

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
#[test]
fn w_run_fire_targeted_rule_rejected_without_cluster() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "targeted", "node-b", true, AUTOPILOT_MODE_LIVE);
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

/// B34（D3 拍板）CLI 全链：默认影子 fire → 建单留痕零派发 → promote 转
/// 全真 → 二次 fire 与现行 live 全同。本地留痕文件（Run 臂新装配的
/// `<workspace>/logs/autopilot_shadow/`）一并断言。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
#[test]
fn w_run_shadow_fire_then_promote_then_live_fire() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    let store = reopen_store(&th.home);
    let id = seed_rule(&store, "shady", "", true, AUTOPILOT_MODE_SHADOW);
    drop(store);
    // 影子 fire：建单照常（Run 臂 Ok），历史落 1 条。
    run(AutopilotAction::Run { id }, false).unwrap();
    let store = reopen_store(&th.home);
    let ap = store.get_autopilot(id).map_err(anyhow::Error::msg).unwrap();
    assert!(
        ap.last_run_at.is_some(),
        "影子 fire 照常写 last_run_at（触发历史真实）"
    );
    let shadow_version = ap.updated_at;
    let history = store
        .list_issues_by_origin("autopilot", &id.to_string(), 20)
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert_eq!(history.len(), 1, "影子 fire 建单照常");
    // 决策意图评论（D2：看板必须有内容）。
    let comments = store
        .list_comments(history[0].id)
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("影子模式") && c.content.contains("决策意图")),
        "影子单应带决策意图系统评论：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 本地留痕文件（D2 补充：logs/autopilot_shadow/ 不可删持久层）。
    // 路径以进程实际生效的 handoff 根为准（OnceLock 全局首装为准：全量
    // 跑时 gateway 场景测试并发抢先装配自己的临时工作区，本测试 Run 臂
    // 的 install 静默 no-op——真实 CLI 单 fire 进程无此态；写入点
    // write_autopilot_shadow_trace 读同一全局，文件必落生效根下）。
    let trace_root = nemesis_web::handlers::board::handoff_workspace_root()
        .cloned()
        .unwrap_or_else(|| crate::common::workspace_path(&th.home));
    let trace = trace_root
        .join("logs")
        .join("autopilot_shadow")
        .join(format!("rule_{id}.jsonl"));
    let trace_body = std::fs::read_to_string(&trace)
        .unwrap_or_else(|e| panic!("影子留痕文件应存在 {}: {e}", trace.display()));
    assert!(trace_body.contains("rule_version"), "{trace_body}");
    assert!(trace_body.contains("\"intent\""), "{trace_body}");
    // 血统：单子 origin_meta 带 rule_version + mode。
    let origin = history[0].origin.as_ref().expect("autopilot 单应有 origin");
    assert_eq!(
        origin.origin_meta.get("mode").map(String::as_str),
        Some("shadow")
    );
    assert!(
        origin
            .origin_meta
            .get("rule_version")
            .is_some_and(|v| v.starts_with(&format!("{id}@"))),
        "血统版本戳应为 id@updated_at 形态"
    );
    drop(store);
    // promote：--live 切全真（模式 patch 推进授权版本 → updated_at 变）。
    // 先跨过秒边界：updated_at 是秒级时间戳（Self::now()），毫秒间隔的
    // create→promote bump 前后可能同值——碰撞属秒辨率设计接受的粒度界
    // （origin_meta 的 mode 字段本就区分形态），测试这里只需让版本差可观测。
    std::thread::sleep(std::time::Duration::from_millis(1100));
    run(
        AutopilotAction::Update {
            id,
            name: None,
            cron: None,
            title: None,
            description: None,
            priority: None,
            project_id: None,
            target: None,
            acceptance_criteria: None,
            session_key: None,
            live: true,
            shadow: false,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let ap = store.get_autopilot(id).map_err(anyhow::Error::msg).unwrap();
    assert_eq!(ap.mode, AUTOPILOT_MODE_LIVE);
    assert_ne!(
        ap.updated_at, shadow_version,
        "promote 是推进授权版本的内容 patch（血统版本戳应变）"
    );
    drop(store);
    // live fire：与现行 live 全同（仅建单 CLI 语义），历史累计 2 条。
    run(AutopilotAction::Run { id }, false).unwrap();
    let store = reopen_store(&th.home);
    let history = store
        .list_issues_by_origin("autopilot", &id.to_string(), 20)
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert_eq!(history.len(), 2, "promote 后二次 fire 再落 1 条");
    let comments = store
        .list_comments(history[0].id)
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert!(
        !comments.iter().any(|c| c.content.contains("影子模式")),
        "live fire 不得再落影子评论"
    );
    // 留痕文件只追加不改写（影子历史保留；trace_root 见上——进程生效根）。
    let trace_body = std::fs::read_to_string(
        trace_root
            .join("logs")
            .join("autopilot_shadow")
            .join(format!("rule_{id}.jsonl")),
    )
    .unwrap();
    assert_eq!(
        trace_body.lines().count(),
        1,
        "live fire 不写影子留痕，文件仍 1 行"
    );
}
