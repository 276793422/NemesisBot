//! `BoardStore` CRUD / 编号 / 状态机 / 审计测试。

use super::*;
use crate::assignment::{Actor, AssignmentType};
use crate::models::{
    CommentType, IssueFilter, IssuePatch, IssueStatus, NewComment, NewIssue, priority,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_store(name: &str) -> (BoardStore, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-storetest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    (store, dir)
}

fn admin() -> Actor {
    Actor::admin("admin")
}

fn new_issue(title: &str) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        ..NewIssue::default()
    }
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// 编号 + 创建
// ---------------------------------------------------------------------------

#[test]
fn test_create_assigns_sequential_numbers() {
    let (store, dir) = temp_store("numbering");
    let a = store.create_issue(new_issue("任务一")).unwrap();
    let b = store.create_issue(new_issue("任务二")).unwrap();
    assert_eq!(a.number, "NB-1");
    assert_eq!(b.number, "NB-2");
    assert_eq!(a.status, IssueStatus::Backlog);
    assert_eq!(a.creator, admin());
    // 位置默认按创建序，先建在前。
    assert!(a.position < b.position);
    cleanup(&dir);
}

#[test]
fn test_create_rejects_empty_title() {
    let (store, dir) = temp_store("empty-title");
    assert!(store.create_issue(new_issue("   ")).is_err());
    cleanup(&dir);
}

#[test]
fn test_create_with_assignee_requires_assignee_id() {
    let (store, dir) = temp_store("assignee-id-required");
    let mut ni = new_issue("缺 id");
    ni.assignee = Some(AssignmentType::Worker);
    ni.assignee_id = None;
    assert!(store.create_issue(ni).is_err());
    cleanup(&dir);
}

#[test]
fn test_create_subscribes_creator_and_assignee() {
    let (store, dir) = temp_store("create-subscribers");
    let mut ni = new_issue("带指派");
    ni.assignee = Some(AssignmentType::ManagerSelf);
    ni.assignee_id = Some("node-a".into());
    let issue = store.create_issue(ni).unwrap();
    let subs = store.list_subscribers(issue.id).unwrap();
    assert!(
        subs.iter()
            .any(|s| s.subscriber == admin() && s.reason == "creator")
    );
    assert!(subs
        .iter()
        .any(|s| s.subscriber == Actor::new("manager_self", "node-a")
            && s.reason == "assignee"));
    // 创建活动 + 指派活动。
    let acts = store.list_activity(issue.id).unwrap();
    assert!(acts.iter().any(|a| a.action == "created"));
    assert!(acts.iter().any(|a| a.action == "assigned"));
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 读取 / 过滤
// ---------------------------------------------------------------------------

#[test]
fn test_get_by_id_and_number_and_missing() {
    let (store, dir) = temp_store("get");
    let a = store.create_issue(new_issue("查询")).unwrap();
    assert_eq!(store.get_issue(a.id).unwrap().title, "查询");
    assert_eq!(store.get_issue_by_number("NB-1").unwrap().id, a.id);
    assert!(store.get_issue(9999).is_err());
    assert!(store.get_issue_by_number("NB-999").is_err());
    cleanup(&dir);
}

#[test]
fn test_list_filters() {
    let (store, dir) = temp_store("filters");
    let proj = store.create_project("P", "", None, "").unwrap();

    let mut assigned = new_issue("被指派的");
    assigned.assignee = Some(AssignmentType::Worker);
    assigned.assignee_id = Some("w1".into());
    assigned.priority = priority::URGENT;
    assigned.project_id = Some(proj.id);
    let a = store.create_issue(assigned).unwrap();

    let mut other = new_issue("搜索关键词长颈鹿");
    other.project_id = Some(proj.id);
    let b = store.create_issue(other).unwrap();
    let c = store.create_issue(new_issue("无项目")).unwrap();

    // status 过滤。
    let all = store.list_issues(&IssueFilter::default()).unwrap();
    assert_eq!(all.len(), 3);
    let backlog = store
        .list_issues(&IssueFilter {
            status: Some(IssueStatus::Backlog),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(backlog.len(), 3);

    // assignee 过滤。
    let by_assignee = store
        .list_issues(&IssueFilter {
            assignee: Some((AssignmentType::Worker, "w1".into())),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_assignee.len(), 1);
    assert_eq!(by_assignee[0].id, a.id);

    // project 过滤。
    let by_proj = store
        .list_issues(&IssueFilter {
            project_id: Some(proj.id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_proj.len(), 2);

    // priority 过滤。
    let by_pri = store
        .list_issues(&IssueFilter {
            priority: Some(priority::URGENT),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_pri.len(), 1);

    // query 子串：标题 + 编号（大小写不敏感走 LIKE，%q%）。
    let by_title = store
        .list_issues(&IssueFilter {
            query: Some("长颈鹿".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].id, b.id);
    let by_number = store
        .list_issues(&IssueFilter {
            query: Some("nb-3".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(by_number.len(), 1);
    assert_eq!(by_number[0].id, c.id);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 更新 patch
// ---------------------------------------------------------------------------

#[test]
fn test_update_issue_records_changed_fields() {
    let (store, dir) = temp_store("update");
    let a = store.create_issue(new_issue("原题")).unwrap();

    // 无字段变化 → 不写 updated 活动。
    let patch = IssuePatch {
        title: Some("原题".into()),
        ..Default::default()
    };
    store.update_issue(a.id, &patch, &admin()).unwrap();
    let acts0 = store.list_activity(a.id).unwrap();
    assert!(!acts0.iter().any(|x| x.action == "updated"));

    let patch = IssuePatch {
        title: Some("新题".into()),
        priority: Some(priority::HIGH),
        ..Default::default()
    };
    let updated = store.update_issue(a.id, &patch, &admin()).unwrap();
    assert_eq!(updated.title, "新题");
    assert_eq!(updated.priority, priority::HIGH);
    let acts = store.list_activity(a.id).unwrap();
    let upd = acts
        .iter()
        .find(|x| x.action == "updated")
        .expect("updated activity missing");
    let details = upd.details.as_deref().unwrap();
    assert!(details.contains("title"), "{details}");
    assert!(details.contains("priority"), "{details}");
    cleanup(&dir);
}

#[test]
fn test_update_missing_issue_errors() {
    let (store, dir) = temp_store("update-missing");
    assert!(
        store
            .update_issue(123, &IssuePatch::default(), &admin())
            .is_err()
    );
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 状态机转移
// ---------------------------------------------------------------------------

#[test]
fn test_transition_legal_and_audited() {
    let (store, dir) = temp_store("transition");
    let a = store.create_issue(new_issue("流转")).unwrap();
    let moved = store
        .transition_issue(a.id, IssueStatus::InProgress, &admin())
        .unwrap();
    assert_eq!(moved.status, IssueStatus::InProgress);

    // status_change 评论 + activity。
    let comments = store.list_comments(a.id).unwrap();
    let sc = comments
        .iter()
        .find(|c| c.ctype == CommentType::StatusChange)
        .expect("status_change comment missing");
    assert!(sc.content.contains("backlog"));
    assert!(sc.content.contains("in_progress"));
    let acts = store.list_activity(a.id).unwrap();
    assert!(acts.iter().any(|x| x.action == "status_changed"));
    cleanup(&dir);
}

#[test]
fn test_transition_illegal_rejected_and_state_unchanged() {
    let (store, dir) = temp_store("transition-illegal");
    let a = store.create_issue(new_issue("跳级")).unwrap();
    // backlog → in_review 非法。
    let err = store
        .transition_issue(a.id, IssueStatus::InReview, &admin())
        .unwrap_err();
    assert!(err.contains("非法状态转移"), "{err}");
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Backlog);
    cleanup(&dir);
}

#[test]
fn test_terminal_state_cannot_leave() {
    let (store, dir) = temp_store("terminal");
    let a = store.create_issue(new_issue("终态")).unwrap();
    store
        .transition_issue(a.id, IssueStatus::Done, &admin())
        .unwrap();
    let err = store
        .transition_issue(a.id, IssueStatus::InProgress, &admin())
        .unwrap_err();
    assert!(err.contains("非法状态转移"), "{err}");
    assert_eq!(store.get_issue(a.id).unwrap().status, IssueStatus::Done);
    cleanup(&dir);
}

#[test]
fn test_self_transition_rejected() {
    let (store, dir) = temp_store("self-transition");
    let a = store.create_issue(new_issue("原地")).unwrap();
    let err = store
        .transition_issue(a.id, IssueStatus::Backlog, &admin())
        .unwrap_err();
    assert!(err.contains("已处于"), "{err}");
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 指派
// ---------------------------------------------------------------------------

#[test]
fn test_assign_and_unassign() {
    let (store, dir) = temp_store("assign");
    let a = store.create_issue(new_issue("派活")).unwrap();
    assert!(a.assignee.is_none());

    let b = store
        .assign_issue(
            a.id,
            Some(AssignmentType::Worker),
            Some("node-b".into()),
            &admin(),
        )
        .unwrap();
    assert_eq!(b.assignee, Some(AssignmentType::Worker));
    assert_eq!(b.assignee_id.as_deref(), Some("node-b"));
    assert!(
        store
            .list_subscribers(a.id)
            .unwrap()
            .iter()
            .any(|s| s.subscriber == Actor::new("worker", "node-b"))
    );

    // 清空：两侧都必须是 None。
    let c = store.assign_issue(a.id, None, None, &admin()).unwrap();
    assert!(c.assignee.is_none());
    cleanup(&dir);
}

#[test]
fn test_assign_validation_errors() {
    let (store, dir) = temp_store("assign-invalid");
    let a = store.create_issue(new_issue("校验")).unwrap();
    // 有 type 无 id。
    assert!(
        store
            .assign_issue(a.id, Some(AssignmentType::Worker), None, &admin())
            .is_err()
    );
    // 空串 id。
    assert!(
        store
            .assign_issue(
                a.id,
                Some(AssignmentType::Worker),
                Some("  ".into()),
                &admin()
            )
            .is_err()
    );
    // 只给 id 不给 type。
    assert!(
        store
            .assign_issue(a.id, None, Some("w".into()), &admin())
            .is_err()
    );
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 评论
// ---------------------------------------------------------------------------

#[test]
fn test_comments_thread_and_subscribe() {
    let (store, dir) = temp_store("comments");
    let a = store.create_issue(new_issue("讨论")).unwrap();
    let c1 = store
        .add_comment(NewComment {
            issue_id: a.id,
            author: Actor::admin("alice"),
            content: "第一层".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let c2 = store
        .add_comment(NewComment {
            issue_id: a.id,
            author: Actor::agent("mgr"),
            content: "回复".into(),
            parent_id: Some(c1.id),
            ctype: CommentType::Comment,
        })
        .unwrap();

    let list = store.list_comments(a.id).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, c1.id);
    assert_eq!(list[1].id, c2.id);
    assert_eq!(list[1].parent_id, Some(c1.id));
    assert_eq!(list[1].ctype, CommentType::Comment);

    // 作者自动订阅。
    let subs = store.list_subscribers(a.id).unwrap();
    assert!(subs.iter().any(|s| s.subscriber == Actor::admin("alice")));
    assert!(subs.iter().any(|s| s.subscriber == Actor::agent("mgr")));

    // 空内容 / 不存在 issue 拒绝。
    assert!(
        store
            .add_comment(NewComment {
                issue_id: a.id,
                author: admin(),
                content: "  ".into(),
                parent_id: None,
                ctype: CommentType::Comment,
            })
            .is_err()
    );
    assert!(
        store
            .add_comment(NewComment {
                issue_id: 9999,
                author: admin(),
                content: "x".into(),
                parent_id: None,
                ctype: CommentType::Comment,
            })
            .is_err()
    );
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 订阅
// ---------------------------------------------------------------------------

#[test]
fn test_subscribe_idempotent_unsubscribe_silent() {
    let (store, dir) = temp_store("subscribe");
    let a = store.create_issue(new_issue("订阅")).unwrap();
    let who = Actor::admin("bob");
    store.subscribe(a.id, &who, "manual").unwrap();
    store.subscribe(a.id, &who, "manual-again").unwrap(); // 幂等覆盖
    let subs = store.list_subscribers(a.id).unwrap();
    assert_eq!(subs.iter().filter(|s| s.subscriber == who).count(), 1);
    // 创建者（admin/admin）也在订阅列表且按 subscriber_id 排序在前，
    // 按 who 精确取行断言 reason 覆盖（不能索引 [0]）。
    let bob_row = subs
        .iter()
        .find(|s| s.subscriber == who)
        .expect("bob subscribed");
    assert_eq!(bob_row.reason, "manual-again");

    store.unsubscribe(a.id, &who).unwrap();
    store.unsubscribe(a.id, &who).unwrap(); // 重复退订静默成功
    // 退订只移除 bob 自己；create_issue 自动加的创建者订阅不受影响。
    let subs = store.list_subscribers(a.id).unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].subscriber, admin());
    assert_eq!(subs[0].reason, "creator");
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 项目 / 附件 / 统计
// ---------------------------------------------------------------------------

#[test]
fn test_projects_crud() {
    let (store, dir) = temp_store("projects");
    assert!(store.list_projects().unwrap().is_empty());
    let p = store
        .create_project("主项目", "描述", Some(&admin()), "🚀")
        .unwrap();
    assert_eq!(p.name, "主项目");
    assert_eq!(p.lead, Some(admin()));
    // 重名拒绝。
    assert!(store.create_project("主项目", "", None, "").is_err());
    // 空名拒绝。
    assert!(store.create_project("  ", "", None, "").is_err());
    assert_eq!(store.get_project(p.id).unwrap().icon, "🚀");
    assert_eq!(store.list_projects().unwrap().len(), 1);
    assert!(store.get_project(999).is_err());
    cleanup(&dir);
}

#[test]
fn test_attachments() {
    let (store, dir) = temp_store("attachments");
    let a = store.create_issue(new_issue("带附件")).unwrap();
    let att = store
        .add_attachment(a.id, "log.txt", "/tmp/logs/log.txt", 42)
        .unwrap();
    assert_eq!(att.filename, "log.txt");
    let list = store.list_attachments(a.id).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].storage_path, "/tmp/logs/log.txt");
    cleanup(&dir);
}

#[test]
fn test_count_by_status() {
    let (store, dir) = temp_store("count");
    let a = store.create_issue(new_issue("一")).unwrap();
    store.create_issue(new_issue("二")).unwrap();
    store
        .transition_issue(a.id, IssueStatus::Done, &admin())
        .unwrap();
    let counts = store.count_by_status().unwrap();
    assert!(counts.contains(&(IssueStatus::Backlog, 1)));
    assert!(counts.contains(&(IssueStatus::Done, 1)));
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 持久化（重开同库数据还在）
// ---------------------------------------------------------------------------

#[test]
fn test_reopen_preserves_data() {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-storetest-{}-reopen-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("board.db");
    let a = {
        let store = BoardStore::open(&db, "NB").unwrap();
        let a = store.create_issue(new_issue("跨会话")).unwrap();
        store
            .transition_issue(a.id, IssueStatus::InProgress, &admin())
            .unwrap();
        a
    };
    // 重开：编号 counter 延续（下一个是 NB-2），状态保留。
    let store2 = BoardStore::open(&db, "NB").unwrap();
    assert_eq!(
        store2.get_issue(a.id).unwrap().status,
        IssueStatus::InProgress
    );
    let b = store2.create_issue(new_issue("续")).unwrap();
    assert_eq!(b.number, "NB-2");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- 派发（W2 P2：issue_dispatch 表 CRUD + 幂等终结）--

use crate::models::dispatch_state;

#[test]
fn test_dispatch_crud_lifecycle() {
    let (store, dir) = temp_store("dispatch-crud");
    let issue = store.create_issue(new_issue("派发链路")).unwrap();

    // 未派发时无 active。
    assert!(!store.has_active_dispatch(issue.id).unwrap());
    assert!(store.get_dispatch("no-such-task").unwrap().is_none());
    assert!(store.list_dispatches(issue.id).unwrap().is_empty());

    // 登记 → active + 记录可查 + 派发活动写入。
    store
        .insert_dispatch("task-1", issue.id, "node-b", &admin())
        .unwrap();
    assert!(store.has_active_dispatch(issue.id).unwrap());
    let rec = store.get_dispatch("task-1").unwrap().expect("record");
    assert_eq!(rec.issue_id, issue.id);
    assert_eq!(rec.worker_id, "node-b");
    assert_eq!(rec.state, dispatch_state::DISPATCHED);
    assert!(rec.completed_at.is_none());
    let acts = store.list_activity(issue.id).unwrap();
    assert!(
        acts.iter()
            .any(|a| a.action == "dispatched"
                && a.details.as_deref().unwrap_or("").contains("task-1"))
    );

    // 历史列表。
    store
        .insert_dispatch("task-2", issue.id, "node-c", &admin())
        .unwrap();
    let history = store.list_dispatches(issue.id).unwrap();
    assert_eq!(history.len(), 2);

    // 终结 task-1（done）→ 幂等语义：首次 true，重复 false。
    assert!(
        store
            .finish_dispatch("task-1", dispatch_state::DONE)
            .unwrap()
    );
    assert!(
        !store
            .finish_dispatch("task-1", dispatch_state::DONE)
            .unwrap()
    );
    let rec = store.get_dispatch("task-1").unwrap().unwrap();
    assert_eq!(rec.state, dispatch_state::DONE);
    assert!(rec.completed_at.is_some());

    // 终结 task-2（failed）→ 无 active（task-1 已终态不计）。
    assert!(
        store
            .finish_dispatch("task-2", dispatch_state::FAILED)
            .unwrap()
    );
    assert!(!store.has_active_dispatch(issue.id).unwrap());

    // 非法终态拒绝。
    assert!(store.finish_dispatch("task-1", "cancelled").is_err());

    // 重复 task_id 拒绝（一 task 挂一 issue）。
    assert!(
        store
            .insert_dispatch("task-1", issue.id, "node-d", &admin())
            .is_err()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dispatch_scoped_to_issue() {
    let (store, dir) = temp_store("dispatch-scope");
    let a = store.create_issue(new_issue("甲")).unwrap();
    let b = store.create_issue(new_issue("乙")).unwrap();
    store
        .insert_dispatch("t-a", a.id, "node-b", &admin())
        .unwrap();
    // b 无 active；a 的记录不串到 b。
    assert!(!store.has_active_dispatch(b.id).unwrap());
    assert!(store.list_dispatches(b.id).unwrap().is_empty());
    assert_eq!(store.list_dispatches(a.id).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// -- 看板拖拽（W2 P3：issue.move 原子「状态 + 排序」）--

#[test]
fn test_move_issue_cross_column_reorder_and_illegal() {
    let (store, dir) = temp_store("move-issue");
    let a = store.create_issue(new_issue("拖拽")).unwrap();

    // 跨列：backlog → in_progress + position 一次完成；status_change 评论 + 活动。
    let moved = store
        .move_issue(a.id, IssueStatus::InProgress, 42, &admin())
        .unwrap();
    assert_eq!(moved.status, IssueStatus::InProgress);
    assert_eq!(moved.position, 42);
    let sc = store
        .list_comments(a.id)
        .unwrap()
        .iter()
        .filter(|c| c.ctype == CommentType::StatusChange)
        .count();
    assert_eq!(sc, 1, "cross-column move writes exactly one status_change");
    let acts = store.list_activity(a.id).unwrap();
    let st = acts
        .iter()
        .find(|x| x.action == "status_changed")
        .expect("status_changed activity");
    assert!(st.details.as_deref().unwrap().contains("in_progress"));

    // 同列重排：status 不变 → reordered 活动，不再写 status_change 评论。
    let reordered = store
        .move_issue(a.id, IssueStatus::InProgress, 7, &admin())
        .unwrap();
    assert_eq!(reordered.status, IssueStatus::InProgress);
    assert_eq!(reordered.position, 7);
    assert!(
        store
            .list_activity(a.id)
            .unwrap()
            .iter()
            .any(|x| x.action == "reordered")
    );
    let sc2 = store
        .list_comments(a.id)
        .unwrap()
        .iter()
        .filter(|c| c.ctype == CommentType::StatusChange)
        .count();
    assert_eq!(sc2, 1, "reorder must not add another status_change");

    // 非法转移：in_progress → backlog 拒绝，状态保持。
    let err = store
        .move_issue(a.id, IssueStatus::Backlog, 1, &admin())
        .unwrap_err();
    assert!(err.contains("非法状态转移"), "{err}");
    assert_eq!(
        store.get_issue(a.id).unwrap().status,
        IssueStatus::InProgress
    );

    // 不存在的 issue。
    assert!(
        store
            .move_issue(999, IssueStatus::Todo, 1, &admin())
            .is_err()
    );
    cleanup(&dir);
}

// -- 通知 / 收件箱（W2 P3）--

#[test]
fn test_notification_assigned_and_status_changed_to_assignee() {
    let (store, dir) = temp_store("notify-assign");
    let a = store.create_issue(new_issue("通知指派")).unwrap();
    let worker = Actor::new("worker", "node-b");

    // 指派 → 被指派人收到 assigned。
    store
        .assign_issue(
            a.id,
            Some(AssignmentType::Worker),
            Some("node-b".into()),
            &admin(),
        )
        .unwrap();
    let inbox = store
        .list_notifications("worker", Some("node-b"), false, 100)
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].kind, "assigned");
    assert_eq!(inbox[0].issue_id, Some(a.id));
    assert!(!inbox[0].read);

    // 重复指派同一人（值未变）→ 不重复通知。
    store
        .assign_issue(
            a.id,
            Some(AssignmentType::Worker),
            Some("node-b".into()),
            &admin(),
        )
        .unwrap();
    assert_eq!(
        store
            .list_notifications("worker", Some("node-b"), false, 100)
            .unwrap()
            .len(),
        1
    );

    // 操作者（admin）移动状态 → 指派对象收到 status_changed。
    store
        .move_issue(a.id, IssueStatus::InProgress, 3, &admin())
        .unwrap();
    let inbox = store
        .list_notifications("worker", Some("node-b"), false, 100)
        .unwrap();
    assert_eq!(inbox.len(), 2);
    assert!(inbox.iter().any(|n| n.kind == "status_changed"));

    // 指派对象自己转移 → 不给自己通知。
    store
        .transition_issue(a.id, IssueStatus::InReview, &worker)
        .unwrap();
    assert_eq!(
        store
            .list_notifications("worker", Some("node-b"), false, 100)
            .unwrap()
            .len(),
        2
    );

    // 清空指派 → 无新通知（收件人没了）。
    store.assign_issue(a.id, None, None, &admin()).unwrap();
    assert_eq!(
        store
            .list_notifications("worker", Some("node-b"), false, 100)
            .unwrap()
            .len(),
        2
    );
    cleanup(&dir);
}

#[test]
fn test_notification_comment_and_mention_precedence() {
    let (store, dir) = temp_store("notify-comment");
    let alice = Actor::admin("alice");
    let mut ni = new_issue("评论通知");
    ni.creator = alice.clone();
    let a = store.create_issue(ni).unwrap();
    let worker = Actor::new("worker", "node-b");
    store
        .assign_issue(
            a.id,
            Some(AssignmentType::Worker),
            Some("node-b".into()),
            &alice,
        )
        .unwrap();

    // alice 评论并 @node-b → node-b 只收 mentioned（优先于 commented，不重复）。
    store
        .add_comment(NewComment {
            issue_id: a.id,
            author: alice.clone(),
            content: "请看 @node-b 这里".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let inbox = store
        .list_notifications("worker", Some("node-b"), false, 100)
        .unwrap();
    assert_eq!(inbox.len(), 2, "assigned + mentioned");
    assert_eq!(inbox[0].kind, "mentioned"); // created_at 降序 → 最新在前
    assert_eq!(inbox[0].content, "请看 @node-b 这里");

    // 无 @ 的普通评论 → node-b 收 commented；作者 alice 不收自己。
    store
        .add_comment(NewComment {
            issue_id: a.id,
            author: alice.clone(),
            content: "普通更新".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let inbox = store
        .list_notifications("worker", Some("node-b"), false, 100)
        .unwrap();
    assert_eq!(inbox.len(), 3);
    assert_eq!(inbox[0].kind, "commented");
    // alice 自己评论：作者被排除，订阅（creator）不产生通知。
    assert!(
        store
            .list_notifications("admin", Some("alice"), false, 100)
            .unwrap()
            .is_empty()
    );

    // worker 评论 @alice → alice（订阅者）收 mentioned。
    store
        .add_comment(NewComment {
            issue_id: a.id,
            author: worker.clone(),
            content: "已完成 @alice".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let alice_inbox = store
        .list_notifications("admin", Some("alice"), false, 100)
        .unwrap();
    assert_eq!(alice_inbox.len(), 1);
    assert_eq!(alice_inbox[0].kind, "mentioned");

    // status_change/system 评论不产生通知。
    store
        .add_comment(NewComment {
            issue_id: a.id,
            author: alice.clone(),
            content: "x → y".into(),
            parent_id: None,
            ctype: CommentType::StatusChange,
        })
        .unwrap();
    assert_eq!(
        store
            .list_notifications("worker", Some("node-b"), false, 100)
            .unwrap()
            .len(),
        3
    );

    // 未命中候选的 @token 静默忽略。
    store
        .add_comment(NewComment {
            issue_id: a.id,
            author: alice.clone(),
            content: "@nobody-in-board 未知提及".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    assert_eq!(
        store
            .list_notifications("worker", Some("node-b"), false, 100)
            .unwrap()
            .len(),
        4 // 只多了一条 commented
    );
    cleanup(&dir);
}

#[test]
fn test_notification_inbox_read_flow_and_admin_wildcard() {
    let (store, dir) = temp_store("notify-inbox");
    let a = store.create_issue(new_issue("收件箱")).unwrap();
    // 两位 admin 收件人（admin wildcard：recipient_id=None 全可见）。
    for (kind, id) in [("admin", "alice"), ("admin", "bob")] {
        store
            .notify(NewNotification {
                recipient: Actor::new(kind, id),
                kind: "commented".into(),
                title: "NB-1 收件箱".to_string(),
                content: "hello".into(),
                issue_id: Some(a.id),
            })
            .unwrap();
    }
    // admin 收件箱（不指定 id）→ 两位的都可见；worker 类型不串。
    assert_eq!(
        store
            .list_notifications("admin", None, false, 100)
            .unwrap()
            .len(),
        2
    );
    assert!(
        store
            .list_notifications("worker", None, false, 100)
            .unwrap()
            .is_empty()
    );

    // unread_only 过滤 + 未读数。
    assert_eq!(
        store
            .list_notifications("admin", None, true, 100)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(store.unread_notification_count("admin", None).unwrap(), 2);

    // 单条已读（幂等）。
    let first = &store.list_notifications("admin", None, false, 100).unwrap()[0];
    assert!(store.mark_notification_read(first.id).unwrap());
    assert!(
        !store.mark_notification_read(first.id).unwrap(),
        "重复标记幂等"
    );
    assert_eq!(store.unread_notification_count("admin", None).unwrap(), 1);
    assert_eq!(
        store
            .list_notifications("admin", None, true, 100)
            .unwrap()
            .len(),
        1
    );

    // 全部已读（返回条数；再跑一次 = 0）。
    assert_eq!(store.mark_all_notifications_read("admin", None).unwrap(), 1);
    assert_eq!(store.mark_all_notifications_read("admin", None).unwrap(), 0);
    assert_eq!(store.unread_notification_count("admin", None).unwrap(), 0);

    // limit 生效（created_at 降序截断）。
    assert_eq!(
        store
            .list_notifications("admin", None, false, 1)
            .unwrap()
            .len(),
        1
    );
    cleanup(&dir);
}

// -- 项目更新 / 附件读取（W2 P3）--

#[test]
fn test_update_project_patch() {
    let (store, dir) = temp_store("project-patch");
    let p = store.create_project("原项目", "说明", None, "🚀").unwrap();

    // 部分更新：status 归档 + 改 icon；其余字段不动。
    let updated = store
        .update_project(
            p.id,
            &ProjectPatch {
                status: Some("archived".into()),
                icon: Some("📦".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(updated.status, "archived");
    assert_eq!(updated.icon, "📦");
    assert_eq!(updated.name, "原项目");
    assert_eq!(updated.description, "说明");

    // 改名 + 空名拒绝 + 不存在报错。
    let renamed = store
        .update_project(
            p.id,
            &ProjectPatch {
                name: Some("新项目".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(renamed.name, "新项目");
    assert!(
        store
            .update_project(
                p.id,
                &ProjectPatch {
                    name: Some("  ".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(store.update_project(999, &ProjectPatch::default()).is_err());

    // 改名撞 UNIQUE。
    store.create_project("另一个", "", None, "").unwrap();
    assert!(
        store
            .update_project(
                p.id,
                &ProjectPatch {
                    name: Some("另一个".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    cleanup(&dir);
}

#[test]
fn test_get_attachment_by_id() {
    let (store, dir) = temp_store("attachment-get");
    let a = store.create_issue(new_issue("附件")).unwrap();
    let att = store
        .add_attachment(a.id, "log.txt", "board/files/x", 42)
        .unwrap();
    let got = store.get_attachment(att.id).unwrap();
    assert_eq!(got.filename, "log.txt");
    assert_eq!(got.storage_path, "board/files/x");
    assert!(store.get_attachment(9999).is_err());
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// P4：派发取消 / 超时兜底（cancel_dispatch / fail_dispatch / 在途列表）
// ---------------------------------------------------------------------------

#[test]
fn test_cancel_dispatch_wins_race_writes_activity() {
    let (store, dir) = temp_store("cancel-dispatch");
    let issue = store.create_issue(new_issue("要取消的")).unwrap();
    store
        .insert_dispatch("task-c1", issue.id, "node-b", &admin())
        .unwrap();

    // 取消 → Some(record)（赢得竞态）+ state=cancelled + completed_at + 活动。
    let rec = store
        .cancel_dispatch("task-c1", &admin())
        .unwrap()
        .expect("won race");
    assert_eq!(rec.state, dispatch_state::CANCELLED);
    assert!(rec.completed_at.is_some());
    let acts = store.list_activity(issue.id).unwrap();
    assert!(acts.iter().any(|a| a.action == "dispatch_cancelled"
        && a.details.as_deref().unwrap_or("").contains("task-c1")));

    // 已取消再取消 → None（幂等跳过，不重复写活动）。
    assert!(
        store
            .cancel_dispatch("task-c1", &admin())
            .unwrap()
            .is_none()
    );
    let acts = store.list_activity(issue.id).unwrap();
    assert_eq!(
        acts.iter()
            .filter(|a| a.action == "dispatch_cancelled")
            .count(),
        1
    );
    // 无活跃派发了。
    assert!(!store.has_active_dispatch(issue.id).unwrap());
    assert!(store.get_active_dispatch(issue.id).unwrap().is_none());
    cleanup(&dir);
}

#[test]
fn test_cancel_dispatch_loses_race_to_callback() {
    let (store, dir) = temp_store("cancel-vs-callback");
    let issue = store.create_issue(new_issue("回调先到")).unwrap();
    store
        .insert_dispatch("task-c2", issue.id, "node-b", &admin())
        .unwrap();
    // 写回回调先终结（done）→ cancel 竞态输 → None + state 保持 done。
    assert!(
        store
            .finish_dispatch("task-c2", dispatch_state::DONE)
            .unwrap()
    );
    assert!(
        store
            .cancel_dispatch("task-c2", &admin())
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.get_dispatch("task-c2").unwrap().unwrap().state,
        dispatch_state::DONE
    );
    // 取消不存在/已终态的 task → None 不报错。
    assert!(
        store
            .cancel_dispatch("no-such", &admin())
            .unwrap()
            .is_none()
    );
    cleanup(&dir);
}

#[test]
fn test_fail_dispatch_timeout_race_and_activity() {
    let (store, dir) = temp_store("fail-dispatch");
    let issue = store.create_issue(new_issue("要超时的")).unwrap();
    store
        .insert_dispatch("task-f1", issue.id, "node-b", &admin())
        .unwrap();

    // sweep 兜底 → Some + failed + dispatch_timeout 活动（details 进活动）。
    let rec = store
        .fail_dispatch("task-f1", "timeout after 3600s")
        .unwrap()
        .expect("won race");
    assert_eq!(rec.state, dispatch_state::FAILED);
    assert!(rec.completed_at.is_some());
    let acts = store.list_activity(issue.id).unwrap();
    assert!(acts.iter().any(|a| {
        a.action == "dispatch_timeout"
            && a.details
                .as_deref()
                .unwrap_or("")
                .contains("timeout after 3600s")
    }));

    // 已 failed 再 fail → None；回调先到同理。
    assert!(store.fail_dispatch("task-f1", "again").unwrap().is_none());
    store
        .insert_dispatch("task-f2", issue.id, "node-c", &admin())
        .unwrap();
    assert!(
        store
            .finish_dispatch("task-f2", dispatch_state::DONE)
            .unwrap()
    );
    assert!(
        store
            .fail_dispatch("task-f2", "late sweep")
            .unwrap()
            .is_none()
    );
    cleanup(&dir);
}

#[test]
fn test_list_active_dispatches_across_issues() {
    let (store, dir) = temp_store("active-list");
    let a = store.create_issue(new_issue("甲")).unwrap();
    let b = store.create_issue(new_issue("乙")).unwrap();
    store
        .insert_dispatch("t-1", a.id, "node-b", &admin())
        .unwrap();
    store
        .insert_dispatch("t-2", b.id, "node-c", &admin())
        .unwrap();

    // 全部在途：跨 issue 2 条。
    let active = store.list_active_dispatches().unwrap();
    assert_eq!(active.len(), 2);

    // 单 issue 取最新活跃（多条取最新一条）。
    store
        .insert_dispatch("t-3", a.id, "node-d", &admin())
        .unwrap();
    let got = store.get_active_dispatch(a.id).unwrap().expect("active");
    assert_eq!(got.task_id, "t-3");

    // 终结其余两条 → 在途列表只剩 t-3。
    assert!(store.finish_dispatch("t-1", dispatch_state::DONE).unwrap());
    store
        .cancel_dispatch("t-2", &admin())
        .unwrap()
        .expect("won");
    let active = store.list_active_dispatches().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].task_id, "t-3");
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// P4：autopilot 规则 CRUD + run 簿记
// ---------------------------------------------------------------------------

fn new_ap(name: &str) -> NewAutopilot {
    NewAutopilot {
        name: name.to_string(),
        cron: "0 9 * * *".to_string(),
        title: "每日站会纪要 {date}".to_string(),
        description: "自动生成".to_string(),
        priority: priority::MEDIUM,
        project_id: None,
        target: String::new(),
        enabled: true,
    }
}

#[test]
fn test_autopilot_crud_and_validation() {
    let (store, dir) = temp_store("autopilot-crud");

    // 创建 → 字段落库 + 默认 cron_job_id=None。
    let ap = store.create_autopilot(&new_ap("日报")).unwrap();
    assert_eq!(ap.name, "日报");
    assert_eq!(ap.cron, "0 9 * * *");
    assert!(ap.enabled);
    assert_eq!(ap.cron_job_id, None);
    assert_eq!(ap.last_run_at, None);

    // 空校验：name/title/cron。
    let mut bad = new_ap("x");
    bad.name = "  ".into();
    assert!(store.create_autopilot(&bad).is_err());
    bad = new_ap("x");
    bad.title = String::new();
    assert!(store.create_autopilot(&bad).is_err());
    bad = new_ap("x");
    bad.cron = String::new();
    assert!(store.create_autopilot(&bad).is_err());

    // 列表 + patch 更新（含禁用 + target）。
    store.create_autopilot(&new_ap("周报")).unwrap();
    assert_eq!(store.list_autopilots().unwrap().len(), 2);
    let updated = store
        .update_autopilot(
            ap.id,
            &AutopilotPatch {
                target: Some("node-b".into()),
                enabled: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(updated.target, "node-b");
    assert!(!updated.enabled);
    assert_eq!(updated.cron, "0 9 * * *"); // 未 patch 的字段不动
    assert!(
        store
            .update_autopilot(
                ap.id,
                &AutopilotPatch {
                    name: Some(" ".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert!(
        store
            .update_autopilot(999, &AutopilotPatch::default())
            .is_err()
    );

    // 删除（幂等）。
    assert!(store.remove_autopilot(ap.id).unwrap());
    assert!(!store.remove_autopilot(ap.id).unwrap());
    assert!(store.get_autopilot(ap.id).is_err());
    cleanup(&dir);
}

#[test]
fn test_autopilot_cron_bookkeeping_and_run_history() {
    let (store, dir) = temp_store("autopilot-run");
    let ap = store.create_autopilot(&new_ap("触发器")).unwrap();

    // 回存 job id → 清除 → 不存在的 id 报错。
    store
        .set_autopilot_cron_job(ap.id, Some("cron-abc"))
        .unwrap();
    assert_eq!(
        store.get_autopilot(ap.id).unwrap().cron_job_id.as_deref(),
        Some("cron-abc")
    );
    store.set_autopilot_cron_job(ap.id, None).unwrap();
    assert_eq!(store.get_autopilot(ap.id).unwrap().cron_job_id, None);
    assert!(store.set_autopilot_cron_job(999, Some("x")).is_err());

    // 触发建 issue（origin=autopilot/{id}）→ mark_run + run 历史按 origin 反查。
    let mut ni = new_issue("日报 2026-08-31");
    ni.origin = Some(crate::models::TaskOrigin {
        origin_type: "autopilot".into(),
        origin_id: ap.id.to_string(),
    });
    let r1 = store.create_issue(ni).unwrap();
    store.mark_autopilot_run(ap.id).unwrap();
    assert!(store.get_autopilot(ap.id).unwrap().last_run_at.is_some());

    // 别的 issue（无 origin / 别的规则）不混进历史。
    store.create_issue(new_issue("无关")).unwrap();
    let history = store
        .list_issues_by_origin("autopilot", &ap.id.to_string(), 10)
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, r1.id);

    // limit 生效。
    assert_eq!(
        store
            .list_issues_by_origin("autopilot", &ap.id.to_string(), 0)
            .unwrap()
            .len(),
        0
    );
    cleanup(&dir);
}

#[test]
fn test_notify_dispatch_event_recipients_dedup() {
    let (store, dir) = temp_store("dispatch-notify");

    // issue：创建者 admin/admin，指派 worker/node-b（创建时两者均已自动订阅）。
    let mut ni = new_issue("会失败的任务");
    ni.assignee = Some(AssignmentType::Worker);
    ni.assignee_id = Some("node-b".into());
    let issue = store.create_issue(ni).unwrap();

    // 额外订阅者（创建者 ∪ 指派 ∪ 订阅者 去重后应多出这一个）。
    store
        .subscribe(issue.id, &Actor::admin("watcher"), "watch")
        .unwrap();

    store
        .notify_dispatch_event(
            issue.id,
            crate::models::notification_kind::DISPATCH_FAILED,
            "超时未回报（3600s）",
        )
        .unwrap();

    // admin wildcard（创建者 + watcher）各一条，worker 一条；kind/issue 归属正确。
    let admins = store.list_notifications("admin", None, false, 100).unwrap();
    assert_eq!(admins.len(), 2, "creator+watcher, deduped: {admins:?}");
    assert!(admins.iter().all(
        |n| n.kind == crate::models::notification_kind::DISPATCH_FAILED
            && n.issue_id == Some(issue.id)
    ));
    let workers = store
        .list_notifications("worker", Some("node-b"), false, 100)
        .unwrap();
    assert_eq!(workers.len(), 1);

    // 不存在的 issue 报错。
    assert!(
        store
            .notify_dispatch_event(999, crate::models::notification_kind::DISPATCH_FAILED, "x")
            .is_err()
    );

    cleanup(&dir);
}
