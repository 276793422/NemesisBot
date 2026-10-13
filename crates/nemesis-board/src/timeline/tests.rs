//! U-C 时间线聚合单元测试（临时 store，零网络零 LLM）。

use super::*;
use crate::assignment::Actor;
use crate::models::{CommentType, NewComment, NewIssue};
use crate::store::BoardStore;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn temp_store(name: &str) -> BoardStore {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-timelinetest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

fn admin() -> Actor {
    Actor::admin("tester")
}

fn issue_at(store: &BoardStore, title: &str) -> crate::models::Issue {
    let ni = NewIssue {
        title: title.to_string(),
        creator: admin(),
        ..NewIssue::default()
    };
    store.create_issue(ni).expect("create issue")
}

#[test]
fn test_timeline_aggregates_all_board_sources_plus_extra() {
    let store = temp_store("agg");
    let t0 = chrono::Utc::now().timestamp();
    let issue = issue_at(&store, "聚合源");
    store
        .add_activity(
            issue.id,
            &admin(),
            "auto_decide",
            Some(r#"{"verdict":"PASS"}"#),
        )
        .expect("activity");
    store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: admin(),
            content: "首行摘要\n第二行全文".into(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .expect("comment");
    store
        .insert_dispatch("task-x1", issue.id, "node-w", &admin())
        .expect("dispatch");

    let extra = vec![TimelineEvent {
        ts: t0 + 30,
        source: "cluster_task",
        kind: "success".into(),
        actor: "node-w".into(),
        summary: "任务结果已存储".into(),
        detail: None,
    }];

    let events = collect_issue_timeline(&store, issue.id, extra);
    let sources: Vec<&str> = events.iter().map(|e| e.source).collect();
    assert!(sources.contains(&"activity"), "sources: {sources:?}");
    assert!(sources.contains(&"comment"));
    assert!(sources.contains(&"dispatch"));
    assert!(sources.contains(&"cluster_task"));

    // 评论：摘要 = 首行，全文进 detail。
    let comment = events.iter().find(|e| e.source == "comment").unwrap();
    assert_eq!(comment.summary, "首行摘要");
    assert_eq!(comment.detail.as_deref(), Some("首行摘要\n第二行全文"));
    // 活动：auto_decide 审计决策天然同源（details 进 detail）。store 自带
    // 的 created 活动（create_issue 落库）同在聚合里——按 kind 定位目标行。
    let activity = events
        .iter()
        .find(|e| e.source == "activity" && e.kind == "auto_decide")
        .unwrap();
    assert!(activity.detail.as_deref().unwrap_or("").contains("PASS"));
    assert!(
        events
            .iter()
            .any(|e| e.source == "activity" && e.kind == "created"),
        "建单活动应入时间线"
    );
    // 派发行：task_id 进 detail。
    let dispatch = events.iter().find(|e| e.source == "dispatch").unwrap();
    assert_eq!(dispatch.detail.as_deref(), Some("task-x1"));
    assert_eq!(dispatch.actor, "node-w");
}

#[test]
fn test_timeline_sorted_by_ts_then_source_rank() {
    let store = temp_store("order");
    // store 写入用当前时钟（不可注入），extra 时刻相对 now 两侧外推——
    // 首尾确定性 + 中段同刻 tie-break 全部可断言且无秒界 flake。
    let t0 = chrono::Utc::now().timestamp();
    let issue = issue_at(&store, "排序");
    store
        .add_activity(issue.id, &admin(), "status_changed", Some("todo→in_review"))
        .expect("activity");
    store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: admin(),
            content: "中段评论".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .expect("comment");
    let extra = vec![
        // 同刻 t0+60：archive(rank 0) 应排在 cluster_task(rank 1) 前。
        TimelineEvent {
            ts: t0 + 60,
            source: "cluster_task",
            kind: "failure".into(),
            actor: "node-w".into(),
            summary: "任务失败".into(),
            detail: None,
        },
        TimelineEvent {
            ts: t0 + 60,
            source: "archive",
            kind: "review".into(),
            actor: String::new(),
            summary: "同刻档案行".into(),
            detail: None,
        },
        // 更早的档案事件：时间序优先于一切分层。
        TimelineEvent {
            ts: t0 - 60,
            source: "archive",
            kind: "dispatch".into(),
            actor: String::new(),
            summary: "更早档案行".into(),
            detail: None,
        },
    ];

    let events = collect_issue_timeline(&store, issue.id, extra);
    let ordered: Vec<(&str, i64)> = events.iter().map(|e| (e.source, e.ts)).collect();
    let (n, last) = (ordered.len(), ordered.len() - 1);
    assert_eq!(ordered[0], ("archive", t0 - 60), "ordered: {ordered:?}");
    assert_eq!(ordered[last - 1], ("archive", t0 + 60));
    assert_eq!(ordered[last], ("cluster_task", t0 + 60));
    // 中段 = store 四行：created（create_issue 落库）+ status_changed（手
    // 动）+ commented（add_comment 顺带落库）三条 activity 按 id 序，同刻
    // rank 下 activity 先于 comment（稳定排序保插入序）。
    let middle = &events[1..n - 2];
    assert_eq!(
        middle
            .iter()
            .map(|e| (e.source, e.kind.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("activity", "created"),
            ("activity", "status_changed"),
            ("activity", "commented"),
            ("comment", "comment"),
        ],
        "middle: {middle:?}"
    );
}

#[test]
fn test_timeline_bad_external_ts_sorts_first_without_panic() {
    let store = temp_store("badts");
    let issue = issue_at(&store, "坏时间戳");
    store
        .add_activity(issue.id, &admin(), "created", None)
        .expect("activity");
    let extra = vec![TimelineEvent {
        ts: parse_ts_to_epoch("not-a-timestamp"),
        source: "archive",
        kind: "review".into(),
        actor: String::new(),
        summary: "坏时间戳档案行".into(),
        detail: None,
    }];
    let events = collect_issue_timeline(&store, issue.id, extra);
    assert_eq!(events.first().unwrap().source, "archive");
    assert_eq!(events.first().unwrap().ts, i64::MIN);
}

#[test]
fn test_parse_ts_to_epoch() {
    let ts = parse_ts_to_epoch("2023-11-14T22:13:20+00:00");
    assert_eq!(ts, 1_700_000_000);
    // 带时区偏移同样归一到 epoch。
    assert_eq!(
        parse_ts_to_epoch("2023-11-15T06:13:20+08:00"),
        1_700_000_000
    );
    assert_eq!(parse_ts_to_epoch("垃圾"), i64::MIN);
    assert_eq!(parse_ts_to_epoch(""), i64::MIN);
}
