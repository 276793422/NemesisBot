//! issue CLI 测试（W2 P1 看板命令离线臂）。
//!
//! 覆盖面：`parse_assignee` / `resolve_issue_id` 纯逻辑（跨平台，不碰 env）
//! + `run()` 全部分发臂（经 `NEMESISBOT_HOME` 临时根 + 共享 SQLite
//!   `{workspace}/board/board.db`，无端口无网络）。
//!   env set_var 是进程级 → run() 级测试持 `crate::GLOBAL_STATE_LOCK` 串行，
//!   并按 2026-09-02 sweep 惯例标 Windows-form（CI Linux nightly 跳过）。

use super::*;

// ---------------------------------------------------------------------------
// parse_assignee（issue.rs:75-89）
// ---------------------------------------------------------------------------

#[test]
fn parse_assignee_worker_type_id_ok() {
    let (at, id) = parse_assignee("worker:node-b").unwrap();
    assert_eq!(at, AssignmentType::Worker);
    assert_eq!(id, "node-b");
}

#[test]
fn parse_assignee_manager_self_typed_ok() {
    let (at, id) = parse_assignee("manager_self:n1").unwrap();
    assert_eq!(at, AssignmentType::ManagerSelf);
    assert_eq!(id, "n1");
}

#[test]
fn parse_assignee_unknown_type_errs() {
    let err = parse_assignee("bot:x").unwrap_err();
    assert!(err.to_string().contains("未知 assignee 类型"), "{err}");
}

#[test]
fn parse_assignee_empty_id_errs() {
    let err = parse_assignee("worker:   ").unwrap_err();
    assert!(err.to_string().contains("id 不能为空"), "{err}");
}

#[test]
fn parse_assignee_bare_manager_self_maps_local() {
    let (at, id) = parse_assignee("manager_self").unwrap();
    assert_eq!(at, AssignmentType::ManagerSelf);
    assert_eq!(id, "local");
}

#[test]
fn parse_assignee_missing_colon_errs() {
    let err = parse_assignee("worker-node").unwrap_err();
    assert!(err.to_string().contains("格式应为 type:id"), "{err}");
}

// ---------------------------------------------------------------------------
// resolve_issue_id（issue.rs:103-112）——直接开临时库，不碰 env
// ---------------------------------------------------------------------------

fn open_temp_store(dir: &tempfile::TempDir) -> BoardStore {
    let db = dir.path().join("board").join("board.db");
    BoardStore::open(&db, "NB")
        .map_err(anyhow::Error::msg)
        .unwrap()
}

#[test]
fn resolve_issue_id_accepts_numeric_id_directly() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_temp_store(&dir);
    let issue = store
        .create_issue(NewIssue {
            title: "t".into(),
            ..NewIssue::default()
        })
        .map_err(anyhow::Error::msg)
        .unwrap();
    // 数字串不走查表——即使库里没有也原样通过（边界：不存在的数字 id）。
    assert_eq!(resolve_issue_id(&store, "1").unwrap(), issue.id);
    assert_eq!(resolve_issue_id(&store, " 2 ").unwrap(), 2);
}

#[test]
fn resolve_issue_id_resolves_nb_number_via_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_temp_store(&dir);
    let issue = store
        .create_issue(NewIssue {
            title: "t".into(),
            ..NewIssue::default()
        })
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert_eq!(resolve_issue_id(&store, &issue.number).unwrap(), issue.id);
}

#[test]
fn resolve_issue_id_unknown_number_errs() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_temp_store(&dir);
    let err = resolve_issue_id(&store, "NB-999").unwrap_err();
    assert!(err.to_string().contains("找不到 issue"), "{err}");
}

// ---------------------------------------------------------------------------
// run() 分发臂（env 级，Windows-form + GLOBAL_STATE_LOCK）
// ---------------------------------------------------------------------------

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

/// 重新打开 run() 刚写过的共享库（WAL 多连接，断言持久化副作用）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn reopen_store(home: &std::path::Path) -> BoardStore {
    let db = crate::common::workspace_path(home)
        .join("board")
        .join("board.db");
    BoardStore::open(&db, "NB")
        .map_err(anyhow::Error::msg)
        .unwrap()
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_create_without_assignee_persists_backlog_issue() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    run(
        IssueAction::Create {
            title: "first bug".into(),
            description: "desc".into(),
            priority: 2,
            assignee: None,
            project_id: None,
            accept: Some("all green".into()),
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let all = store
        .list_issues(&IssueFilter::default())
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title, "first bug");
    assert_eq!(all[0].priority, 2);
    assert_eq!(all[0].status, IssueStatus::Backlog);
    assert_eq!(all[0].assignee, None);
    assert_eq!(all[0].acceptance_criteria.as_deref(), Some("all green"));
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_create_with_worker_assignee_persists_assignee() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    run(
        IssueAction::Create {
            title: "assigned".into(),
            description: String::new(),
            priority: 1,
            assignee: Some("worker:node-b".into()),
            project_id: None,
            accept: None,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let all = store
        .list_issues(&IssueFilter::default())
        .map_err(anyhow::Error::msg)
        .unwrap();
    assert_eq!(all[0].assignee, Some(AssignmentType::Worker));
    assert_eq!(all[0].assignee_id.as_deref(), Some("node-b"));
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_list_filters_status_query_and_rejects_unknown_status() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _th = TempHomeEnv::new();
    for t in ["alpha task", "beta thing"] {
        run(
            IssueAction::Create {
                title: t.into(),
                description: String::new(),
                priority: 1,
                assignee: None,
                project_id: None,
                accept: None,
            },
            false,
        )
        .unwrap();
    }
    // 空库情形由其他测试覆盖；这里全量 2 条。
    run(
        IssueAction::List {
            status: None,
            assignee: None,
            query: None,
            project_id: None,
        },
        false,
    )
    .unwrap();
    // query 子串过滤命中 1 条。
    run(
        IssueAction::List {
            status: None,
            assignee: None,
            query: Some("beta".into()),
            project_id: None,
        },
        false,
    )
    .unwrap();
    // status 过滤合法值。
    run(
        IssueAction::List {
            status: Some("backlog".into()),
            assignee: None,
            query: None,
            project_id: None,
        },
        false,
    )
    .unwrap();
    // 未知 status → Err（issue.rs:171 的 ok_or_else 臂）。
    let err = run(
        IssueAction::List {
            status: Some("weird".into()),
            assignee: None,
            query: None,
            project_id: None,
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("未知 status"), "{err}");
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_get_by_number_and_unknown_id_errs() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    run(
        IssueAction::Create {
            title: "gettable".into(),
            description: "has desc".into(),
            priority: 1,
            assignee: None,
            project_id: None,
            accept: None,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    store
        .add_comment(NewComment {
            issue_id: 1,
            author: Actor::admin("tester"),
            content: "a comment".into(),
            parent_id: None,
            ctype: nemesis_board::models::CommentType::Comment,
        })
        .map_err(anyhow::Error::msg)
        .unwrap();
    // 人读编号路径（NB-1）+ 评论/活动列表渲染。
    run(
        IssueAction::Get {
            issue: "NB-1".into(),
        },
        false,
    )
    .unwrap();
    // 数字 id 路径。
    run(IssueAction::Get { issue: "1".into() }, false).unwrap();
    // 不存在 → Err。
    let err = run(
        IssueAction::Get {
            issue: "NB-9".into(),
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("找不到 issue"), "{err}");
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_assign_set_clear_and_missing_assignee_errs() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    run(
        IssueAction::Create {
            title: "assignable".into(),
            description: String::new(),
            priority: 1,
            assignee: None,
            project_id: None,
            accept: None,
        },
        false,
    )
    .unwrap();
    // 指派 worker。
    run(
        IssueAction::Assign {
            issue: "NB-1".into(),
            assignee: Some("worker:node-b".into()),
            clear: false,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let issue = store.get_issue(1).map_err(anyhow::Error::msg).unwrap();
    assert_eq!(issue.assignee, Some(AssignmentType::Worker));
    drop(store);
    // --clear 解除指派。
    run(
        IssueAction::Assign {
            issue: "1".into(),
            assignee: None,
            clear: true,
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let issue = store.get_issue(1).map_err(anyhow::Error::msg).unwrap();
    assert_eq!(issue.assignee, None);
    drop(store);
    // 既无 --assignee 又无 --clear → Err（issue.rs:222）。
    let err = run(
        IssueAction::Assign {
            issue: "1".into(),
            assignee: None,
            clear: false,
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("--assignee"), "{err}");
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_status_transition_valid_invalid_name_and_illegal_move() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    for _ in 0..2 {
        run(
            IssueAction::Create {
                title: "s".into(),
                description: String::new(),
                priority: 1,
                assignee: None,
                project_id: None,
                accept: None,
            },
            false,
        )
        .unwrap();
    }
    // 合法转移 backlog→todo。
    run(
        IssueAction::Status {
            issue: "NB-1".into(),
            status: "todo".into(),
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    assert_eq!(
        store
            .get_issue(1)
            .map_err(anyhow::Error::msg)
            .unwrap()
            .status,
        IssueStatus::Todo
    );
    drop(store);
    // 未知 status 名 → Err（issue.rs:233）。
    let err = run(
        IssueAction::Status {
            issue: "1".into(),
            status: "weird".into(),
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("未知 status"), "{err}");
    // 状态机非法转移 backlog→in_review → Err（map_err(err) 臂）。
    let err = run(
        IssueAction::Status {
            issue: "2".into(),
            status: "in_review".into(),
        },
        false,
    )
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_reopen_cancelled_issue_back_to_backlog() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    run(
        IssueAction::Create {
            title: "doomed".into(),
            description: String::new(),
            priority: 1,
            assignee: None,
            project_id: None,
            accept: None,
        },
        false,
    )
    .unwrap();
    run(
        IssueAction::Status {
            issue: "1".into(),
            status: "cancelled".into(),
        },
        false,
    )
    .unwrap();
    run(
        IssueAction::Reopen {
            issue: "NB-1".into(),
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    assert_eq!(
        store
            .get_issue(1)
            .map_err(anyhow::Error::msg)
            .unwrap()
            .status,
        IssueStatus::Backlog
    );
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn w_run_comment_projects_stats_all_ok() {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let th = TempHomeEnv::new();
    run(
        IssueAction::Create {
            title: "commented".into(),
            description: String::new(),
            priority: 1,
            assignee: None,
            project_id: None,
            accept: None,
        },
        false,
    )
    .unwrap();
    run(
        IssueAction::Comment {
            issue: "NB-1".into(),
            content: "looks fine".into(),
        },
        false,
    )
    .unwrap();
    let store = reopen_store(&th.home);
    let comments = store.list_comments(1).map_err(anyhow::Error::msg).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].content, "looks fine");
    drop(store);
    run(IssueAction::Projects, false).unwrap();
    run(IssueAction::Stats, false).unwrap();
}
