//! cov 补测（2026-09-25）：`resolve_autopilot_job` 错误臂 + `fire_board_autopilot`
//! 三臂（停用跳过 / 本地建单 / 配置了 target 但集群缺席拒绝）。
//! 全进程内：BoardStore 落 tempdir sqlite，无网络、无 cron 调度、无弹窗。
//!
//! estop 闸（fire_autopilot 入口 BOARD_ESTOP OnceLock）在单测进程内不装配
//! （「单测不装配零波及」先例）→ 判定为未急停，不影响本组断言。
//!
//! （2026-11-04）：sweep 策略化补测——assisted 零回归臂（超时
//! 失败/回流/让位）+ 自治臂（预算冻结/0=关/冻结类/estop 回落/同节点重派
//! 失败回落/离线无可换节点回落）。**已知边界**：offline Cluster 的
//! rpc_client_arc() 恒 None → dispatch_issue_core 恒 Err「RPC client 未
//! 就绪」→ 自治重派**成功臂**（🔁 评论 + record_auto_decide redispatch）
//! 进程内不可达，归 cluster-uat 真双节点流验证。

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
        session_key: None,
        mode: nemesis_board::models::AUTOPILOT_MODE_LIVE.to_string(),
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
// cov 补测：`sweep_dispatch_timeouts`（派发超时清扫，
// 此前零覆盖）。offline Cluster（不 start()：无 UDP/RPC/线程）；超时判定
// 传 timeout_secs=0 让 age≥0 恒真，免回填时间戳。
// 签名改 deps 四参（autonomous/max_redispatch），
// 既有用例全部转 assisted（现状行为零回归面）。
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

/// sweep 依赖集（BoardReviewDeps 测试构造，照抄 board_review/tests.rs 模式）。
fn mk_deps(
    tmp: &tempfile::TempDir,
    store: &std::sync::Arc<nemesis_board::BoardStore>,
    cluster: &std::sync::Arc<nemesis_cluster::cluster::Cluster>,
) -> crate::board_review::BoardReviewDeps {
    crate::board_review::BoardReviewDeps {
        store: store.clone(),
        workspace: tmp.path().to_path_buf(),
        home: tmp.path().to_path_buf(),
        moderator_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        cluster: cluster.clone(),
        estop: std::sync::Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: crate::board_review::SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    }
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_empty_dispatch_list_is_early_return() {
    let (tmp, store) = temp_store();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    // 空活跃列表 → 早退，无 panic 无副作用。
    sweep_dispatch_timeouts(&deps, 0, false, 2);
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
    let deps = mk_deps(&tmp, &store, &cluster);
    // timeout_secs=0：任何已派发记录立即超时；peer 不在注册表 → offline=false，
    // 走超时失败分支（非离线分支）。assisted（autonomous=false）= 现状行为。
    sweep_dispatch_timeouts(&deps, 0, false, 2);

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
    let deps = mk_deps(&tmp, &store, &cluster);
    // timeout 远大于 age（≈0）且 peer 缺席不算离线 → continue 分支。
    sweep_dispatch_timeouts(&deps, 3600, false, 2);
    let rec = store.get_dispatch("cov-task-2").unwrap().expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::DISPATCHED);
    assert!(rec.completed_at.is_none(), "未超时不得写完结时间");
    assert_eq!(store.list_active_dispatches().unwrap().len(), 1);
}

/// （机制加固）：清扫失败回流验收决策链——in_progress 单
/// 转移 in_review 且 issue_id 进返回列表（调用方据此 spawn 评审）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_failure_flows_back_to_review_chain() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .transition_issue(
            issue_id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    store
        .insert_dispatch(
            "cov-task-m52",
            issue_id,
            "worker-ghost",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, false, 2);

    assert_eq!(for_review, vec![issue_id], "超时失败单必须回流评审链");
    let issue = store.get_issue(issue_id).unwrap();
    assert_eq!(issue.status, nemesis_board::IssueStatus::InReview);
    let rec = store
        .get_dispatch("cov-task-m52")
        .unwrap()
        .expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::FAILED);
}

/// 对照：非 in_progress 单（人工已动/已关闭）不回流——让位人工。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_failure_skips_non_inprogress_issue() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store); // 默认 backlog，不推 in_progress
    store
        .insert_dispatch(
            "cov-task-m52b",
            issue_id,
            "worker-ghost",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, false, 2);

    assert!(
        for_review.is_empty(),
        "非 in_progress 单不得回流：{for_review:?}"
    );
    assert_eq!(
        store.get_issue(issue_id).unwrap().status,
        nemesis_board::IssueStatus::Backlog,
        "单据状态不得被 sweep 动"
    );
}

// ---------------------------------------------------------------------------
// cov 补测：sweep 自治臂。autonomous=true 时
// 失败判定不再一律落终态+回流，改走监察环链（预算冻结 / 冻结类 / 重派 /
// 回落）。进程内 offline Cluster 无 RPC client → 重派恒 Err → 只覆盖
// 决策臂与回落臂；成功臂归 cluster-uat。
// ---------------------------------------------------------------------------

/// 预算冻结夹具：同一 issue 两笔已失败派发 + 一笔活跃派发
/// → 派发轮次 = 行数−1 = 2。
fn seed_budget_fixture(store: &nemesis_board::BoardStore) -> i64 {
    let issue_id = cov_issue(store);
    for (i, task) in ["cov-bud-1", "cov-bud-2"].iter().enumerate() {
        store
            .insert_dispatch(
                task,
                issue_id,
                "worker-a",
                &nemesis_board::Actor::system("cov"),
            )
            .unwrap();
        store
            .fail_dispatch(task, &format!("第 {i} 轮失败"))
            .unwrap();
    }
    store
        .insert_dispatch(
            "cov-bud-3",
            issue_id,
            "worker-a",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    issue_id
}

/// 监察环预算硬防线：轮次 ≥ max_redispatch → 🛑 冻结停车（不回流评审链、
/// 不自动重派）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_budget_exhaustion_freezes_parking() {
    let (tmp, store) = temp_store();
    let issue_id = seed_budget_fixture(&store);
    store
        .transition_issue(
            issue_id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 2);

    // 冻结臂不回流评审链（autonomous 前置 unlimited=true，评审重派环会绕过
    // 监察环硬防线）——in_review 冻结等人工。
    assert!(
        for_review.is_empty(),
        "预算冻结不得回流评审链：{for_review:?}"
    );
    assert_eq!(
        store.get_issue(issue_id).unwrap().status,
        nemesis_board::IssueStatus::InReview,
        "冻结停车必须落 in_review"
    );
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("🛑") && c.content.contains("监察环重派预算耗尽")),
        "必须有预算耗尽冻结评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    let rec = store.get_dispatch("cov-bud-3").unwrap().expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::FAILED);
}

/// max_redispatch=0 = 关自动重派：首笔超时即冻结（0 视作预算即刻耗尽）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_zero_budget_disables_redispatch() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .insert_dispatch(
            "cov-bud-zero",
            issue_id,
            "worker-a",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 0);

    assert!(
        for_review.is_empty(),
        "0=关不得自动重派也不得回流：{for_review:?}"
    );
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("🛑") && c.content.contains("监察环重派预算耗尽")),
        "0 预算必须走冻结臂"
    );
}

/// 冻结类失败分类（unknown_side_effect）：副作用后果不明，不自动重派。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_freeze_class_does_not_redispatch() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .insert_dispatch(
            "cov-fc-1",
            issue_id,
            "worker-a",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    // 模拟 worker 写回失败评论带 fail_class 标记（classify_terminal_failure
    // 同款行格式）。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id,
            author: nemesis_board::Actor::system("worker"),
            content: "交付失败\nfail_class: unknown_side_effect\n副作用无法确认".into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 2);

    assert!(
        for_review.is_empty(),
        "冻结类不得重派也不得回流：{for_review:?}"
    );
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("🛑") && c.content.contains("unknown_side_effect")),
        "冻结类必须有分类名冻结评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 无重派尝试：活跃派发表空（fail 后没有新 insert_dispatch）。
    assert!(store.list_active_dispatches().unwrap().is_empty());
}

/// estop 不变量：急停生效中自治整体回落 assisted（回流评审链，无冻结无重派）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_estop_falls_back_to_assisted() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .transition_issue(
            issue_id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    store
        .insert_dispatch(
            "cov-estop-1",
            issue_id,
            "worker-a",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    deps.estop.trigger();
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 2);

    // 回落 assisted：回流评审链（调用方 spawn 评审，estop 下自会停车）。
    assert_eq!(for_review, vec![issue_id], "estop 下必须回落 assisted 回流");
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        !comments.iter().any(|c| c.content.contains("🛑")),
        "estop 下不得走自治冻结臂"
    );
    assert!(
        comments.iter().any(|c| c.content.contains("⛔")),
        "assisted 落定面（⛔ 评论）必须在"
    );
}

/// 无失败分类的自治超时：同节点瞬态重试 → offline Cluster 无 RPC client
/// → 重派 Err → 回落评审链（重派失败臂）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_timeout_replay_dispatch_error_falls_back_to_review() {
    let (tmp, store) = temp_store();
    let issue_id = cov_issue(&store);
    store
        .transition_issue(
            issue_id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    store
        .insert_dispatch(
            "cov-replay-1",
            issue_id,
            "worker-ghost",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 2);

    // 重派失败（RPC client 未就绪）→ 回落评审链。
    assert_eq!(for_review, vec![issue_id], "重派失败必须回落评审链");
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("🔁") && c.content.contains("回落验收评审链")),
        "必须有重派失败回落评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert_eq!(
        store.get_issue(issue_id).unwrap().status,
        nemesis_board::IssueStatus::InReview
    );
}

/// 离线触发 + 无可换节点：排除离线 worker 后空候选 → 回落评审链。
/// 派发时间回填 −700s（> 600s 离线宽限、< timeout 3600s）→ 纯离线分支。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_offline_no_switch_candidates_falls_back_to_review() {
    let (tmp, store) = temp_store();
    let db = tmp.path().join("board.db");
    let issue_id = cov_issue(&store);
    store
        .transition_issue(
            issue_id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    store
        .insert_dispatch(
            "cov-off-1",
            issue_id,
            "cov-worker",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    // 注册表种 worker 后标离线（地址逐节点独立——占位升级语义不变式）。
    let cluster = offline_cluster(tmp.path());
    cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
        id: "cov-worker".into(),
        name: "cov-worker".into(),
        address: "127.0.0.1:1991".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_cluster::types::NodeRole::Worker,
        category: "development".into(),
        capabilities: Vec::new(),
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    cluster.handle_node_offline("cov-worker", "cov-test");
    // 回填派发时间越过离线宽限期。
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute(
        "UPDATE issue_dispatch SET dispatched_at = dispatched_at - 700 WHERE task_id = ?1",
        ["cov-off-1"],
    )
    .unwrap();
    drop(conn);

    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 3600, true, 2);

    // 离线触发 → 换节点优先 → 排除离线 worker 后空候选 → 回落评审链。
    assert_eq!(for_review, vec![issue_id], "无候选必须回落评审链");
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("worker 离线") && c.content.contains("无可用换节点候选")),
        "必须有离线定性 + 无候选回落评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// cov 补测：B37 人优先槽位（D6 拍板 b 形态）——自治重派前置闸。目标
// worker 满载（在途 ≥ worker_max_inflight，缺省 1）时本 tick 不 fail 不
// 重派（record 保持 active + 去重 ⏸ 评论）。
// ---------------------------------------------------------------------------

/// 满载夹具：同 worker 两笔在途——一笔「别单占用槽」+ 一笔「被清扫对象」。
/// 负载按 fail 后口径（被清扫记录自扣）→ 剩 1 ≥ cap 1 = 满载。
fn seed_inflight_fixture(store: &nemesis_board::BoardStore) -> i64 {
    let other = cov_issue(store);
    store
        .insert_dispatch(
            "cov-full-other",
            other,
            "worker-x",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    let issue_id = cov_issue(store);
    store
        .transition_issue(
            issue_id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    store
        .insert_dispatch(
            "cov-full-1",
            issue_id,
            "worker-x",
            &nemesis_board::Actor::system("cov"),
        )
        .unwrap();
    issue_id
}

/// 满载前置闸：本 tick 不失败不重派（record 保持 active）+ ⏸ 暂缓评论
/// 落单 + 二次 sweep 去重不刷屏。对照既有
/// `sweep_autonomous_timeout_replay_dispatch_error_falls_back_to_review`
/// （单记录独占 worker：自扣后 0 < cap → 闸放行）——那是「自扣防 livelock」
/// 的回归锚。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_target_inflight_full_defers_fail_with_dedup() {
    let (tmp, store) = temp_store();
    let issue_id = seed_inflight_fixture(&store);
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 2);

    assert!(
        for_review.is_empty(),
        "满载 tick 不得失败/回流：{for_review:?}"
    );
    let rec = store.get_dispatch("cov-full-1").unwrap().expect("记录仍在");
    assert_eq!(
        rec.state,
        nemesis_board::models::dispatch_state::DISPATCHED,
        "满载 tick 不得把被清扫记录标失败（worker 可能仍在跑）"
    );
    assert_eq!(
        store.list_active_dispatches().unwrap().len(),
        2,
        "两笔在途都保持 active"
    );
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments.iter().any(|c| {
            c.content.contains("在途派发已达上限") && c.content.contains("自动派发暂缓")
        }),
        "必须有满载暂缓评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains('⛔') || c.content.contains('🛑')),
        "满载 tick 不得落失败/冻结评论，实际：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 二次 sweep（同状态）：同因 ⏸ 只落一条（last_system_comment 去重）。
    sweep_dispatch_timeouts(&deps, 0, true, 2);
    let comments = store.list_comments(issue_id).unwrap();
    let n = comments
        .iter()
        .filter(|c| c.content.contains("在途派发已达上限"))
        .count();
    assert_eq!(n, 1, "同因暂缓评论必须去重，实际 {n} 条");
}

/// cap=0（`board.worker_max_inflight=0` = 不限，旧行为）：闸完全不设防，
/// 超时照常失败 → 同 worker 重试（offline Cluster 无 RPC → 回落评审链）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_autonomous_unlimited_cap_gate_inert() {
    let (tmp, store) = temp_store();
    let issue_id = seed_inflight_fixture(&store);
    // home/config.json 带 board.worker_max_inflight=0（board_flags_from_home
    // 每次现读——世代快照同链，测试直接写盘即可）。
    std::fs::write(
        tmp.path().join("config.json"),
        r#"{"board":{"worker_max_inflight":0}}"#,
    )
    .unwrap();
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, true, 2);

    assert_eq!(
        for_review,
        vec![issue_id],
        "cap=0 不限 → 不设闸，重派失败回落评审链"
    );
    let rec = store.get_dispatch("cov-full-1").unwrap().expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::FAILED);
}

/// assisted（autonomous=false）不设闸：满载也照常失败回流（人工在场模式，
/// 评审链自己处置——D6 让路只作用于无人值守批量面）。
#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn sweep_assisted_mode_skips_inflight_gate() {
    let (tmp, store) = temp_store();
    // 夹具已把本单推 in_progress（回流闸需要）。
    let issue_id = seed_inflight_fixture(&store);
    let cluster = offline_cluster(tmp.path());
    let deps = mk_deps(&tmp, &store, &cluster);
    let for_review = sweep_dispatch_timeouts(&deps, 0, false, 2);

    assert_eq!(
        for_review,
        vec![issue_id],
        "assisted 不设闸：照常回流评审链"
    );
    let rec = store.get_dispatch("cov-full-1").unwrap().expect("记录仍在");
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::FAILED);
}
