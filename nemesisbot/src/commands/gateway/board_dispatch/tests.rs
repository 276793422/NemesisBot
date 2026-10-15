//! board_dispatch 档案停摆哨兵单测（BUG-2026-10-15 C）。
//!
//! 场景：阈值内不警示 / 超阈值警示一次且幂等 / 新在途评论重置警示周期 /
//! 非 in_progress 单据不警示。

use super::{ARCHIVE_STALL_WARN_SECS, sweep_archive_stalls};
use nemesis_board::assignment::Actor;
use nemesis_board::models::{CommentType, IssueStatus, NewComment, NewIssue};

fn fixture(name: &str) -> nemesis_board::BoardStore {
    let dir = std::env::temp_dir().join(format!("nmb-stall-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap()
}

fn in_progress_issue(store: &nemesis_board::BoardStore, title: &str) -> i64 {
    let issue = store
        .create_issue(NewIssue {
            title: title.to_string(),
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    issue.id
}

fn add_inflight(store: &nemesis_board::BoardStore, issue_id: i64) {
    store
        .add_comment(NewComment {
            issue_id,
            author: Actor::system("board"),
            content: "📦 交付已收，变更集在途——执行档案落地后自动合并并进入验收评审。".into(),
            parent_id: None,
            ctype: CommentType::System,
        })
        .unwrap();
}

/// 阈值内不警示；超阈值（测试注入 0）警示一次，且幂等不重复。
#[test]
fn stall_alerts_once_then_idempotent() {
    let store = fixture("idem");
    let id = in_progress_issue(&store, "stalled");
    add_inflight(&store, id);

    let now = chrono::Utc::now().timestamp();
    assert_eq!(
        sweep_archive_stalls(&store, ARCHIVE_STALL_WARN_SECS, now),
        0,
        "阈值内不警示"
    );
    assert_eq!(sweep_archive_stalls(&store, 0, now), 1, "超阈值警示一次");
    assert_eq!(
        sweep_archive_stalls(&store, 0, now),
        0,
        "警示已留痕不得重复"
    );

    let stall = store
        .list_comments(id)
        .unwrap()
        .into_iter()
        .filter(|c| c.content.contains("⚠ 执行档案迟迟未落地"))
        .count();
    assert_eq!(stall, 1);
}

/// 重派形态：警示后的新在途评论开启新周期，可再次警示。
#[test]
fn new_inflight_comment_resets_episode() {
    let store = fixture("reset");
    let id = in_progress_issue(&store, "redispatched");
    let now = chrono::Utc::now().timestamp();
    add_inflight(&store, id);
    assert_eq!(sweep_archive_stalls(&store, 0, now), 1);

    // 重派 → 新在途评论（插入序在旧警示之后）→ 新周期再警示一次。
    add_inflight(&store, id);
    assert_eq!(sweep_archive_stalls(&store, 0, now), 1);

    let stall = store
        .list_comments(id)
        .unwrap()
        .into_iter()
        .filter(|c| c.content.contains("⚠ 执行档案迟迟未落地"))
        .count();
    assert_eq!(stall, 2);
}

/// 非 in_progress 单据（带在途评论）不警示。
#[test]
fn non_in_progress_issues_not_flagged() {
    let store = fixture("nop");
    let issue = store
        .create_issue(NewIssue {
            title: "still-todo".into(),
            ..Default::default()
        })
        .unwrap();
    add_inflight(&store, issue.id);
    let now = chrono::Utc::now().timestamp();
    assert_eq!(sweep_archive_stalls(&store, 0, now), 0);
}
