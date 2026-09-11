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
    let proj = store.create_project("P", "", None, "", "").unwrap();

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
        .create_project("主项目", "描述", Some(&admin()), "🚀", "")
        .unwrap();
    assert_eq!(p.name, "主项目");
    assert_eq!(p.lead, Some(admin()));
    // 重名拒绝。
    assert!(store.create_project("主项目", "", None, "", "").is_err());
    // 空名拒绝。
    assert!(store.create_project("  ", "", None, "", "").is_err());
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
    let p = store
        .create_project("原项目", "说明", None, "🚀", "")
        .unwrap();

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
    store.create_project("另一个", "", None, "", "").unwrap();
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
        auto_plan: false,
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

// ---------------------------------------------------------------------------
// Swarm M1：planner 拆解支撑（required 列 + 依赖表 + 子单列表）
// ---------------------------------------------------------------------------

#[test]
fn test_required_role_and_tags_roundtrip() {
    let (store, dir) = temp_store("required-roundtrip");
    let mut ni = new_issue("带派发需求");
    ni.required_role = Some("worker".into());
    ni.required_tags = vec!["rust".into(), "backend".into()];
    let issue = store.create_issue(ni).unwrap();
    let back = store.get_issue(issue.id).unwrap();
    assert_eq!(back.required_role.as_deref(), Some("worker"));
    assert_eq!(
        back.required_tags,
        vec!["rust".to_string(), "backend".to_string()]
    );

    // 未带需求的 issue：None / 空（宽容默认，不是 Option<Vec>）。
    let plain = store.create_issue(new_issue("无需求")).unwrap();
    let back = store.get_issue(plain.id).unwrap();
    assert_eq!(back.required_role, None);
    assert!(back.required_tags.is_empty());

    cleanup(&dir);
}

#[test]
fn test_required_role_whitespace_normalizes_to_none() {
    let (store, dir) = temp_store("required-blank");
    let mut ni = new_issue("空白角色");
    ni.required_role = Some("   ".into());
    let issue = store.create_issue(ni).unwrap();
    let back = store.get_issue(issue.id).unwrap();
    assert_eq!(back.required_role, None, "空白角色应与 None 同义");
    cleanup(&dir);
}

#[test]
fn test_dependencies_set_and_query_both_directions() {
    let (store, dir) = temp_store("deps-directions");
    let a = store.create_issue(new_issue("A")).unwrap();
    let b = store.create_issue(new_issue("B")).unwrap();
    let c = store.create_issue(new_issue("C")).unwrap();

    store
        .set_issue_dependencies(c.id, &[a.id, b.id])
        .expect("设置依赖边");
    assert_eq!(store.dependencies_of(c.id).unwrap(), vec![a.id, b.id]);
    assert_eq!(store.dependents_of(a.id).unwrap(), vec![c.id]);
    assert_eq!(store.dependents_of(b.id).unwrap(), vec![c.id]);
    // C 不被任何人依赖；A/B 无依赖。
    assert!(store.dependents_of(c.id).unwrap().is_empty());
    assert!(store.dependencies_of(a.id).unwrap().is_empty());

    cleanup(&dir);
}

#[test]
fn test_dependencies_replace_semantics() {
    let (store, dir) = temp_store("deps-replace");
    let a = store.create_issue(new_issue("A")).unwrap();
    let b = store.create_issue(new_issue("B")).unwrap();
    let c = store.create_issue(new_issue("C")).unwrap();

    store.set_issue_dependencies(c.id, &[a.id]).unwrap();
    store.set_issue_dependencies(c.id, &[b.id]).unwrap();
    assert_eq!(
        store.dependencies_of(c.id).unwrap(),
        vec![b.id],
        "重设必须整体替换"
    );
    assert!(
        store.dependents_of(a.id).unwrap().is_empty(),
        "旧边必须清掉"
    );

    // 空切片 = 清空。
    store.set_issue_dependencies(c.id, &[]).unwrap();
    assert!(store.dependencies_of(c.id).unwrap().is_empty());

    cleanup(&dir);
}

#[test]
fn test_dependencies_reject_unknown_issue_fk() {
    let (store, dir) = temp_store("deps-fk");
    let a = store.create_issue(new_issue("A")).unwrap();
    // 引用不存在的 issue id：FK 约束诚实拒绝（不静默吞）。
    assert!(store.set_issue_dependencies(a.id, &[999_999]).is_err());
    assert!(store.dependencies_of(a.id).unwrap().is_empty());
    cleanup(&dir);
}

#[test]
fn test_dependencies_dedup_repeats() {
    let (store, dir) = temp_store("deps-dedup");
    let a = store.create_issue(new_issue("A")).unwrap();
    let c = store.create_issue(new_issue("C")).unwrap();
    store.set_issue_dependencies(c.id, &[a.id, a.id]).unwrap();
    assert_eq!(
        store.dependencies_of(c.id).unwrap(),
        vec![a.id],
        "重复边去重"
    );
    cleanup(&dir);
}

#[test]
fn test_list_children_returns_creation_order() {
    let (store, dir) = temp_store("list-children");
    let parent = store.create_issue(new_issue("父单")).unwrap();
    let c1 = store
        .create_issue(NewIssue {
            title: "子一".into(),
            parent_issue_id: Some(parent.id),
            ..NewIssue::default()
        })
        .unwrap();
    let c2 = store
        .create_issue(NewIssue {
            title: "子二".into(),
            parent_issue_id: Some(parent.id),
            ..NewIssue::default()
        })
        .unwrap();
    // 无关父单的 issue 不混入。
    let _other = store.create_issue(new_issue("别人家")).unwrap();

    let children = store.list_children(parent.id).unwrap();
    assert_eq!(
        children.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![c1.id, c2.id],
        "按创建序返回且只含本父单子单"
    );
    assert!(store.list_children(999_999).unwrap().is_empty());

    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 讨论频道 + 任务资产（Swarm M2）
// ---------------------------------------------------------------------------

use crate::models::{NewChannel, NewChannelMessage, channel_message_type};

fn sys_actor() -> Actor {
    Actor::system("board")
}

#[test]
fn test_ensure_default_channels_is_idempotent() {
    let (store, dir) = temp_store("ensure-channels");
    store.ensure_default_channels().unwrap();
    store.ensure_default_channels().unwrap(); // 二次 ensure 零副作用
    let names: Vec<String> = store
        .list_channels()
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, vec!["#dev", "#qa", "#general"]);
    cleanup(&dir);
}

#[test]
fn test_create_channel_normalizes_and_rejects_duplicates() {
    let (store, dir) = temp_store("create-channel");
    // 缺 # 前缀自动补。
    let c = store
        .create_channel(NewChannel {
            name: "design".to_string(),
            topic: "设计讨论".to_string(),
        })
        .unwrap();
    assert_eq!(c.name, "#design");
    assert_eq!(c.topic, "设计讨论");
    // 重名报错（含已归一化形态再建）。
    let err = store
        .create_channel(NewChannel {
            name: "#design".to_string(),
            topic: String::new(),
        })
        .unwrap_err();
    assert!(err.contains("already exists"), "got: {err}");
    // 空名拒绝。
    assert!(
        store
            .create_channel(NewChannel {
                name: "   ".to_string(),
                topic: String::new(),
            })
            .is_err()
    );
    // 按名查询（输入同样归一化）。
    let got = store.get_channel_by_name("design").unwrap().unwrap();
    assert_eq!(got.id, c.id);
    assert!(store.get_channel_by_name("#nope").unwrap().is_none());
    cleanup(&dir);
}

#[test]
fn test_channel_membership_join_leave_list() {
    let (store, dir) = temp_store("membership");
    store.ensure_default_channels().unwrap();
    let qa = store.get_channel_by_name("#qa").unwrap().unwrap();
    let missing_id = 999_999;

    // 不存在的频道报错（非 FK 裸错）。
    let err = store
        .join_channel(missing_id, Actor::agent("node-b"))
        .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");

    // 入频道幂等。
    store.join_channel(qa.id, Actor::agent("node-b")).unwrap();
    store.join_channel(qa.id, Actor::agent("node-b")).unwrap();
    store.join_channel(qa.id, Actor::admin("admin")).unwrap();
    let members = store.list_channel_members(qa.id).unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().all(|m| m.channel_id == qa.id));
    assert_eq!(members[0].member.kind, "admin"); // 排序 member_type, member_id
    assert_eq!(members[1].member.id, "node-b");

    // 离频道幂等；未入成员离队也是 no-op。
    store.leave_channel(qa.id, &Actor::agent("node-b")).unwrap();
    store
        .leave_channel(qa.id, &Actor::agent("never-joined"))
        .unwrap();
    let members = store.list_channel_members(qa.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].member.id, "admin");
    cleanup(&dir);
}

/// Swarm M2：first-join 判据 —— 成员跨全表零行才算全新节点。
#[test]
fn test_has_any_channel_membership_first_join_predicate() {
    let (store, dir) = temp_store("first-join");
    store.ensure_default_channels().unwrap();
    let dev = store.get_channel_by_name("#dev").unwrap().unwrap();
    let general = store.get_channel_by_name("#general").unwrap().unwrap();

    // 全新节点：任何频道都无行 → 可自动收编。
    assert!(
        !store
            .has_any_channel_membership(&Actor::agent("node-x"))
            .unwrap()
    );

    // 入了 #dev 后：不再算全新（announce 不重复收编）。
    store.join_channel(dev.id, Actor::agent("node-x")).unwrap();
    assert!(
        store
            .has_any_channel_membership(&Actor::agent("node-x"))
            .unwrap()
    );

    // 只在 #general 的成员同样判「已见」——被管理员 leave 出 #dev 的
    // 成员若在其他频道有行就不会被拉回。
    store
        .join_channel(general.id, Actor::agent("node-y"))
        .unwrap();
    assert!(
        store
            .has_any_channel_membership(&Actor::agent("node-y"))
            .unwrap()
    );
    store
        .leave_channel(general.id, &Actor::agent("node-y"))
        .unwrap();
    assert!(
        !store
            .has_any_channel_membership(&Actor::agent("node-y"))
            .unwrap()
    );
    cleanup(&dir);
}

#[test]
fn test_channel_message_append_list_and_cursor() {
    let (store, dir) = temp_store("messages");
    store.ensure_default_channels().unwrap();
    let dev = store.get_channel_by_name("#dev").unwrap().unwrap();

    // 纯空白内容拒绝。
    assert!(
        store
            .append_channel_message(NewChannelMessage {
                channel_id: dev.id,
                sender: sys_actor(),
                content: "   ".to_string(),
                parent_id: None,
                mtype: String::new(),
            })
            .is_err()
    );

    let m1 = store
        .append_channel_message(NewChannelMessage {
            channel_id: dev.id,
            sender: Actor::admin("admin"),
            content: "第一条".to_string(),
            parent_id: None,
            mtype: String::new(),
        })
        .unwrap();
    assert_eq!(m1.mtype, channel_message_type::TEXT);
    let m2 = store
        .append_channel_message(NewChannelMessage {
            channel_id: dev.id,
            sender: Actor::agent("node-b"),
            content: "线程回复".to_string(),
            parent_id: Some(m1.id),
            mtype: channel_message_type::SYSTEM.to_string(),
        })
        .unwrap();
    assert_eq!(m2.mtype, channel_message_type::SYSTEM);
    assert_eq!(m2.parent_id, Some(m1.id));
    // 别的频道消息不混入（ensure 顺序 dev=1, qa=2, general=3）。
    let _other = store
        .append_channel_message(NewChannelMessage {
            channel_id: dev.id + 1,
            sender: sys_actor(),
            content: "别频道".to_string(),
            parent_id: None,
            mtype: String::new(),
        })
        .unwrap();

    let all = store.list_channel_messages(dev.id, 0, 100).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, m1.id);
    assert_eq!(all[1].id, m2.id);

    // after_id 游标：补拉与翻页语义。
    let page2 = store.list_channel_messages(dev.id, m1.id, 100).unwrap();
    assert_eq!(page2.iter().map(|m| m.id).collect::<Vec<_>>(), vec![m2.id]);
    let first_only = store.list_channel_messages(dev.id, 0, 1).unwrap();
    assert_eq!(first_only.len(), 1);
    assert_eq!(first_only[0].id, m1.id);

    // 未读游标只前进（回拨被 MAX 拒绝）。
    store.join_channel(dev.id, Actor::admin("admin")).unwrap();
    store
        .mark_channel_seen(dev.id, &Actor::admin("admin"), m1.id)
        .unwrap();
    store
        .mark_channel_seen(dev.id, &Actor::admin("admin"), m2.id)
        .unwrap();
    store
        .mark_channel_seen(dev.id, &Actor::admin("admin"), m1.id)
        .unwrap();
    let members = store.list_channel_members(dev.id).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].member.id, "admin");
    assert_eq!(members[0].last_seen_message_id, m2.id);
    cleanup(&dir);
}

#[test]
fn test_sweep_channel_messages_respects_retention() {
    let (store, dir) = temp_store("sweep");
    store.ensure_default_channels().unwrap();
    let dev = store.get_channel_by_name("#dev").unwrap().unwrap();
    for i in 0..5 {
        store
            .append_channel_message(NewChannelMessage {
                channel_id: dev.id,
                sender: sys_actor(),
                content: format!("消息{i}"),
                parent_id: None,
                mtype: String::new(),
            })
            .unwrap();
    }

    // retention 0 = 永久保留（清扫直接 no-op）。
    assert_eq!(store.sweep_channel_messages(0).unwrap(), 0);
    assert_eq!(
        store.list_channel_messages(dev.id, 0, 100).unwrap().len(),
        5
    );

    // 全部消息都是刚写入的 → 90 天保留扫不掉。
    assert_eq!(store.sweep_channel_messages(90).unwrap(), 0);

    // 直接回拨 created_at 到 100 天前 → 90 天保留应全清。
    {
        let conn = rusqlite::Connection::open(dir.join("board.db")).unwrap();
        let old = chrono::Utc::now().timestamp() - 100 * 24 * 3600;
        conn.execute(
            "UPDATE channel_message SET created_at = ?1",
            rusqlite::params![old],
        )
        .unwrap();
    }
    assert_eq!(store.sweep_channel_messages(90).unwrap(), 5);
    assert!(
        store
            .list_channel_messages(dev.id, 0, 100)
            .unwrap()
            .is_empty()
    );
    cleanup(&dir);
}

#[test]
fn test_asset_register_lookup_upsert() {
    let (store, dir) = temp_store("asset-crud");
    assert!(store.lookup_asset("no-such-ref").unwrap().is_none());

    let a = store
        .register_asset(NewAsset {
            ref_name: "report.md".to_string(),
            origin_issue: Some(1),
            sha256: "aaa".to_string(),
            size: 100,
        })
        .unwrap();
    assert_eq!(a.path, "report.md"); // path 恒 = ref
    assert_eq!(a.origin_issue, Some(1));

    // 同 ref 重登记 = 幂等 upsert（新内容覆盖索引）。
    let b = store
        .register_asset(NewAsset {
            ref_name: "report.md".to_string(),
            origin_issue: Some(2),
            sha256: "bbb".to_string(),
            size: 200,
        })
        .unwrap();
    assert_eq!(b.id, a.id);
    let got = store.lookup_asset("report.md").unwrap().unwrap();
    assert_eq!(got.sha256, "bbb");
    assert_eq!(got.size, 200);
    assert_eq!(got.origin_issue, Some(2));
    assert_eq!(store.assets_for_issue(2).unwrap().len(), 1);
    assert!(store.assets_for_issue(1).unwrap().is_empty());

    // 空 ref 拒绝。
    assert!(
        store
            .register_asset(NewAsset {
                ref_name: " ".to_string(),
                origin_issue: None,
                sha256: String::new(),
                size: 0,
            })
            .is_err()
    );
    cleanup(&dir);
}

#[test]
fn test_asset_unbind_uses_reference_counting() {
    let (store, dir) = temp_store("asset-unbind");
    // 同一 ref 绑两个 issue。
    for issue in [1, 2] {
        store
            .register_asset(NewAsset {
                ref_name: "shared.bin".to_string(),
                origin_issue: Some(issue),
                sha256: "s".to_string(),
                size: 1,
            })
            .unwrap();
    }
    store
        .register_asset(NewAsset {
            ref_name: "only-one.bin".to_string(),
            origin_issue: Some(1),
            sha256: "o".to_string(),
            size: 2,
        })
        .unwrap();

    // 解绑 issue 1：shared.bin 仍被 issue 2 引用（不返回），only-one.bin 彻底释放。
    let released = store.unbind_assets_for_issue(1).unwrap();
    assert_eq!(released, vec!["only-one.bin".to_string()]);
    assert!(store.lookup_asset("shared.bin").unwrap().is_some());
    assert!(store.lookup_asset("only-one.bin").unwrap().is_none());

    // 解绑 issue 2：shared.bin 失去最后一个引用 → 释放。
    let released = store.unbind_assets_for_issue(2).unwrap();
    assert_eq!(released, vec!["shared.bin".to_string()]);
    assert!(store.lookup_asset("shared.bin").unwrap().is_none());
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// M3 信封落库（幂等 + seq 台账 + 补拉）
// ---------------------------------------------------------------------------

/// G12：同 (origin_node, client_msg_id) 重发返回首响，不重复落库；
/// issue 评论与频道消息两路都走一遍。
#[test]
fn test_post_discussion_envelope_idempotent_both_targets() {
    let (store, dir) = temp_store("envelope-idem");
    store.ensure_default_channels().unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: "幂等评论".to_string(),
            ..Default::default()
        })
        .unwrap();
    let dev = store.get_channel_by_name("#dev").unwrap().unwrap();
    let author = Actor::agent("node-b");

    // 首发落库：issue 评论 + 频道消息，各带唯一幂等键。
    let first = store
        .post_discussion_envelope(
            "node-b",
            "u-issue-1",
            crate::models::thread_kind::ISSUE,
            issue.id,
            &author,
            "看一下这个问题",
            None,
            "discussion",
        )
        .unwrap();
    assert!(first.is_new);
    assert_eq!(first.response["comment_id"], first.message_id);
    assert!(first.seq > 0);

    let first_ch = store
        .post_discussion_envelope(
            "node-b",
            "u-ch-1",
            crate::models::thread_kind::CHANNEL,
            dev.id,
            &author,
            "频道里说一句",
            None,
            "text",
        )
        .unwrap();
    assert!(first_ch.is_new);
    assert_eq!(first_ch.response["message_id"], first_ch.message_id);

    // 重发：is_new=false，首响原样返回，库不增长，seq 不推进。
    let dup = store
        .post_discussion_envelope(
            "node-b",
            "u-issue-1",
            crate::models::thread_kind::ISSUE,
            issue.id,
            &author,
            "看一下这个问题",
            None,
            "discussion",
        )
        .unwrap();
    assert!(!dup.is_new);
    assert_eq!(dup.response, first.response);
    assert_eq!(store.list_comments(issue.id).unwrap().len(), 1);

    let dup_ch = store
        .post_discussion_envelope(
            "node-b",
            "u-ch-1",
            crate::models::thread_kind::CHANNEL,
            dev.id,
            &author,
            "频道里说一句",
            None,
            "text",
        )
        .unwrap();
    assert!(!dup_ch.is_new);
    assert_eq!(dup_ch.response, first_ch.response);
    assert_eq!(
        store.list_channel_messages(dev.id, 0, 100).unwrap().len(),
        1
    );

    // 不同 origin 的同 id 不冲突（幂等键按节点隔离）。
    let other = store
        .post_discussion_envelope(
            "node-c",
            "u-issue-1",
            crate::models::thread_kind::ISSUE,
            issue.id,
            &Actor::agent("node-c"),
            "另一节点独立键",
            None,
            "discussion",
        )
        .unwrap();
    assert!(other.is_new);
    assert_eq!(store.list_comments(issue.id).unwrap().len(), 2);

    // 落库失败（不存在的频道）→ 认领行回滚，同 id 重试可成功。
    let err = store
        .post_discussion_envelope(
            "node-d",
            "u-fail-1",
            crate::models::thread_kind::CHANNEL,
            999_999,
            &Actor::agent("node-d"),
            "投不进去",
            None,
            "text",
        )
        .unwrap_err();
    assert!(err.contains("FOREIGN KEY"), "got: {err}");
    let retry = store
        .post_discussion_envelope(
            "node-d",
            "u-fail-1",
            crate::models::thread_kind::CHANNEL,
            dev.id,
            &Actor::agent("node-d"),
            "换个地方再试",
            None,
            "text",
        )
        .unwrap();
    assert!(retry.is_new, "失败回滚后同 id 重试应作为首次处理");
    cleanup(&dir);
}

/// seq 台账：单调递增 + board.sync 增量补拉（G8 的存储侧）。
#[test]
fn test_seq_ledger_and_list_messages_since() {
    let (store, dir) = temp_store("seq-ledger");
    store.ensure_default_channels().unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: "台账".to_string(),
            ..Default::default()
        })
        .unwrap();
    let qa = store.get_channel_by_name("#qa").unwrap().unwrap();

    assert_eq!(store.latest_seq().unwrap(), 0, "空表 latest=0");

    // 三条消息：issue 评论 ×2 + 频道消息 ×1（混排）。
    let p1 = store
        .post_discussion_envelope(
            "node-b",
            "s-1",
            crate::models::thread_kind::ISSUE,
            issue.id,
            &Actor::agent("node-b"),
            "第一条评论",
            None,
            "discussion",
        )
        .unwrap();
    let p2 = store
        .post_discussion_envelope(
            "node-c",
            "s-2",
            crate::models::thread_kind::CHANNEL,
            qa.id,
            &Actor::agent("node-c"),
            "@node-b 频道喊你",
            None,
            "text",
        )
        .unwrap();
    let p3 = store
        .post_discussion_envelope(
            "node-b",
            "s-3",
            crate::models::thread_kind::ISSUE,
            issue.id,
            &Actor::admin("admin"),
            "回复第一条",
            Some(p1.message_id),
            "question",
        )
        .unwrap();
    assert!(
        p1.seq < p2.seq && p2.seq < p3.seq,
        "seq 单调递增: {} {} {}",
        p1.seq,
        p2.seq,
        p3.seq
    );
    assert_eq!(store.latest_seq().unwrap(), p3.seq);

    // 补拉：since=p1.seq → 拿到 p2/p3，含原表内容与发送者。
    let entries = store.list_messages_since(p1.seq, 100).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].seq, p2.seq);
    assert_eq!(entries[0].thread_kind, "channel");
    assert_eq!(entries[0].sender_id, "node-c");
    assert_eq!(entries[0].content, "@node-b 频道喊你");
    assert_eq!(entries[1].seq, p3.seq);
    assert_eq!(entries[1].thread_kind, "issue");
    assert_eq!(entries[1].kind_tag, "question");
    assert_eq!(entries[1].parent_id, Some(p1.message_id));

    // since=0 全量；limit 截断取最早。
    assert_eq!(store.list_messages_since(0, 100).unwrap().len(), 3);
    assert_eq!(store.list_messages_since(0, 2).unwrap().len(), 2);
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// team_memory（M4.5 集体记忆 §6.5：蒸馏写闸 + 检索源）
// ---------------------------------------------------------------------------

use crate::models::NewTeamMemory;

fn new_memory(category: &str, scope: &str, content: &str) -> NewTeamMemory {
    NewTeamMemory {
        category: category.to_string(),
        scope: scope.to_string(),
        content: content.to_string(),
        source: "NB-7".to_string(),
        author: "node-a".to_string(),
    }
}

#[test]
fn test_team_memory_add_list_and_scope_normalization() {
    let (store, dir) = temp_store("team-memory-add");
    let (id, merged) = store
        .add_team_memory(new_memory("pitfall", "  Auth  ", "token 过期要刷新"))
        .unwrap();
    assert!(!merged);
    // scope 归一化（trim + 小写）落库。
    let all = store.list_team_memory(None, false).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, id);
    assert_eq!(all[0].scope, "auth");
    assert_eq!(all[0].use_count, 0);
    assert!(!all[0].deprecated);
    // scope 过滤（大小写不敏感）。
    assert_eq!(
        store.list_team_memory(Some("AUTH"), false).unwrap().len(),
        1
    );
    assert_eq!(
        store.list_team_memory(Some("nope"), false).unwrap().len(),
        0
    );
    // 空 scope/content 拒绝（蒸馏纪律兜底）。
    assert!(
        store
            .add_team_memory(new_memory("pitfall", "  ", "x"))
            .is_err()
    );
    assert!(
        store
            .add_team_memory(new_memory("pitfall", "auth", "   "))
            .is_err()
    );
    cleanup(&dir);
}

#[test]
fn test_team_memory_dedup_merges_into_old_entry() {
    let (store, dir) = temp_store("team-memory-dedup");
    let (id1, _) = store
        .add_team_memory(new_memory("pitfall", "auth", "token 过期要刷新"))
        .unwrap();
    // 完全相同 → 并进旧条目。
    let (id2, merged2) = store
        .add_team_memory(new_memory("pattern", "auth", "token 过期要刷新"))
        .unwrap();
    assert!(merged2);
    assert_eq!(id2, id1);
    // 空格差异视为重复。
    let (_, merged3) = store
        .add_team_memory(new_memory("pitfall", "auth", "token  过期要刷新"))
        .unwrap();
    assert!(merged3);
    // 内容互为包含（子串）→ 从宽并入。
    let (_, merged4) = store
        .add_team_memory(new_memory(
            "pitfall",
            "auth",
            "token 过期要刷新，先刷新再重试",
        ))
        .unwrap();
    assert!(merged4);
    // 不同 scope 不合并。
    let (_, merged5) = store
        .add_team_memory(new_memory("pitfall", "db", "token 过期要刷新"))
        .unwrap();
    assert!(!merged5);
    // 旧条目 use_count=3（三次并入）；总数 2（auth 原条 + db 新条）。
    let all = store.list_team_memory(None, false).unwrap();
    assert_eq!(all.len(), 2);
    let merged_entry = all.iter().find(|e| e.id == id1).unwrap();
    assert_eq!(merged_entry.use_count, 3);
    // 并进旧条目时 category/source/author 不变（旧条目身份稳定）。
    assert_eq!(merged_entry.category, "pitfall");
    cleanup(&dir);
}

#[test]
fn test_team_memory_search_mark_deprecate_remove() {
    let (store, dir) = temp_store("team-memory-lifecycle");
    let (id1, _) = store
        .add_team_memory(new_memory("pitfall", "auth", "token 过期要刷新"))
        .unwrap();
    let (id2, _) = store
        .add_team_memory(new_memory("pattern", "rust", "先 cargo check 再改"))
        .unwrap();
    // 关键词检索（scope/content/source 命中）。
    assert_eq!(store.search_team_memory("过期").unwrap().len(), 1);
    assert_eq!(store.search_team_memory("RUST").unwrap().len(), 1);
    assert_eq!(store.search_team_memory("NB-7").unwrap().len(), 2);
    // use_count 批量计数（注入端调用）。
    store.mark_team_memory_used(&[id1, id1, id2]).unwrap();
    let all = store.list_team_memory(None, false).unwrap();
    assert_eq!(all[0].id, id1, "use_count 降序排前");
    assert_eq!(all[0].use_count, 2);
    // 软删后列表默认滤掉；include_deprecated 可见。
    store.set_team_memory_deprecated(id1, true).unwrap();
    assert_eq!(store.list_team_memory(None, false).unwrap().len(), 1);
    let with_dep = store.list_team_memory(None, true).unwrap();
    assert_eq!(with_dep.len(), 2);
    assert!(with_dep.iter().find(|e| e.id == id1).unwrap().deprecated);
    // 不存在的 id 报错。
    assert!(store.set_team_memory_deprecated(9999, true).is_err());
    assert!(store.remove_team_memory(9999).is_err());
    // 彻底删除。
    store.remove_team_memory(id2).unwrap();
    assert_eq!(store.list_team_memory(None, true).unwrap().len(), 1);
    cleanup(&dir);
}

#[test]
fn test_team_memory_migration_preserves_existing_data() {
    // v8 迁移幂等：旧库（含 issue 数据）打开后 team_memory 表在场且旧数据
    // 完好（user_version 迁移链回归）。
    let (store, dir) = temp_store("team-memory-migration");
    let issue = store.create_issue(new_issue("迁移前的 issue")).unwrap();
    store
        .add_team_memory(new_memory("convention", "board", "经验先落库"))
        .unwrap();
    drop(store);
    // 重新打开同一库。
    let store2 = BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    assert_eq!(store2.list_team_memory(None, false).unwrap().len(), 1);
    assert!(store2.get_issue(issue.id).is_ok());
    cleanup(&dir);
}

// ---------------------------------------------------------------------------
// 全自动流转 P3：auto_plan serde 兼容 + project.status 宽容读取
// ---------------------------------------------------------------------------

#[test]
fn test_autopilot_serde_roundtrip_auto_plan_default() {
    // 存量 JSON（无 auto_plan 键）→ 反序列化 false（行为不变）；显式 true
    // 透传；序列化带键。
    let legacy = r#"{"id":1,"name":"日报","cron":"0 9 * * *","title":"t","priority":1,
        "project_id":null,"target":"","enabled":true,"cron_job_id":null,
        "last_run_at":null,"created_at":0,"updated_at":0}"#;
    let ap: Autopilot = serde_json::from_str(legacy).unwrap();
    assert!(
        !ap.auto_plan,
        "缺省必须反序列化为 false（存量规则行为不变）"
    );
    let mut ap = ap;
    ap.auto_plan = true;
    let json = serde_json::to_string(&ap).unwrap();
    assert!(
        json.contains("\"auto_plan\":true"),
        "序列化应带 auto_plan 键: {json}"
    );
}

#[test]
fn test_project_status_lenient_read_unknown_value() {
    // 存量库里的未知 status 字符串 → 读取宽容映射 active（WARN 一次），
    // 不炸不拒读。
    let (store, dir) = temp_store("project-status-lenient");
    let pid = store.create_project("老项目", "", None, "", "").unwrap().id;
    // 直接 SQL 改成词表外的值（模拟存量的自由字符串时代遗留数据）。
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE project SET status = 'frozen_weird' WHERE id = ?1",
            params![pid],
        )
        .unwrap();
    }
    let p = store.get_project(pid).unwrap();
    assert_eq!(p.status, "active", "未知值读取时宽容映射 active");
    // 未知值写不进去（update_project 对词表外 loud 拒绝）。
    assert!(
        store
            .update_project(
                pid,
                &crate::models::ProjectPatch {
                    status: Some("another_weird".to_string()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    cleanup(&dir);
}

// -- 回归（全自动流转 P4 UAT 抓真 bug 后钉死）--

/// 同秒派发乱序回归：`dispatched_at` 秒级精度下，同一秒内连续 insert 的
/// 多条派发必须按插入序读回（rowid 序），"最新派发"不得被随机 UUID 决胜
/// 打乱——D3 连续同节点判定与回调路由都吃这个序（cluster-uat T34 实测）。
#[test]
fn test_dispatch_list_order_is_insertion_order_within_same_second() {
    let (store, dir) = temp_store("dispatch-order");
    let issue = store.create_issue(new_issue("同秒三连派")).unwrap();
    store
        .insert_dispatch("task-zzz", issue.id, "node-b", &admin())
        .unwrap();
    store
        .insert_dispatch("task-aaa", issue.id, "node-c", &admin())
        .unwrap();
    store
        .insert_dispatch("task-mmm", issue.id, "node-d", &admin())
        .unwrap();
    let list = store.list_dispatches(issue.id).unwrap();
    let workers: Vec<&str> = list.iter().map(|d| d.worker_id.as_str()).collect();
    assert_eq!(
        workers,
        vec!["node-b", "node-c", "node-d"],
        "同秒多条派发按插入序读回（不受 task_id UUID 排序影响）"
    );
    // 活跃派发取最新插入的（回调写回路由语义）。
    let active = store.get_active_dispatch(issue.id).unwrap().unwrap();
    assert_eq!(active.worker_id, "node-d");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 项目级验收标准（v10 列）round-trip：create 写入 → get 读回；patch 更新；
/// 缺省为空串。
#[test]
fn test_project_acceptance_criteria_roundtrip() {
    let (store, dir) = temp_store("project-ac");
    let p = store
        .create_project(
            "带标准项目",
            "说明",
            None,
            "🚀",
            "交付说明文本。\n<REVIEW_FAIL>",
        )
        .unwrap();
    assert_eq!(p.acceptance_criteria, "交付说明文本。\n<REVIEW_FAIL>");
    // 缺省空串。
    let p2 = store
        .create_project("无标准项目", "", None, "", "")
        .unwrap();
    assert_eq!(p2.acceptance_criteria, "");
    // patch 更新。
    let updated = store
        .update_project(
            p.id,
            &crate::models::ProjectPatch {
                acceptance_criteria: Some("新标准".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(updated.acceptance_criteria, "新标准");
    // patch None 不动。
    let untouched = store.get_project(p.id).unwrap();
    assert_eq!(untouched.acceptance_criteria, "新标准");
    let _ = std::fs::remove_dir_all(&dir);
}

// -- E2 决策流审计（全自动流转 P5）--

/// list_recent_activity：JOIN 补齐单号/标题、按 id 倒序、limit 截断、
/// action 过滤只留 auto_decide 词表。
#[test]
fn test_audit_list_recent_activity_filter_and_limit() {
    let (store, dir) = temp_store("audit-list");
    let a = store.create_issue(new_issue("决策甲")).unwrap();
    let b = store.create_issue(new_issue("决策乙")).unwrap();
    store
        .add_activity(
            a.id,
            &admin(),
            "auto_decide",
            Some(r#"{"decision":"auto_accept"}"#),
        )
        .unwrap();
    store
        .add_activity(
            b.id,
            &admin(),
            "auto_decide",
            Some(r#"{"decision":"redispatch"}"#),
        )
        .unwrap();
    // 非 auto_decide 词表行（状态变更）不应进决策流。
    store
        .add_activity(a.id, &admin(), "status_change", Some("x"))
        .unwrap();
    store
        .add_activity(
            b.id,
            &admin(),
            "auto_decide",
            Some(r#"{"decision":"escalate_human"}"#),
        )
        .unwrap();

    // 全量倒序 + JOIN 字段。
    let rows = store.list_recent_activity(500, None).unwrap();
    let auto: Vec<_> = rows
        .iter()
        .filter(|r| r.activity.action == "auto_decide")
        .collect();
    assert_eq!(auto.len(), 3);
    assert_eq!(auto[0].issue_title, "决策乙");
    assert_eq!(auto[2].issue_number, a.number);
    assert_eq!(
        auto[0].activity.details.as_deref(),
        Some(r#"{"decision":"escalate_human"}"#)
    );

    // limit 截断取最新。
    let top1 = store.list_recent_activity(1, None).unwrap();
    assert_eq!(top1.len(), 1);
    assert_eq!(top1[0].activity.action, "auto_decide");
    assert_eq!(top1[0].issue_title, "决策乙");

    // action 过滤。
    let filtered = store
        .list_recent_activity(500, Some("status_change"))
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].activity.action, "status_change");
    let _ = std::fs::remove_dir_all(&dir);
}

/// rollback_decision：done 单回滚 → in_review + 系统评论 + 活动记录；
/// 非 auto_decide 拒；已非 done 拒（再回滚防重）；activity 不存在拒。
#[test]
fn test_audit_rollback_done_to_in_review_and_reentry_rejected() {
    let (store, dir) = temp_store("audit-rollback");
    let mut issue = store.create_issue(new_issue("自动收货回滚")).unwrap();
    store
        .add_activity(
            issue.id,
            &admin(),
            "auto_decide",
            Some(r#"{"decision":"auto_accept","verdict":"PASS"}"#),
        )
        .unwrap();
    // 非 auto_decide 活动不能回滚。
    store
        .add_activity(issue.id, &admin(), "status_change", Some("x"))
        .unwrap();
    let noise = store
        .list_recent_activity(1, Some("status_change"))
        .unwrap()[0]
        .activity
        .id;
    assert!(store.rollback_decision(noise).is_err());

    // 推到 done（模拟自动收货后的状态）。
    store
        .transition_issue(issue.id, IssueStatus::Done, &admin())
        .unwrap();
    let activity_id = store.list_recent_activity(1, Some("auto_decide")).unwrap()[0]
        .activity
        .id;

    let rolled = store.rollback_decision(activity_id).unwrap();
    assert_eq!(rolled.status, IssueStatus::InReview);
    issue = store.get_issue(issue.id).unwrap();
    assert_eq!(issue.status, IssueStatus::InReview);

    // 系统评论 + 回滚活动已落。
    let comments = store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("审计回滚")),
        "回滚必须留系统评论"
    );
    let acts = store
        .list_recent_activity(500, Some("status_changed"))
        .unwrap();
    assert!(
        acts.iter().any(|r| r
            .activity
            .details
            .as_deref()
            .unwrap_or("")
            .contains("audit_rollback")),
        "回滚必须留 status_changed 活动"
    );

    // 再回滚拒：单已退回 in_review，非 done。
    let err = store.rollback_decision(activity_id).unwrap_err();
    assert!(err.contains("done"), "再回滚应拒绝：{err}");

    // activity 不存在拒。
    assert!(store.rollback_decision(999_999).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
