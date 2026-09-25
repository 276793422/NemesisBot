// store.rs 覆盖率补充测试（指派侧道 178/188、574-591、723-740、800-825、
// 3064-3091、3246；回滚 1020-1031/1072-1080；pending merge 去重 1279；
// 派发事件通知 1419-1426；通知全员归零 1498-1530；标签授予 1817/1828；
// 重平衡候选 1867-1882；取消/失败派发 1940/1990；autopilot 校验与部分
// 更新 2071-2078/2166/2986-2989；频道重名 2254；上行信封 2381-2443/2504；
// 注入计数空表 2894）。
//
// 豁免（防御臂，无确定性触发形态）：
// - 308-316（descendants 的查询失败 warn 臂）：需要 BFS 中途 sqlite 故障；
// - 2209-2214（状态直方图的 FromSqlConversionFailure）：需要库外改库写入
//   非法 status 值（store 无此写入路径）。

use super::*;
use crate::assignment::{Actor, AssignmentType};
use crate::models::{
    CommentType, IssueStatus, NewAsset, NewChannel, NewComment, NewIssue, PendingMerge,
    dispatch_state, notification_kind, priority, thread_kind,
};

fn temp_store(name: &str) -> BoardStore {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-store-cov-{}-{name}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

fn admin() -> Actor {
    Actor::admin("admin")
}

fn worker_actor(id: &str) -> Actor {
    Actor::new(AssignmentType::Worker.as_str(), id)
}

fn new_issue(title: &str) -> NewIssue {
    NewIssue {
        title: title.to_string(),
        ..NewIssue::default()
    }
}

fn issue_with_assignee(store: &BoardStore, assignee: &str) -> crate::models::Issue {
    store
        .create_issue(NewIssue {
            title: "带指派的任务".to_string(),
            assignee: Some(AssignmentType::Worker),
            assignee_id: Some(assignee.to_string()),
            ..Default::default()
        })
        .unwrap()
}

fn unread(store: &BoardStore, actor: &Actor) -> i64 {
    store
        .unread_notification_count(&actor.kind, Some(&actor.id))
        .unwrap()
}

// ---------------------------------------------------------------------------
// 指派侧道：创建/转移/改派/移动的通知与订阅
// ---------------------------------------------------------------------------

/// 带指派创建 → assignee 进订阅表（178/188 的活动+订阅双写）。
#[test]
fn create_with_assignee_subscribes_assignee() {
    let store = temp_store("create-assignee");
    let issue = issue_with_assignee(&store, "node-a");
    let subs = store.list_subscribers(issue.id).unwrap();
    assert!(
        subs.iter()
            .any(|s| s.subscriber.id == "node-a" && s.reason == "assignee")
    );
    let acts = store.list_activity(issue.id).unwrap();
    assert!(acts.iter().any(|a| a.action == "assigned"));
}

/// 指派对象非操作者时的状态转移通知（574-591）。
#[test]
fn transition_notifies_assignee() {
    let store = temp_store("transition-notify");
    let issue = issue_with_assignee(&store, "node-a");
    let assignee = worker_actor("node-a");
    assert_eq!(unread(&store, &assignee), 0);

    store
        .transition_issue(issue.id, IssueStatus::InProgress, &admin())
        .unwrap();
    assert!(
        unread(&store, &assignee) > 0,
        "指派对象必须收到状态变化通知"
    );
}

/// 改派给新节点 → 新指派人收通知 + 进订阅（723-740）。
#[test]
fn reassign_notifies_new_assignee() {
    let store = temp_store("reassign");
    let issue = issue_with_assignee(&store, "node-a");
    let node_b = worker_actor("node-b");

    store
        .assign_issue(
            issue.id,
            Some(AssignmentType::Worker),
            Some("node-b".to_string()),
            &admin(),
        )
        .unwrap();

    assert!(unread(&store, &node_b) > 0, "新指派人必须收 ASSIGNED 通知");
    let subs = store.list_subscribers(issue.id).unwrap();
    assert!(subs.iter().any(|s| s.subscriber.id == "node-b"));
    // 自指派（操作者=被指派人）不发通知。
    let before = unread(&store, &admin());
    store
        .assign_issue(
            issue.id,
            Some(AssignmentType::ManagerSelf),
            Some("admin".to_string()),
            &admin(),
        )
        .unwrap();
    assert_eq!(unread(&store, &admin()), before, "自己指派自己不通知");
}

/// move_issue 带状态变化 → 走通知道（800-815）；纯挪位 → 只记
/// reordered 活动（825）。
#[test]
fn move_issue_status_change_and_reorder_arms() {
    let store = temp_store("move");
    let issue = issue_with_assignee(&store, "node-a");
    let assignee = worker_actor("node-a");

    // 状态变化移动：Backlog → InProgress（带指派 → 通知）。
    store
        .move_issue(issue.id, IssueStatus::InProgress, 5, &admin())
        .unwrap();
    assert!(unread(&store, &assignee) > 0);

    // 同状态纯挪位：reordered 活动，无新通知。
    let before = unread(&store, &assignee);
    store
        .move_issue(issue.id, IssueStatus::InProgress, 9, &admin())
        .unwrap();
    assert_eq!(unread(&store, &assignee), before);
    let acts = store.list_activity(issue.id).unwrap();
    assert!(acts.iter().any(|a| a.action == "reordered"));
}

// ---------------------------------------------------------------------------
// 回滚
// ---------------------------------------------------------------------------

/// done 单据终态回滚（1072-1080）：done → true + System 评论；非 done →
/// false；审计回滚（1020-1031）：auto_decide 活动撤销 → 退回 in_review。
#[test]
fn done_rollback_and_decision_rollback() {
    let store = temp_store("rollback");
    let issue = issue_with_assignee(&store, "node-a");
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &admin())
        .unwrap();

    // 非 done → Ok(false)。
    assert!(!store.rollback_done_to_in_review(issue.id, "预热").unwrap());

    store
        .transition_issue(issue.id, IssueStatus::Done, &admin())
        .unwrap();
    assert!(
        store
            .rollback_done_to_in_review(issue.id, "发现漏测")
            .unwrap()
    );
    let rolled = store.get_issue(issue.id).unwrap();
    assert_eq!(rolled.status, IssueStatus::InReview);
    // 二次回滚：已非 done → false。
    assert!(!store.rollback_done_to_in_review(issue.id, "重复").unwrap());

    // 走完 done 后留 auto_decide 决策活动，再审计回滚。
    store
        .transition_issue(issue.id, IssueStatus::Done, &admin())
        .unwrap();
    store
        .add_activity(
            issue.id,
            &Actor::system("auto-accept"),
            "auto_decide",
            Some("验收通过自动收货"),
        )
        .unwrap();
    let activity = store
        .list_activity(issue.id)
        .unwrap()
        .into_iter()
        .find(|a| a.action == "auto_decide")
        .unwrap();
    let back = store.rollback_decision(activity.id).unwrap();
    assert_eq!(back.status, IssueStatus::InReview);
}

/// 非 auto_decide 活动不可回滚（对照）。
#[test]
fn rollback_rejects_non_auto_decide_activity() {
    let store = temp_store("rollback-guard");
    let issue = issue_with_assignee(&store, "node-a");
    store
        .add_activity(issue.id, &admin(), "commented", None)
        .unwrap();
    let activity = store
        .list_activity(issue.id)
        .unwrap()
        .into_iter()
        .find(|a| a.action == "commented")
        .unwrap();
    assert!(store.rollback_decision(activity.id).is_err());
}

// ---------------------------------------------------------------------------
// 项目 pending merge / 通知 / 标签
// ---------------------------------------------------------------------------

/// append_pending_merge 同 task_id 去重早退（1279）。
#[test]
fn append_pending_merge_dedupes_by_task_id() {
    let store = temp_store("merge-dedup");
    let project = store
        .create_project("合并", "", None, "", "", None)
        .unwrap();
    let entry = |reason: &str| PendingMerge {
        task_id: "task-1".to_string(),
        issue_id: 1,
        placement_dir: "changeset".to_string(),
        reason: reason.to_string(),
        parked_at_ms: 1,
    };
    store
        .append_pending_merge(project.id, entry("第一次"))
        .unwrap();
    store
        .append_pending_merge(project.id, entry("重复"))
        .unwrap();
    let first = store.pop_pending_merge(project.id).unwrap().unwrap();
    assert_eq!(first.task_id, "task-1");
    assert_eq!(first.reason, "第一次", "重复登记不得顶掉先入队者");
    assert!(store.pop_pending_merge(project.id).unwrap().is_none());
}

/// notify_dispatch_event 的指派对象并集臂（1419-1426）。
#[test]
fn dispatch_event_reaches_assignee() {
    let store = temp_store("dispatch-event");
    let issue = issue_with_assignee(&store, "node-a");
    let assignee = worker_actor("node-a");

    store
        .notify_dispatch_event(issue.id, notification_kind::STATUS_CHANGED, "派发失败")
        .unwrap();
    assert!(unread(&store, &assignee) > 0, "指派对象必须收到派发事件");
}

/// mark_all_notifications_read / unread_notification_count 的 None 形态
/// （1498-1504 / 1523-1530）。
#[test]
fn mark_all_and_unread_without_recipient_filter() {
    let store = temp_store("notify-all");
    let issue = issue_with_assignee(&store, "node-a");
    let assignee = worker_actor("node-a");
    store
        .notify_dispatch_event(issue.id, "kind-x", "内容")
        .unwrap();
    assert!(unread(&store, &assignee) > 0);

    let kind = assignee.kind.clone();
    let marked = store.mark_all_notifications_read(&kind, None).unwrap();
    assert!(marked >= 1);
    assert_eq!(store.unread_notification_count(&kind, None).unwrap(), 0);
}

/// grant_tags_to_node：空串跳过（1817）+ 大小写不敏感去重后 granted 为空
/// 跳过 set_meta（1828）。
#[test]
fn grant_tags_skips_empty_and_duplicates() {
    let store = temp_store("tags");
    let granted = store
        .grant_tags_to_node(
            "node-a",
            &["qa".to_string(), "".to_string(), "  ".to_string()],
        )
        .unwrap();
    assert_eq!(granted, vec!["qa".to_string()]);

    // 同标签（含大小写变体）再授 → 无新增。
    let again = store
        .grant_tags_to_node("node-a", &["QA".to_string(), "qa".to_string()])
        .unwrap();
    assert!(again.is_empty());
    let map = store.granted_tags_map().unwrap();
    assert_eq!(map.get("node-a").unwrap(), &vec!["qa".to_string()]);
}

// ---------------------------------------------------------------------------
// 派发生命周期
// ---------------------------------------------------------------------------

/// 重平衡候选（1867-1882）+ 取消（1940）+ 失败（1990）+ dispatched 活动。
#[test]
fn dispatch_lifecycle_rebalance_cancel_fail() {
    let store = temp_store("dispatch");
    let issue = issue_with_assignee(&store, "node-a");
    store
        .insert_dispatch("task-1", issue.id, "worker-a", &admin())
        .unwrap();
    // inserted 落 dispatched 活动（3187 的 Ok 收尾）。
    assert!(
        store
            .list_activity(issue.id)
            .unwrap()
            .iter()
            .any(|a| a.action == "dispatched")
    );

    // 重平衡候选：排除自己后可见他人 queued 单。
    let others = store.list_queued_dispatches_excluding("worker-b").unwrap();
    assert!(others.iter().any(|d| d.task_id == "task-1"));
    assert!(
        store
            .list_queued_dispatches_excluding("worker-a")
            .unwrap()
            .is_empty()
    );

    // 取消（DISPATCHED 态可取消）→ cancelled 记录 + 审计活动。
    let cancelled = store.cancel_dispatch("task-1", &admin()).unwrap().unwrap();
    assert_eq!(cancelled.state, dispatch_state::CANCELLED);
    assert!(
        store
            .list_activity(issue.id)
            .unwrap()
            .iter()
            .any(|a| a.action == "dispatch_cancelled")
    );
    // 已终结再取消 → None。
    assert!(store.cancel_dispatch("task-1", &admin()).unwrap().is_none());

    // 失败派发（新行）→ dispatch_timeout/失败活动。
    store
        .insert_dispatch("task-2", issue.id, "worker-a", &admin())
        .unwrap();
    let failed = store.fail_dispatch("task-2", "worker 离线超时").unwrap();
    assert!(failed.is_some());
    assert_ne!(failed.unwrap().state, dispatch_state::DISPATCHED);
}

// ---------------------------------------------------------------------------
// autopilot
// ---------------------------------------------------------------------------

/// update_autopilot 的三个空值拒绝臂（2071-2078）+ 字段变化生效
/// （apply_set_opt 2986-2989）+ last_run 触碰不存在的 id（2166）。
#[test]
fn autopilot_patch_validation_and_partial_update() {
    let store = temp_store("autopilot");
    let ap = store
        .create_autopilot(&crate::models::NewAutopilot {
            name: "每夜巡检".to_string(),
            cron: "0 3 * * *".to_string(),
            title: "巡检".to_string(),
            description: String::new(),
            priority: priority::MEDIUM,
            project_id: None,
            target: String::new(),
            enabled: false,
            auto_plan: false,
            acceptance_criteria: None,
        })
        .unwrap();

    // 空值拒绝三臂。
    let patch = crate::models::AutopilotPatch {
        name: Some("   ".to_string()),
        ..Default::default()
    };
    assert!(store.update_autopilot(ap.id, &patch).is_err());
    let patch = crate::models::AutopilotPatch {
        title: Some(String::new()),
        ..Default::default()
    };
    assert!(store.update_autopilot(ap.id, &patch).is_err());
    let patch = crate::models::AutopilotPatch {
        cron: Some(" ".to_string()),
        ..Default::default()
    };
    assert!(store.update_autopilot(ap.id, &patch).is_err());

    // 字段实际变化 → apply_set_opt 变更臂。
    let patch = crate::models::AutopilotPatch {
        name: Some("改名".to_string()),
        description: Some("新描述".to_string()),
        ..Default::default()
    };
    let updated = store.update_autopilot(ap.id, &patch).unwrap();
    assert_eq!(updated.name, "改名");

    // last_run 触碰不存在的 id。
    assert!(store.mark_autopilot_run(424_242).is_err());
}

// ---------------------------------------------------------------------------
// 频道 / 上行信封
// ---------------------------------------------------------------------------

/// 频道重名拒绝（2254）+ 频道消息落库（2427）+ 幂等预检未认领 None
/// （2504）+ 空内容/空 client_msg_id/未知线程类型拒绝（2381/2384/2443）。
#[test]
fn discussion_envelope_channel_and_guards() {
    let store = temp_store("envelope");
    let channel = store
        .create_channel(NewChannel {
            name: "general".to_string(),
            topic: "闲聊".to_string(),
        })
        .unwrap();
    // 重名。
    let err = store
        .create_channel(NewChannel {
            name: "general".to_string(),
            topic: String::new(),
        })
        .unwrap_err();
    assert!(err.contains("already exists"), "{err}");

    let sender = worker_actor("node-a");
    // 空内容。
    assert!(
        store
            .post_discussion_envelope(
                "node-a",
                "mid-1",
                thread_kind::CHANNEL,
                channel.id,
                &sender,
                "   ",
                None,
                "text",
            )
            .is_err()
    );
    // 空 client_msg_id。
    assert!(
        store
            .post_discussion_envelope(
                "node-a",
                "  ",
                thread_kind::CHANNEL,
                channel.id,
                &sender,
                "hi",
                None,
                "text",
            )
            .is_err()
    );
    // 未知线程类型。
    assert!(
        store
            .post_discussion_envelope(
                "node-a", "mid-2", "wiki", channel.id, &sender, "hi", None, "text",
            )
            .unwrap_err()
            .contains("unknown thread kind")
    );

    // 全新请求预检 → None（2504）。
    assert!(store.check_duplicate("node-a", "mid-9").unwrap().is_none());

    // 频道消息正常落库。
    let posted = store
        .post_discussion_envelope(
            "node-a",
            "mid-3",
            thread_kind::CHANNEL,
            channel.id,
            &sender,
            "大家好",
            None,
            "text",
        )
        .unwrap();
    assert!(posted.is_new);

    // 同 (origin, client_msg_id) 重放 → 幂等非新建返回。
    let replay = store
        .post_discussion_envelope(
            "node-a",
            "mid-3",
            thread_kind::CHANNEL,
            channel.id,
            &sender,
            "大家好",
            None,
            "text",
        )
        .unwrap();
    assert!(!replay.is_new);
    // 幂等预检此刻应命中缓存首响（2504 的 Some 侧）。
    assert!(store.check_duplicate("node-a", "mid-3").unwrap().is_some());
}

// ---------------------------------------------------------------------------
// 评论通知扇出 / 团队经验 / 提及解析
// ---------------------------------------------------------------------------

/// 评论扇出（3064-3091）：@指派人收 mentioned、订阅者（创建者）收
/// commented、作者本人不收；空 @ 形态（@!）不误命中（3246）。
#[test]
fn comment_fanout_mention_and_recipients() {
    let store = temp_store("fanout");
    let issue = issue_with_assignee(&store, "node-a");
    let assignee = worker_actor("node-a");
    let commenter = worker_actor("node-c");

    let before_assignee = unread(&store, &assignee);
    store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: commenter.clone(),
            content: "@node-a 请看一下，@! 不是有效提及".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();

    // 指派人收 mentioned（而非普通 commented）。
    let notes = store
        .list_notifications(&assignee.kind, Some(&assignee.id), false, 50)
        .unwrap();
    assert!(notes.iter().any(|n| n.kind == notification_kind::MENTIONED));
    assert!(unread(&store, &assignee) > before_assignee);

    // 创建者（订阅者）收 commented。
    let creator_notes = store
        .list_notifications("admin", Some("admin"), false, 50)
        .unwrap();
    assert!(
        creator_notes
            .iter()
            .any(|n| n.kind == notification_kind::COMMENTED)
    );

    // 评论作者本人不收自己的通知。
    assert_eq!(unread(&store, &commenter), 0);
}

/// mark_team_memory_used 空表早退（2894）+ 登记资产等邻近读面（对照）。
#[test]
fn team_memory_used_empty_ids_is_noop() {
    let store = temp_store("tmu");
    store.mark_team_memory_used(&[]).unwrap();
}

/// 登记资产的 NewAsset 形态对照（asset 侧道读面）。
#[test]
fn new_asset_shape() {
    let a = NewAsset {
        ref_name: "report.md".to_string(),
        origin_issue: None,
        sha256: "ab".repeat(32),
        size: 12,
    };
    assert_eq!(a.size, 12);
}

// ===========================================================================
// Wave4 覆盖批次（2026-09-25）：set_asset_signing/asset_signing 注入面、
// last_system_comment、list_dispatch_park_candidates、get_meta。
// ===========================================================================

/// dispatch 签发上下文：未注入 → None；注入后 → Some（首次为准，重复 set 忽略）。
#[test]
fn asset_signing_context_roundtrip() {
    let store = temp_store("signing");
    assert!(store.asset_signing().is_none(), "未注入 = None");

    let ctx = crate::asset_token::AssetSignContext {
        secret: b"k".to_vec(),
        node_url: crate::asset_token::AdvertisedUrl::default(),
        node_id: "node-a".to_string(),
    };
    store.set_asset_signing(ctx);
    let got = store.asset_signing().expect("注入后必 Some");
    assert_eq!(got.secret, b"k".to_vec());
    assert_eq!(got.node_id, "node-a");

    // 重复 set 以首次为准（OnceLock 语义）。
    store.set_asset_signing(crate::asset_token::AssetSignContext {
        secret: b"k2".to_vec(),
        node_url: crate::asset_token::AdvertisedUrl::default(),
        node_id: "node-b".to_string(),
    });
    let again = store.asset_signing().unwrap();
    assert_eq!(again.secret, b"k".to_vec());
    assert_eq!(again.node_id, "node-a");
}

/// last_system_comment：无评论 → None；混合评论取最后一条 system。
#[test]
fn last_system_comment_returns_latest_system_only() {
    let store = temp_store("lastsys");
    let issue = store.create_issue(new_issue("暂缓去重")).unwrap();
    assert_eq!(store.last_system_comment(issue.id).unwrap(), None);

    store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: admin(),
            content: "人写的普通评论".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    assert_eq!(store.last_system_comment(issue.id).unwrap(), None);

    for (content, ctype) in [
        ("系统甲", CommentType::System),
        ("又一条人评论", CommentType::Comment),
        ("系统乙", CommentType::System),
    ] {
        store
            .add_comment(NewComment {
                issue_id: issue.id,
                author: admin(),
                content: content.into(),
                parent_id: None,
                ctype,
            })
            .unwrap();
    }

    assert_eq!(
        store.last_system_comment(issue.id).unwrap().as_deref(),
        Some("系统乙")
    );
}

/// list_dispatch_park_candidates：backlog/todo + planner 来源 或 ⏸ 暂缓
/// 系统评论 → 入选；普通 backlog / 终态 planner → 排除。
#[test]
fn dispatch_park_candidates_filter_by_origin_and_marker() {
    let store = temp_store("park");

    let planner_origin = crate::models::TaskOrigin {
        origin_type: "planner".into(),
        origin_id: "p1".into(),
    };
    let mut planner_issue = new_issue("planner 拆解子单");
    planner_issue.origin = Some(planner_origin);
    let a = store.create_issue(planner_issue).unwrap();

    let b = store.create_issue(new_issue("普通 backlog")).unwrap();

    let mut manual = new_issue("手动单被暂缓过");
    manual.origin = Some(crate::models::TaskOrigin {
        origin_type: "cli".into(),
        origin_id: "c1".into(),
    });
    let c = store.create_issue(manual).unwrap();
    store
        .add_comment(NewComment {
            issue_id: c.id,
            author: admin(),
            content: "⏸ 自动派发暂缓：无可匹配节点".into(),
            parent_id: None,
            ctype: CommentType::System,
        })
        .unwrap();

    // 终态 planner 单不候选。
    let mut done_planner = new_issue("已完成的 planner 单");
    done_planner.origin = Some(crate::models::TaskOrigin {
        origin_type: "planner".into(),
        origin_id: "p2".into(),
    });
    let d = store.create_issue(done_planner).unwrap();
    store
        .transition_issue(d.id, IssueStatus::Done, &admin())
        .unwrap();

    let mut cands = store.list_dispatch_park_candidates().unwrap();
    cands.sort();
    assert!(cands.contains(&a.id), "planner 来源 backlog 必候选");
    assert!(cands.contains(&c.id), "有 ⏸ 暂缓系统评论必候选");
    assert!(!cands.contains(&b.id), "普通 backlog 不候选");
    assert!(!cands.contains(&d.id), "终态不候选");
}

/// get_meta：seeded 前缀在场；缺失键 → None；与 set_meta 往返。
#[test]
fn get_meta_roundtrip_and_missing_key() {
    let store = temp_store("meta");
    assert_eq!(
        store.get_meta("number_prefix").unwrap().as_deref(),
        Some("NB")
    );
    assert_eq!(store.get_meta("no-such-key").unwrap(), None);
    store.set_meta("wave4", "v1").unwrap();
    assert_eq!(store.get_meta("wave4").unwrap().as_deref(), Some("v1"));
}

// ===========================================================================
// wave6 追加：错误路径触发（SQLite trigger RAISE(ABORT) 定点引爆 insert 助手
// → 精确命中调用点 `?` 传播臂）+ 原始 conn 数据手术（列型污染 / 表删除）。
// ===========================================================================

/// 裸连执行 DDL（建/删 trigger；conn 字段对子模块可见）。
fn w6_exec(store: &BoardStore, ddl: &str) {
    store.conn.lock().unwrap().execute_batch(ddl).unwrap();
}

/// 定点引爆：activity_log 插入时对指定 action RAISE(ABORT)。
fn w6_abort_activity(store: &BoardStore, name: &str, action: &str) {
    w6_exec(
        store,
        &format!(
            "CREATE TRIGGER {name} BEFORE INSERT ON activity_log
             WHEN NEW.action = '{action}'
             BEGIN SELECT RAISE(ABORT, 'cov-boom'); END;"
        ),
    );
}

/// 定点引爆：notification 插入时对指定 kind RAISE(ABORT)。
fn w6_abort_notify(store: &BoardStore, name: &str, kind: &str) {
    w6_exec(
        store,
        &format!(
            "CREATE TRIGGER {name} BEFORE INSERT ON notification
             WHEN NEW.kind = '{kind}'
             BEGIN SELECT RAISE(ABORT, 'cov-boom'); END;"
        ),
    );
}

/// create_issue 的「created」活动写入失败 → 整单建单回滚（178）。
#[test]
fn w6_create_issue_created_activity_failure() {
    let store = temp_store("w6-ci-a");
    w6_abort_activity(&store, "t_created", "created");
    assert!(store.create_issue(new_issue("活动写失败")).is_err());
}

/// create_issue 带指派时的「assigned」活动写失败 → 建单失败（188）。
#[test]
fn w6_create_issue_assigned_activity_failure() {
    let store = temp_store("w6-ci-b");
    w6_abort_activity(&store, "t_assigned", "assigned");
    let mut ni = new_issue("带指派建单失败");
    ni.assignee = Some(AssignmentType::Worker);
    ni.assignee_id = Some("w1".into());
    assert!(store.create_issue(ni).is_err());
}

/// transition_issue 的 status_changed 活动写失败 → 转移回滚（574）。
#[test]
fn w6_transition_activity_failure() {
    let store = temp_store("w6-tr-a");
    let i = store.create_issue(new_issue("转移活动失败")).unwrap();
    w6_abort_activity(&store, "t_sc", "status_changed");
    assert!(
        store
            .transition_issue(i.id, IssueStatus::InProgress, &admin())
            .is_err()
    );
}

/// transition_issue 的指派对象通知写失败 → 转移回滚（591）。
#[test]
fn w6_transition_notify_failure() {
    let store = temp_store("w6-tr-b");
    let mut ni = new_issue("转移通知失败");
    ni.assignee = Some(AssignmentType::Worker);
    ni.assignee_id = Some("w1".into());
    let i = store.create_issue(ni).unwrap();
    w6_abort_notify(&store, "t_tn", notification_kind::STATUS_CHANGED);
    assert!(
        store
            .transition_issue(i.id, IssueStatus::InProgress, &admin())
            .is_err()
    );
}

/// reopen_issue 的 system 评论写失败 → 回滚（664）。
#[test]
fn w6_reopen_comment_failure() {
    let store = temp_store("w6-ro-a");
    let i = store.create_issue(new_issue("重开评论失败")).unwrap();
    store
        .transition_issue(i.id, IssueStatus::Cancelled, &admin())
        .unwrap();
    w6_exec(
        &store,
        "CREATE TRIGGER t_rc BEFORE INSERT ON comment
         WHEN NEW.ctype = 'system'
         BEGIN SELECT RAISE(ABORT, 'cov-boom-reopen'); END;",
    );
    // reopen 内部先走 transition_issue（其评论是 status_change，不触发射
    // 击），错误必须来自 reopen 自己的 system 审计评论插入（664）。
    let err = store.reopen_issue(i.id, &admin()).unwrap_err();
    assert!(
        err.contains("cov-boom-reopen"),
        "错误必须来自 reopen 内部的评论插入：{err}"
    );
}

/// 父单已取消时子单不可单独 reopen（617-623，F-U4-3）。
#[test]
fn w6_reopen_child_of_cancelled_parent_rejected() {
    let store = temp_store("w6-ro-b");
    let p = store.create_issue(new_issue("父单")).unwrap();
    let mut ni = new_issue("子单");
    ni.parent_issue_id = Some(p.id);
    let c = store.create_issue(ni).unwrap();
    store
        .transition_issue(p.id, IssueStatus::Cancelled, &admin())
        .unwrap();
    store
        .transition_issue(c.id, IssueStatus::Cancelled, &admin())
        .unwrap();
    let err = store.reopen_issue(c.id, &admin()).unwrap_err();
    assert!(err.contains("父单"), "{err}");
}

/// assign_issue 的 assigned 活动写失败（723）。
#[test]
fn w6_assign_activity_failure() {
    let store = temp_store("w6-as-a");
    let i = store.create_issue(new_issue("指派活动失败")).unwrap();
    w6_abort_activity(&store, "t_aa", "assigned");
    assert!(
        store
            .assign_issue(
                i.id,
                Some(AssignmentType::Worker),
                Some("w1".into()),
                &admin()
            )
            .is_err()
    );
}

/// assign_issue 的指派通知写失败（740）。
#[test]
fn w6_assign_notify_failure() {
    let store = temp_store("w6-as-b");
    let i = store.create_issue(new_issue("指派通知失败")).unwrap();
    w6_abort_notify(&store, "t_an", notification_kind::ASSIGNED);
    assert!(
        store
            .assign_issue(
                i.id,
                Some(AssignmentType::Worker),
                Some("w1".into()),
                &admin()
            )
            .is_err()
    );
}

/// move_issue 跨列：status_changed 活动写失败（800）。
#[test]
fn w6_move_status_activity_failure() {
    let store = temp_store("w6-mv-a");
    let i = store.create_issue(new_issue("拖拽活动失败")).unwrap();
    w6_abort_activity(&store, "t_ma", "status_changed");
    assert!(
        store
            .move_issue(i.id, IssueStatus::InProgress, 0, &admin())
            .is_err()
    );
}

/// move_issue 跨列：指派对象通知写失败（814-815）。
#[test]
fn w6_move_notify_failure() {
    let store = temp_store("w6-mv-b");
    let mut ni = new_issue("拖拽通知失败");
    ni.assignee = Some(AssignmentType::Worker);
    ni.assignee_id = Some("w1".into());
    let i = store.create_issue(ni).unwrap();
    w6_abort_notify(&store, "t_mn", notification_kind::STATUS_CHANGED);
    assert!(
        store
            .move_issue(i.id, IssueStatus::InProgress, 0, &admin())
            .is_err()
    );
}

/// move_issue 同列重排：reordered 活动写失败（825）。
#[test]
fn w6_move_reordered_activity_failure() {
    let store = temp_store("w6-mv-c");
    let i = store.create_issue(new_issue("重排活动失败")).unwrap();
    w6_abort_activity(&store, "t_mr", "reordered");
    assert!(
        store
            .move_issue(i.id, IssueStatus::Backlog, 7, &admin())
            .is_err()
    );
}

/// bulk_archive_cancelled 的审计活动写失败（436）。
#[test]
fn w6_bulk_archive_activity_failure() {
    let store = temp_store("w6-ba");
    let i = store.create_issue(new_issue("清理活动失败")).unwrap();
    store
        .transition_issue(i.id, IssueStatus::Cancelled, &admin())
        .unwrap();
    w6_abort_activity(&store, "t_ba", "issue_bulk_archive");
    assert!(store.bulk_archive_cancelled(&[i.id], &admin()).is_err());
}

/// update_issue 的 updated 活动写失败（527）。
#[test]
fn w6_update_issue_activity_failure() {
    let store = temp_store("w6-up");
    let i = store.create_issue(new_issue("更新活动失败")).unwrap();
    w6_abort_activity(&store, "t_up", "updated");
    let patch = IssuePatch {
        title: Some("改题".into()),
        ..Default::default()
    };
    assert!(store.update_issue(i.id, &patch, &admin()).is_err());
}

/// rollback_decision 的 system 评论写失败（1020）。
#[test]
fn w6_rollback_decision_comment_failure() {
    let store = temp_store("w6-rd-a");
    let i = store.create_issue(new_issue("审计回滚评论失败")).unwrap();
    store
        .transition_issue(i.id, IssueStatus::Done, &admin())
        .unwrap();
    // rollback_decision 只接受 action='auto_decide' 的活动（982 守卫）——
    // 直接种一条合规活动，否则在守卫处早退根本走不到评论插入。
    w6_exec(
        &store,
        &format!(
            "INSERT INTO activity_log (issue_id, actor_type, actor_id, action, details, created_at)
             VALUES ({}, 'system', 'board', 'auto_decide', NULL, strftime('%s','now'));",
            i.id
        ),
    );
    let act_id: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM activity_log WHERE action = 'auto_decide' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    w6_exec(
        &store,
        "CREATE TRIGGER t_rdc BEFORE INSERT ON comment
         BEGIN SELECT RAISE(ABORT, 'cov-boom-rd'); END;",
    );
    let err = store.rollback_decision(act_id).unwrap_err();
    assert!(
        err.contains("cov-boom-rd"),
        "错误必须来自 rollback 内部的评论插入：{err}"
    );
}

/// rollback_decision 的 status_changed 活动写失败（1031）。
#[test]
fn w6_rollback_decision_activity_failure() {
    let store = temp_store("w6-rd-b");
    let i = store.create_issue(new_issue("审计回滚活动失败")).unwrap();
    store
        .transition_issue(i.id, IssueStatus::Done, &admin())
        .unwrap();
    // 同 982 守卫：必须种一条 action='auto_decide' 的合规活动。
    w6_exec(
        &store,
        &format!(
            "INSERT INTO activity_log (issue_id, actor_type, actor_id, action, details, created_at)
             VALUES ({}, 'system', 'board', 'auto_decide', NULL, strftime('%s','now'));",
            i.id
        ),
    );
    let act_id: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT id FROM activity_log WHERE action = 'auto_decide' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    w6_abort_activity(&store, "t_rda", "status_changed");
    let err = store.rollback_decision(act_id).unwrap_err();
    assert!(
        err.contains("cov-boom"),
        "错误必须来自 rollback 内部的活动插入：{err}"
    );
}

/// rollback_done_to_in_review 的 system 评论写失败（1072）。
#[test]
fn w6_rollback_done_comment_failure() {
    let store = temp_store("w6-rw-a");
    let i = store.create_issue(new_issue("终态回滚评论失败")).unwrap();
    store
        .transition_issue(i.id, IssueStatus::Done, &admin())
        .unwrap();
    w6_exec(
        &store,
        "CREATE TRIGGER t_rwc BEFORE INSERT ON comment
         BEGIN SELECT RAISE(ABORT, 'cov-boom'); END;",
    );
    assert!(
        store
            .rollback_done_to_in_review(i.id, "评测锚点未过")
            .is_err()
    );
}

/// rollback_done_to_in_review 的 status_changed 活动写失败（1080）。
#[test]
fn w6_rollback_done_activity_failure() {
    let store = temp_store("w6-rw-b");
    let i = store.create_issue(new_issue("终态回滚活动失败")).unwrap();
    store
        .transition_issue(i.id, IssueStatus::Done, &admin())
        .unwrap();
    w6_abort_activity(&store, "t_rwa", "status_changed");
    assert!(
        store
            .rollback_done_to_in_review(i.id, "评测锚点未过")
            .is_err()
    );
}

/// grant_tags_to_node 的 set_meta 持久化失败（1828）。
#[test]
fn w6_grant_tags_persist_failure() {
    let store = temp_store("w6-gt");
    w6_exec(
        &store,
        "CREATE TRIGGER t_gt BEFORE INSERT ON board_meta
         WHEN NEW.key = 'granted_tags'
         BEGIN SELECT RAISE(ABORT, 'cov-boom'); END;",
    );
    assert!(
        store
            .grant_tags_to_node("node-x", &["rust".into()])
            .is_err()
    );
}

/// cancel_dispatch 的 dispatch_cancelled 活动写失败（1940）。
#[test]
fn w6_cancel_dispatch_activity_failure() {
    let store = temp_store("w6-cd");
    let i = store.create_issue(new_issue("取消派发活动失败")).unwrap();
    store
        .insert_dispatch("task-w6-cd", i.id, "w1", &admin())
        .unwrap();
    w6_abort_activity(&store, "t_cd", "dispatch_cancelled");
    assert!(store.cancel_dispatch("task-w6-cd", &admin()).is_err());
}

/// fail_dispatch 的 dispatch_timeout 活动写失败（1990）。
#[test]
fn w6_fail_dispatch_activity_failure() {
    let store = temp_store("w6-fd");
    let i = store.create_issue(new_issue("超时终结活动失败")).unwrap();
    store
        .insert_dispatch("task-w6-fd", i.id, "w1", &admin())
        .unwrap();
    w6_abort_activity(&store, "t_fd", "dispatch_timeout");
    assert!(store.fail_dispatch("task-w6-fd", "sweep").is_err());
}

/// notify_dispatch_event 收件人装配：创建者不在订阅者 → 补创建者（1419）；
/// 指派不在订阅者 → 补指派（1424-1426）。
#[test]
fn w6_notify_dispatch_recipients_creator_and_assignee() {
    let store = temp_store("w6-nd");
    let i = store.create_issue(new_issue("派发事件通知")).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("DELETE FROM issue_subscriber", []).unwrap();
        conn.execute(
            "UPDATE issue SET assignee_type = 'worker', assignee_id = 'w9' WHERE id = ?1",
            params![i.id],
        )
        .unwrap();
    }
    store
        .notify_dispatch_event(i.id, "w6_failed", "超时")
        .unwrap();
    let n: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM notification", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2, "创建者 + 指派各一条");
}

/// notify_dispatch_event 的通知插入失败（1439）。
#[test]
fn w6_notify_dispatch_insert_failure() {
    let store = temp_store("w6-nd-b");
    let i = store.create_issue(new_issue("派发通知写失败")).unwrap();
    w6_exec(
        &store,
        "CREATE TRIGGER t_nd BEFORE INSERT ON notification
         BEGIN SELECT RAISE(ABORT, 'cov-boom'); END;",
    );
    assert!(
        store
            .notify_dispatch_event(i.id, "w6_failed", "超时")
            .is_err()
    );
}

/// mark_all_notifications_read 单收件人形态（1498-1504）。
#[test]
fn w6_mark_all_notifications_read_single_recipient() {
    let store = temp_store("w6-mr");
    let i = store.create_issue(new_issue("收件箱归零")).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("DELETE FROM issue_subscriber", []).unwrap();
        conn.execute(
            "UPDATE issue SET assignee_type = 'worker', assignee_id = 'w9' WHERE id = ?1",
            params![i.id],
        )
        .unwrap();
    }
    store
        .notify_dispatch_event(i.id, "w6_failed", "超时")
        .unwrap();
    // 精确到 w9：只清 1 条（admin 那条不动）。
    let n = store
        .mark_all_notifications_read("worker", Some("w9"))
        .unwrap();
    assert_eq!(n, 1);
    let unread: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM notification WHERE read = 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unread, 1, "admin 那条仍未读");
}

/// count_by_status 对库外写入的非法 status 诚实报错（2209-2214）。
#[test]
fn w6_count_by_status_rejects_corrupt_status() {
    let store = temp_store("w6-cs");
    store.create_issue(new_issue("脏状态")).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("UPDATE issue SET status = 'junk'", [])
            .unwrap();
    }
    let err = store.count_by_status().unwrap_err();
    assert!(err.contains("unknown status"), "{err}");
}

/// create_channel 非重名类插入失败走 else 臂（2254）。
#[test]
fn w6_create_channel_non_unique_failure() {
    let store = temp_store("w6-cc");
    w6_exec(
        &store,
        "CREATE TRIGGER t_cc BEFORE INSERT ON channel
         BEGIN SELECT RAISE(ABORT, 'w6 channel boom'); END;",
    );
    let err = store
        .create_channel(NewChannel {
            name: "#w6".into(),
            topic: String::new(),
        })
        .unwrap_err();
    assert!(err.contains("w6 channel boom"), "{err}");
    assert!(!err.contains("already exists"), "{err}");
}

/// post_discussion_envelope 落 issue 评论失败 → 上行整体失败（2427）。
#[test]
fn w6_post_envelope_comment_insert_failure() {
    let store = temp_store("w6-pe");
    let i = store.create_issue(new_issue("上行评论失败")).unwrap();
    w6_exec(
        &store,
        "CREATE TRIGGER t_pe BEFORE INSERT ON comment
         BEGIN SELECT RAISE(ABORT, 'cov-boom'); END;",
    );
    assert!(
        store
            .post_discussion_envelope(
                "node-a",
                "m-w6-1",
                thread_kind::ISSUE,
                i.id,
                &worker_actor("w1"),
                "hello",
                None,
                "comment",
            )
            .is_err()
    );
}

/// check_duplicate 缓存列被库外写成非法 UTF-8 → 诚实报错（2504）。
#[test]
fn w6_check_duplicate_invalid_utf8_cache() {
    let store = temp_store("w6-dup");
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO msg_dedup (origin_node, client_msg_id, first_response, created_at)
             VALUES ('node-a', 'm-w6-2', X'FFFE', 1)",
            [],
        )
        .unwrap();
    }
    assert!(store.check_duplicate("node-a", "m-w6-2").is_err());
}

/// add_comment 的 commented 活动写失败（3064）。
#[test]
fn w6_add_comment_activity_failure() {
    let store = temp_store("w6-ac-a");
    let i = store.create_issue(new_issue("评论活动失败")).unwrap();
    w6_abort_activity(&store, "t_aca", "commented");
    assert!(
        store
            .add_comment(NewComment {
                issue_id: i.id,
                author: worker_actor("w1"),
                content: "评论".into(),
                parent_id: None,
                ctype: CommentType::Comment,
            })
            .is_err()
    );
}

/// add_comment 通知装配：指派不在订阅者 → 补进收件人（3074-3075）。
#[test]
fn w6_add_comment_assignee_pushed_into_recipients() {
    let store = temp_store("w6-ac-b");
    let i = store.create_issue(new_issue("评论通知补指派")).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE issue SET assignee_type = 'worker', assignee_id = 'w9' WHERE id = ?1",
            params![i.id],
        )
        .unwrap();
    }
    store
        .add_comment(NewComment {
            issue_id: i.id,
            author: worker_actor("w1"),
            content: "看一下 @w9".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let conn = store.conn.lock().unwrap();
    let mentioned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notification WHERE kind = 'mentioned'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let commented: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notification WHERE kind = 'commented'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mentioned, 1, "@w9 提及一条");
    assert_eq!(commented, 1, "admin（订阅者）评论一条");
}

/// add_comment 的提及通知写失败（3090-3091）。
#[test]
fn w6_add_comment_mention_notify_failure() {
    let store = temp_store("w6-ac-c");
    let i = store.create_issue(new_issue("提及通知失败")).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE issue SET assignee_type = 'worker', assignee_id = 'w9' WHERE id = ?1",
            params![i.id],
        )
        .unwrap();
    }
    w6_abort_notify(&store, "t_mn2", notification_kind::MENTIONED);
    assert!(
        store
            .add_comment(NewComment {
                issue_id: i.id,
                author: worker_actor("w1"),
                content: "看 @w9".into(),
                parent_id: None,
                ctype: CommentType::Comment,
            })
            .is_err()
    );
}

/// add_comment 的评论通知写失败（3107）。
#[test]
fn w6_add_comment_commented_notify_failure() {
    let store = temp_store("w6-ac-d");
    let i = store.create_issue(new_issue("评论通知失败")).unwrap();
    w6_abort_notify(&store, "t_cn", notification_kind::COMMENTED);
    assert!(
        store
            .add_comment(NewComment {
                issue_id: i.id,
                author: worker_actor("w1"),
                content: "普通评论".into(),
                parent_id: None,
                ctype: CommentType::Comment,
            })
            .is_err()
    );
}

/// insert_dispatch_locked 的 dispatched 活动写失败（3187）。
#[test]
fn w6_insert_dispatch_activity_failure() {
    let store = temp_store("w6-id");
    let i = store.create_issue(new_issue("派发活动失败")).unwrap();
    w6_abort_activity(&store, "t_id", "dispatched");
    assert!(
        store
            .insert_dispatch("task-w6-id", i.id, "w1", &admin())
            .is_err()
    );
}

/// extract_mentions：孤立 @ / 纯标点尾缀的 @token 静默忽略（3243 的
/// strip_prefix 失败 continue 臂）。
///
/// 豁免（3246 的 mentioned.is_empty() 臂）：可证死代码——`trim_end_matches`
/// 的保留谓词是「字母数字/`_`/`-`」，而 `@` 本身不满足，任何会被剥出
/// 空尾巴的 token 连 `@` 一起被剥掉（如 `@!!` → `""`），`strip_prefix('@')`
/// 只能得 None 走 3243；`Some("")` 要求 trimmed 恰为 `"@"`，但末位 `@`
/// 必然已被尾剥。该臂在当前实现下不可达，不追。
#[test]
fn w6_extract_mentions_lone_at_token_skipped() {
    let hits = extract_mentions("@ and @!! plain", &[worker_actor("w1")]);
    assert!(hits.is_empty(), "{hits:?}");
}

/// descendants：依赖边查询失败按空处理 + WARN（308-309）——表删除后 BFS 照走。
#[test]
fn w6_descendants_dependents_query_failure_tolerated() {
    let store = temp_store("w6-de-a");
    let p = store.create_issue(new_issue("父")).unwrap();
    let mut ni = new_issue("子");
    ni.parent_issue_id = Some(p.id);
    let c = store.create_issue(ni).unwrap();
    w6_exec(&store, "DROP TABLE issue_dependency;");
    let out = store.descendants(&[p.id], DescendantEdges::CascadeUnion);
    assert_eq!(out.len(), 1, "子单仍经父子边可达");
    assert_eq!(out[0].id, c.id);
}

/// descendants：子单读取失败按空处理 + WARN（315-316）——列型污染。
#[test]
fn w6_descendants_children_query_failure_tolerated() {
    let store = temp_store("w6-de-b");
    let p = store.create_issue(new_issue("父")).unwrap();
    let mut ni = new_issue("子");
    ni.parent_issue_id = Some(p.id);
    store.create_issue(ni).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("UPDATE issue SET priority = 'zz'", [])
            .unwrap();
    }
    let out = store.descendants(&[p.id], DescendantEdges::CascadeUnion);
    assert!(out.is_empty(), "读取失败节点被跳过");
}

/// update_issue 可空列 patch：Some(新值) 落 changes 并生效（2986-2989）。
#[test]
fn w6_update_issue_optional_fields_apply() {
    let store = temp_store("w6-opt");
    let i = store.create_issue(new_issue("可空列更新")).unwrap();
    let patch = IssuePatch {
        due_date: Some(1_800_000_000),
        acceptance_criteria: Some("[CHECK] file:a.txt exists".into()),
        ..Default::default()
    };
    let updated = store.update_issue(i.id, &patch, &admin()).unwrap();
    assert_eq!(updated.due_date, Some(1_800_000_000));
    assert_eq!(
        updated.acceptance_criteria.as_deref(),
        Some("[CHECK] file:a.txt exists")
    );
}
