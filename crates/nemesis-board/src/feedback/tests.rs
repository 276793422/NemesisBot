//! 终判 👍/👎 测试：三闸（人类/图元/收口）+ 子树归因 + 幂等键 + 回执。

use super::*;
use crate::assignment::Actor;
use crate::models::{CommentType, IssueStatus, NewComment, NewIssue};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// 唯一临时目录（crate 无 tempfile 依赖，与 store/tests.rs 同款）。
fn temp_root(name: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-fbtest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn temp_store(name: &str) -> BoardStore {
    let dir = temp_root(name);
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

fn admin() -> Actor {
    Actor::admin("tester")
}

fn make_issue(store: &BoardStore, title: &str, parent: Option<i64>) -> crate::models::Issue {
    store
        .create_issue(NewIssue {
            title: title.to_string(),
            description: "说明".to_string(),
            parent_issue_id: parent,
            creator: admin(),
            ..Default::default()
        })
        .unwrap()
}

/// 按状态机链路推进到 Done（todo→in_progress→in_review→done）。
fn mark_done(store: &BoardStore, id: i64) {
    let actor = Actor::system("test");
    store
        .transition_issue(id, IssueStatus::Todo, &actor)
        .unwrap();
    store
        .transition_issue(id, IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(id, IssueStatus::InReview, &actor)
        .unwrap();
    store
        .transition_issue(id, IssueStatus::Done, &actor)
        .unwrap();
}

/// 完成一次派发（insert + finish DONE）。
fn complete_dispatch(store: &BoardStore, task_id: &str, issue_id: i64, worker: &str) {
    store
        .insert_dispatch(task_id, issue_id, worker, &Actor::system("test"))
        .unwrap();
    assert!(
        store
            .finish_dispatch(task_id, dispatch_state::DONE)
            .unwrap()
    );
}

/// 人类发一条终判评论并过闸处理。
fn human_verdict(
    store: &BoardStore,
    issue_id: i64,
    content: &str,
) -> Result<Option<VerdictOutcome>, String> {
    let c = store
        .add_comment(NewComment {
            issue_id,
            author: admin(),
            content: content.to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    process_final_verdict_comment(store, issue_id, c.id, &c.author.kind, &c.content)
}

#[test]
fn non_human_author_is_skipped() {
    let store = temp_store("agent_skip");
    let issue = make_issue(&store, "agent 评论不触发", None);
    mark_done(&store, issue.id);
    let c = store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::agent("node-a"),
            content: "👍 自评".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let outcome =
        process_final_verdict_comment(&store, issue.id, c.id, &c.author.kind, &c.content).unwrap();
    assert_eq!(outcome, None);
}

#[test]
fn plain_and_ambiguous_comments_are_skipped() {
    let store = temp_store("plain");
    let issue = make_issue(&store, "普通评论", None);
    mark_done(&store, issue.id);
    // 普通文本（无图元）。
    assert_eq!(human_verdict(&store, issue.id, "辛苦了").unwrap(), None);
    // 歧义双图元（1/1）不猜。
    assert_eq!(
        human_verdict(&store, issue.id, "👍👎 说不好").unwrap(),
        None
    );
}

#[test]
fn non_done_issue_is_skipped() {
    let store = temp_store("not_done");
    let issue = make_issue(&store, "还在进行中", None);
    // 保持 todo 态。
    assert_eq!(human_verdict(&store, issue.id, "👍").unwrap(), None);
}

#[test]
fn thumbs_up_records_last_dispatch_and_acks() {
    let store = temp_store("up");
    let issue = make_issue(&store, "认可单", None);
    mark_done(&store, issue.id);
    complete_dispatch(&store, "task-up-1", issue.id, "node-a");
    complete_dispatch(&store, "task-up-2", issue.id, "node-b");
    let outcome = human_verdict(&store, issue.id, "👍 干得漂亮").unwrap();
    let o = outcome.expect("应命中终判");
    assert!(o.verdict);
    assert_eq!(o.recorded, 1);
    assert_eq!(o.skipped, 0);

    // 回执：System 评论挂在终判评论下。
    let comments = store.list_comments(issue.id).unwrap();
    let ack = comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::System)
        .expect("应有系统回执");
    assert!(
        ack.content.contains("已入 worker 画像"),
        "回执文案：{}",
        ack.content
    );
}

#[test]
fn thumbs_down_records_negative_verdict() {
    let store = temp_store("down");
    let issue = make_issue(&store, "否决单", None);
    mark_done(&store, issue.id);
    complete_dispatch(&store, "task-dn-1", issue.id, "node-a");
    let outcome = human_verdict(&store, issue.id, "👎 不合格，返工").unwrap();
    let o = outcome.expect("应命中终判");
    assert!(!o.verdict);
    assert_eq!(o.recorded, 1);
}

#[test]
fn subtree_done_expansion_with_missing_dispatch_skips() {
    let store = temp_store("tree");
    let parent = make_issue(&store, "父单", None);
    let child = make_issue(&store, "子单", Some(parent.id));
    mark_done(&store, parent.id);
    mark_done(&store, child.id);
    // 只有子单有完成派发；父单无派发记录 → skipped。
    complete_dispatch(&store, "task-tree-1", child.id, "node-b");
    let outcome = human_verdict(&store, parent.id, "👍").unwrap();
    let o = outcome.expect("应命中终判");
    assert_eq!(o.recorded, 1);
    assert_eq!(o.skipped, 1);
    // 回执聚合两条数字。
    let comments = store.list_comments(parent.id).unwrap();
    let ack = comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::System)
        .expect("应有系统回执");
    assert!(ack.content.contains('1'), "聚合数字：{}", ack.content);
}

#[test]
fn no_dispatch_anywhere_zero_recorded_ack_differs() {
    let store = temp_store("zero");
    let issue = make_issue(&store, "无派发单", None);
    mark_done(&store, issue.id);
    let outcome = human_verdict(&store, issue.id, "👍").unwrap();
    let o = outcome.expect("应命中终判");
    assert_eq!(o.recorded, 0);
    assert_eq!(o.skipped, 1);
    let comments = store.list_comments(issue.id).unwrap();
    let ack = comments
        .iter()
        .rev()
        .find(|c| c.ctype == CommentType::System)
        .expect("应有系统回执");
    assert!(
        ack.content.contains("不入 worker 画像"),
        "零记账回执文案：{}",
        ack.content
    );
}

#[test]
fn same_comment_rerun_is_idempotent() {
    let store = temp_store("idem");
    let issue = make_issue(&store, "幂等单", None);
    mark_done(&store, issue.id);
    complete_dispatch(&store, "task-idem-1", issue.id, "node-a");
    let c = store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: admin(),
            content: "👍".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let first =
        process_final_verdict_comment(&store, issue.id, c.id, &c.author.kind, &c.content).unwrap();
    assert_eq!(first.map(|o| o.recorded), Some(1));
    // 同一条评论再过一遍闸（模拟重复投递）：记账幂等跳过。
    let second =
        process_final_verdict_comment(&store, issue.id, c.id, &c.author.kind, &c.content).unwrap();
    let o2 = second.expect("应命中终判");
    assert_eq!(o2.recorded, 0);
    assert_eq!(o2.skipped, 1);
}
