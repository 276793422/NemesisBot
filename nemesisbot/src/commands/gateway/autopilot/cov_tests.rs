//! cov 补测（2026-09-25）：`resolve_autopilot_job` 错误臂 + `fire_board_autopilot`
//! 三臂（停用跳过 / 本地建单 / 配置了 target 但集群缺席拒绝）。
//! 全进程内：BoardStore 落 tempdir sqlite，无网络、无 cron 调度、无弹窗。
//!
//! estop 闸（fire_autopilot 入口 BOARD_ESTOP OnceLock）在单测进程内不装配
//! （「单测不装配零波及」先例）→ 判定为未急停，不影响本组断言。

use super::*;

/// tempdir + 独立 sqlite BoardStore（表结构随 open 自建）。
fn temp_store() -> (tempfile::TempDir, std::sync::Arc<nemesis_board::BoardStore>) {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("board.db");
    let store =
        std::sync::Arc::new(nemesis_board::BoardStore::open(&db, "NB").expect("open board store"));
    (tmp, store)
}

fn new_ap(name: &str, target: &str, enabled: bool) -> nemesis_board::NewAutopilot {
    nemesis_board::NewAutopilot {
        name: name.to_string(),
        cron: "0 9 * * *".to_string(),
        title: "cov {date}".to_string(),
        description: String::new(),
        priority: 2,
        project_id: None,
        target: target.to_string(),
        enabled,
        auto_plan: false,
        acceptance_criteria: None,
    }
}

#[cfg(feature = "board")]
#[test]
fn resolve_rejects_bad_job_name() {
    // Ok 值含非 Debug 的 BoardStore → let-else 拆 Err（不能用 unwrap_err）。
    let Err(err) = resolve_autopilot_job("autopilot-12", None) else {
        panic!("无前缀 job 名必须解析失败");
    };
    assert!(err.contains("无法解析"), "{err}");
    assert!(err.contains("autopilot-12"), "{err}");
    let Err(err) = resolve_autopilot_job("board-ap:notanumber", None) else {
        panic!("非数字 id 必须解析失败");
    };
    assert!(err.contains("无法解析"), "{err}");
}

#[cfg(feature = "board")]
#[test]
fn resolve_rejects_missing_store() {
    let Err(err) = resolve_autopilot_job("board-ap:1", None) else {
        panic!("store 缺席必须报错");
    };
    assert!(err.contains("board service not available"), "{err}");
}

#[cfg(feature = "board")]
#[test]
fn resolve_rejects_unknown_autopilot_id() {
    let (_tmp, store) = temp_store();
    let Err(err) = resolve_autopilot_job("board-ap:4242", Some(&store)) else {
        panic!("不存在的 autopilot id 必须报错");
    };
    assert!(err.contains("4242"), "{err}");
    assert!(err.contains("加载失败"), "{err}");
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn fire_disabled_rule_skips_without_side_effects() {
    let (_tmp, store) = temp_store();
    let ap = store
        .create_autopilot(&new_ap("停用规则", "", false))
        .unwrap();
    assert!(!ap.enabled);
    let out =
        fire_board_autopilot(&format!("board-ap:{}", ap.id), Some(&store), None, None).unwrap();
    assert!(out.contains("已停用"), "{out}");
    assert!(out.contains("停用规则"), "{out}");
    // 跳过 = 不建单。
    let issues = store
        .list_issues(&nemesis_board::models::IssueFilter::default())
        .unwrap();
    assert_eq!(issues.len(), 0, "停用规则不得建单");
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn fire_enabled_rule_creates_local_issue() {
    let (_tmp, store) = temp_store();
    let ap = store.create_autopilot(&new_ap("日报", "", true)).unwrap();
    let out =
        fire_board_autopilot(&format!("board-ap:{}", ap.id), Some(&store), None, None).unwrap();
    assert!(out.contains("已触发"), "{out}");
    assert!(out.contains("日报"), "{out}");
    let issues = store
        .list_issues(&nemesis_board::models::IssueFilter::default())
        .unwrap();
    assert_eq!(issues.len(), 1, "触发一次 = 恰一张单");
    assert!(
        issues[0].title.starts_with("cov "),
        "标题模板应渲染（{{date}} 替换），实际：{}",
        issues[0].title
    );
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn fire_with_target_but_no_cluster_is_rejected_before_issue() {
    let (_tmp, store) = temp_store();
    let ap = store
        .create_autopilot(&new_ap("派发规则", "worker-1", true))
        .unwrap();
    let err = fire_board_autopilot(
        &format!("board-ap:{}", ap.id),
        Some(&store),
        None, // 集群缺席
        None,
    )
    .unwrap_err();
    assert!(err.contains("触发失败"), "{err}");
    assert!(err.contains("集群未运行"), "{err}");
    // 建单前拒绝：无孤儿单。
    let issues = store
        .list_issues(&nemesis_board::models::IssueFilter::default())
        .unwrap();
    assert_eq!(issues.len(), 0, "集群缺席时不得留下已建单");
}

// ---------------------------------------------------------------------------
// cov 补测（2026-09-25 第二批）：`sweep_dispatch_timeouts`（派发超时清扫，
// 此前零覆盖）。offline Cluster（不 start()：无 UDP/RPC/线程）；超时判定
// 传 timeout_secs=0 让 age≥0 恒真，免回填时间戳。
// ---------------------------------------------------------------------------

/// 离线 Cluster 构造（with_workspace 只装配不监听；get_peer 查空注册表恒 None）。
fn offline_cluster(
    workspace: &std::path::Path,
) -> std::sync::Arc<nemesis_cluster::cluster::Cluster> {
    std::sync::Arc::new(nemesis_cluster::cluster::Cluster::with_workspace(
        nemesis_cluster::types::ClusterConfig {
            node_id: "cov-master".into(),
            bind_address: "127.0.0.1:0".into(),
            peers: vec![],
            node_name: "cov-master".into(),
        },
        workspace.to_path_buf(),
    ))
}

fn cov_issue(store: &nemesis_board::BoardStore) -> i64 {
    store
        .create_issue(nemesis_board::NewIssue {
            title: "cov sweep 单".into(),
            creator: nemesis_board::Actor::system("cov"),
            ..Default::default()
        })
        .expect("create issue")
        .id
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_empty_dispatch_list_is_early_return() {
    let (tmp, store) = temp_store();
    let cluster = offline_cluster(tmp.path());
    // 空活跃列表 → 早退，无 panic 无副作用。
    sweep_dispatch_timeouts(&store, &cluster, 0);
    assert!(store.list_active_dispatches().unwrap().is_empty());
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_times_out_stale_dispatch_marks_failed_and_comments() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .insert_dispatch(
            "cov-task-1",
            issue_id,
            "worker-ghost",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    // timeout_secs=0：任何已派发记录立即超时；peer 不在注册表 → offline=false，
    // 走超时失败分支（非离线分支）。
    sweep_dispatch_timeouts(&store, &cluster, 0);

    let rec = store.get_dispatch("cov-task-1").unwrap().expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::FAILED);
    assert!(rec.completed_at.is_some(), "失败记录必须有完结时间");
    assert!(store.list_active_dispatches().unwrap().is_empty());
    // ⛔ 系统评论落到原单。
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("派发超时")),
        "必须有超时系统评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_within_timeout_leaves_dispatch_untouched() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .insert_dispatch(
            "cov-task-2",
            issue_id,
            "worker-ghost",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    // timeout 远大于 age（≈0）且 peer 缺席不算离线 → continue 分支。
    sweep_dispatch_timeouts(&store, &cluster, 3600);
    let rec = store.get_dispatch("cov-task-2").unwrap().expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::DISPATCHED);
    assert!(rec.completed_at.is_none(), "未超时不得写完结时间");
    assert_eq!(store.list_active_dispatches().unwrap().len(), 1);
}
