//! Swarm M4：三态处置决策表单测（纯函数，零 I/O）。
//!
//! 评审 I/O 链路（评论/状态转移/重派）由 cluster-uat T28 端到端覆盖；
//! 此处钉死决策表的全部边界组合，防三态语义回归。全自动流转 P1/E3 追加
//! `unlimited`（无限模式）维度用例。

use super::*;

use nemesis_board::ReviewVerdict;

// ---------- decide_review_action ----------

#[test]
fn pass_with_auto_accept_moves_to_done() {
    assert_eq!(
        decide_review_action(ReviewVerdict::Pass, 0, 2, true, false),
        ReviewAction::AutoAccept
    );
}

#[test]
fn pass_without_auto_accept_stays_in_review() {
    // 默认配置（auto_accept=false）：PASS 只出意见不越权。无限模式不改变
    // PASS 语义（收货仍由 auto_accept 单独授权）。
    assert_eq!(
        decide_review_action(ReviewVerdict::Pass, 0, 2, false, false),
        ReviewAction::SuggestManual
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Pass, 9, 2, false, true),
        ReviewAction::SuggestManual
    );
}

#[test]
fn fail_redispatches_while_budget_remains() {
    // round=0（首派失败）与 round=1（一次重派后仍失败）都还剩预算。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 0, 2, false, false),
        ReviewAction::Redispatch
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 1, 2, false, false),
        ReviewAction::Redispatch
    );
}

#[test]
fn fail_at_budget_limit_escalates() {
    // round=2 >= max_redispatch=2 → 保险丝熔断，转人工。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 2, 2, false, false),
        ReviewAction::EscalateHuman
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 3, 2, true, false),
        ReviewAction::EscalateHuman
    );
}

#[test]
fn fail_with_zero_budget_never_redispatches() {
    // max_redispatch=0 = 关闭自动重派（FAIL 直转人工）。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 0, 0, false, false),
        ReviewAction::EscalateHuman
    );
}

#[test]
fn unsure_always_escalates() {
    assert_eq!(
        decide_review_action(ReviewVerdict::Unsure, 0, 2, false, false),
        ReviewAction::EscalateHuman
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Unsure, 5, 2, true, false),
        ReviewAction::EscalateHuman
    );
}

// ---------- 无限模式（board.unlimited_mode，P1/E3）----------

#[test]
fn unlimited_ignores_redispatch_budget() {
    // 预算耗尽（round>=max）甚至 max=0，无限模式下 FAIL 都继续重派。
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 2, 2, false, true),
        ReviewAction::Redispatch
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 42, 2, true, true),
        ReviewAction::Redispatch
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Fail, 0, 0, false, true),
        ReviewAction::Redispatch
    );
}

#[test]
fn unlimited_unsure_redispatches_instead_of_escalating() {
    // 无限模式：UNSURE 带「无法定案」意见继续重派，不转人工。
    assert_eq!(
        decide_review_action(ReviewVerdict::Unsure, 0, 2, false, true),
        ReviewAction::Redispatch
    );
    assert_eq!(
        decide_review_action(ReviewVerdict::Unsure, 7, 2, true, true),
        ReviewAction::Redispatch
    );
}

// ---------- render_reasons ----------

#[test]
fn reasons_render_as_bullet_list() {
    let s = render_reasons(&["自检对照成立".to_string(), "交付物齐全".to_string()]);
    assert!(s.contains("- 自检对照成立"));
    assert!(s.contains("- 交付物齐全"));
}

#[test]
fn empty_reasons_get_honest_note() {
    let s = render_reasons(&[]);
    assert!(s.contains("未给出理由"));
}

// ---------- #0 前置修复回归：AutoAccept → on_issue_settled 补派链 ----------
//
// T1-2 断链回归（全自动流转 P2 交付物 #0）：AutoAccept 收货必须触发
// `on_issue_settled`（父单状态同步 + 依赖子单补派）——修复前
// `transition_issue(done)` 后直接返回，依赖子单停在 backlog 永不派出、
// 父单永不收口。夹具：parent + sub1 + sub2（sub2 依赖 sub1，显式 worker
// 指派绕过匹配器），`auto_accept_and_settle(sub1)` 后同步断言：
//   - sub1 = done
//   - sub2 = in_progress + list_dispatches 非空（补派已发车）
//   - parent = in_progress（首张子单派出联动）

fn settle_deps_with_store(
    name: &str,
    sub2_depends_on_sub1: bool,
) -> (
    BoardReviewDeps,
    nemesis_board::Issue,
    nemesis_board::Issue,
    nemesis_board::Issue,
) {
    use nemesis_board::{Actor, AssignmentType, NewIssue};
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;

    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-review-settle-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();

    // Cluster::new 仅内存态（无 socket，同 board_bus/agent_factory 测试）；
    // dispatch_issue_core 过 rpc_client_arc() 闸需要 Some——注入空客户端
    // （无 resolver，spawn 出的真实 RPC 送达快速失败 → 派发行标 FAILED +
    // 系统评论，均不影响下方同步断言）。
    let cluster = Arc::new(Cluster::new(ClusterConfig {
        node_id: "node-a".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    }));
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    // F1（2026-09-30 派发目标校验前置）：dispatch_issue_core 现在在解析
    // 失败处显式拒绝（「未知节点」），注册表为空会让派发/重派折戟。脚手架
    // 注册测试常用的 worker 节点（保持原测试意图：测派发决策链，不是测
    // 未注册目标容忍度）。
    for id in ["node-b", "node-c", "node-d"] {
        // 地址必须逐节点独立：merge_real_node_info 的占位升级语义会把
        // 「同地址旧条目」当占位符移除（真注册表不变式），共用地址会让
        // 先注册的节点被后注册的依次顶掉。
        cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
            id: id.to_string(),
            name: format!("{id}-name"),
            address: format!("127.0.0.1:199{}", id.chars().last().unwrap()),
            rpc_port: 0,
            addresses: Vec::new(),
            role: nemesis_cluster::types::NodeRole::Worker,
            category: "development".to_string(),
            capabilities: Vec::new(),
            tags: Vec::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            runtime_backend: None,
            node_type: "agent".to_string(),
            build: String::new(),
        });
    }

    let creator = Actor::agent("node-a");
    let parent = store
        .create_issue(NewIssue {
            title: "回归父单".to_string(),
            description: "P2 #0 回归夹具".to_string(),
            priority: 2,
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let sub1 = store
        .create_issue(NewIssue {
            title: "回归子单1".to_string(),
            description: String::new(),
            priority: 2,
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    let sub2 = store
        .create_issue(NewIssue {
            title: "回归子单2".to_string(),
            description: String::new(),
            priority: 2,
            assignee: Some(AssignmentType::Worker),
            assignee_id: Some("node-b".to_string()),
            creator,
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    if sub2_depends_on_sub1 {
        store.set_issue_dependencies(sub2.id, &[sub1.id]).unwrap();
    }

    let deps = BoardReviewDeps {
        store,
        workspace: dir.clone(),
        home: dir,
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    };
    (deps, parent, sub1, sub2)
}

#[tokio::test]
async fn auto_accept_triggers_on_issue_settled_chain() {
    use nemesis_board::IssueStatus;

    let (deps, parent, sub1, sub2) = settle_deps_with_store("chain", true);

    auto_accept_and_settle(&deps, sub1.id, &["自检对照成立".to_string()])
        .expect("auto_accept_and_settle 必须成功");

    assert_eq!(
        deps.store.get_issue(sub1.id).unwrap().status,
        IssueStatus::Done,
        "收货子单应落 done"
    );
    assert_eq!(
        deps.store.get_issue(sub2.id).unwrap().status,
        IssueStatus::InProgress,
        "T1-2 断链回归：依赖子单必须被补派（backlog → in_progress）"
    );
    assert!(
        !deps.store.list_dispatches(sub2.id).unwrap().is_empty(),
        "补派子单应有派发登记行"
    );
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InProgress,
        "首张子单派出后父单应联动 in_progress"
    );
}

#[tokio::test]
async fn auto_accept_settle_spares_unrelated_sub() {
    use nemesis_board::IssueStatus;

    // 无依赖边的兄弟子单：settle 联动只补派 dependents，不得误派/误收。
    let (deps, parent, sub1, sub2) = settle_deps_with_store("nodep", false);

    auto_accept_and_settle(&deps, sub1.id, &[]).expect("auto_accept_and_settle 必须成功");

    assert_eq!(
        deps.store.get_issue(sub1.id).unwrap().status,
        IssueStatus::Done
    );
    assert_eq!(
        deps.store.get_issue(sub2.id).unwrap().status,
        IssueStatus::Backlog,
        "无依赖边的兄弟子单不得被误派"
    );
    assert!(
        deps.store.list_dispatches(sub2.id).unwrap().is_empty(),
        "无依赖边的兄弟子单不应有派发行"
    );
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::Backlog,
        "子单未全部落定时父单不得联动"
    );
}

// ---------- P2（B1）锚点双检两臂：短路 FAIL / 全过进语义项 ----------
//
// 夹具刻意不带模型（moderator_loop 留空）：短路臂必须不触碰 LLM 照样闭环；
// 语义项在 loop 闸处诚实跳过（Ok(false)）。LLM 臂本体由 cluster-uat T29
// 端到端覆盖。

/// 验收测试夹具：无 config.json（embedded defaults：auto_review=true、
/// max_redispatch=2、auto_accept=false、unlimited=false），workspace
/// 目录独立（锚点路径安全边界），Cluster 注入空 RpcClient 让重派发车。
fn review_deps(name: &str) -> (BoardReviewDeps, std::path::PathBuf) {
    review_deps_ex(name, true)
}

/// `seed_worker_nodes=false` = 注册表留空（d3 无候选回落等测试的前提就是
/// 「在线无他人」；其余测试默认 true 保持派发决策链可走通）。
fn review_deps_ex(name: &str, seed_worker_nodes: bool) -> (BoardReviewDeps, std::path::PathBuf) {
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;

    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-review-anchors-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();

    let cluster = Arc::new(Cluster::new(ClusterConfig {
        node_id: "node-a".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    }));
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    // F1（2026-09-30 派发目标校验前置）：dispatch_issue_core 现在在解析
    // 失败处显式拒绝（「未知节点」），注册表为空会让派发/重派折戟。脚手架
    // 注册测试常用的 worker 节点（保持原测试意图：测派发决策链，不是测
    // 未注册目标容忍度）。
    if !seed_worker_nodes {
        let deps = BoardReviewDeps {
            store,
            workspace: workspace.clone(),
            home: dir,
            moderator_loop: Arc::new(std::sync::OnceLock::new()),
            cluster,
            estop: Arc::new(nemesis_agent::estop::EstopState::new()),
            estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
            selfcheck: SelfcheckRegistry::new(),
            #[cfg(feature = "security")]
            security_slot: None,
        };
        return (deps, workspace);
    }
    for id in ["node-b", "node-c", "node-d"] {
        // 地址必须逐节点独立：merge_real_node_info 的占位升级语义会把
        // 「同地址旧条目」当占位符移除（真注册表不变式），共用地址会让
        // 先注册的节点被后注册的依次顶掉。
        cluster.merge_real_node_info(&nemesis_cluster::cluster::RealNodeInfo {
            id: id.to_string(),
            name: format!("{id}-name"),
            address: format!("127.0.0.1:199{}", id.chars().last().unwrap()),
            rpc_port: 0,
            addresses: Vec::new(),
            role: nemesis_cluster::types::NodeRole::Worker,
            category: "development".to_string(),
            capabilities: Vec::new(),
            tags: Vec::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            runtime_backend: None,
            node_type: "agent".to_string(),
            build: String::new(),
        });
    }

    let deps = BoardReviewDeps {
        store,
        workspace: workspace.clone(),
        home: dir,
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    };
    (deps, workspace)
}

/// G4 交付证据闸夹具（2026-10-18 批六）：与 [`review_deps`] 同构，但
/// Cluster 走 `with_workspace`（生产路径）→ tasks.db 在场（deliveries
/// 记账可读写）。`Cluster::new` 的 task_db=None 内存态验证不了「交付记录
/// 在场 → PASS 放行」臂——缺失会退化成夹具缺 db 的巧合而非账本事实。
fn review_deps_taskdb(name: &str) -> (BoardReviewDeps, std::path::PathBuf) {
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;

    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-review-taskdb-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();

    let cluster = Arc::new(Cluster::with_workspace(
        ClusterConfig {
            node_id: "node-a".to_string(),
            bind_address: "127.0.0.1:0".to_string(),
            peers: vec![],
            node_name: String::new(),
        },
        workspace.clone(),
    ));
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    assert!(
        cluster.task_db().is_some(),
        "with_workspace 必须带 tasks.db（生产路径）"
    );

    let deps = BoardReviewDeps {
        store,
        workspace: workspace.clone(),
        home: dir,
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    };
    (deps, workspace)
}

/// 建一个 in_review 的待验收单并落 worker 交付汇报（验收输入）。
fn issue_in_review(
    store: &nemesis_board::BoardStore,
    title: &str,
    acceptance_criteria: &str,
    delivery: &str,
) -> nemesis_board::Issue {
    use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};
    let reviewer = Actor::agent("node-a");
    let issue = store
        .create_issue(NewIssue {
            title: title.to_string(),
            description: String::new(),
            priority: 2,
            acceptance_criteria: Some(acceptance_criteria.to_string()),
            creator: reviewer.clone(),
            ..Default::default()
        })
        .unwrap();
    // backlog → in_progress → in_review（状态机两跳）。
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    store
        .transition_issue(issue.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::agent("node-b"),
            content: delivery.to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();
    issue
}

#[tokio::test]
async fn anchor_fail_short_circuits_llm_and_redispatches() {
    use nemesis_board::IssueStatus;

    let (deps, ws) = review_deps("anchor-fail");
    // 失败锚点用 re: 形态（交付文本不含该标志 → FAIL）。P1 拓扑硬闸
    //（2026-09-12）拒绝 远端目标 + file: 锚点 派发，重派臂要真发车，
    // 锚点集必须对远端合法。
    let issue = issue_in_review(
        &deps.store,
        "锚点失败短路",
        "[CHECK] re:验收完成标志XYZ\n[CHECK] re:交付完成",
        "## 结论\n交付完成",
    );
    assert!(!ws.join("out").exists());
    // 一条已完结的历史派发（worker 回报后才有验收；round = 1-1 = 0 <
    // max_redispatch=2 → FAIL 走重派臂）。must be 终态——活跃派发会被
    // dispatch_issue_core 的重复派发闸拒绝。
    deps.store
        .insert_dispatch(
            "task-anchor-1",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-anchor-1", nemesis_board::models::dispatch_state::DONE)
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("锚点短路 FAIL 必须闭环不报错");
    assert!(reviewed, "短路 FAIL 应完成评审闭环（非跳过）");

    // 明细评论落库：差距段 + 失败锚点原文可读。
    let comments = deps.store.list_comments(issue.id).unwrap();
    let fail = comments
        .iter()
        .find(|c| c.content.contains("客观锚点检查失败"))
        .expect("必须有锚点失败评论（短路产物）");
    assert!(
        fail.content.contains("验收完成标志XYZ"),
        "明细须含失败锚点目标: {}",
        fail.content
    );
    // 短路 = 未触 LLM（loop 为空也没炸）且 FAIL 照常走重派臂发车。
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress,
        "锚点 FAIL 语义 = ReviewVerdict::Fail，重派臂应发车"
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 2);
}

#[tokio::test]
async fn anchor_pass_falls_through_to_semantic_gate() {
    use nemesis_board::IssueStatus;

    let (deps, ws) = review_deps("anchor-pass");
    std::fs::create_dir_all(ws.join("out")).unwrap();
    std::fs::write(ws.join("out").join("done.md"), "包含验收要点").unwrap();
    let issue = issue_in_review(
        &deps.store,
        "锚点全过进语义",
        "[CHECK] file:out/done.md contains:验收要点\n[CHECK] re:交付完成",
        "## 结论\n交付完成",
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .unwrap();
    // 锚点全过 → 进 LLM 语义项；loop 未装配 → 诚实跳过（非短路闭环）。
    assert!(
        !reviewed,
        "锚点全过后应进语义项（loop 缺位 = Ok(false) 跳过，不是 Ok(true) 闭环）"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "锚点全过不得落失败评论"
    );
    // PASS 摘要评论（T2-1：验收评论含锚点摘要，确定性审计痕迹）。
    let pass = comments
        .iter()
        .find(|c| c.content.contains("客观锚点检查通过"))
        .expect("锚点全过必须落 PASS 摘要评论");
    assert!(
        pass.content.contains("文件包含「验收要点」") && pass.content.contains("（2 条）"),
        "PASS 摘要须含 per-anchor 明细与计数: {}",
        pass.content
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "未短路未评审：状态不得被锚点层改动"
    );
}

// 任务卡回显不是交付（2026-09-12 UAT T30 回归锁）：回显评论（含任务卡头）
// 即便锚点行剥离后，上轮失败明细引文散文里的裸词仍会二阶自命中——评审
// 取文本必须整条排除任务卡回显，回显 worker 诚实 FAIL 走重派臂。
#[tokio::test]
async fn task_card_echo_is_not_delivery_evidence_for_anchors() {
    use nemesis_board::IssueStatus;

    let (deps, _ws) = review_deps("card-echo");
    // 回显形态 = 重派任务卡原样抄回：验收标准引文（锚点行原文）+ 上轮
    // 失败明细引文（散文含裸 needle）。若回显未被排除，锚点行剥离后裸词
    // 仍命中 → 假 PASS；排除后 worker_report 为空 → 诚实 FAIL。
    let echo = "# 看板任务 NB-21\n\n## 标题\n回显探针 · 子任务1\n\n## 验收标准\n交付说明文本。\n[CHECK] re:回显探针NEEDLE9\n\n## 上轮验收意见（本次重派原因，必须针对性整改）\n客观锚点检查失败（确定性核验，未进入 AI 语义评审）：\n- ❌ `[CHECK] re:回显探针NEEDLE9`（交付文本正则）：交付文本未命中 /回显探针NEEDLE9/\n";
    let issue = issue_in_review(
        &deps.store,
        "任务卡回显不是交付",
        "[CHECK] re:回显探针NEEDLE9",
        echo,
    );
    // 终态历史派发（round = 1-1 = 0 < max_redispatch → FAIL 走重派臂）。
    deps.store
        .insert_dispatch(
            "task-echo-1",
            issue.id,
            "node-c",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-echo-1", nemesis_board::models::dispatch_state::DONE)
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("回显评审必须闭环不报错");
    assert!(reviewed, "锚点 FAIL 短路应完成评审闭环");

    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "回显不得满足锚点：必须落失败评论"
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查通过")),
        "回显二阶自命中被堵后不得出现 PASS 摘要"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress,
        "锚点 FAIL = 重派臂发车"
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 2);
}

#[tokio::test]
async fn parent_anchor_fail_blocks_auto_close_without_llm() {
    use nemesis_board::IssueStatus;

    let (deps, _ws) = review_deps("parent-anchor-fail");
    // 父单收口臂要求 auto_close_parent=true（BoardFlagConfig 全字段
    // serde(default)，最小配置即可）。parent_remediation_max=0：本测钉
    // 「锚点 FAIL → 转人工」语义（默认开时带差距 FAIL 先走自动返工承接）。
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true,"review":{"parent_remediation_max":0}}}"#,
    )
    .unwrap();
    let parent = issue_in_review(
        &deps.store,
        "父单锚点失败短路",
        "[CHECK] file:reports/summary.md exists",
        "（父单验收输入=子单汇总）",
    );

    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    assert!(reviewed, "父单锚点短路 FAIL 应闭环");
    let comments = deps.store.list_comments(parent.id).unwrap();
    let fail = comments
        .iter()
        .find(|c| c.content.contains("客观锚点检查失败"))
        .expect("父单必须有锚点失败评论");
    assert!(fail.content.contains("reports/summary.md"));
    // 父单不派发、无重派承接单位：FAIL = 转人工保持 in_review。
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview,
        "父单锚点 FAIL 转人工（不派发不重派）"
    );
}

#[tokio::test]
async fn malicious_anchor_falls_back_with_warning_comment() {
    use nemesis_board::IssueStatus;

    // T2-3：路径形态不安全的锚点行（`..` 穿越）= 解析期拒绝 → 回落语义
    // 项 + 系统告警评论，不短路 FAIL（标准自身的毛病不惩罚执行者）。剩余
    // 合法锚点全过 → 进语义项；loop 缺位 → 诚实跳过。
    let (deps, ws) = review_deps("anchor-malicious");
    std::fs::create_dir_all(ws.join("out")).unwrap();
    std::fs::write(ws.join("out").join("ok.md"), "ok").unwrap();
    let issue = issue_in_review(
        &deps.store,
        "恶意锚点回落语义",
        "[CHECK] file:../outside.txt exists\n[CHECK] file:out/ok.md exists",
        "交付完成",
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .unwrap();
    assert!(
        !reviewed,
        "不安全行被剥离后剩余合法锚点全过 → 进语义项（loop 缺位跳过）"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    let warn = comments
        .iter()
        .find(|c| c.content.contains("不安全/非法锚点行"))
        .expect("必须有锚点告警评论（T2-3）");
    assert!(
        warn.content.contains("../outside.txt"),
        "告警须含被拒锚点原文: {}",
        warn.content
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "不安全锚点不得短路 FAIL（回落语义，非执行者责任）"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "状态不得被锚点层改动"
    );
}

#[tokio::test]
async fn parent_malicious_anchor_warns_without_fail() {
    use nemesis_board::IssueStatus;

    // 父单同款（横向对称）：不安全锚点行 → 告警评论 + 不短路 FAIL。
    let (deps, ws) = review_deps("parent-anchor-malicious");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("reports")).unwrap();
    std::fs::write(ws.join("reports").join("summary.md"), "汇总完成").unwrap();
    let parent = issue_in_review(
        &deps.store,
        "父单恶意锚点回落语义",
        "[CHECK] file:C:\\Windows\\system32\\config exists\n[CHECK] file:reports/summary.md exists",
        "（父单验收输入=子单汇总）",
    );

    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    assert!(!reviewed, "剩余合法锚点全过 → 进语义项（loop 缺位跳过）");
    let comments = deps.store.list_comments(parent.id).unwrap();
    let warn = comments
        .iter()
        .find(|c| c.content.contains("不安全/非法锚点行"))
        .expect("父单必须有锚点告警评论");
    assert!(warn.content.contains("C:\\Windows"));
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "父单不安全锚点不得短路 FAIL"
    );
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview
    );
}

#[tokio::test]
async fn parent_anchor_pass_falls_through_to_semantic_gate() {
    use nemesis_board::IssueStatus;

    let (deps, ws) = review_deps("parent-anchor-pass");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("reports")).unwrap();
    std::fs::write(ws.join("reports").join("summary.md"), "汇总完成").unwrap();
    let parent = issue_in_review(
        &deps.store,
        "父单锚点全过进语义",
        "[CHECK] file:reports/summary.md exists",
        "（父单验收输入=子单汇总）",
    );

    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    // 锚点全过 → 进 LLM 语义项；loop 缺位 → 诚实跳过，保持 in_review。
    assert!(!reviewed, "父单锚点全过后应进语义项（loop 缺位跳过）");
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview
    );
    let comments = deps.store.list_comments(parent.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "父单锚点全过不得落失败评论"
    );
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查通过")),
        "父单锚点全过必须落 PASS 摘要评论"
    );
}

// ---------------------------------------------------------------------------
// 全自动流转 P4：B2a 工具模式 / D3 换节点重派 / E1 预算保险丝 / B2b 取证
// 提示词与路由表 / estop 三态停车 / F3 项目收口落盘
// ---------------------------------------------------------------------------

// ---------- B2a：pick_review_tool_mode 决策表 ----------

#[test]
fn b2a_tool_mode_only_for_local_worker_with_multi_turns() {
    // 本机 worker + 轮数 >1 → 只读取证（全只读白名单）。
    assert_eq!(
        pick_review_tool_mode(Some("node-a"), "node-a", 3),
        ReviewToolMode::ReadOnly { max_turns: 3 }
    );
    // 远端 worker → EvidenceOnly（evidence-on-demand，2026-10-15）：交付
    // 档案已由系统合并到本机（board-projects/<项目>/），档案围栏工具按
    // 单号解析档案根读到的就是本次交付——旧「工件不在本机一律纯文本」
    // 的张冠李戴顾虑对档案根工具不成立。
    assert_eq!(
        pick_review_tool_mode(Some("node-b"), "node-a", 3),
        ReviewToolMode::EvidenceOnly { max_turns: 3 }
    );
    // 无派发历史（如人工转入 in_review）→ 纯文本（零派发=零档案合并
    // 记录，档案工具无输入，保持历史形态）。
    assert_eq!(
        pick_review_tool_mode(None, "node-a", 3),
        ReviewToolMode::NoTools
    );
    // max_turns=1：与历史行为字节等价（预算不达标恒 NoTools）。
    assert_eq!(
        pick_review_tool_mode(Some("node-a"), "node-a", 1),
        ReviewToolMode::NoTools
    );
    assert_eq!(
        pick_review_tool_mode(Some("node-b"), "node-a", 1),
        ReviewToolMode::NoTools
    );
    // max_turns=0 = 不限取证轮数（2026-10-09 C1 预算耗尽根修②）：本机
    // worker 开只读取证，u32::MAX 为 DetachedOpts 层「实践不限」哨兵
    //（DetachedOpts 的 0 = 回落主配置默认，不能直传）。
    assert_eq!(
        pick_review_tool_mode(Some("node-a"), "node-a", 0),
        ReviewToolMode::ReadOnly {
            max_turns: u32::MAX
        }
    );
    // 不限档不改拓扑分流：远端 worker → EvidenceOnly 不限；无派发历史
    // 仍纯文本。
    assert_eq!(
        pick_review_tool_mode(Some("node-b"), "node-a", 0),
        ReviewToolMode::EvidenceOnly {
            max_turns: u32::MAX
        }
    );
    assert_eq!(
        pick_review_tool_mode(None, "node-a", 0),
        ReviewToolMode::NoTools
    );
}

// ---------- PR4：评审预算感知注入块 ----------

#[test]
fn review_budget_block_matches_tool_mode() {
    // 纯文本单轮：明确「只有这一轮」+ UNSURE 兜底纪律。
    let no_tools = render_review_budget_block(ReviewToolMode::NoTools);
    assert!(no_tools.contains("只有这一轮"), "{no_tools}");
    assert!(no_tools.contains("UNSURE"), "{no_tools}");
    assert!(no_tools.contains("严禁"), "{no_tools}");

    // 有限预算：轮数数字在场 + 收敛纪律。
    let limited = render_review_budget_block(ReviewToolMode::ReadOnly { max_turns: 5 });
    assert!(limited.contains("5 轮"), "{limited}");
    assert!(limited.contains("UNSURE"), "{limited}");
    assert!(limited.contains("只读工具"), "{limited}");

    // 不限哨兵（cfg 0=不限 → u32::MAX）：轮数不限但不许为取证而取证。
    let unlimited = render_review_budget_block(ReviewToolMode::ReadOnly {
        max_turns: u32::MAX,
    });
    assert!(unlimited.contains("轮数不限"), "{unlimited}");
    assert!(unlimited.contains("UNSURE"), "{unlimited}");
    assert!(!unlimited.contains("轮**"), "{unlimited}");

    // EvidenceOnly（evidence-on-demand）：档案取证工具名在场（远端评审员
    // 的取证通路就是它）+ 有限/不限两形态预算纪律。
    let evidence = render_review_budget_block(ReviewToolMode::EvidenceOnly { max_turns: 4 });
    assert!(evidence.contains("board_evidence_read"), "{evidence}");
    assert!(evidence.contains("4 轮"), "{evidence}");
    assert!(evidence.contains("UNSURE"), "{evidence}");
    let evidence_unlimited = render_review_budget_block(ReviewToolMode::EvidenceOnly {
        max_turns: u32::MAX,
    });
    assert!(
        evidence_unlimited.contains("board_evidence_read")
            && evidence_unlimited.contains("轮数不限"),
        "{evidence_unlimited}"
    );

    // 四臂都不泄漏「猜测 PASS」的口子（纪律底线恒在场）。
    for (name, block) in [
        ("no_tools", &no_tools),
        ("limited", &limited),
        ("unlimited", &unlimited),
        ("evidence", &evidence),
        ("evidence_unlimited", &evidence_unlimited),
    ] {
        assert!(
            block.contains("UNSURE") && block.contains("严禁"),
            "{name} 臂必须有 UNSURE + 严禁纪律: {block}"
        );
    }
}

// ---------- evidence-on-demand：未附内容注记两臂 ----------

#[test]
fn unattached_note_gives_tool_pointer_only_when_available() {
    // 有工具（ReadOnly/EvidenceOnly）：指路 board_evidence_read + 参数形态。
    let with_tool = render_unattached_note(3, true);
    assert!(with_tool.contains("3 个文件未附内容"), "{with_tool}");
    assert!(with_tool.contains("board_evidence_read"), "{with_tool}");
    assert!(with_tool.contains("offset/limit"), "{with_tool}");
    // NoTools：保持历史形态（「未附内容」子串是注入块测试的钉子），
    // 不许提工具（没工具还指路=误导）。
    let no_tool = render_unattached_note(2, false);
    assert!(no_tool.contains("未附内容"), "{no_tool}");
    assert!(!no_tool.contains("board_evidence_read"), "{no_tool}");
}

// ---------- D3：pick_redispatch_target 决策表 ----------

/// 给 issue 落一条已完结派发行（D3 历史链构造件）。
fn seed_dispatch(store: &nemesis_board::BoardStore, task: &str, issue_id: i64, worker: &str) {
    store
        .insert_dispatch(
            task,
            issue_id,
            worker,
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    store
        .finish_dispatch(task, nemesis_board::models::dispatch_state::DONE)
        .unwrap();
}

#[tokio::test]
async fn d3_no_history_is_honest_error() {
    let (deps, _ws) = review_deps("d3-no-history");
    let issue = issue_in_review(&deps.store, "无历史", "", "交付");
    let err = pick_redispatch_target(&deps, &issue, &[]).expect_err("无历史派发必须诚实报错");
    assert!(err.contains("无历史派发记录"), "got: {err}");
}

#[tokio::test]
async fn d3_single_dispatch_stays_same_worker() {
    let (deps, _ws) = review_deps("d3-single");
    let issue = issue_in_review(&deps.store, "首败", "", "交付");
    seed_dispatch(&deps.store, "t-d3-1", issue.id, "node-b");
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    let choice = pick_redispatch_target(&deps, &issue, &dispatches).unwrap();
    assert_eq!(choice, RedispatchTargetChoice::Same("node-b".to_string()));
    assert!(!choice.is_switch());
}

#[tokio::test]
async fn d3_two_consecutive_same_switches_to_unused_ranked_peer() {
    let (deps, _ws) = review_deps("d3-switch");
    // 注入一个在线 worker 节点（handle_discovered_node 置 Online；rpc_port=0
    // 会被 G14 闸拒绝，必须给非零端口）。
    deps.cluster.handle_discovered_node(
        "node-b",
        "Node-B",
        vec!["127.0.0.1".to_string()],
        19001,
        "worker",
        "development",
        vec![],
        vec![],
        "standard",
    );
    let issue = issue_in_review(&deps.store, "连败换人", "", "交付");
    seed_dispatch(&deps.store, "t-d3-2", issue.id, "node-c");
    seed_dispatch(&deps.store, "t-d3-3", issue.id, "node-c");
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    let choice = pick_redispatch_target(&deps, &issue, &dispatches).unwrap();
    assert_eq!(choice, RedispatchTargetChoice::Switch("node-b".to_string()));
    assert!(choice.is_switch(), "连续 ≥2 次同 worker 必须换节点");
}

#[tokio::test]
async fn d3_two_consecutive_same_without_candidates_falls_back() {
    let (deps, _ws) = review_deps_ex("d3-fallback", false);
    // 不注入任何在线节点：历史 worker 已是唯一选择 → 回落 Same（WARN 留痕，
    // 不阻塞流程）。
    let issue = issue_in_review(&deps.store, "连败无候选", "", "交付");
    seed_dispatch(&deps.store, "t-d3-4", issue.id, "node-b");
    seed_dispatch(&deps.store, "t-d3-5", issue.id, "node-b");
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    let choice = pick_redispatch_target(&deps, &issue, &dispatches).unwrap();
    assert_eq!(choice, RedispatchTargetChoice::Same("node-b".to_string()));
    // 历史有 node-b/node-c 混派但在线无他人 → 仍回落尾部 worker。
    seed_dispatch(&deps.store, "t-d3-6", issue.id, "node-c");
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    let choice = pick_redispatch_target(&deps, &issue, &dispatches).unwrap();
    assert_eq!(choice, RedispatchTargetChoice::Same("node-c".to_string()));
}

#[tokio::test]
async fn d3_switch_skips_historically_used_workers() {
    let (deps, _ws) = review_deps("d3-skip-used");
    // 两个在线 worker：node-b 已用过，次优必须跳到 node-c。
    for (id, port) in [("node-b", 19002u16), ("node-c", 19003u16)] {
        deps.cluster.handle_discovered_node(
            id,
            id,
            vec!["127.0.0.1".to_string()],
            port,
            "worker",
            "development",
            vec![],
            vec![],
            "standard",
        );
    }
    let issue = issue_in_review(&deps.store, "跳过用过节点", "", "交付");
    seed_dispatch(&deps.store, "t-d3-7", issue.id, "node-b");
    seed_dispatch(&deps.store, "t-d3-8", issue.id, "node-b");
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    let choice = pick_redispatch_target(&deps, &issue, &dispatches).unwrap();
    assert_eq!(choice, RedispatchTargetChoice::Switch("node-c".to_string()));
}

/// file: 锚点单的换节点死锁回归锁（2026-10-08 NB-4 实证）：即使有匹配的
/// 在线远端 worker，连续 ≥2 次同 worker 也必须回落 Same（拓扑约束——远端
/// 候选全被 rank_dispatch_candidates 过滤，Switch 产物必被派发拓扑闸拒绝
/// 成死锁停车），且不得误伤无锚点单的换节点面。
#[tokio::test]
async fn d3_file_anchor_issue_stays_same_despite_online_peers() {
    let (deps, _ws) = review_deps("d3-file-anchor");
    // 在线 worker 存在（旧代码此时会 Switch 出非法远端目标 → 死锁）。
    deps.cluster.handle_discovered_node(
        "node-b",
        "Node-B",
        vec!["127.0.0.1".to_string()],
        19004,
        "worker",
        "development",
        vec![],
        vec![],
        "standard",
    );
    let issue = issue_in_review(
        &deps.store,
        "拓扑受限单",
        "[CHECK] file:deliverables/spec.md exists",
        "交付",
    );
    seed_dispatch(&deps.store, "t-d3-9", issue.id, "node-c");
    seed_dispatch(&deps.store, "t-d3-10", issue.id, "node-c");
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    let choice = pick_redispatch_target(&deps, &issue, &dispatches).unwrap();
    assert_eq!(
        choice,
        RedispatchTargetChoice::Same("node-c".to_string()),
        "file: 锚点单必须维持本节点重派，不得 Switch 远端"
    );
    assert!(!choice.is_switch());
}

// ---------- P34：重派决策量化（worker × 任务类型指纹三档）----------
//
// 纯逻辑（分桶/三档边界/稳定分区）与记账幂等在 nemesis-board
// `fingerprint` 模块单测；此处钉决策表接入的行为面：成功率改变换节点
// 人选 / 评论带档位可审计 / 开关默认关行为与现状字节一致 / 样本不足
// neutral 不插队。双节点真流挂账 cluster-uat（主会话跑）。

/// P34 夹具：node-c 连败 2 轮（锚点短路 FAIL 标准形态）+ 在线候选
/// node-b/node-d（字典序 b 在前 = 无加权时必选 b）；指纹按同款
/// task_type_of 算出（无标签 → 标题分桶，与评审侧记账同一真相源）。
fn p34_deps(name: &str, title: &str) -> (BoardReviewDeps, nemesis_board::Issue, String) {
    let (deps, _ws) = review_deps(name);
    for (id, port) in [("node-b", 19012u16), ("node-d", 19014u16)] {
        deps.cluster.handle_discovered_node(
            id,
            id,
            vec!["127.0.0.1".to_string()],
            port,
            "worker",
            "development",
            vec![],
            vec![],
            "standard",
        );
    }
    let issue = issue_in_review(&deps.store, title, W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, &format!("t-{name}-1"), issue.id, "node-c");
    seed_dispatch(&deps.store, &format!("t-{name}-2"), issue.id, "node-c");
    let task_type = nemesis_board::task_type_of(title, &[]);
    (deps, issue, task_type)
}

#[tokio::test]
async fn p34_fingerprint_prefers_higher_success_rate_and_audits_tier() {
    let (deps, issue, task_type) = p34_deps("p34-prefer", "p34 量化选人标题甲");
    // node-d prefer（3/3）、node-b avoid（0/3）——无加权时字典序 b 在前必选 b。
    for (i, t) in ["a", "b", "c"].iter().enumerate() {
        deps.store
            .record_fingerprint_outcome("node-d", &task_type, &format!("t-p34-pd-{i}-{t}"), true)
            .unwrap();
    }
    for (i, t) in ["a", "b", "c"].iter().enumerate() {
        deps.store
            .record_fingerprint_outcome("node-b", &task_type, &format!("t-p34-pb-{i}-{t}"), false)
            .unwrap();
    }
    write_board_cfg(
        &deps.home,
        serde_json::json!({ "fingerprint_weighting": true, "max_redispatch": 5 }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("加权换节点闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let sw = comments
        .iter()
        .find(|c| c.content.contains("换节点执行"))
        .expect("🔁 换节点评论在场");
    assert!(
        sw.content.contains("→ node-d"),
        "prefer 者被优先选中（非字典序首位 b）: {}",
        sw.content
    );
    assert!(
        sw.content.contains("档位 prefer，成功率 3/3"),
        "决策评论带档位留痕: {}",
        sw.content
    );
    // 发车记账落到新 worker（第 3 条派发 = node-d）。
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    assert_eq!(dispatches.len(), 3);
    assert_eq!(dispatches.last().unwrap().worker_id, "node-d");
    // 本轮评审的 FAIL 也按同一 task_type 记到本轮执行者 node-c 头上
    // （记账写点 = 评审定案，与决策消费同源）。
    assert_eq!(
        deps.store.worker_fingerprint("node-c", &task_type).unwrap(),
        Some((0, 1)),
        "评审 FAIL 记账落到本轮执行 worker"
    );
}

#[tokio::test]
async fn p34_flag_off_keeps_legacy_pick_and_comment() {
    let (deps, issue, task_type) = p34_deps("p34-off", "p34 开关关标题乙");
    // 明明 node-b 全败、node-d 全成——开关关时不看指纹，匹配器字典序优先。
    for (i, t) in ["a", "b", "c"].iter().enumerate() {
        deps.store
            .record_fingerprint_outcome("node-b", &task_type, &format!("t-p34-ob-{i}-{t}"), false)
            .unwrap();
    }
    for (i, t) in ["a", "b", "c"].iter().enumerate() {
        deps.store
            .record_fingerprint_outcome("node-d", &task_type, &format!("t-p34-od-{i}-{t}"), true)
            .unwrap();
    }
    // 不写 fingerprint_weighting（serde 默认 false）。
    write_board_cfg(&deps.home, serde_json::json!({ "max_redispatch": 5 }));

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("开关关闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let sw = comments
        .iter()
        .find(|c| c.content.contains("换节点执行"))
        .expect("🔁 换节点评论在场");
    assert!(
        sw.content.contains("→ node-b") && !sw.content.contains("node-d"),
        "开关关 = 匹配器原始序（现状行为字节一致）: {}",
        sw.content
    );
    assert!(
        !sw.content.contains("档位"),
        "开关关不带档位注记: {}",
        sw.content
    );
}

#[tokio::test]
async fn p34_sample_starved_high_rate_stays_neutral() {
    let (deps, issue, task_type) = p34_deps("p34-neutral", "p34 样本不足标题丙");
    // node-d 2/2（成功率 1.0 但样本 <3 = neutral）——若样本闸失效它会被
    // 当 prefer 插队到 node-b 前；neutral 下保持匹配器原序选 node-b。
    deps.store
        .record_fingerprint_outcome("node-d", &task_type, "t-p34-nd-1", true)
        .unwrap();
    deps.store
        .record_fingerprint_outcome("node-d", &task_type, "t-p34-nd-2", true)
        .unwrap();
    write_board_cfg(
        &deps.home,
        serde_json::json!({ "fingerprint_weighting": true, "max_redispatch": 5 }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("样本不足闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let sw = comments
        .iter()
        .find(|c| c.content.contains("换节点执行"))
        .expect("🔁 换节点评论在场");
    assert!(
        sw.content.contains("→ node-b"),
        "样本 <3 = neutral 不插队: {}",
        sw.content
    );
    // node-b 无任何指纹记录 → 注记诚实注明无历史样本（不虚构 0/0）。
    assert!(
        sw.content.contains("（档位 neutral（无历史样本））"),
        "无样本注记诚实: {}",
        sw.content
    );
    assert_eq!(
        deps.store
            .list_dispatches(issue.id)
            .unwrap()
            .last()
            .unwrap()
            .worker_id,
        "node-b"
    );
}

// ---------- E1：budget_breach 三维矩阵 ----------

#[tokio::test]
async fn e1_subissue_cap_dimension() {
    let (deps, _ws) = review_deps("e1-subs");
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "预算父单".to_string(),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    for i in 0..3 {
        deps.store
            .create_issue(nemesis_board::NewIssue {
                title: format!("子{i}"),
                creator: creator.clone(),
                parent_issue_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
    }

    // 上限 2 < 3 个子单 → 超限，文案带维度名与数值。
    let mut cfg = nemesis_config::BoardFlagConfig::default();
    cfg.budget.max_subissues_per_parent = 2;
    let breach = budget_breach(&deps, &parent, &cfg).expect("3 子单超上限 2 必须报超支");
    assert!(
        breach.contains("max_subissues_per_parent") && breach.contains('3'),
        "got: {breach}"
    );
    // 上限 3（不大于）与 0（该维关闭）→ 不超。
    cfg.budget.max_subissues_per_parent = 3;
    assert!(budget_breach(&deps, &parent, &cfg).is_none());
    cfg.budget.max_subissues_per_parent = 0;
    assert!(budget_breach(&deps, &parent, &cfg).is_none());
}

#[tokio::test]
async fn e1_total_dispatch_dimension_counts_whole_chain() {
    let (deps, _ws) = review_deps("e1-dispatches");
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "派发预算父单".to_string(),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "派发预算子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    // 全链 = 父 2 + 子 1 = 3 行（从子单视角查也应聚合到根链）。
    seed_dispatch(&deps.store, "t-e1-1", parent.id, "node-b");
    seed_dispatch(&deps.store, "t-e1-2", parent.id, "node-b");
    seed_dispatch(&deps.store, "t-e1-3", child.id, "node-c");

    let mut cfg = nemesis_config::BoardFlagConfig::default();
    cfg.budget.max_total_redispatch = 2;
    let breach = budget_breach(&deps, &child, &cfg).expect("全链 3 次超上限 2 必须报超支");
    assert!(
        breach.contains("max_total_redispatch") && breach.contains('3'),
        "got: {breach}"
    );
    // 恰好 3 = 上限（只拦 >）→ 不超；0 = 关闭。
    cfg.budget.max_total_redispatch = 3;
    assert!(budget_breach(&deps, &child, &cfg).is_none());
    cfg.budget.max_total_redispatch = 0;
    assert!(budget_breach(&deps, &child, &cfg).is_none());
}

#[tokio::test]
async fn e1_wall_clock_dimension_and_saturating_arithmetic() {
    let (deps, _ws) = review_deps("e1-wallclock");
    let issue = issue_in_review(&deps.store, "墙钟预算", "", "交付");

    let mut cfg = nemesis_config::BoardFlagConfig::default();
    // 0 = 关闭（无论存活多久）。
    cfg.budget.wall_clock_budget_secs = 0;
    assert!(budget_breach(&deps, &issue, &cfg).is_none());
    // u64::MAX 秒 × 1000 ms 的 saturating_mul 不得 panic/溢出成负数。
    cfg.budget.wall_clock_budget_secs = u64::MAX;
    assert!(
        budget_breach(&deps, &issue, &cfg).is_none(),
        "刚建的单不可能超 u64::MAX 秒"
    );
    // D9-1 单位回归锁：created_at 是 store::now() 的 unix 秒，曾直接从
    // 毫秒 now 里减——差值恒带 ~1.77e12 ms 偏移，1h 预算对刚建的单也
    // 即刻误报超支。真实单位语义下新单不得报 1h 墙钟超支。
    cfg.budget.wall_clock_budget_secs = 3600;
    assert!(
        budget_breach(&deps, &issue, &cfg).is_none(),
        "刚建的单不得报 1h 墙钟超支（created_at 秒/毫秒单位混用回归）"
    );
    // 1 秒预算：等过线 → 超限（文案带秒数）。
    cfg.budget.wall_clock_budget_secs = 1;
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let breach = budget_breach(&deps, &issue, &cfg).expect("存活 >1s 必须报墙钟超支");
    assert!(breach.contains("wall_clock_budget_secs"), "got: {breach}");
}

// ---------- B2b：SelfcheckRegistry 路由表 + 取证提示词 ----------

#[test]
fn selfcheck_registry_routes_task_to_issue() {
    let reg = SelfcheckRegistry::new();
    assert!(!reg.has_inflight(7), "空表不得报在途");
    reg.register("task-x".to_string(), 7);
    assert!(reg.has_inflight(7));
    assert!(!reg.has_inflight(8), "在途判定按 issue_id 精确匹配");
    assert_eq!(reg.take("task-x"), Some(7), "take 应路由到 issue 7");
    assert_eq!(
        reg.take("task-x"),
        None,
        "take 必须消费掉条目（防重复二段验收）"
    );
    assert!(!reg.has_inflight(7), "消费后不得再报在途");
}

#[test]
fn selfcheck_prompt_carries_marker_request_and_ac() {
    use nemesis_board::NewIssue;
    let store = nemesis_board::BoardStore::open(
        &std::env::temp_dir().join(format!("nb-selfcheck-prompt-{}", std::process::id())),
        "NB",
    )
    .unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: "取证提示词".to_string(),
            acceptance_criteria: Some("1. 输出 hello".to_string()),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let prompt = build_selfcheck_prompt(&issue, "请贴出 run.md 全文与执行日志尾部");
    assert!(
        prompt.contains("[取证请求 board_selfcheck:"),
        "marker 必须在: {prompt}"
    );
    assert!(prompt.contains(&issue.number.to_string()));
    assert!(
        prompt.contains("请贴出 run.md 全文与执行日志尾部"),
        "取证指令必须透传"
    );
    assert!(
        prompt.contains("1. 输出 hello"),
        "验收标准必须随行（对照取证）"
    );
    assert!(
        prompt.contains("不要下验收结论"),
        "执行 worker 不得越权下结论"
    );
    // 2026-10-17 契约更新（NB-125 根修）：store 创建时自动生成 AC——
    // 「无 AC」单如今带推导判据，取证 prompt 直接拿它当对照。
    let bare = store
        .create_issue(NewIssue {
            title: "无 AC".to_string(),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let prompt = build_selfcheck_prompt(&bare, "贴日志");
    assert!(
        prompt.contains("（系统自动生成）") && prompt.contains("目标已达成并验证"),
        "创建时生成的 AC 必须随取证 prompt 下发: {prompt}"
    );
    // 真·AC 缺失（存量单形态）仍诚实占位。
    let mut legacy = bare.clone();
    legacy.acceptance_criteria = None;
    let prompt = build_selfcheck_prompt(&legacy, "贴日志");
    assert!(
        prompt.contains("（未提供）"),
        "存量缺 AC 必须占位: {prompt}"
    );
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("nb-selfcheck-prompt-{}", std::process::id())),
    );
}

// ---------- estop：三态停车（Issue / Project 维度入队） ----------

#[tokio::test]
async fn estop_parks_issue_review_with_kind() {
    let (deps, _ws) = review_deps("estop-issue");
    let issue = issue_in_review(&deps.store, "急停停车", "", "交付");
    deps.estop.trigger();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .unwrap();
    assert!(!reviewed, "estop 冻结中不得评审");
    let parked = deps.estop_parked.lock().unwrap().clone();
    assert_eq!(
        parked,
        vec![(ParkedKind::Issue, issue.id)],
        "停车队列必须带 Issue 维度供 release watcher 路由: {parked:?}"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("estop 急停中")),
        "冻结必须落留痕评论"
    );
}

#[tokio::test]
async fn estop_parks_project_review_with_kind() {
    let (deps, _ws) = review_deps("estop-project");
    let pid = deps
        .store
        .create_project("急停项目", "", None, "", "", None)
        .unwrap()
        .id;
    deps.estop.trigger();

    let reviewed = review_project_completion(&deps, pid).await.unwrap();
    assert!(!reviewed, "estop 冻结中不得做项目收口评审");
    let parked = deps.estop_parked.lock().unwrap().clone();
    assert_eq!(
        parked,
        vec![(ParkedKind::Project, pid)],
        "项目收口停车必须带 Project 维度: {parked:?}"
    );
}

// BUG-6 回归锁（2026-10-13）：停车去重键必须是 (kind, issue_id) 而非裸
// issue_id——同一单先走子单评审停车（Issue）、后走父单收口停车（Parent）
// 时，后者不得被前者吞号（吞号 = 该维度永不复评）。
#[tokio::test]
async fn estop_park_dedup_does_not_swallow_cross_kind() {
    let (deps, _ws) = review_deps("estop-cross-kind");
    let parent = issue_in_review(&deps.store, "跨维度停车", "", "交付");
    deps.estop.trigger();

    // 第一检查点：父单也被当普通单评审 → (Issue, id) 入队。
    let reviewed = review_issue(&deps, parent.id, ReviewCtx::first_stage())
        .await
        .unwrap();
    assert!(!reviewed, "estop 冻结中不得评审");

    // 第二检查点：同一单走父单收口 → (Parent, id) 必须独立入队，不得吞。
    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    assert!(!reviewed, "estop 冻结中不得做父单收口评审");

    let parked = deps.estop_parked.lock().unwrap().clone();
    assert!(
        parked.contains(&(ParkedKind::Issue, parent.id))
            && parked.contains(&(ParkedKind::Parent, parent.id)),
        "同单跨维度停车必须两维都在队（BUG-6：裸 issue_id 去重会吞第二维）: {parked:?}"
    );
    // 重复触发同一维度不刷队（去重仍生效）。
    let _ = review_parent_issue(&deps, parent.id).await.unwrap();
    let parked = deps.estop_parked.lock().unwrap().clone();
    assert_eq!(
        parked
            .iter()
            .filter(|&&(k, _)| k == ParkedKind::Parent)
            .count(),
        1,
        "同维度重复停车必须去重: {parked:?}"
    );
}

// ---------- F3：review_project_completion 跳过路径（不触 LLM） ----------

#[tokio::test]
async fn f3_config_gate_skips_project_review() {
    let (deps, _ws) = review_deps("f3-gate-off");
    // 无 config.json：auto_review 默认 true，但 review.auto_close_project
    // 默认 false → 双保险第二闸拦住。
    let pid = deps
        .store
        .create_project("默认关项目", "", None, "", "", None)
        .unwrap()
        .id;
    let reviewed = review_project_completion(&deps, pid).await.unwrap();
    assert!(!reviewed, "auto_close_project 默认关不得自动收口");
    // 显式 auto_review=false 同样拦住（即便 auto_close_project=true）。
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_review":false,"review":{"auto_close_project":true}}}"#,
    )
    .unwrap();
    let reviewed = review_project_completion(&deps, pid).await.unwrap();
    assert!(!reviewed, "auto_review 总闸关时不得自动收口");
}

#[tokio::test]
async fn f3_non_done_parent_and_cancelled_child_skip_review() {
    use nemesis_board::{IssueStatus, NewIssue};

    let (deps, _ws) = review_deps("f3-race-guards");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"review":{"auto_close_project":true}}}"#,
    )
    .unwrap();
    let creator = nemesis_board::Actor::agent("node-a");
    // ① 存在非 done 顶层父单（竞态防御复核）→ 跳过。
    let pid = deps
        .store
        .create_project("竞态项目", "", None, "", "", None)
        .unwrap()
        .id;
    deps.store
        .create_issue(NewIssue {
            title: "未完成父单".to_string(),
            creator: creator.clone(),
            project_id: Some(pid),
            ..Default::default()
        })
        .unwrap();
    let reviewed = review_project_completion(&deps, pid).await.unwrap();
    assert!(!reviewed, "存在非 done 顶层父单必须跳过");
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "active",
        "跳过路径不得动项目状态"
    );

    // ② cancelled 子单 = 范围缺口 → 转人工跳过（即便父单全 done）。
    let pid2 = deps
        .store
        .create_project("缺口项目", "", None, "", "", None)
        .unwrap()
        .id;
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "含缺口父单".to_string(),
            creator: creator.clone(),
            project_id: Some(pid2),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(NewIssue {
            title: "被取消子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            project_id: Some(pid2),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::Done, &creator)
        .unwrap();
    deps.store
        .transition_issue(child.id, IssueStatus::Cancelled, &creator)
        .unwrap();
    let reviewed = review_project_completion(&deps, pid2).await.unwrap();
    assert!(!reviewed, "cancelled 子单=范围缺口必须转人工跳过");
    assert_eq!(deps.store.get_project(pid2).unwrap().status, "active");
}

// ---------- F3：apply_project_review_outcome 三态落盘（不经 LLM） ----------

/// F3 落盘夹具：项目 + 1 个顶层父单（返回三者）。
fn f3_fixture(deps: &BoardReviewDeps, name: &str) -> (i64, nemesis_board::Issue) {
    use nemesis_board::NewIssue;
    let pid = deps
        .store
        .create_project(name, "", None, "", "", None)
        .unwrap()
        .id;
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: format!("{name}父单"),
            creator: nemesis_board::Actor::agent("admin"),
            project_id: Some(pid),
            ..Default::default()
        })
        .unwrap();
    (pid, parent)
}

fn f3_output(verdict: nemesis_board::ReviewVerdict, gap: &str) -> nemesis_board::ReviewOutput {
    nemesis_board::ReviewOutput {
        verdict,
        reasons: vec!["测试理由".to_string()],
        gap: gap.to_string(),
        experience: None,
        need_evidence: None,
        evidence_request: None,
    }
}

#[tokio::test]
async fn f3_pass_completes_project() {
    let (deps, _ws) = review_deps("f3-pass");
    let (pid, parent) = f3_fixture(&deps, "收口通过");
    apply_project_review_outcome(
        &deps,
        &deps.store.get_project(pid).unwrap(),
        &[parent],
        &f3_output(nemesis_board::ReviewVerdict::Pass, ""),
    )
    .unwrap();
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "completed",
        "PASS 必须 completed（auto_close_project 即收口授权）"
    );
}

#[tokio::test]
async fn f3_fail_rolls_back_completed_and_comments_parents() {
    let (deps, _ws) = review_deps("f3-rollback");
    let (pid, parent) = f3_fixture(&deps, "收口回滚");
    // 状态机主链两跳到 completed（active 不可直达 completed）。
    deps.store
        .update_project(
            pid,
            &nemesis_board::ProjectPatch {
                status: Some("in_progress".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    deps.store
        .update_project(
            pid,
            &nemesis_board::ProjectPatch {
                status: Some("completed".to_string()),
                ..Default::default()
            },
        )
        .unwrap();

    apply_project_review_outcome(
        &deps,
        &deps.store.get_project(pid).unwrap(),
        std::slice::from_ref(&parent),
        &f3_output(nemesis_board::ReviewVerdict::Fail, "缺少 B 模块交付"),
    )
    .unwrap();
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "in_progress",
        "FAIL 必须 completed→in_progress 回滚（F2 状态机合法转移）"
    );
    let comments = deps.store.list_comments(parent.id).unwrap();
    let c = comments
        .iter()
        .find(|c| c.content.contains("收口验收未定案"))
        .expect("缺口评论必须落父单线程");
    assert!(
        c.content.contains("缺少 B 模块交付"),
        "差距必须可见: {}",
        c.content
    );
    assert!(
        c.content.contains("不自动重开父单"),
        "边界声明必须在: {}",
        c.content
    );
    // 父单状态不动（F3 不自动重开）：落盘前后 status 必须一致。
    let before = deps.store.get_issue(parent.id).unwrap().status;
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        before,
        "F3 不得自动重开/改动父单状态"
    );
}

#[tokio::test]
async fn f3_fail_on_active_project_keeps_status_but_comments() {
    let (deps, _ws) = review_deps("f3-active");
    // 项目本就 active/in_progress：FAIL 不需要回滚（只评论）。
    let (pid, parent) = f3_fixture(&deps, "活跃项目");
    assert_eq!(deps.store.get_project(pid).unwrap().status, "active");
    apply_project_review_outcome(
        &deps,
        &deps.store.get_project(pid).unwrap(),
        std::slice::from_ref(&parent),
        &f3_output(nemesis_board::ReviewVerdict::Unsure, ""),
    )
    .unwrap();
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "active",
        "非 completed 项目无状态可回滚，必须原样保留"
    );
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("收口验收未定案")),
        "UNSURE 也要落转人工评论"
    );
}

#[tokio::test]
async fn f3_archived_project_never_rolled_back() {
    let (deps, _ws) = review_deps("f3-archived");
    let (pid, parent) = f3_fixture(&deps, "归档项目");
    deps.store
        .update_project(
            pid,
            &nemesis_board::ProjectPatch {
                status: Some("archived".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    apply_project_review_outcome(
        &deps,
        &deps.store.get_project(pid).unwrap(),
        std::slice::from_ref(&parent),
        &f3_output(nemesis_board::ReviewVerdict::Fail, "迟到的差评"),
    )
    .unwrap();
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "archived",
        "archived 完全不动（只 warn + 评论）"
    );
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("迟到的差评")),
        "评论照常留痕（人工可读）"
    );
}

// ===== P5/B3 多检查员聚合（aggregate_verdicts 纯函数穷举）=====

fn pv(verdict: ReviewVerdict) -> nemesis_board::ReviewOutput {
    nemesis_board::ReviewOutput {
        verdict,
        reasons: Vec::new(),
        gap: String::new(),
        experience: None,
        need_evidence: None,
        evidence_request: None,
    }
}

#[test]
fn aggregate_all_pass() {
    let out = aggregate_verdicts(
        3,
        vec![
            pv(ReviewVerdict::Pass),
            pv(ReviewVerdict::Pass),
            pv(ReviewVerdict::Pass),
        ],
    );
    assert_eq!(out.verdict, ReviewVerdict::Pass);
    assert!(
        out.reasons[0].contains("3 位检查员投票：PASS×3 FAIL×0 UNSURE×0"),
        "投票注记首条：{:?}",
        out.reasons[0]
    );
}

#[test]
fn aggregate_all_fail() {
    let mut o = pv(ReviewVerdict::Fail);
    o.gap = "差距 A".into();
    let out = aggregate_verdicts(2, vec![o, pv(ReviewVerdict::Fail)]);
    assert_eq!(out.verdict, ReviewVerdict::Fail);
    assert_eq!(out.gap, "差距 A", "首个非空 gap 胜出");
}

#[test]
fn aggregate_majority_pass_2_1_drops_evidence_request() {
    // 2 PASS + 1 FAIL(need_evidence)：多数已定案 → PASS；个别证据请求不改写结论。
    let mut o = pv(ReviewVerdict::Fail);
    o.need_evidence = Some(true);
    o.evidence_request = Some("取证".into());
    let out = aggregate_verdicts(3, vec![pv(ReviewVerdict::Pass), o, pv(ReviewVerdict::Pass)]);
    assert_eq!(out.verdict, ReviewVerdict::Pass);
    assert_eq!(out.need_evidence, None, "定案结论不被个别证据请求改写");
    assert_eq!(out.evidence_request, None);
}

#[test]
fn aggregate_majority_fail_2_1() {
    let out = aggregate_verdicts(
        3,
        vec![
            pv(ReviewVerdict::Fail),
            pv(ReviewVerdict::Pass),
            pv(ReviewVerdict::Fail),
        ],
    );
    assert_eq!(out.verdict, ReviewVerdict::Fail);
}

#[test]
fn aggregate_tie_goes_unsure_and_keeps_evidence() {
    // 平票 1:1 → Unsure 转人工；任一路 need_evidence 保留（B2b 挂起可用）。
    let mut o = pv(ReviewVerdict::Fail);
    o.need_evidence = Some(true);
    o.evidence_request = Some("回报关键输出原文".into());
    let out = aggregate_verdicts(2, vec![pv(ReviewVerdict::Pass), o]);
    assert_eq!(out.verdict, ReviewVerdict::Unsure);
    assert_eq!(out.need_evidence, Some(true));
    assert_eq!(out.evidence_request.as_deref(), Some("回报关键输出原文"));
}

#[test]
fn aggregate_all_unsure_unsure() {
    let mut o = pv(ReviewVerdict::Unsure);
    o.need_evidence = Some(true);
    let out = aggregate_verdicts(2, vec![o, pv(ReviewVerdict::Unsure)]);
    assert_eq!(out.verdict, ReviewVerdict::Unsure);
    assert_eq!(out.need_evidence, Some(true));
}

#[test]
fn aggregate_reasons_merge_dedup_and_invalid_note() {
    // requested=3 但只有 2 路有效：无效注记 + 各路理由去重合并保序。
    let mut a = pv(ReviewVerdict::Pass);
    a.reasons = vec!["共享理由".into(), "A 独有".into()];
    let mut b = pv(ReviewVerdict::Pass);
    b.reasons = vec!["共享理由".into(), " B 独有 ".into()];
    let out = aggregate_verdicts(3, vec![a, b]);
    assert!(
        out.reasons
            .iter()
            .any(|r| r.contains("另有 1 位检查员输出无效未计票")),
        "无效路注记：{:?}",
        out.reasons
    );
    let shared = out
        .reasons
        .iter()
        .filter(|r| r.trim() == "共享理由")
        .count();
    assert_eq!(shared, 1, "跨路重复理由去重");
    assert!(out.reasons.iter().any(|r| r == "A 独有"));
    assert!(
        out.reasons.iter().any(|r| r == "B 独有"),
        "理由 trim 后入列"
    );
    // 投票注记恒为首条。
    assert!(out.reasons[0].starts_with("3 位检查员投票："));
}

#[test]
fn aggregate_single_checker_passthrough() {
    // checkers=1 回归：单路聚合 = 原输出语义（verdict/gap/experience 原样）。
    let mut o = pv(ReviewVerdict::Pass);
    o.reasons = vec!["唯一理由".into()];
    o.gap = String::new();
    let out = aggregate_verdicts(1, vec![o]);
    assert_eq!(out.verdict, ReviewVerdict::Pass);
    assert_eq!(
        out.reasons[0],
        "1 位检查员投票：PASS×1 FAIL×0 UNSURE×0（多数票裁决）"
    );
    assert_eq!(out.reasons[1], "唯一理由");
    assert_eq!(out.gap, "");
}

/// 并行面板保序（goal B3 测试项）：Semaphore 限并发 + join_all 汇集后，
/// 各检查员产出经聚合仍按输入序参与裁决（首个非空 gap/experience 取
/// 输入序首位）。用面板同构的并发形态验证组合语义。
#[tokio::test]
async fn panel_pipeline_preserves_checker_order() {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(PANEL_CONCURRENCY));
    let mut futs = Vec::new();
    for i in 0..5usize {
        let sem = std::sync::Arc::clone(&sem);
        futs.push(async move {
            let _permit = sem.acquire_owned().await;
            let mut o = pv(ReviewVerdict::Unsure);
            o.gap = format!("gap-from-checker-{i}");
            o
        });
    }
    let results: Vec<nemesis_board::ReviewOutput> = futures::future::join_all(futs).await;
    let out = aggregate_verdicts(5, results);
    assert_eq!(
        out.gap, "gap-from-checker-0",
        "join_all 保序 → 首个非空 gap 属 0 号检查员"
    );
}

// ===== P4/F3 回归：项目级验收标准聚合（join_project_review_ac 纯函数）=====

fn parent_with_ac(number: &str, ac: Option<&str>) -> nemesis_board::Issue {
    use nemesis_board::NewIssue;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let store = nemesis_board::BoardStore::open(
        &std::env::temp_dir().join(format!("nb-review-joinac-{}-{n}", std::process::id())),
        "NB",
    )
    .unwrap();
    let mut issue = store
        .create_issue(NewIssue {
            title: format!("父单 {number}"),
            acceptance_criteria: ac.map(String::from),
            ..Default::default()
        })
        .unwrap();
    // 聚合函数只读 number/acceptance_criteria 两个字段；number 直接改内存
    // 副本为测试标注值（建单分配的自增号对断言无意义）。
    issue.number = number.to_string();
    issue
}

#[test]
fn join_project_ac_project_first_then_parents() {
    // 2026-10-17 契约更新：store 建单自动生成 AC——要测聚合函数的 None 臂
    // （空 AC 父单不产生空段）必须显式抹掉生成值，模拟存量缺 AC 单。
    let mut nb2 = parent_with_ac("NB-2", None);
    nb2.acceptance_criteria = None;
    let parents = vec![
        parent_with_ac("NB-1", Some("标准一")),
        nb2,
        parent_with_ac("NB-3", Some("  标准三  ")),
    ];
    let joined = join_project_review_ac(Some("项目级标准"), &parents);
    // 【label】独立成行（2026-09-12 根修）：拼进 AC 首行会把每段第一条
    // [CHECK] 锚点挤离行首，被 parse_anchors 静默吞掉——label 必须自成
    // 一行，AC 原样顶格。
    assert!(
        joined.starts_with("【项目级】\n项目级标准\n"),
        "项目级 AC 排首段（label 独立成行）：{joined}"
    );
    assert!(joined.contains("【NB-1】\n标准一\n"));
    assert!(!joined.contains("NB-2"), "空 AC 父单不产生空段");
    assert!(joined.contains("【NB-3】\n标准三\n"), "父单 AC trim 后入段");
    assert!(joined.contains("---"), "段间分隔符");
}

/// 锚点行顶格保全：AC 内的 [CHECK] 行经聚合后必须仍在行首（否则锚点
/// 被吞、客观核验静默失效——S2 真机实证过 re:index.html 蒸发）。
#[test]
fn join_project_ac_preserves_anchor_line_positions() {
    let parents = vec![parent_with_ac(
        "NB-1",
        Some("[CHECK] re:子单交付词\n[CHECK] file:out/x.md exists"),
    )];
    let joined = join_project_review_ac(Some("[CHECK] re:项目级词"), &parents);
    for line in joined.lines() {
        if line.contains("[CHECK]") {
            assert!(
                line.trim_start().starts_with("[CHECK]"),
                "[CHECK] 行必须顶格（label 不得拼进锚点行）: {line}"
            );
        }
    }
    assert!(joined.contains("\n[CHECK] re:项目级词"));
}

#[test]
fn join_project_ac_empty_project_ac_skipped() {
    let parents = vec![parent_with_ac("NB-1", Some("标准一"))];
    let joined = join_project_review_ac(Some("  "), &parents);
    assert!(
        joined.starts_with("【NB-1】"),
        "空白项目级 AC 跳过：{joined}"
    );
    // 全空 → 空串（评审 prompt 拿 None 语义由调用方处理）。
    let none = join_project_review_ac(None, &[]);
    assert_eq!(none, "");
}

// ---------- E2 决策审计词表（全自动流转 P5）----------

/// 单一写入点词表断言：全部 8 个处置臂（auto_accept/suggest_manual/
/// redispatch/escalate_human/parent_auto_close/parent_escalate_human/
/// project_complete/project_escalate_human）都经 `record_auto_decide`
/// 落 `auto_decide` 活动，details = `{decision, verdict, ..extra}` 合并，
/// actor = agent(node_id)。本测试钉死写入点的 action 词、details 形状
/// 与 extra 键合并语义（审计流 board.audit.list / rollback 依赖它）。
#[test]
fn record_auto_decide_writes_action_word_and_details_shape() {
    use nemesis_board::NewIssue;
    let (deps, _ws) = review_deps("audit-word");
    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "词表断言".into(),
            ..Default::default()
        })
        .unwrap();

    record_auto_decide(
        &deps,
        issue.id,
        "auto_accept",
        "PASS",
        serde_json::json!({ "gap": "", "note": "锚点全过" }),
    );
    // 失败静默（warn）不炸验收：不存在的 issue_id 不 panic。
    record_auto_decide(&deps, 999_999, "redispatch", "FAIL", serde_json::json!({}));

    let rows = deps
        .store
        .list_recent_activity(500, Some("auto_decide"))
        .unwrap();
    assert_eq!(rows.len(), 1, "失败调用不落行");
    let row = &rows[0];
    assert_eq!(row.activity.action, "auto_decide");
    assert_eq!(row.activity.actor.kind, "agent");
    assert_eq!(row.activity.actor.id, "node-a");
    let details: serde_json::Value =
        serde_json::from_str(row.activity.details.as_deref().unwrap()).unwrap();
    assert_eq!(details["decision"], "auto_accept");
    assert_eq!(details["verdict"], "PASS");
    assert_eq!(details["note"], "锚点全过");
    assert!(details.get("gap").is_some(), "extra 键合并进 details");
}

// ---------- E1 二期：token 预算维度（board.budget.max_tokens_per_parent）----------

/// 纯聚合：父链全部派发行的 worker 回传用量求和。回归锚（T37 run 4 真机
/// bug）：派发行 worker_id = 派发时 peer 名（`node-b`），账本键 worker 段 =
/// 传输层运行时节点 id（`node-laptop-runtime-b`）——两者不同源，按
/// `/{task_id}` 后缀聚合才能对上；无 usage 行的派发计 0；陌生键的行不计入。
#[test]
fn e1_token_dimension_aggregates_worker_usage() {
    let dir = std::env::temp_dir().join(format!("nb-review-token-ds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ds = nemesis_data::DataStore::open(&dir.join("data.db")).unwrap();
    let (deps, _ws) = review_deps("e1-tokens");
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "token 预算父单".to_string(),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "token 预算子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    // 派发行 worker_id 记 peer 名（与 dispatch_issue_core 落库同形态）。
    seed_dispatch(&deps.store, "tok-1", parent.id, "node-b");
    seed_dispatch(&deps.store, "tok-2", child.id, "node-c");
    seed_dispatch(&deps.store, "tok-3", parent.id, "node-d"); // 无 usage 行

    let log = |key: &str, input: i64, output: i64| nemesis_data::RequestLog {
        id: 0,
        trace_id: String::new(),
        model: "m".into(),
        provider_type: String::new(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: 0.0,
        latency_ms: 0,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 0,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: key.to_string(),
    };
    // 账本键 worker 段 = 运行时节点 id（与回调 _rpc.from 同形态），
    // 刻意 ≠ 派发行的 peer 名——精确键在此 fixture 下必须失配。
    ds.insert_request_log(&log("cluster_rpc:node-laptop-runtime-b/tok-1", 100, 20))
        .unwrap();
    ds.insert_request_log(&log("cluster_rpc:node-laptop-runtime-c/tok-2", 50, 30))
        .unwrap();
    // 陌生键（其他任务/会话）不得串账。
    ds.insert_request_log(&log("cluster_rpc:node-z/other", 999, 999))
        .unwrap();

    let total = chain_token_usage_ds(&ds, &deps.store, parent.id);
    assert_eq!(total, 100 + 20 + 50 + 30, "父 + 子派发用量求和 = 200");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 评审 loop 未装配（OnceLock 空）→ 维度 4 诚实放行（预算是保险丝不是
/// 安全闸；budget_breach 不得因缺账本而炸）。
#[tokio::test]
async fn e1_token_dimension_without_datastore_is_honest_pass() {
    let (deps, _ws) = review_deps("e1-token-nods");
    let issue = issue_in_review(&deps.store, "无账本", "", "交付");
    let mut cfg = nemesis_config::BoardFlagConfig::default();
    cfg.budget.max_tokens_per_parent = 1;
    assert!(
        budget_breach(&deps, &issue, &cfg).is_none(),
        "无 DataStore 时 token 维放行"
    );
}

/// 闸 × 阈值矩阵（走 budget_breach 全链 = moderator_loop.data_store 装配
/// 路径）：用量 200 > 阈值 199 → 报超支（文案带维度名与数值）；阈值 200
/// 恰好相等 → 放行（保险丝只在突破时熔断）。
#[tokio::test]
async fn e1_token_breach_gate_matrix_through_moderator_loop() {
    use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::AgentConfig;

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<nemesis_agent::types::ChatOptions>,
            _tools: Vec<nemesis_agent::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "null".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let dir = std::env::temp_dir().join(format!("nb-review-token-gate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ds = Arc::new(nemesis_data::DataStore::open(&dir.join("data.db")).unwrap());
    ds.insert_request_log(&nemesis_data::RequestLog {
        id: 0,
        trace_id: String::new(),
        model: "m".into(),
        provider_type: String::new(),
        input_tokens: 150,
        output_tokens: 50,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: 0.0,
        latency_ms: 0,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 0,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: "cluster_rpc:node-b/tok-gate-1".to_string(),
    })
    .unwrap();

    let (mut deps, _ws) = review_deps("e1-token-gate");
    let mut agent_loop = AgentLoop::new(Box::new(NullProvider), AgentConfig::default());
    agent_loop.set_data_store(ds);
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    let _ = deps.moderator_loop.set(Arc::new(agent_loop));

    let issue = issue_in_review(&deps.store, "token 闸矩阵", "", "交付");
    seed_dispatch(&deps.store, "tok-gate-1", issue.id, "node-b");

    let mut cfg = nemesis_config::BoardFlagConfig::default();
    cfg.budget.max_tokens_per_parent = 199;
    let breach = budget_breach(&deps, &issue, &cfg).expect("200 > 199 必须报超支");
    assert!(
        breach.contains("max_tokens_per_parent") && breach.contains("200"),
        "文案必须带维度名与数值: {breach}"
    );
    cfg.budget.max_tokens_per_parent = 200;
    assert!(
        budget_breach(&deps, &issue, &cfg).is_none(),
        "200 <= 200 放行（保险丝只在突破时熔断）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
// ------------------------------------------------------------------
// estop × 集群回归（集群完备性加固 2026-09-11）：
// park_sweep_gate（estop 短路 + 节流，gateway sweep 回调的触发闸）
// + spawn_estop_resume_watcher（release → 队列 drain → 按维路由复评）。
// ------------------------------------------------------------------

#[test]
fn sweep_gate_estop_short_circuits_without_consuming_throttle() {
    let base = std::time::Instant::now();
    let mut last: Option<std::time::Instant> = None;

    // 急停挂起：首信号也拒。
    assert!(!super::park_sweep_gate(
        true,
        &mut last,
        base,
        std::time::Duration::from_secs(10)
    ));
    // 关键语义：拒绝**不消耗节流窗口**（last 仍为 None）——急停期间的
    // announce 不吃掉释放后的首次重试机会。
    assert!(last.is_none(), "estop 拒绝不得盖章节流时间戳");

    // 释放后同刻重试：立即放行并盖章。
    assert!(super::park_sweep_gate(
        false,
        &mut last,
        base,
        std::time::Duration::from_secs(10)
    ));
    assert_eq!(last, Some(base));
}

#[test]
fn sweep_gate_throttle_window_boundaries() {
    let base = std::time::Instant::now();
    let mut last: Option<std::time::Instant> = None;
    let win = std::time::Duration::from_secs(10);

    // 首信号放行（无历史）。
    assert!(super::park_sweep_gate(false, &mut last, base, win));
    // 窗口内（+5s）拒绝。
    assert!(!super::park_sweep_gate(
        false,
        &mut last,
        base + std::time::Duration::from_secs(5),
        win
    ));
    // 恰好等于窗口（+10s）：>= 语义放行。
    assert!(super::park_sweep_gate(false, &mut last, base + win, win));
    // 盖章推进到 base+win 后，窗口内再拒。
    assert!(!super::park_sweep_gate(
        false,
        &mut last,
        base + win + std::time::Duration::from_secs(3),
        win
    ));
}

/// release watcher 链路：estop 停车（Issue 维度入队）→ watcher 订阅 →
/// release → 队列 drain（复评 spawn 后 loop 未就绪诚实跳过，不触 LLM）。
#[tokio::test]
async fn estop_release_watcher_drains_parked_queue() {
    let (deps, dir) = review_deps("estop-release-watcher");
    let issue = issue_in_review(&deps.store, "释放恢复", "", "交付");

    // 急停中评审 → 停车入队。
    deps.estop.trigger();
    let reviewed = super::review_issue(&deps, issue.id, super::ReviewCtx::first_stage())
        .await
        .unwrap();
    assert!(!reviewed);
    assert_eq!(
        deps.estop_parked.lock().unwrap().len(),
        1,
        "释放前停车队列应有 1 条"
    );

    // watcher 订阅后释放 → 队列必须被 drain（路由复评）。
    super::spawn_estop_resume_watcher(deps.clone());
    deps.estop.release();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if deps.estop_parked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        deps.estop_parked.lock().unwrap().is_empty(),
        "release 后 watcher 必须 drain 停车队列"
    );

    // 再次急停-停车-释放：watcher 持续存活（多轮恢复）。
    deps.estop.trigger();
    let reviewed = super::review_issue(&deps, issue.id, super::ReviewCtx::first_stage())
        .await
        .unwrap();
    assert!(!reviewed);
    deps.estop.release();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if deps.estop_parked.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        deps.estop_parked.lock().unwrap().is_empty(),
        "第二轮 release 也必须 drain"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ===== 项目收口聚合下钻 + 转人工评论去重（2026-09-12 双端真机 S2 根修）=====

/// 项目收口的锚点实核文本必须含叶子子单的交付评论：worker 交付落在子单、
/// 父单自身无 Delivery 时，聚合下钻后 `re:` 锚点应 PASS（进入 LLM 臂；loop
/// 未就绪 → 诚实跳过返回 false），不得再对空 summary 短路 FAIL 转人工
/// （旧实现聚合到空集的症状：三条锚点对「（无结构化交付汇报）」全败）。
#[tokio::test]
async fn project_anchor_check_reads_leaf_subissue_deliveries() {
    use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, _ws) = review_deps("project-agg-drill");
    // 项目收口评审受 auto_close_project 闸（默认关）——测试显式开闸，
    // 保证走的是锚点检查链而非 config 闸提前返回（否则断言假绿）。
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_review":true,"review":{"auto_close_project":true}}}"#,
    )
    .unwrap();
    let reviewer = Actor::agent("node-a");
    let project = deps
        .store
        .create_project(
            "聚合下钻",
            "d",
            None,
            "",
            "[CHECK] re:叶子交付关键词XYZ",
            None,
        )
        .unwrap();

    // 顶层父单：挂项目、推到 done（收口前置条件）、无任何 Delivery。
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "父单".into(),
            description: String::new(),
            priority: 2,
            creator: reviewer.clone(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::Done, &reviewer)
        .unwrap();

    // 叶子子单：交付评论含项目级锚点关键词（真实数据流：交付在叶子）。
    let sub = deps
        .store
        .create_issue(NewIssue {
            title: "子单".into(),
            description: String::new(),
            priority: 2,
            creator: reviewer.clone(),
            parent_issue_id: Some(parent.id),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: sub.id,
            author: Actor::agent("node-b"),
            content: "## 结论\n完成，产出叶子交付关键词XYZ。".into(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = super::review_project_completion(&deps, project.id)
        .await
        .expect("项目收口评审必须闭环不报错");
    // 锚点 PASS → 进 LLM 臂 → moderator_loop 未就绪 → 诚实跳过（非 FAIL
    // 闭环；FAIL 会返回 Ok(true)）。
    assert!(
        !reviewed,
        "锚点应 PASS 并止于 LLM 未就绪跳过（非短路 FAIL）"
    );

    // 无转人工评论、无 escalate 审计——聚合下钻生效的直接证据。
    let comments = deps.store.list_comments(parent.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("收口验收未定案")),
        "父单不得出现转人工评论（锚点应已 PASS）"
    );
    let audit = deps
        .store
        .list_recent_activity(100, Some("project_escalate_human"))
        .unwrap_or_default();
    assert!(
        !audit.iter().any(|d| d.activity.issue_id == parent.id),
        "不得落 project_escalate_human 审计"
    );
    let _ = std::fs::remove_dir_all(&_ws);
}

/// 反向护栏 + 评论去重：交付真缺关键词 → 锚点 FAIL 短路 → 转人工评论落
/// 库，且每条失败锚点在评论中只出现一次（gap/reasons 双拼根修）。
#[tokio::test]
async fn project_anchor_fail_escalates_with_deduped_comment() {
    use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, _ws) = review_deps("project-agg-dedup");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_review":true,"review":{"auto_close_project":true}}}"#,
    )
    .unwrap();
    let reviewer = Actor::agent("node-a");
    let project = deps
        .store
        .create_project("去重", "d", None, "", "[CHECK] re:不存在关键词XYZ", None)
        .unwrap();

    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "父单".into(),
            description: String::new(),
            priority: 2,
            creator: reviewer.clone(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::Done, &reviewer)
        .unwrap();

    let sub = deps
        .store
        .create_issue(NewIssue {
            title: "子单".into(),
            description: String::new(),
            priority: 2,
            creator: reviewer.clone(),
            parent_issue_id: Some(parent.id),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: sub.id,
            author: Actor::agent("node-b"),
            content: "## 结论\n完成。".into(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = super::review_project_completion(&deps, project.id)
        .await
        .expect("锚点短路 FAIL 必须闭环不报错");
    assert!(reviewed, "短路 FAIL 应完成闭环（转人工）");

    let comments = deps.store.list_comments(parent.id).unwrap();
    let esc = comments
        .iter()
        .find(|c| c.content.contains("收口验收未定案"))
        .expect("必须有转人工评论");
    assert_eq!(
        esc.content.matches("[CHECK] re:不存在关键词XYZ").count(),
        1,
        "失败锚点在转人工评论中只允许出现一次（gap/reasons 双拼根修）:\n{}",
        esc.content
    );
    let _ = std::fs::remove_dir_all(&_ws);
}

/// gap 非空时 reasons 不重复渲染；gap 空（LLM 未填差距）reasons 照常兜底。
#[test]
fn reasons_skipped_when_gap_carries_details() {
    let mk = |gap: &str| nemesis_board::ReviewOutput {
        verdict: nemesis_board::ReviewVerdict::Fail,
        reasons: vec!["锚点失败: [CHECK] re:x — 未命中".into()],
        gap: gap.to_string(),
        experience: None,
        need_evidence: None,
        evidence_request: None,
    };
    let mut c1 = String::from("head");
    super::append_reasons_unless_gapped(&mut c1, &mk("客观锚点检查失败（明细在场）"));
    assert!(!c1.contains("理由："), "gap 明细在场时不得再拼 reasons");
    let mut c2 = String::from("head");
    super::append_reasons_unless_gapped(&mut c2, &mk(""));
    assert!(c2.contains("理由："), "gap 空时 reasons 是唯一理由来源");
}

// ---------- P2A 能力类失败保护（2026-09-12，NB-15 根修） ----------

/// 构造一条最小评论（latest_fail_class 只看 content）。
fn p2a_comment(content: &str) -> nemesis_board::Comment {
    nemesis_board::Comment {
        id: 1,
        issue_id: 1,
        author: nemesis_board::Actor::agent("node-b"),
        content: content.to_string(),
        parent_id: None,
        ctype: nemesis_board::CommentType::Comment,
        created_at: 0,
    }
}

/// ⛔ 失败评论形态（gateway.rs write_back_board_dispatch 唯一写入点）。
fn worker_fail_comment(response: &str, fail_class: &str) -> String {
    format!("⛔ worker 汇报失败：\n\n{response}\n\nfail_class: {fail_class}")
}

#[test]
fn p2a_latest_fail_class_extracts_known_marker() {
    let c = vec![p2a_comment(&worker_fail_comment(
        "工具参数校验连续失败 2 次，已停止重试。最近工具：'write_file'。",
        "validation_budget",
    ))];
    assert_eq!(latest_fail_class(&c), Some("validation_budget"));
}

#[test]
fn p2a_latest_fail_class_scans_newest_first() {
    // 列表顺序 = 落库时间序（旧→新）；重派轮次取最新失败分类。
    let old = p2a_comment(&worker_fail_comment(
        "Error: request timed out",
        "llm_timeout",
    ));
    let new = p2a_comment(&worker_fail_comment(
        "工具参数校验连续失败 2 次…",
        "validation_budget",
    ));
    assert_eq!(
        latest_fail_class(&[old.clone(), new.clone()]),
        Some("validation_budget"),
        "最新评论在后（rev 扫描）"
    );
    assert_eq!(latest_fail_class(&[new, old]), Some("llm_timeout"));
}

#[test]
fn p2a_latest_fail_class_rejects_unknown_and_forged_values() {
    // 未知类值不认（防 worker 文本 / 人工评论误触发闸门）。
    let forged = vec![p2a_comment("fail_class: totally_made_up")];
    assert_eq!(latest_fail_class(&forged), None);
    // 非行首出现的标记不认（必须 trim 后整行匹配前缀）。
    let inline = vec![p2a_comment("说明：见 fail_class: validation_budget 一行")];
    assert_eq!(latest_fail_class(&inline), None);
}

#[test]
fn p2a_latest_fail_class_none_without_marker() {
    let plain = vec![p2a_comment("交付完成"), p2a_comment("❌ agent 验收未通过")];
    assert_eq!(latest_fail_class(&plain), None);
    assert_eq!(latest_fail_class(&[]), None);
}

#[test]
fn p2a_capability_classes_and_hints_contract() {
    // 闸门只认两类能力类失败；每类都有对应的人工处置建议。
    assert_eq!(CAPABILITY_FAIL_CLASSES, ["validation_budget", "escalation"]);
    let hint_budget = capability_fail_hint("validation_budget");
    assert!(
        hint_budget.contains("probe") && hint_budget.contains("set-tier"),
        "validation_budget 建议应含 probe/set-tier 指引: {hint_budget}"
    );
    let hint_esc = capability_fail_hint("escalation");
    assert!(
        hint_esc.contains("改写任务") || hint_esc.contains("换一种思路"),
        "escalation 建议应含改写/换思路指引: {hint_esc}"
    );
    // 未知类走兜底（非空即可读）。
    assert!(!capability_fail_hint("exec_failed").is_empty());
}

/// 单节点历史 + 能力类失败（validation_budget）：非无限模式不盲重派，
/// 转人工 + 审计留痕，状态保持 in_review。
#[tokio::test]
async fn p2a_capability_fail_blocks_blind_same_target_redispatch() {
    use nemesis_board::{CommentType, IssueStatus, NewComment};

    let (deps, _ws) = review_deps("p2a-cap-block");
    // re: 失败锚点（远端派发合法形态，P1 拓扑硬闸不拦）——闸的拦截效果
    // 由此可完全归因于能力类失败保护，而非拓扑硬闸误伤。
    let issue = issue_in_review(
        &deps.store,
        "能力失败保护",
        "[CHECK] re:验收完成标志XYZ\n[CHECK] re:交付完成",
        "## 结论\n交付完成",
    );
    deps.store
        .insert_dispatch(
            "task-p2a-1",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-p2a-1", nemesis_board::models::dispatch_state::DONE)
        .unwrap();
    // worker 失败回报（gateway writeback 形态）：⛔ 评论尾带 fail_class 标记行。
    deps.store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: worker_fail_comment(
                "工具参数校验连续失败 2 次，已停止重试。",
                "validation_budget",
            ),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("能力类失败闸必须闭环不报错");
    assert!(reviewed);

    // 同目标（单节点历史）+ 能力类失败 → 不发车：派发数不涨，状态保持 in_review。
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "能力类失败 + 同目标：禁止盲重派"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
    // 转人工评论带 fail_class 与处置建议；闸在 ❌ 重派评论之前生效。
    let comments = deps.store.list_comments(issue.id).unwrap();
    let guard = comments
        .iter()
        .find(|c| c.content.contains("能力类失败保护"))
        .expect("必须有保护性转人工评论");
    assert!(guard.content.contains("validation_budget"));
    assert!(guard.content.contains("set-tier"), "处置建议应含档位指引");
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("❌ agent 验收未通过")),
        "闸在 ❌ 重派评论之前生效，不得再落重派意见"
    );
    // 审计留痕：decision=escalate_human + reason=capability_fail_same_target。
    let audit = deps.store.list_activity(issue.id).unwrap();
    let decide = audit
        .iter()
        .find(|a| a.action == "auto_decide")
        .expect("必须有 auto_decide 审计");
    let details = decide.details.as_deref().unwrap_or("");
    assert!(details.contains("escalate_human"), "{details}");
    assert!(details.contains("capability_fail_same_target"), "{details}");
    let _ = std::fs::remove_dir_all(&deps.home);
}

/// unlimited_mode 契约（无条件流转，estop 是保险丝）：能力类失败 + 同目标
/// 仅 WARN 留痕继续，照常发车。
#[tokio::test]
async fn p2a_capability_fail_unlimited_mode_warns_but_continues() {
    use nemesis_board::{CommentType, NewComment};

    let (deps, _ws) = review_deps("p2a-cap-unlimited");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board": {"unlimited_mode": true}}"#,
    )
    .unwrap();
    let issue = issue_in_review(
        &deps.store,
        "能力失败保护（无限模式）",
        "[CHECK] re:验收完成标志XYZ\n[CHECK] re:交付完成",
        "## 结论\n交付完成",
    );
    deps.store
        .insert_dispatch(
            "task-p2a-2",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-p2a-2", nemesis_board::models::dispatch_state::DONE)
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: worker_fail_comment(
                "检测到循环无法打破：exec 已 6 次报相同错误…",
                "escalation",
            ),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("unlimited WARN 路径必须闭环不报错");
    assert!(reviewed);

    // WARN 评论在场 + 照常发车（派发数 2）。
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("能力类失败") && c.content.contains("仅告警继续")),
        "必须有 unlimited WARN 评论: {:?}",
        comments
            .iter()
            .map(|c| c.content.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        2,
        "unlimited_mode 契约：告警继续（estop 是保险丝）"
    );
    let _ = std::fs::remove_dir_all(&deps.home);
}

// ---------- PR5：worker 循环升级标记 ----------

/// `detect_worker_loop_marker` 语义钉死：只认 worker 侧（kind=agent 且
/// id≠评审方）评论的行首标记；评审员/系统评论引用原文不触发（防自触发，
/// M8 同哲学）；缩进也算（trim_start 语义）；多次喊停取最后一条说明。
#[test]
fn worker_loop_marker_detection_semantics() {
    let dir = std::env::temp_dir().join(format!("nb-pr5-marker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "标记语义单".into(),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();

    // 无评论 → None。
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", None).unwrap(),
        None
    );

    // 评审员（node-a = 评审方自己）评论含标记 → 不触发（防自触发）。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-a"),
            content: "差距：worker 声称 REDISPATCH_LOOP: 但未证实".into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Comment,
        })
        .unwrap();
    // 系统评论含标记 → 不触发。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::system("board-review"),
            content: "REDISPATCH_LOOP: 系统注记也不算".into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::System,
        })
        .unwrap();
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", None).unwrap(),
        None,
        "评审员/系统评论不得触发"
    );

    // worker 评论正文提及（非行首）→ 不触发。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "前缀提及 REDISPATCH_LOOP: 在行中间不算".into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Comment,
        })
        .unwrap();
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", None).unwrap(),
        None,
        "行中提及不算"
    );

    // worker 行首标记 → 触发；缩进也算；多次喊停取最后一条。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content:
                "## 结论\n仍失败\nREDISPATCH_LOOP: 第一轮喊停\n  REDISPATCH_LOOP: 并发窗口无法收敛"
                    .into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Delivery,
        })
        .unwrap();
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", None).unwrap(),
        Some("并发窗口无法收敛".to_string()),
        "行首/缩进算，取最后一条说明"
    );

    // 空 marker → 占位说明。
    let issue2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "空标记单".into(),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue2.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "REDISPATCH_LOOP:".into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Delivery,
        })
        .unwrap();
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue2.id, "node-a", None).unwrap(),
        Some("（无说明）".to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 轮次绑定语义（2026-10-16 NB-121 死循环根修）：`dispatched_before` 之后
/// 写入的 worker 标记才生效；最新派发之前的旧轮标记已由重派动作答复，
/// 不得阻塞换条件后的新轮（cutoff 用哨兵值验证，不依赖真实时钟）。
#[test]
fn worker_loop_marker_stale_after_redispatch() {
    let dir = std::env::temp_dir().join(format!("nb-pr5-marker-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "旧标记轮次绑定单".into(),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "REDISPATCH_LOOP: 旧轮喊停".into(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Delivery,
        })
        .unwrap();
    // 无派发史（cutoff=None）→ 旧行为：标记生效。
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", None).unwrap(),
        Some("旧轮喊停".to_string())
    );
    // cutoff=极早（0）→ 全部评论都比派发新 → 生效。
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", Some(0)).unwrap(),
        Some("旧轮喊停".to_string())
    );
    // cutoff=极晚（i64::MAX）→ 最新派发之后无新评论 → 旧轮标记不再生效。
    assert_eq!(
        super::detect_worker_loop_marker(&store, issue.id, "node-a", Some(i64::MAX)).unwrap(),
        None,
        "最新派发之前的旧轮标记已由重派答复，不得阻塞新轮"
    );
    // 新轮内 worker 再喊停（时间在 cutoff 之后）→ 新标记生效。
    // 注：store 层 created_at 取真实时钟，必然 > 0；用 cutoff=0 代表
    // 「派发发生在任意评论之前」的极端形态已覆盖过滤分支本身。
    let _ = std::fs::remove_dir_all(&dir);
}

/// PR5 worker 循环升级：worker 交付带 `REDISPATCH_LOOP:` 标记 → FAIL 决策
/// 链跳过重派直接转人工（不新增派发行；unlimited_mode 覆盖不了——喊停
/// 必须被尊重）。锚点 FAIL 短路提供确定性 verdict，全程无 LLM。
#[tokio::test]
async fn worker_loop_marker_stops_redispatch_even_unlimited() {
    use nemesis_board::{CommentType, IssueStatus, NewComment};

    let (deps, _ws) = review_deps("pr5-loop-marker");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board": {"unlimited_mode": true}}"#,
    )
    .unwrap();
    // re: 失败锚点（远端派发合法形态，拓扑硬闸不拦）→ 短路确定性 FAIL。
    let issue = issue_in_review(
        &deps.store,
        "worker 循环升级",
        "[CHECK] re:验收完成标志XYZ",
        "## 结论\n交付完成",
    );
    deps.store
        .insert_dispatch(
            "task-pr5-1",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-pr5-1", nemesis_board::models::dispatch_state::DONE)
        .unwrap();
    // worker 交付含行首循环标记（重派轮次段契约形态：标记行 + 已尝试清单）。
    deps.store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "## 结论\n已尝试多轮仍失败\nREDISPATCH_LOOP: 并发窗口在本节点环境无法收敛\n已尝试方案：\n- 方案A：加锁 失败\n- 方案B：退避重试 失败"
                .to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("循环升级路径必须闭环不报错");
    assert!(reviewed);

    // 不发车：派发数不涨；状态保持 in_review（转人工）。
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "worker 喊停后不得再重派（unlimited_mode 也覆盖不了）"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
    // 转人工评论含标记说明与处置指引；不得再落 ❌ 重派意见。
    let comments = deps.store.list_comments(issue.id).unwrap();
    let esc = comments
        .iter()
        .find(|c| c.content.contains("worker 循环升级"))
        .expect("必须有循环升级转人工评论");
    assert!(esc.content.contains("无法收敛"), "{}", esc.content);
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("❌ agent 验收未通过")),
        "喊停闸在重派意见之前生效"
    );
    // 审计留痕：decision=escalate_human + reason=worker_loop_marker。
    let audit = deps.store.list_activity(issue.id).unwrap();
    let decide = audit
        .iter()
        .find(|a| a.action == "auto_decide")
        .expect("必须有 auto_decide 审计");
    let details = decide.details.as_deref().unwrap_or("");
    assert!(details.contains("escalate_human"), "{details}");
    assert!(details.contains("worker_loop_marker"), "{details}");
    let _ = std::fs::remove_dir_all(&deps.home);
}

/// P8 会话 token 预算（2026-10-10 集群无人值守硬化 A2）：⛔ 失败评论带
/// `fail_class: token_budget` 标记行 → Redispatch 决策链不重派直接转人工
/// （unlimited_mode 覆盖不了——预算耗尽是资源闸不是行为异常：重派会被
/// worker 入口拒接原样退回，换节点也不解，只有人工裁决能解）。锚点 FAIL
/// 短路提供确定性 verdict，全程无 LLM。
#[tokio::test]
async fn token_budget_fail_class_stops_redispatch_even_unlimited() {
    use nemesis_board::{CommentType, IssueStatus, NewComment};

    let (deps, _ws) = review_deps("p8-token-budget");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board": {"unlimited_mode": true}}"#,
    )
    .unwrap();
    // re: 失败锚点（远端派发合法形态，拓扑硬闸不拦）→ 短路确定性 FAIL。
    let issue = issue_in_review(
        &deps.store,
        "token 预算耗尽",
        "[CHECK] re:验收完成标志XYZ",
        "## 结论\n预算耗尽",
    );
    deps.store
        .insert_dispatch(
            "task-p8-1",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-p8-1", nemesis_board::models::dispatch_state::DONE)
        .unwrap();
    // worker ⛔ 失败评论（write_back 契约形态：失败正文 + 尾部
    // fail_class: <class> 标记行——classify_terminal_failure 识别文案
    // 以 💰 marker 开头后的标记行落库）。
    deps.store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "⛔ 任务失败\n💰 会话 token 预算耗尽（1500/1000）：本会话累计 token 用量已达预算上限，已停止处理。\nfail_class: token_budget"
                .to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("token 预算路径必须闭环不报错");
    assert!(reviewed);

    // 不发车：派发数不涨；状态保持 in_review（转人工）。
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "token 预算耗尽后不得再重派（unlimited_mode 也覆盖不了）"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
    // 转人工评论含 💰 marker 与处置指引；不得再落 ❌ 重派意见。（定位用
    // config 键名——worker 失败评论正文也含「会话 token 预算耗尽」字样，
    // 直接 find marker 会误命中 worker 评论。）
    let comments = deps.store.list_comments(issue.id).unwrap();
    let esc = comments
        .iter()
        .find(|c| c.content.contains("agents.defaults.session_token_budget"))
        .expect("必须有 token 预算转人工评论");
    assert!(
        esc.content.contains("会话 token 预算耗尽"),
        "{}",
        esc.content
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("❌ agent 验收未通过")),
        "token 预算闸在重派意见之前生效"
    );
    // 审计留痕：decision=escalate_human + reason=token_budget。
    let audit = deps.store.list_activity(issue.id).unwrap();
    let decide = audit
        .iter()
        .find(|a| a.action == "auto_decide")
        .expect("必须有 auto_decide 审计");
    let details = decide.details.as_deref().unwrap_or("");
    assert!(details.contains("escalate_human"), "{details}");
    assert!(details.contains("token_budget"), "{details}");
    let _ = std::fs::remove_dir_all(&deps.home);
}

// ---------------------------------------------------------------------------
// 看板项目档案 P6（F9）：收口总结纯函数（facts/tree/root 守门）
// ---------------------------------------------------------------------------

#[test]
fn test_project_archive_root_guards_none_missing_and_existing() {
    // None（存量项目未绑定）/ 不存在路径 / 存在目录 三态。
    assert!(super::project_archive_root(None).is_err());
    assert!(super::project_archive_root(Some("Z:/definitely/not/here-nb")).is_err());
    let dir = std::env::temp_dir().join(format!("nb-summary-root-{}-guard", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(super::project_archive_root(Some(dir.to_str().unwrap())).is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_build_summary_facts_lists_issues_last_comment_and_decisions() {
    let dir = std::env::temp_dir().join(format!("nb-summary-facts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let project = store
        .create_project("总结项目", "描述X", None, "", "", None)
        .unwrap();
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父单A".into(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    let _child = store
        .create_issue(nemesis_board::NewIssue {
            title: "子单B".into(),
            project_id: Some(project.id),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: parent.id,
            author: nemesis_board::Actor::agent("node-x"),
            content: "✅ worker 汇报完成：交付 FINAL-EXCERPT".into(),
            parent_id: None,
            ctype: nemesis_board::models::CommentType::Comment,
        })
        .unwrap();
    store
        .add_activity(
            parent.id,
            &nemesis_board::Actor::system("t"),
            "auto_decide",
            Some(&serde_json::json!({"decision": "conflict_auto_resolve"}).to_string()),
        )
        .unwrap();

    let issues = store
        .list_issues(&nemesis_board::models::IssueFilter {
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    let decisions = store
        .list_recent_activity(100, Some("auto_decide"))
        .unwrap();
    let facts = super::build_summary_facts(&store, &project, &issues, &decisions);

    assert!(facts.contains("总结项目"), "项目名应在清单: {facts}");
    assert!(
        facts.contains("父单A") && facts.contains("子单B"),
        "逐单应在清单: {facts}"
    );
    assert!(facts.contains("FINAL-EXCERPT"), "末评摘录应在清单: {facts}");
    assert!(
        facts.contains("conflict_auto_resolve"),
        "决策流应在清单: {facts}"
    );

    // 决策流按项目过滤：他单 auto_decide 不得混入。
    let other = store
        .create_issue(nemesis_board::NewIssue {
            title: "外部单".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .add_activity(
            other.id,
            &nemesis_board::Actor::system("t"),
            "auto_decide",
            Some(&serde_json::json!({"decision": "redispatch"}).to_string()),
        )
        .unwrap();
    let decisions = store
        .list_recent_activity(100, Some("auto_decide"))
        .unwrap();
    let facts = super::build_summary_facts(&store, &project, &issues, &decisions);
    assert!(
        !facts.contains("redispatch"),
        "他单决策不得混入本项目清单: {facts}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_render_archive_tree_lists_dirs_hides_dotfiles() {
    let dir = std::env::temp_dir().join(format!("nb-summary-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::create_dir_all(dir.join("records/NB-1/execution")).unwrap();
    std::fs::write(dir.join("docs/summary.md"), "x").unwrap();
    std::fs::write(dir.join("project.json"), "{}").unwrap();
    std::fs::write(dir.join(".gitignore"), "y").unwrap();

    let tree = super::render_archive_tree(&dir, 2);
    assert!(tree.contains("docs/"), "目录应带斜杠: {tree}");
    assert!(tree.contains("summary.md"), "文件应列出: {tree}");
    // depth=2 语义：records/ 的子目录（NB-1/）可见，孙子（execution/）以 …
    // 语义截断——附录只概览两层。
    assert!(tree.contains("NB-1/"), "二层目录应列出: {tree}");
    assert!(!tree.contains("execution/"), "三层目录应截断: {tree}");
    assert!(
        !tree.contains(".gitignore"),
        "点文件不应出现在结构树: {tree}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- F9 收口总结：失败不阻塞收口（goal P6 判据单测）----------
//
// run_project_summary 的前置检查在任何 LLM 调用之前失败（本测：主 agent
// 未就绪——moderator_loop 空槽）→ 返回 Err；summarize_fail_note 落
// timeline「summary」失败事件 + 顶层父单 System 评论；项目状态不动
// （completed 保持——失败不回滚收口状态机）。

#[tokio::test]
async fn summary_generation_failure_leaves_trace_and_keeps_completed() {
    use nemesis_board::archive::ensure_scaffold;
    use nemesis_board::models::IssueFilter;
    use nemesis_board::{Actor, NewIssue, ProjectPatch};

    let dir = std::env::temp_dir().join(format!("nb-summary-fail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();

    let archive_dir = dir.join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    ensure_scaffold(&archive_dir, 1, "收口总结失败测", "active").unwrap();

    let project = store
        .create_project(
            "收口总结失败测",
            "F9 失败留痕夹具",
            None,
            "",
            "",
            Some(archive_dir.to_str().unwrap()),
        )
        .unwrap();
    // active → in_progress → completed（状态机两步；run_project_summary
    // 要求 completed 才继续）。
    store
        .update_project(
            project.id,
            &ProjectPatch {
                status: Some("in_progress".into()),
                ..Default::default()
            },
        )
        .unwrap();
    store
        .update_project(
            project.id,
            &ProjectPatch {
                status: Some("completed".into()),
                ..Default::default()
            },
        )
        .unwrap();

    let parent = store
        .create_issue(NewIssue {
            title: "父单".to_string(),
            description: String::new(),
            priority: 2,
            project_id: Some(project.id),
            creator: Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();

    let deps = BoardReviewDeps {
        store: store.clone(),
        workspace: dir.clone(),
        home: dir.clone(),
        moderator_loop: Arc::new(std::sync::OnceLock::new()), // 空槽 = 主 agent 未就绪
        cluster: Arc::new(nemesis_cluster::cluster::Cluster::new(
            nemesis_cluster::types::ClusterConfig {
                node_id: "node-a".to_string(),
                bind_address: "127.0.0.1:0".to_string(),
                peers: vec![],
                node_name: String::new(),
            },
        )),
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    };

    let err = super::run_project_summary(&deps, project.id)
        .await
        .expect_err("主 agent 未就绪必须 Err");
    assert!(
        err.contains("主 agent"),
        "失败原因应指明主 agent 未就绪: {err}"
    );

    super::summarize_fail_note(&deps, project.id, &err);

    // 顶层父单收到失败评论（诚实留痕）。
    let comments = store.list_comments(parent.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("AI 收口总结生成失败") && c.content.contains(err.as_str())),
        "父单应有失败评论: {comments:?}"
    );
    // 档案 timeline 落 summary 失败事件。
    let timeline = std::fs::read_to_string(archive_dir.join("timeline.jsonl")).unwrap_or_default();
    assert!(
        timeline.contains("收口总结生成失败"),
        "timeline 应有失败事件: {timeline}"
    );
    // 收口状态机不被失败回滚：项目保持 completed。
    assert_eq!(store.get_project(project.id).unwrap().status, "completed");
    // 项目单清单可正常拉取（失败路径没有弄脏过滤器语义）。
    let _ = store
        .list_issues(&IssueFilter {
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- 启动重放守卫（R4-BUG-2：评审链非重启韧性） ----------

/// 重放守卫夹具：建一张临时 store + 一个走到 in_review 的单据，
/// 返回 (store, issue_id, actor)。
fn replay_guard_fixture(name: &str) -> (Arc<nemesis_board::BoardStore>, i64, nemesis_board::Actor) {
    use nemesis_board::{Actor, IssueStatus, NewIssue};
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-review-replay-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    let actor = Actor::agent("node-a");
    let issue = store
        .create_issue(NewIssue {
            title: format!("重放守卫 {name}"),
            description: String::new(),
            priority: 2,
            creator: actor.clone(),
            ..Default::default()
        })
        .expect("create issue");
    // backlog → in_progress → in_review（合法链）。
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &actor)
        .expect("to in_progress");
    store
        .transition_issue(issue.id, IssueStatus::InReview, &actor)
        .expect("to in_review");
    (store, issue.id, actor)
}

#[test]
fn replay_guard_never_reviewed_replays() {
    // in_review 但从未评审（崩溃杀掉 spawn 的典型窗口）→ 重放。
    let (store, id, _actor) = replay_guard_fixture("never-reviewed");
    assert!(!super::review_already_concluded(&store, id));
}

#[test]
fn replay_guard_decided_sticks() {
    // 转移后评审已定案（含转人工）→ sticky，不重放（重放不得翻盘）。
    let (store, id, actor) = replay_guard_fixture("decided");
    store
        .add_activity(
            id,
            &actor,
            "auto_decide",
            Some(r#"{"decision":"auto_accept"}"#),
        )
        .unwrap();
    assert!(super::review_already_concluded(&store, id));
}

#[test]
fn replay_guard_stale_decide_replays() {
    // 旧轮结论（重派回 in_progress 再交付进 in_review）→ 本轮评审还没跑 → 重放。
    let (store, id, actor) = replay_guard_fixture("stale-decide");
    store
        .add_activity(
            id,
            &actor,
            "auto_decide",
            Some(r#"{"decision":"redispatch"}"#),
        )
        .unwrap();
    store
        .transition_issue(id, nemesis_board::IssueStatus::InProgress, &actor)
        .unwrap();
    store
        .transition_issue(id, nemesis_board::IssueStatus::InReview, &actor)
        .unwrap();
    assert!(!super::review_already_concluded(&store, id));
}

#[test]
fn replay_guard_rollback_window_sticks() {
    // audit.rollback 的人工纠错窗口：rollback 落的 status_changed 活动
    // details 是 audit_rollback 文本（非 JSON），不计为正常 in_review 转移
    // → in_review 归属上一次已定案的转移 → 不重放（重放会立刻翻盘毁掉窗口）。
    let (store, id, actor) = replay_guard_fixture("rollback");
    store
        .add_activity(
            id,
            &actor,
            "auto_decide",
            Some(r#"{"decision":"auto_accept"}"#),
        )
        .unwrap();
    store
        .add_activity(
            id,
            &actor,
            "status_changed",
            Some("audit_rollback:done→in_review:activity_id=1"),
        )
        .unwrap();
    assert!(super::review_already_concluded(&store, id));
}

#[test]
fn replay_guard_decide_without_transition_sticks() {
    // 有结论但查不到正常进入转移（旧数据）→ 保守不重放。
    let (store, id, actor) = replay_guard_fixture("no-enter");
    // 直接插一条 auto_decide，但把它前面唯一的正常转移换成非 in_review 目标
    // 是造不出「无进入转移」的——改用 rollback 形态把唯一转移标记为非正常。
    store
        .add_activity(
            id,
            &actor,
            "status_changed",
            Some("audit_rollback:done→in_review:activity_id=1"),
        )
        .unwrap();
    store
        .add_activity(
            id,
            &actor,
            "auto_decide",
            Some(r#"{"decision":"auto_accept"}"#),
        )
        .unwrap();
    assert!(super::review_already_concluded(&store, id));
}

#[test]
fn replay_guard_project_conclusion_probe() {
    // 项目级结论探测：任一顶层父单携带本项目的 auto_decide → 已结论。
    use nemesis_board::{Actor, NewIssue};
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-review-replay-proj-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    let actor = Actor::agent("node-a");
    // 引用完整性闸（2026-10-17 系统性检查）后 create_issue 校验 project 存在——
    // fixture 必须先建真实项目再用其 id。
    let proj7 = store
        .create_project("重放项目7", "", None, "📌", "", None)
        .unwrap()
        .id;
    let proj9 = store
        .create_project("重放项目9", "", None, "📌", "", None)
        .unwrap()
        .id;
    let mk = |title: &str, pid: i64| -> i64 {
        store
            .create_issue(NewIssue {
                title: title.to_string(),
                description: String::new(),
                priority: 2,
                creator: actor.clone(),
                project_id: Some(pid),
                ..Default::default()
            })
            .unwrap()
            .id
    };
    let p1 = mk("父单1", proj7);
    let p2 = mk("父单2", proj7);
    // 无项目级结论 → 重放。
    assert!(!super::project_review_already_concluded(
        &store,
        &[p1, p2],
        proj7
    ));
    // 父单2 带项目 7 的收口结论（escalate sticky 同样算）→ 不重放。
    store
        .add_activity(
            p2,
            &actor,
            "auto_decide",
            Some(
                format!(r#"{{"decision":"project_escalate_human","verdict":"fail","project_id":{proj7}}}"#)
                    .as_str(),
            ),
        )
        .unwrap();
    assert!(super::project_review_already_concluded(
        &store,
        &[p1, p2],
        proj7
    ));
    // 结论挂在其他项目名下（项目9）→ 项目 7 仍视为未结论。
    let p3 = mk("父单3", proj9);
    store
        .add_activity(
            p3,
            &actor,
            "auto_decide",
            Some(
                format!(
                    r#"{{"decision":"project_complete","verdict":"pass","project_id":{proj9}}}"#
                )
                .as_str(),
            ),
        )
        .unwrap();
    assert!(!super::project_review_already_concluded(
        &store,
        &[p1],
        proj7
    ));
}

// ---------- render_project_artifacts_evidence（F-U3-6，2026-09-15 U3）----------
// 父单收口评审输入此前只有 worker Delivery 声明摘要——U3 真机实证评审员
// 因「产物内容未随报提供」对齐全在案的产物判 UNSURE 转人工。证据段把
// 项目目录真实清单+内容节选交给评审员，声明 vs 实物可对照。

#[test]
fn artifacts_evidence_lists_files_with_content_and_excludes_pipeline_dirs() {
    let root = std::env::temp_dir().join(format!("u3f6-a-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::create_dir_all(root.join("records/NB-10/execution")).unwrap();
    std::fs::create_dir_all(root.join("__pycache__")).unwrap();
    std::fs::write(root.join("report.md"), "# 测试报告\n20 passed").unwrap();
    std::fs::write(
        root.join("tests/test_calc.py"),
        "def test_add():\n    assert 1+1==2",
    )
    .unwrap();
    std::fs::write(root.join("calculator.py"), "def add(a,b):\n    return a+b").unwrap();
    std::fs::write(root.join(".baseline.json"), "{}").unwrap();
    std::fs::write(root.join("records/x.txt"), "should be excluded").unwrap();
    std::fs::write(root.join("__pycache__/c.pyc"), b"\x00\x01").unwrap();

    let out = super::render_project_artifacts_evidence(&root);
    assert!(out.contains("项目目录实物证据"), "标题在场");
    assert!(out.contains("report.md"), "根文件在清单: {out}");
    assert!(out.contains("tests/test_calc.py"), "子目录文件用相对路径");
    assert!(out.contains("20 passed"), "小文本附内容节选");
    assert!(!out.contains("records/x.txt"), "records/ 管线目录排除");
    assert!(!out.contains(".baseline.json"), "基线戳排除");
    assert!(!out.contains("__pycache__/c.pyc"), "__pycache__ 排除");
    let _ = std::fs::remove_dir_all(&root);
}

/// SAN-09：排除谓词与 transfer walk 同源（.venv 等依赖/缓存目录不挤占
/// 评审 prompt 上限）+ symlink 不跟随（外部树不得拉进评审 prompt）。
#[test]
fn artifacts_evidence_excludes_noise_dirs_and_never_follows_symlinks() {
    let root = std::env::temp_dir().join(format!("san9-a-{}", std::process::id()));
    let outside = std::env::temp_dir().join(format!("san9-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(root.join(".venv/lib")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("delivery.md"), "# 交付").unwrap();
    std::fs::write(root.join("src/main.py"), "print('hi')").unwrap();
    std::fs::write(root.join(".venv/lib/pkg.py"), "noise").unwrap();
    std::fs::write(outside.join("secret.txt"), "top secret").unwrap();

    let out = super::render_project_artifacts_evidence(&root);
    assert!(out.contains("delivery.md"), "真实交付在清单: {out}");
    assert!(out.contains("src/main.py"), "普通子目录照常列举");
    assert!(
        !out.contains(".venv"),
        "transfer walk 同源排除 .venv: {out}"
    );
    assert!(!out.contains("pkg.py"), ".venv 内容不进 prompt");

    // symlink → 外部树（Windows 无特权可能建不出来；建不出即跳过该臂）。
    #[cfg(unix)]
    let link_ok = std::os::unix::fs::symlink(&outside, root.join("external")).is_ok();
    #[cfg(windows)]
    let link_ok = std::os::windows::fs::symlink_dir(&outside, root.join("external")).is_ok();
    if link_ok {
        let out = super::render_project_artifacts_evidence(&root);
        assert!(
            !out.contains("secret.txt"),
            "symlink 不跟随：外部树不得进评审 prompt: {out}"
        );
        assert!(!out.contains("external"), "symlink 条目本身也不列举: {out}");
    }
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn artifacts_evidence_empty_dir_and_unreadable_dir_are_honest() {
    // 空目录 → 诚实注记「无任何交付产物」。
    let empty = std::env::temp_dir().join(format!("u3f6-b-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    let out = super::render_project_artifacts_evidence(&empty);
    assert!(out.contains("项目目录为空"), "{out}");
    let _ = std::fs::remove_dir_all(&empty);

    // 目录不可读（用文件路径冒充目录）→ 诚实注记不 panic。
    let not_dir = std::env::temp_dir().join(format!("u3f6-c-{}", std::process::id()));
    std::fs::write(&not_dir, "i am a file").unwrap();
    let out = super::render_project_artifacts_evidence(&not_dir);
    assert!(out.contains("项目目录不可读"), "{out}");
    let _ = std::fs::remove_file(&not_dir);
}

#[test]
fn artifacts_evidence_skips_large_and_binary_files_from_content_but_lists_them() {
    let root = std::env::temp_dir().join(format!("u3f6-d-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // 大文本（> 64KB 节选上限）：列清单不附内容。2026-10-17 上限 8KB→64KB
    // （NB-101 项目收口误升级根修：白盒验收报告 12.9KB 被旧上限整文件排除）。
    let big = "x".repeat(70 * 1024);
    std::fs::write(root.join("big.txt"), &big).unwrap();
    // 二进制扩展名：列清单不附内容。
    std::fs::write(root.join("model.bin"), b"\x00\x01\x02").unwrap();

    let out = super::render_project_artifacts_evidence(&root);
    assert!(
        out.contains("big.txt") && out.contains("71680 字节"),
        "{out}"
    );
    assert!(!out.contains("xxxxx"), "大文件不附内容节选");
    assert!(out.contains("model.bin"), "二进制也在清单");
    assert!(out.contains("未附内容节选"), "诚实披露哪些文件没有节选");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn artifacts_evidence_excerpts_follow_mtime_desc_final_evidence_first() {
    // NB-101 项目收口误升级根修（2026-10-17）：旧实现按路径字母序消耗节选
    // 预算，外围产物（design/*、evidence/*.json）吃光 12 文件预算，最终
    // 验收报告从未进入评审视野。节选必须按修改时间倒序——收口期最后触碰
    // 的文件（最终证据）排最前；「early.md」路径序在前但 mtime 更早。
    let root = std::env::temp_dir().join(format!("u3f6-e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("design")).unwrap();
    std::fs::write(root.join("design/ui-spec.md"), "design doc body").unwrap();
    std::fs::write(root.join("early.md"), "EARLY_MARKER 旧产物").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(80));
    std::fs::write(root.join("zz-final-report.md"), "FINAL_MARKER 最终验收报告").unwrap();

    let out = super::render_project_artifacts_evidence(&root);
    assert!(
        out.contains("FINAL_MARKER"),
        "最终证据必须有内容节选: {out}"
    );
    assert!(out.contains("EARLY_MARKER"), "预算充足时旧产物也有节选");
    let final_pos = out
        .find("【zz-final-report.md】")
        .expect("final excerpt header");
    let early_pos = out.find("【early.md】").expect("early excerpt header");
    assert!(
        final_pos < early_pos,
        "节选按 mtime 倒序：最终证据在前（final@{final_pos} < early@{early_pos}）"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn artifacts_evidence_walk_not_amputated_by_listing_cap() {
    // NB-101 项目收口四轮误升级根修（2026-10-17）：旧实现用清单上限 200
    // 早停 walk——清单展示与节选候选池共用同一截断，路径序靠后的文件既
    // 不进清单也不进节选池，评审员据「报告引用的测试源文件不在清单」误判
    // 交付不一致（实录：清单停在 test/ 第 3 个文件，integration/regression/
    // sim/ui/world 测试全缺席）。修后：候选池吃满全树，清单截断只是展示
    // 预算且注记显式否定「不在清单=不存在」。
    let root = std::env::temp_dir().join(format!("u3f6-walk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // 造 250 个路径序靠前的旧文件（filler-000..249），把「新文件」挤到
    // 清单展示位之后。
    for i in 0..250 {
        std::fs::write(root.join(format!("filler-{i:03}.md")), "filler").unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(80));
    std::fs::write(root.join("zz-late-test-file.test.js"), "LATE_TEST_MARKER").unwrap();

    let out = super::render_project_artifacts_evidence(&root);
    // 节选候选池不受清单展示上限影响：路径序最末、mtime 最新的文件必须有节选。
    assert!(
        out.contains("LATE_TEST_MARKER"),
        "路径序靠后的最新文件必须进节选池: {}",
        &out[..out.len().min(400)]
    );
    // 全树 251 个文件：清单展示上限内不截断（400 > 251），总数如实。
    assert!(
        out.contains("共 251 个文件："),
        "{out_first}",
        out_first = &out[..out.len().min(400)]
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------- render_discipline_evidence（件4 组件5，纪律闭环评审注入）----------
// 变更集携带 `.discipline/` 时注入声明+证伪证据（抗糊弄二道闸：闸门保证
// 存在性/证伪真跑过，声明质量交评审员对照判断）；非纪律任务诚实跳过。
// 数据源 = 合并 commit 内 blob 原文（F-U3-7 同源，非执行者自述）。

#[test]
fn discipline_evidence_renders_declaration_and_falsification_from_commit() {
    let root = std::env::temp_dir().join(format!("disc-ev-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".discipline")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join(".discipline/declaration.json"),
        serde_json::json!({
            "root_cause": "src/parser.rs:42 未判空",
            "truth_source": "issue #12 复现栈",
            "invariant": "既有签名不破坏",
            "impact": "仅 parser 模块",
            "single_variable": "只改 parse_expr 判空分支",
            "falsification_cmd": "cargo test parse_expr",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join(".discipline/falsification-1.json"),
        serde_json::json!({
            "run": 1, "session": "sk-1",
            "command": "cargo test parse_expr", "passed": false,
            "output_excerpt": "test parse_expr ... FAILED",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join(".discipline/falsification-2.json"),
        serde_json::json!({
            "run": 2, "session": "sk-1",
            "command": "cargo test parse_expr", "passed": true,
            "output_excerpt": "test result: ok. 1 passed",
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(root.join("src/parser.rs"), "fn parse_expr() {}").unwrap();
    nemesis_board::git_repo::ensure_repo(&root).unwrap();
    nemesis_board::git_repo::commit_worktree(&root, "fix parser with discipline").unwrap();
    let oid = nemesis_board::git_repo::head_commit_hex(&root)
        .expect("head")
        .expect("commit exists");
    let files = nemesis_board::git_repo::commit_changed_files(&root, &oid).unwrap();

    let out = super::render_discipline_evidence(&root, &oid, &files)
        .expect("变更集含 .discipline/ 必须注入");
    assert!(out.contains("纪律闭环证据"), "标题在场: {out}");
    assert!(out.contains("src/parser.rs:42"), "声明 root_cause 原文在场");
    assert!(out.contains("证伪记录 falsification-1.json"), "证伪1在场");
    assert!(out.contains("证伪记录 falsification-2.json"), "证伪2在场");
    assert!(out.contains("✅ 通过"), "证伪2通过态: {out}");
    assert!(out.contains("❌ 未通过"), "证伪1失败态: {out}");
    assert!(out.contains("cargo test parse_expr"), "证伪命令在场");
    assert!(out.contains("评审提示"), "抗糊弄二道闸评审提示在场");

    // 变更集不含 .discipline/（非纪律任务）→ None 诚实跳过（不拿陈旧
    // 产物污染无关任务评审）。
    let non_disc: Vec<_> = files
        .iter()
        .filter(|(p, _)| !p.starts_with(".discipline/"))
        .cloned()
        .collect();
    assert!(super::render_discipline_evidence(&root, &oid, &non_disc).is_none());
    let _ = std::fs::remove_dir_all(&root);
}

// ---- S-O2 迟到评审守卫（2026-09-16 showcase 复跑 NB-2 实证）----

#[test]
fn stale_review_guard_passes_in_review_and_discards_after_manual_move() {
    let (deps, _ws) = review_deps("s-o2-guard");
    let issue = issue_in_review(&deps.store, "守卫单测", "", "交付内容");

    // 单据仍在 in_review → 结论有效。
    assert!(
        super::review_still_relevant(&deps, issue.id, ReviewVerdict::Pass.as_str(), "子单验收"),
        "in_review 单据的评审结论应放行"
    );

    // 人工干预在先（cancel→reopen，NB-2 实证路径）→ 结论丢弃 + 审计留痕。
    let reviewer = nemesis_board::Actor::agent("human-admin");
    deps.store
        .transition_issue(issue.id, IssueStatus::Cancelled, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::Backlog, &reviewer)
        .unwrap();
    assert!(
        !super::review_still_relevant(&deps, issue.id, ReviewVerdict::Pass.as_str(), "子单验收"),
        "非 in_review 单据的迟到结论必须丢弃"
    );
    // 状态未被守卫副作用改动。
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::Backlog
    );
    // 审计留痕：stale_review_discarded 决策已入账。
    let acts = deps.store.list_activity(issue.id).unwrap();
    let discarded = acts
        .iter()
        .find(|a| {
            a.action == "auto_decide"
                && a.details
                    .as_deref()
                    .unwrap_or("")
                    .contains("stale_review_discarded")
        })
        .expect("必须有 stale_review_discarded 审计条目");
    let details: serde_json::Value =
        serde_json::from_str(discarded.details.as_deref().unwrap_or("{}")).unwrap();
    assert_eq!(details["current_status"], "backlog");
    assert_eq!(details["verdict"], "PASS");
}

/// 复刻 NB-2 实证全链（2026-09-16 showcase 复跑）：mock provider 在评审
/// LLM「在飞」期间执行人工 cancel→reopen（真实时序——起点校验时单据还在
/// in_review，结论返回时已被人工改走），并返回合法 PASS 结论 → 迟到 PASS
/// 必须整体让位：不落「验收通过」评论、不置 done、不重派（backlog→done
/// 是合法边，transition_issue 不挡——这正是必须显式复查的原因）。
#[tokio::test]
async fn stale_review_anchor_fail_yields_to_manual_intervention() {
    use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::AgentConfig;

    struct StaleRaceProvider {
        store: Arc<nemesis_board::BoardStore>,
        issue_id: i64,
    }
    #[async_trait::async_trait]
    impl LlmProvider for StaleRaceProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<nemesis_agent::types::ChatOptions>,
            _tools: Vec<nemesis_agent::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            // 模拟「评审在飞窗口内」的人工干预（幂等：只在仍 in_review 时改）。
            let human = nemesis_board::Actor::agent("human-admin");
            if self.store.get_issue(self.issue_id).unwrap().status == IssueStatus::InReview {
                self.store
                    .transition_issue(self.issue_id, IssueStatus::Cancelled, &human)
                    .unwrap();
                self.store
                    .transition_issue(self.issue_id, IssueStatus::Backlog, &human)
                    .unwrap();
            }
            // 返回合法 PASS 结论——结论本身完全成立（旧交付线程），
            // 但单据已归人工，迟到结论必须让位。
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: r#"{"verdict":"PASS","reasons":["自检对照成立"],"gap":""}"#.to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let (mut deps, _ws) = review_deps("s-o2-fullchain");
    let issue = issue_in_review(&deps.store, "迟到评审让位", "", "## 结论\n交付完成");
    deps.store
        .insert_dispatch(
            "task-so2-1",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch("task-so2-1", nemesis_board::models::dispatch_state::DONE)
        .unwrap();

    let provider = StaleRaceProvider {
        store: deps.store.clone(),
        issue_id: issue.id,
    };
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    let _ = deps.moderator_loop.set(Arc::new(AgentLoop::new(
        Box::new(provider),
        AgentConfig::default(),
    )));

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("守卫让位是正常闭环，不报错");
    assert!(!reviewed, "迟到结论让位应返回未处置");

    // 状态停在 backlog：迟到 PASS 不置 done（NB-2 实证的核心断言）。
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::Backlog,
        "人工 reopen 后迟到评审不得动状态"
    );
    // 派发数不变：无重派发车。
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "迟到结论不得触发重派"
    );
    // 无任何处置评论（✅ 验收通过 / ❌ 重派）落库。
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("agent 验收通过") || c.content.contains("次重派")),
        "迟到结论不得落处置评论: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 审计留痕可见（决策流可解释为什么这单没人管）。
    let acts = deps.store.list_activity(issue.id).unwrap();
    assert!(
        acts.iter().any(|a| a
            .details
            .as_deref()
            .unwrap_or("")
            .contains("stale_review_discarded")),
        "必须有 stale_review_discarded 审计留痕"
    );
}

// ===========================================================================
// 2026-09-24 覆盖率补测（board_review.rs 未覆盖行 sweep）
//
// 复用既有夹具（review_deps / issue_in_review / seed_dispatch），新增脚本化
// mock LLM 注入 moderator_loop，把此前只有 cluster-uat 端到端才碰得到的
// LLM 门控路径（三态处置 / 解析失败漏斗 / 自检取证 / 父单收口 / 项目收口 /
// 收口总结）拉进进程内单测。
// ===========================================================================

/// 脚本化 mock LLM：按序弹出脚本项（Ok=返回文本 / Err=调用失败），耗尽后
/// 回落 `fallback`。`on_call` 可选副作用钩子（模拟评审在飞窗口的状态变化）。
struct ScriptedLlm {
    script: std::sync::Mutex<std::collections::VecDeque<Result<String, String>>>,
    fallback: String,
    on_call: Option<Box<dyn Fn() + Send + Sync>>,
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ScriptedLlm {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        if let Some(hook) = &self.on_call {
            hook();
        }
        let next = self
            .script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front();
        let raw = match next {
            Some(Ok(text)) => text,
            Some(Err(e)) => return Err(e),
            None => self.fallback.clone(),
        };
        Ok(nemesis_agent::r#loop::LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: raw,
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 捕获 prompt 的 mock LLM：把每次调用的消息全文存下来（注入段断言用），
/// 固定返回 `reply`。
struct CapturingLlm {
    prompts: std::sync::Mutex<Vec<String>>,
    reply: String,
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for CapturingLlm {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        let joined = messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n---\n");
        self.prompts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(joined);
        Ok(nemesis_agent::r#loop::LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: self.reply.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 合法评审 JSON（reasons 固定一条，gap 可控）。
fn review_json(verdict: &str, gap: &str) -> String {
    format!(r#"{{"verdict":"{verdict}","reasons":["系统原因甲"],"gap":"{gap}"}}"#)
}

/// Box 适配壳：AgentLoop::new 收 `Box<dyn LlmProvider>`，而测试侧要保留
/// Arc 句柄读捕获状态（prompts / script 剩量）——委托壳包一层再装箱。
struct SharedLlm(std::sync::Arc<dyn nemesis_agent::r#loop::LlmProvider>);

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for SharedLlm {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        self.0.chat(model, messages, options, tools).await
    }
}

/// 把脚本化 provider 装进 moderator_loop（后置装配桥）。
fn attach_loop(
    deps: &BoardReviewDeps,
    provider: std::sync::Arc<dyn nemesis_agent::r#loop::LlmProvider>,
) {
    let _ = deps
        .moderator_loop
        .set(Arc::new(nemesis_agent::r#loop::AgentLoop::new(
            Box::new(SharedLlm(provider)),
            nemesis_agent::types::AgentConfig::default(),
        )));
}

/// 写 board 配置（deps.home/config.json；load_board_flags 现读真相源）。
fn write_board_config(home: &std::path::Path, board_json: &str) {
    std::fs::write(
        home.join("config.json"),
        format!(r#"{{"board":{board_json}}}"#),
    )
    .unwrap();
}

/// in_review + 一条已完结派发（node-b）+ 交付汇报的标准待验单。
fn delivered_issue(
    store: &nemesis_board::BoardStore,
    name: &str,
    task: &str,
) -> nemesis_board::Issue {
    let issue = issue_in_review(store, name, "", "## 结论\n交付完成");
    seed_dispatch(store, task, issue.id, "node-b");
    issue
}

/// 指定创建者的待验单（admin 创建者用于 @人 断言）。
fn issue_in_review_as(
    store: &nemesis_board::BoardStore,
    title: &str,
    creator: nemesis_board::Actor,
) -> nemesis_board::Issue {
    use nemesis_board::{IssueStatus, NewIssue};
    let reviewer = nemesis_board::Actor::agent("node-a");
    let issue = store
        .create_issue(NewIssue {
            title: title.to_string(),
            description: String::new(),
            priority: 2,
            creator,
            ..Default::default()
        })
        .unwrap();
    store
        .transition_issue(issue.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    store
        .transition_issue(issue.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    issue
}

// ---------- review_issue 入口闸（miss 457,477-478,486-490,522,549） ----------

#[tokio::test]
async fn sweep_skip_when_auto_review_disabled() {
    let (deps, _ws) = review_deps("sweep-auto-review-off");
    write_board_config(&deps.home, r#"{"auto_review":false}"#);
    let issue = issue_in_review(&deps.store, "总闸关闭", "", "交付");
    let before = deps.store.list_comments(issue.id).unwrap().len();
    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("总闸关闭是诚实跳过，不报错");
    assert!(!reviewed, "auto_review=false 应跳过");
    // 跳过路径不落任何**评审**评论：在场评论维持夹具预置集不变。
    let after = deps.store.list_comments(issue.id).unwrap();
    assert_eq!(after.len(), before, "跳过路径不得新增评论");
}

#[tokio::test]
async fn sweep_skip_when_issue_not_in_review() {
    let (deps, _ws) = review_deps("sweep-status-gate");
    // backlog 单：write_back 与人工竞态时让位人工，不覆盖。
    let issue = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "还在backlog".to_string(),
            description: String::new(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("非 in_review 是诚实跳过");
    assert!(!reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        nemesis_board::IssueStatus::Backlog,
        "跳过路径不得动状态"
    );
}

#[tokio::test]
async fn sweep_worker_report_falls_back_to_latest_agent_comment() {
    // 任务卡回显 Delivery 不算交付：降级取最新 agent 文本评论。
    // 夹具不带预置交付（issue_in_review_as 只建到 in_review）——有合法
    // Delivery 在场时永远轮不到降级臂，场景必须成立降级前提。
    let (deps, _ws) = review_deps("sweep-echo-fallback");
    let issue = issue_in_review_as(
        &deps.store,
        "回显降级",
        nemesis_board::Actor::agent("node-a"),
    );
    // 追加一条回显 Delivery + 一条真实 agent 文本评论。
    deps.store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: format!("{}\n任务卡原样抄回", nemesis_board::TASK_CARD_HEADER),
            parent_id: None,
            ctype: nemesis_board::CommentType::Delivery,
        })
        .unwrap();
    deps.store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "实际完成情况说明：功能A已完成".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Comment,
        })
        .unwrap();

    let provider = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: review_json("PASS", ""),
    });
    attach_loop(&deps, provider.clone());

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("降级评审必须闭环");
    assert!(reviewed);
    let prompts = provider.prompts.lock().unwrap().join("\n");
    assert!(
        prompts.contains("实际完成情况说明"),
        "降级路径应取最新 agent 文本评论为汇报"
    );
    // 回显 Delivery 不进「worker 汇报」槽（生产语义：只从交付输入剔除，
    // 评论线程仍保留在场作上下文——对全 prompt 负断言是错的）。
    let report_slot = prompts
        .split("## worker 汇报")
        .nth(1)
        .expect("prompt 必含 worker 汇报段")
        .split("\n## ")
        .next()
        .expect("报告段非空");
    assert!(
        report_slot.contains("实际完成情况说明"),
        "报告槽必须是降级 agent 评论: {report_slot}"
    );
    assert!(
        !report_slot.contains("任务卡原样抄回"),
        "任务卡回显不得混入交付输入槽: {report_slot}"
    );
}

// ---------- B2b 二段评审证据注入（miss 571-575）+ F-U3 变更集注入（584-649） ----------

/// 搭一个带 git 仓库的项目档案目录（脚手架 + 基线 commit），返回 (root, base_oid)。
fn git_project_root(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-review-sweep-proj-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    nemesis_board::archive::ensure_scaffold(&dir, 1, "p", "active").unwrap();
    nemesis_board::git_repo::ensure_repo(&dir).unwrap();
    std::fs::write(dir.join("common.h"), b"baseline\n").unwrap();
    let base = nemesis_board::git_repo::commit_worktree(&dir, "init baseline")
        .unwrap()
        .unwrap();
    (dir, base)
}

#[tokio::test]
async fn sweep_second_stage_evidence_is_injected_into_prompt() {
    let (deps, _ws) = review_deps("sweep-evidence-inject");
    let issue = delivered_issue(&deps.store, "二段证据注入", "t-sweep-ev-1");

    let provider = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: review_json("PASS", ""),
    });
    attach_loop(&deps, provider.clone());

    let ctx = ReviewCtx {
        allow_selfcheck: false,
        evidence: Some("测试日志：assertion passed ×3".to_string()),
        retry: 0,
    };
    let reviewed = review_issue(&deps, issue.id, ctx)
        .await
        .expect("二段评审闭环");
    assert!(reviewed);
    let prompts = provider.prompts.lock().unwrap().join("\n");
    assert!(
        prompts.contains("## 自检证据（执行 worker 取证回报）"),
        "二段评审 prompt 必须带证据段标题"
    );
    assert!(
        prompts.contains("assertion passed ×3"),
        "证据正文必须原样注入"
    );
}

#[tokio::test]
async fn sweep_merged_changeset_and_discipline_evidence_injected() {
    use nemesis_board::NewIssue;
    let (deps, _ws) = review_deps("sweep-merged-inject");
    let (root, _base) = git_project_root("merged-inject");
    let project = deps
        .store
        .create_project("p", "d", None, "", "", Some(root.to_str().unwrap()))
        .unwrap();

    // 变更集内容：文本文件（带内容节选）+ 二进制（只列清单）+ 纪律产物。
    std::fs::write(root.join("report.md"), "# 交付报告\n结论：完成\n").unwrap();
    std::fs::write(root.join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
    std::fs::create_dir_all(root.join(".discipline")).unwrap();
    std::fs::write(
        root.join(".discipline/declaration.json"),
        r#"{"root_cause":"a.cpp:1"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".discipline/falsification-1.json"),
        r#"{"run":1,"passed":true,"command":"cargo test x","output_excerpt":"ok"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".discipline/waive-audit.jsonl"),
        r#"{"reason":"超范围","ts":"t"}"#,
    )
    .unwrap();
    let oid = nemesis_board::git_repo::commit_worktree(&root, "merge changeset")
        .unwrap()
        .unwrap();

    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "变更集注入".to_string(),
            description: String::new(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    let reviewer = nemesis_board::Actor::agent("node-a");
    deps.store
        .transition_issue(issue.id, nemesis_board::IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(issue.id, nemesis_board::IssueStatus::InReview, &reviewer)
        .unwrap();
    deps.store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "## 结论\n交付完成".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Delivery,
        })
        .unwrap();
    // 系统合并记录（changeset_merged 活动，details 带 commit）——注入的触发键。
    deps.store
        .add_activity(
            issue.id,
            &nemesis_board::Actor::system("board"),
            crate::board_archive_ingest::ACTION_MERGED,
            Some(&serde_json::json!({ "commit": oid }).to_string()),
        )
        .unwrap();

    let provider = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: review_json("PASS", ""),
    });
    attach_loop(&deps, provider.clone());

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("变更集注入评审闭环");
    assert!(reviewed);
    let prompts = provider.prompts.lock().unwrap().join("\n");
    assert!(
        prompts.contains("## 变更集合并结果（系统客观数据）"),
        "必须有合并结果证据段"
    );
    assert!(prompts.contains("report.md"), "清单须含文本文件名");
    assert!(prompts.contains("交付报告"), "文本文件须附 blob 内容节选");
    assert!(prompts.contains("未附内容"), "二进制文件须有诚实注记");
    assert!(
        prompts.contains("## 纪律闭环证据（系统客观数据）"),
        "变更集携带 .discipline/ 时必须注入纪律证据段"
    );
    assert!(prompts.contains("declaration.json"), "声明文件须呈现");
    assert!(
        prompts.contains("✅ 通过（exit 0）"),
        "证伪记录须按结构化呈现通过态"
    );
    assert!(prompts.contains("waive 审计"), "waive 留痕须呈现");
}

// NB-102 真机实证（2026-10-15）：旧节选上限 2000 字符把 PRD 截在「核心玩法
// 循环」开头，评审员以「未展示功能需求/非功能需求/逐条验收标准，无法对照
// 验收标准确认」确定性 UNSURE → max_unsure_streak 2 轮即停车。回归锁：
// 单文件 12000 字符预算内的交付文本全文呈现（2000 字之后的段落必须在场）。
#[tokio::test]
async fn sweep_merged_changeset_content_beyond_old_snippet_cap_is_injected() {
    use nemesis_board::NewIssue;
    let (deps, _ws) = review_deps("sweep-blob-cap");
    let (root, _base) = git_project_root("blob-cap");
    let project = deps
        .store
        .create_project("p", "d", None, "", "", Some(root.to_str().unwrap()))
        .unwrap();

    let mut body =
        String::from("# PRD 变体 A\n\n## 核心玩法循环\n开头的玩法描述。\n\n## 功能需求\n");
    while body.chars().count() < 3000 {
        body.push_str("- 需求条目：系统应支持网格建造与拆除。\n");
    }
    body.push_str("\n\n## 非功能需求\n渲染 60fps。\n\n## 验收标准\nGiven 玩家点击网格 When 放置建筑 Then 建筑出现在网格上。\n");
    assert!(body.chars().count() > 2000, "夹具须超过旧 2000 上限");
    // 生产同形：工作集 seed commit（gitignore 尊重 /docs/，走 commit_worktree）
    // + worker 变更集经 merge_changeset 合入（内存 index 绕 gitignore——真实
    // NB-102 的 docs/prd-variant-A.md 正是经此路径入库）。
    std::fs::write(root.join("README.md"), "# demo\n").unwrap();
    let baseline = nemesis_board::git_repo::commit_worktree(&root, "工作集 seed")
        .unwrap()
        .expect("seed 工作集必有首笔 commit");
    let merged = nemesis_board::git_repo::merge_changeset(
        &root,
        &nemesis_board::git_repo::MergeInput {
            baseline_commit: baseline.clone(),
            upserts: vec![nemesis_board::git_repo::ChangesetFile {
                path: "docs/prd-variant-A.md".to_string(),
                content: body.into_bytes(),
                executable: false,
            }],
            deletions: vec![],
        },
    )
    .unwrap();
    let oid = match merged {
        nemesis_board::git_repo::MergeOutcome::Merged { commit_oid } => commit_oid,
        other => panic!("夹具合入须无冲突，实得 {other:?}"),
    };

    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "长文档节选回归".to_string(),
            description: String::new(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    let reviewer = nemesis_board::Actor::agent("node-a");
    deps.store
        .transition_issue(issue.id, nemesis_board::IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(issue.id, nemesis_board::IssueStatus::InReview, &reviewer)
        .unwrap();
    deps.store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "## 结论\n交付完成".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Delivery,
        })
        .unwrap();
    deps.store
        .add_activity(
            issue.id,
            &nemesis_board::Actor::system("board"),
            crate::board_archive_ingest::ACTION_MERGED,
            Some(&serde_json::json!({ "commit": oid }).to_string()),
        )
        .unwrap();

    let provider = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: review_json("PASS", ""),
    });
    attach_loop(&deps, provider.clone());

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("评审闭环");
    assert!(reviewed);
    let prompts = provider.prompts.lock().unwrap().join("\n");
    assert!(prompts.contains("docs/prd-variant-A.md"), "清单含交付文件");
    assert!(
        prompts.contains("## 功能需求") && prompts.contains("网格建造与拆除"),
        "旧 2000 上限之后的功能需求段必须注入: {}",
        &prompts[prompts.len().saturating_sub(3000)..]
    );
    assert!(
        prompts.contains("Then 建筑出现在网格上"),
        "文档尾部验收标准段必须注入"
    );
}

// ---------- render_discipline_evidence 退化形态（miss 1695-1696,1725-1742） ----------

#[test]
fn sweep_discipline_evidence_degenerate_forms_honest_notes() {
    let (root, _base) = git_project_root("discipline-degenerate");
    // 只提交一个非 JSON 的证伪记录（falsification-7 留在 files 列表但不在
    // commit 内 → 读取失败臂）。
    std::fs::create_dir_all(root.join(".discipline")).unwrap();
    std::fs::write(root.join(".discipline/falsification-2.json"), "不是json").unwrap();
    std::fs::write(root.join(".discipline/waive-audit.jsonl"), "{}\n").unwrap();
    let oid = nemesis_board::git_repo::commit_worktree(&root, "disc")
        .unwrap()
        .unwrap();

    let files = vec![
        (
            ".discipline/falsification-2.json".to_string(),
            "A".to_string(),
        ),
        (
            ".discipline/falsification-7.json".to_string(),
            "A".to_string(),
        ),
        (".discipline/waive-audit.jsonl".to_string(), "A".to_string()),
        ("src/main.rs".to_string(), "M".to_string()),
    ];
    let out = render_discipline_evidence(&root, &oid, &files).expect("有 .discipline/ 必须出段");
    assert!(
        out.contains("（变更集未携带或读取失败）"),
        "无 declaration.json 时须诚实注记"
    );
    assert!(out.contains("非 JSON，原文"), "解析失败的证伪记录须附原文");
    assert!(
        out.contains("（读取失败）"),
        "commit 内缺失的证伪记录须注记读取失败"
    );
    assert!(out.contains("waive 审计"), "waive 审计须呈现");
    // 无 .discipline/ → None 诚实跳过。
    assert_eq!(
        render_discipline_evidence(&root, &oid, &[("src/lib.rs".to_string(), "M".to_string())]),
        None,
        "非纪律任务必须返回 None"
    );
}

// ---------- chain_token_usage_ds（miss 318-335） ----------

#[tokio::test]
async fn sweep_chain_token_usage_ds_sums_root_and_child_tasks() {
    use nemesis_data::RequestLog;
    let dir = std::env::temp_dir().join(format!("nb-sweep-tokchain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let ds = Arc::new(nemesis_data::DataStore::open(&dir.join("data.db")).unwrap());

    let creator = nemesis_board::Actor::agent("node-a");
    let root = store
        .create_issue(nemesis_board::NewIssue {
            title: "根任务".to_string(),
            description: String::new(),
            priority: 2,
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let child = store
        .create_issue(nemesis_board::NewIssue {
            title: "子任务".to_string(),
            description: String::new(),
            priority: 2,
            creator,
            parent_issue_id: Some(root.id),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch(
            "tok-root-1",
            root.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    store
        .insert_dispatch(
            "tok-child-1",
            child.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();

    let mk_log = |task: &str, input: i64, output: i64| RequestLog {
        id: 0,
        trace_id: String::new(),
        model: "m".into(),
        provider_type: String::new(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: 0.0,
        latency_ms: 0,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 0,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: format!("cluster_rpc:node-b/{task}"),
    };
    ds.insert_request_log(&mk_log("tok-root-1", 150, 50))
        .unwrap();
    ds.insert_request_log(&mk_log("tok-child-1", 10, 5))
        .unwrap();
    ds.insert_request_log(&mk_log("unrelated", 999, 999))
        .unwrap();

    let total = chain_token_usage_ds(&ds, &store, root.id);
    assert_eq!(total, 215, "根 + 子任务的 token 必须聚合（无关任务不计）");
    // 无派发的单 = 0。
    let empty = store
        .create_issue(nemesis_board::NewIssue {
            title: "无派发".to_string(),
            description: String::new(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(chain_token_usage_ds(&ds, &store, empty.id), 0);
}

// ---------- run_review_llm（miss 1170-1193）/ run_review_panel（1216-1243） ----------

#[tokio::test]
async fn sweep_run_review_llm_call_failure_fails_immediately() {
    let (deps, _ws) = review_deps("sweep-llm-callfail");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "网络断了".to_string()
            )])),
            fallback: review_json("PASS", ""),
            on_call: None,
        }),
    );
    let mut prompt = "评审一下".to_string();
    let err = run_review_llm(
        deps.moderator_loop.get().unwrap(),
        &mut prompt,
        ReviewToolMode::NoTools,
        nemesis_prompts::board::ReviewTier::Thorough,
        None,
        None,
    )
    .await
    .expect_err("调用失败必须报错");
    assert!(
        err.contains("LLM 调用失败") && err.contains("网络断了"),
        "调用失败错误文本须带分类前缀: {err}"
    );
}

#[tokio::test]
async fn sweep_run_review_llm_exhausts_three_parse_rounds() {
    let (deps, _ws) = review_deps("sweep-llm-parsefail");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([
                Ok("第一轮乱码".to_string()),
                Ok("第二轮还是乱码".to_string()),
                Ok("第三轮依旧".to_string()),
            ])),
            fallback: String::new(),
            on_call: None,
        }),
    );
    let mut prompt = "评审一下".to_string();
    let err = run_review_llm(
        deps.moderator_loop.get().unwrap(),
        &mut prompt,
        ReviewToolMode::NoTools,
        nemesis_prompts::board::ReviewTier::Thorough,
        None,
        None,
    )
    .await
    .expect_err("3 轮解析全败必须报错");
    assert!(
        err.contains("连续 3 轮无法解析"),
        "解析失败错误文本须带轮数前缀: {err}"
    );
}

#[tokio::test]
async fn sweep_run_review_llm_retries_then_succeeds_and_readonly_mode() {
    let (deps, _ws) = review_deps("sweep-llm-retry");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([
                Ok("乱码".to_string()),
                Ok(review_json("FAIL", "缺陷X")),
            ])),
            fallback: String::new(),
            on_call: None,
        }),
    );
    let mut prompt = "评审一下".to_string();
    // ReadOnly 模式（B2a 本机 worker 多轮取证面）照常走同一解析回灌。
    let out = run_review_llm(
        deps.moderator_loop.get().unwrap(),
        &mut prompt,
        ReviewToolMode::ReadOnly { max_turns: 3 },
        nemesis_prompts::board::ReviewTier::Thorough,
        None,
        None,
    )
    .await
    .expect("重试后必须成功");
    assert_eq!(out.verdict, nemesis_board::ReviewVerdict::Fail);
    assert_eq!(out.gap, "缺陷X");
}

#[tokio::test]
async fn sweep_run_review_panel_single_delegates_and_multi_aggregates() {
    let (mut deps, _ws) = review_deps("sweep-panel");
    // 单路：直接透传（与历史行为字节等价）。
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Ok(review_json(
                "PASS", "",
            ))])),
            fallback: String::new(),
            on_call: None,
        }),
    );
    let out = run_review_panel(
        deps.moderator_loop.get().unwrap(),
        "评审一下",
        ReviewToolMode::NoTools,
        1,
        nemesis_prompts::board::ReviewTier::Thorough,
        None,
        None,
    )
    .await
    .expect("单路必须成功");
    assert_eq!(out.verdict, nemesis_board::ReviewVerdict::Pass);

    // 多路（3 检查员）：PASS/PASS/FAIL 多数票 → Pass。
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([
                Ok(review_json("PASS", "")),
                Ok(review_json("PASS", "")),
                Ok(review_json("FAIL", "缺陷")),
            ])),
            fallback: String::new(),
            on_call: None,
        }),
    );
    let out = run_review_panel(
        deps.moderator_loop.get().unwrap(),
        "评审一下",
        ReviewToolMode::NoTools,
        3,
        nemesis_prompts::board::ReviewTier::Thorough,
        None,
        None,
    )
    .await
    .expect("多数票聚合必须成功");
    assert_eq!(out.verdict, nemesis_board::ReviewVerdict::Pass);

    // 多路全败（2 检查员 × 3 轮乱码）：返回末路错误。
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::new()),
            fallback: "全是乱码".to_string(),
            on_call: None,
        }),
    );
    let err = run_review_panel(
        deps.moderator_loop.get().unwrap(),
        "评审一下",
        ReviewToolMode::NoTools,
        2,
        nemesis_prompts::board::ReviewTier::Thorough,
        None,
        None,
    )
    .await
    .expect_err("全路解析失败必须报错");
    assert!(err.contains("连续 3 轮无法解析"), "got: {err}");
}

// ===== review_issue LLM 语义臂全链（2026-09-25 覆盖补齐） =====
//
// 既有锚点短路/注入段测试覆盖了 review_issue 的判定前半程；本节把
// 「LLM 结论 → 三态处置」后半程通过真 review_issue 链路钉死：AutoAccept
// 收货、UNSURE 转人工、预算耗尽转人工、预算内重派、调用失败诚实评论、
// unlimited 解析失败重派、B2b 取证派发失败兜底、经验蒸馏。

fn decided_word(store: &nemesis_board::BoardStore, issue_id: i64) -> Option<String> {
    store
        .list_activity(issue_id)
        .ok()?
        .into_iter()
        .filter(|a| a.action == "auto_decide")
        .filter_map(|a| {
            a.details
                .as_deref()
                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                .and_then(|v| {
                    v.get("decision")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                })
        })
        .next_back()
}

#[tokio::test]
async fn llm_pass_auto_accept_settles_via_review_chain() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-pass-accept");
    write_board_config(&deps.home, r#"{"auto_accept":true}"#);
    let issue = delivered_issue(&deps.store, "语义通过自动收货", "t-llm-pass-1");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("PASS+auto_accept 全链闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::Done,
        "PASS + auto_accept=true 必须落 done"
    );
    assert_eq!(
        decided_word(&deps.store, issue.id).as_deref(),
        Some("auto_accept")
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("自动收货") && c.content.contains("系统原因甲")),
        "收货评论必须带理由渲染: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn llm_pass_without_auto_accept_suggests_manual() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-pass-manual");
    let issue = delivered_issue(&deps.store, "语义通过待人工", "t-llm-pass-2");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("PASS 默认配置闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "PASS + auto_accept=false 保持 in_review"
    );
    assert_eq!(
        decided_word(&deps.store, issue.id).as_deref(),
        Some("suggest_manual")
    );
    assert!(
        deps.store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("验收通过，待人工确认")),
        "必须落待人工确认评论"
    );
}

#[tokio::test]
async fn llm_unsure_escalates_and_keeps_in_review() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-unsure");
    let issue = delivered_issue(&deps.store, "语义无法定案", "t-llm-unsure-1");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("UNSURE", "证据不足"),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("UNSURE 闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "UNSURE 转人工不重派"
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 1);
    assert_eq!(
        decided_word(&deps.store, issue.id).as_deref(),
        Some("escalate_human")
    );
    assert!(
        deps.store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("🤷 验收 agent 无法定案，请人工裁决")),
        "必须落转人工评论"
    );
}

#[tokio::test]
async fn llm_fail_budget_exhausted_escalates_with_gap() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-fail-budget0");
    write_board_config(&deps.home, r#"{"max_redispatch":0}"#);
    let issue = delivered_issue(&deps.store, "预算为零转人工", "t-llm-fail-1");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "缺口甲"),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("FAIL+零预算闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "零预算 FAIL 转人工"
    );
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "零预算不重派"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    let esc = comments
        .iter()
        .find(|c| c.content.contains("验收 agent 无法定案"))
        .expect("必须落转人工评论");
    assert!(
        esc.content.contains("重派预算已耗尽（0 次）") && esc.content.contains("缺口甲"),
        "预算耗尽评论必须带轮数与差距: {}",
        esc.content
    );
}

#[tokio::test]
async fn llm_fail_within_budget_redispatches_and_records() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-fail-redispatch");
    let issue = delivered_issue(&deps.store, "预算内重派", "t-llm-fail-2");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "输出缺运行证明"),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("FAIL+预算内闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        2,
        "预算内 FAIL 必须发第二次派发"
    );
    assert_eq!(
        decided_word(&deps.store, issue.id).as_deref(),
        Some("redispatch")
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    let gap = comments
        .iter()
        .find(|c| c.content.contains("❌ agent 验收未通过"))
        .expect("必须落差距评论");
    assert!(
        gap.content.contains("第 1/2 次重派") && gap.content.contains("输出缺运行证明"),
        "差距评论必须带轮次与差距正文: {}",
        gap.content
    );
    // 重派臂发车后单据离开 in_review（dispatch_issue_core 联动）。
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
}

#[tokio::test]
async fn llm_call_failure_posts_honest_review_failed_comment() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-call-fail");
    let issue = delivered_issue(&deps.store, "调用失败诚实评论", "t-llm-callfail-1");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "网络断了".to_string()
            )])),
            fallback: String::new(),
            on_call: None,
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("调用失败按诚实评论闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "调用失败默认配置不重派"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.content.contains("验收评审自身失败"))
        .expect("必须落评审自身失败评论");
    assert!(
        note.content.contains("LLM 调用失败") && note.content.contains("网络断了"),
        "调用失败必须带分类前缀原文（不得误报成 3 轮解析失败）: {}",
        note.content
    );
}

// ===== G4 交付证据闸（2026-10-18 批六） =====
//
// issue.requires_evidence（AC 含 file: 锚点建单自动置位）∪ 全局
// board.review.require_delivery_evidence：PASS 放行前强制校验最新派发
// 任务在 tasks.db deliveries 的记录在场（delivered/confirmed）；缺失 =
// 确定性降级 FAIL 走既有 FAIL 决策表（零新增分支）。夹具用
// with_workspace（tasks.db 在场）保证「缺失」是账本事实而非缺 db 巧合。

#[tokio::test]
async fn evidence_gate_missing_delivery_downgrades_pass_to_fail() {
    use nemesis_board::IssueStatus;

    let (deps, ws) = review_deps_taskdb("evidence-missing");
    // file: 锚点 + 实物在场 → 锚点过 → 进 LLM 臂拿 PASS → 闸降级。
    std::fs::create_dir_all(ws.join("out")).unwrap();
    std::fs::write(ws.join("out").join("result.md"), "产物").unwrap();
    // max_redispatch=0：降级 FAIL 直接转人工（file: 锚点本就进不了重派臂
    // ——P1 拓扑硬闸拒绝远端目标 + file: 锚点派发）。
    write_board_config(&deps.home, r#"{"max_redispatch":0}"#);
    let issue = issue_in_review(
        &deps.store,
        "缺交付证据降级",
        "[CHECK] file:out/result.md exists",
        "## 结论\n交付完成",
    );
    assert!(
        issue.requires_evidence,
        "AC 含 file: 锚点建单自动置位 requires_evidence"
    );
    seed_dispatch(&deps.store, "t-evidence-miss", issue.id, "node-b");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("降级 FAIL 走既有决策表闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "零预算 + 降级 FAIL = 转人工（不凭自然语言 PASS 进 done）"
    );
    assert_eq!(
        decided_word(&deps.store, issue.id).as_deref(),
        Some("escalate_human"),
        "降级 FAIL 必须走既有 FAIL 决策表（零预算 → 转人工），无新分支"
    );
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "降级路径不额外派发"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("交付证据闸")),
        "必须落交付证据闸留痕评论: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert!(
        comments.iter().any(|c| c.content.contains("缺交付证据")),
        "差距正文必须带「缺交付证据」前缀: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn evidence_gate_present_delivery_passes_and_anchor_summary_merged() {
    use nemesis_board::IssueStatus;

    let (deps, ws) = review_deps_taskdb("evidence-present");
    std::fs::create_dir_all(ws.join("out")).unwrap();
    std::fs::write(ws.join("out").join("result.md"), "产物").unwrap();
    write_board_config(&deps.home, r#"{"auto_accept":true}"#);
    let issue = issue_in_review(
        &deps.store,
        "交付记录在场放行",
        "[CHECK] file:out/result.md exists",
        "## 结论\n交付完成",
    );
    seed_dispatch(&deps.store, "t-evidence-hit", issue.id, "node-b");
    // worker 回调已落账（A 端 handle_task_complete 同款：kind=cluster_result、
    // 幂等键=task_id、落库即 delivered）。F5：夹具镜像真实 wire 形态——
    // payload 必须带 delivery 结构（capture：commit/tree 不可变交付点 +
    // patch_path 留底），空壳行不再满足闸。
    deps.cluster
        .task_db()
        .unwrap()
        .record_delivery(
            "cluster_result",
            None,
            "t-evidence-hit",
            &serde_json::json!({
                "node": "node-b",
                "delivery": {
                    "commit": "evidencecommit01",
                    "tree": "evidencetree01",
                    "patch_truncated": false,
                    "patch_path": "logs/deliveries/t-evidence-hit.patch"
                }
            }),
        )
        .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("交付在场 PASS 全链闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::Done,
        "交付记录在场 → 闸放行 → auto_accept 落 done"
    );
    assert!(
        !deps
            .store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("交付证据闸")),
        "在场时不得落降级留痕评论"
    );
    // anchor_summary 注记：锚点双检结果已合并进交付 payload（审计单行可
    // 读），且浅合并不覆盖既有键。
    let rec = deps
        .cluster
        .task_db()
        .unwrap()
        .delivery_by_key("t-evidence-hit")
        .unwrap()
        .expect("交付记录在场");
    assert_eq!(rec.payload["node"], "node-b", "浅合并保留既有 payload 键");
    assert_eq!(
        rec.payload["anchor_summary"]["passed"],
        serde_json::Value::Bool(true),
        "锚点双检通过摘要必须注记进交付 payload: {}",
        rec.payload
    );
    assert_eq!(rec.payload["anchor_summary"]["total"], serde_json::json!(1));
    assert!(
        rec.payload["anchor_summary"]["detail"]
            .as_str()
            .unwrap()
            .contains("result.md"),
        "摘要明细含锚点目标: {}",
        rec.payload["anchor_summary"]["detail"]
    );
}

#[tokio::test]
async fn evidence_gate_global_switch_applies_to_plain_issues() {
    use nemesis_board::IssueStatus;

    let (deps, _ws) = review_deps_taskdb("evidence-global");
    // 普通 AC（无 file: 锚点）→ requires_evidence 不置位；全局开关加码。
    write_board_config(
        &deps.home,
        r#"{"max_redispatch":0,"review":{"require_delivery_evidence":true}}"#,
    );
    let issue = issue_in_review(
        &deps.store,
        "全局交付证据加码",
        "普通文字验收标准",
        "## 结论\n交付完成",
    );
    assert!(
        !issue.requires_evidence,
        "无 file: 锚点不置位（全局闸是加码不是替代）"
    );
    seed_dispatch(&deps.store, "t-evidence-global", issue.id, "node-b");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("全局开关降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "全局开关 + 无交付记录 → PASS 降级 FAIL → 零预算转人工"
    );
    assert_eq!(
        decided_word(&deps.store, issue.id).as_deref(),
        Some("escalate_human")
    );
    assert!(
        deps.store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("交付证据闸")),
        "全局开关路径同样落降级留痕评论"
    );
}

// ===== F5 结构完整性升级（二轮复评 2026-10-18）：空壳记录不再满足闸 =====
//
// 完成回调无条件记账（payload 可为 Null），「记录在场」不足以证明走了
// 交付管线。闸升级：payload.delivery 必须在场 + commit/tree 非空 +
// 截断必有 patch_path 留底；缺口文案分档。版本绑定 = 最新派发 task_id。

/// 全局开关 + 普通单 + 指定交付 payload 的公共夹具（PASS 臂直通断言用）。
async fn evidence_gate_fixture(
    payload: serde_json::Value,
    tag: &str,
) -> (BoardReviewDeps, std::path::PathBuf, i64) {
    let (deps, ws) = review_deps_taskdb(tag);
    write_board_config(
        &deps.home,
        r#"{"auto_accept":true,"review":{"require_delivery_evidence":true}}"#,
    );
    let issue = issue_in_review(
        &deps.store,
        &format!("证据结构 {tag}"),
        "普通文字验收标准",
        "## 结论\n交付完成",
    );
    seed_dispatch(&deps.store, &format!("t-{tag}"), issue.id, "node-b");
    deps.cluster
        .task_db()
        .unwrap()
        .record_delivery("cluster_result", None, &format!("t-{tag}"), &payload)
        .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );
    (deps, ws, issue.id)
}

#[tokio::test]
async fn evidence_gate_null_payload_shell_row_downgrades() {
    use nemesis_board::IssueStatus;

    // Null 空壳行（完成回调无条件记账的最坏形态）：记录在场但无 delivery
    // 结构 → 不再满足闸，降级 FAIL（零预算转人工）。
    let (deps, _ws, issue_id) =
        evidence_gate_fixture(serde_json::Value::Null, "evidence-null-shell").await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("空壳行降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::InReview,
        "Null 空壳 payload 不得再当交付证据（PASS 降级 FAIL → 转人工）"
    );
    let comments = deps.store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("无 delivery 结构")),
        "缺口文案必须指明 delivery 结构缺席: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn evidence_gate_truncated_with_patch_path_passes() {
    use nemesis_board::IssueStatus;

    // 截断 + patch_path 留底在场：结构完整 → 放行（G4 契约：截断必有留底）。
    let (deps, _ws, issue_id) = evidence_gate_fixture(
        serde_json::json!({
            "node": "node-b",
            "delivery": {
                "commit": "trunccommit01",
                "tree": "trunctree01",
                "patch_truncated": true,
                "patch_path": "logs/deliveries/t-evidence-trunc-ok.patch"
            }
        }),
        "evidence-trunc-ok",
    )
    .await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("截断有留底放行闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::Done,
        "截断 + patch_path 留底 = 结构完整 → 闸放行 → auto_accept 落 done"
    );
}

#[tokio::test]
async fn evidence_gate_truncated_without_patch_path_downgrades() {
    use nemesis_board::IssueStatus;

    // 截断但无 patch_path（G4 契约破坏形态）：降级 FAIL。
    let (deps, _ws, issue_id) = evidence_gate_fixture(
        serde_json::json!({
            "node": "node-b",
            "delivery": {
                "commit": "truncnoleave1",
                "tree": "truncnoleavt1",
                "patch_truncated": true
            }
        }),
        "evidence-trunc-bad",
    )
    .await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("截断无留底降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::InReview,
        "截断无留底 = 结构不完整 → 降级 FAIL → 零预算转人工"
    );
    let comments = deps.store.list_comments(issue_id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("patch_path")),
        "缺口文案必须指明留底缺失: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn evidence_gate_stale_attempt_record_does_not_satisfy() {
    use nemesis_board::IssueStatus;

    // 版本绑定：记录挂在旧派发 task_id 名下，最新派发无账 → 闸按最新锚查
    // 账本 → FAIL（评估方「产物属于旧 attempt」的确定性镜像）。
    let (deps, _ws) = review_deps_taskdb("evidence-stale");
    write_board_config(
        &deps.home,
        r#"{"max_redispatch":0,"review":{"require_delivery_evidence":true}}"#,
    );
    let issue = issue_in_review(
        &deps.store,
        "旧 attempt 记录不背书",
        "普通文字验收标准",
        "## 结论\n交付完成",
    );
    seed_dispatch(&deps.store, "t-evidence-stale-old", issue.id, "node-b");
    seed_dispatch(&deps.store, "t-evidence-stale-new", issue.id, "node-b");
    // 完整交付结构挂在旧 attempt 名下——结构再完整也满足不了新锚。
    deps.cluster
        .task_db()
        .unwrap()
        .record_delivery(
            "cluster_result",
            None,
            "t-evidence-stale-old",
            &serde_json::json!({
                "node": "node-b",
                "delivery": {
                    "commit": "stalecommit01",
                    "tree": "staletree01",
                    "patch_truncated": false,
                    "patch_path": "logs/deliveries/t-evidence-stale-old.patch"
                }
            }),
        )
        .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("旧 attempt 绑定降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "旧 attempt 记录不满足最新派发锚 → 降级 FAIL → 转人工"
    );
}

// ===== ②typed 证据块（诚实边界收口 2026-10-19） =====
//
// delivery.files / delivery.checks 在场时无论开关都验形态（path 复用
// anchor 形态闸 + sha256 64-hex + checks wire 契约）；缺席块行为不变。
// 严格闸 board.review.evidence_typed_required（默认 false）：true 后
// files 必须在场（git tree diff 确定性派生，缺席 = 证据面残缺）；checks
// 恒可选（没跑检查 = 诚实的事实陈述，强制会诱发造值）。

/// 合法 typed 块样例（files 1 条 + checks 1 条，形态全合规）。
fn typed_blocks_json() -> serde_json::Value {
    serde_json::json!({
        "node": "node-b",
        "delivery": {
            "commit": "typedcommit01",
            "tree": "typedtree001",
            "patch_truncated": false,
            "patch_path": "logs/deliveries/t-typed-ok.patch",
            "files_changed": 1,
            "files": [
                {"path": "src/lib.rs", "sha256": "0123abcd".repeat(8)}
            ],
            "checks": [
                {"tool": "exec", "cmd": "cargo test -p demo", "ok": true, "summary": "5 passed"}
            ]
        }
    })
}

// R5 回归锁（原诊断探针翻转）：真实「纯删除」交付在 R5 后是完整的证据面——
// capture 侧 files 恒序列化且含 delete 条目（old_sha256 承载证据），形态闸
// kind 分臂放行，评审通过离开 InReview。
#[tokio::test]
async fn deletion_only_capture_satisfies_strict_evidence_gate() {
    let tmp = tempfile::tempdir().unwrap();
    git2::Repository::init(tmp.path()).unwrap();
    std::fs::write(tmp.path().join("obsolete.txt"), "obsolete\n").unwrap();
    let cp = Arc::new(nemesis_agent::checkpoint::CheckpointStore::new(
        None,
        tmp.path().to_path_buf(),
    ));
    cp.begin(1, "review deletion baseline");
    let base = cp.current_tree_hex().unwrap();
    std::fs::remove_file(tmp.path().join("obsolete.txt")).unwrap();
    let delivery = crate::delivery_gate::capture_task_delivery_with_store(
        &cp,
        "t-review-delete-only",
        Some(&base),
        "node-b",
        Vec::new(),
    )
    .unwrap();
    assert_eq!(delivery["files_changed"], 1);
    let files = delivery["files"]
        .as_array()
        .expect("files 恒序列化（R5 契约）");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "obsolete.txt");
    assert_eq!(files[0]["kind"], "delete");
    assert!(
        files[0].get("sha256").is_none(),
        "delete 无新内容可哈希（sha256 空串不序列化）"
    );
    assert_eq!(
        files[0]["old_sha256"].as_str().unwrap().len(),
        64,
        "old_sha256 = 变更前 blob 摘要"
    );
    let (deps, _ws, id) = evidence_gate_fixture(
        serde_json::json!({"node":"node-b", "delivery":delivery}),
        "review-delete-only",
    )
    .await;
    write_board_config(
        &deps.home,
        r#"{"auto_accept":true,"review":{"require_delivery_evidence":true,"evidence_typed_required":true}}"#,
    );
    review_issue(&deps, id, ReviewCtx::first_stage())
        .await
        .unwrap();
    assert_eq!(
        deps.store.get_issue(id).unwrap().status,
        nemesis_board::IssueStatus::Done,
        "纯删除交付 = 完整证据 → 闸放行 → auto_accept 落 done"
    );
}

#[tokio::test]
async fn evidence_typed_blocks_valid_shape_passes() {
    use nemesis_board::IssueStatus;

    let (deps, _ws, issue_id) = evidence_gate_fixture(typed_blocks_json(), "typed-ok").await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("typed 块合法放行闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::Done,
        "files/checks 形态全合规 → 闸放行 → auto_accept 落 done"
    );
}

#[tokio::test]
async fn evidence_typed_files_bad_sha256_shape_fails() {
    use nemesis_board::IssueStatus;

    let mut payload = typed_blocks_json();
    payload["delivery"]["files"][0]["sha256"] = serde_json::json!("nothex");
    let (deps, _ws, issue_id) = evidence_gate_fixture(payload, "typed-badsha").await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("sha256 形态坏降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::InReview,
        "sha256 非 64-hex = 形态非法 → 降级 FAIL → 转人工"
    );
    assert!(
        deps.store
            .list_comments(issue_id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("typed files 块形态非法")),
        "缺口文案必须指明 files 形态非法: {:?}",
        deps.store
            .list_comments(issue_id)
            .unwrap()
            .iter()
            .map(|c| &c.content)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn evidence_typed_files_escape_path_fails() {
    use nemesis_board::IssueStatus;

    // `..` 越界组件（anchor 形态闸同源拒绝）与盘符形态双臂都 FAIL。
    for (tag, bad_path) in [
        ("typed-dotdot", "../../etc/passwd"),
        ("typed-drive", "C:\\evil.txt"),
    ] {
        let mut payload = typed_blocks_json();
        payload["delivery"]["files"][0]["path"] = serde_json::json!(bad_path);
        let (deps, _ws, issue_id) = evidence_gate_fixture(payload, tag).await;
        let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
            .await
            .expect("越界 path 降级闭环");
        assert!(reviewed);
        assert_eq!(
            deps.store.get_issue(issue_id).unwrap().status,
            IssueStatus::InReview,
            "非法 path「{bad_path}」必须 FAIL"
        );
        assert!(
            deps.store
                .list_comments(issue_id)
                .unwrap()
                .iter()
                .any(|c| c.content.contains("typed files 块形态非法")),
            "非法 path「{bad_path}」落形态非法留痕"
        );
    }
}

#[tokio::test]
async fn evidence_typed_files_non_array_fails() {
    use nemesis_board::IssueStatus;

    // 键在场但非数组 = 形态非法（不静默滑过）。
    let mut payload = typed_blocks_json();
    payload["delivery"]["files"] = serde_json::json!("not-an-array");
    let (deps, _ws, issue_id) = evidence_gate_fixture(payload, "typed-nonarray").await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("非数组 files 降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::InReview,
        "files 非数组 = 形态非法 → FAIL"
    );
    assert!(
        deps.store
            .list_comments(issue_id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("必须是数组")),
        "缺口文案必须指明数组形态要求"
    );
}

#[tokio::test]
async fn evidence_typed_checks_bad_entry_fails() {
    use nemesis_board::IssueStatus;

    // checks 条目缺 ok / 空 cmd 两种坏形态都 FAIL。
    for (tag, mut_entry) in [
        (
            "typed-check-noOk",
            serde_json::json!({"tool":"exec","cmd":"cargo test","summary":"x"}),
        ),
        (
            "typed-check-emptyCmd",
            serde_json::json!({"tool":"exec","cmd":"   ","ok":true,"summary":"x"}),
        ),
    ] {
        let mut payload = typed_blocks_json();
        payload["delivery"]["checks"][0] = mut_entry;
        let (deps, _ws, issue_id) = evidence_gate_fixture(payload, tag).await;
        let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
            .await
            .expect("checks 坏形态降级闭环");
        assert!(reviewed);
        assert_eq!(
            deps.store.get_issue(issue_id).unwrap().status,
            IssueStatus::InReview,
            "checks 条目坏形态（{tag}）必须 FAIL"
        );
        assert!(
            deps.store
                .list_comments(issue_id)
                .unwrap()
                .iter()
                .any(|c| c.content.contains("typed checks 块形态非法")),
            "checks 坏形态（{tag}）落形态非法留痕"
        );
    }
}

#[tokio::test]
async fn evidence_typed_absent_with_strict_on_fails() {
    use nemesis_board::IssueStatus;

    // 严格闸开 + files 缺席（旧 F5 形态交付）= FAIL：git 交付都应可派生
    // files，缺席 = 证据面残缺。checks 缺席照常可选（不因 checks 拦人）。
    let (deps, _ws, issue_id) = evidence_gate_fixture(
        serde_json::json!({
            "node": "node-b",
            "delivery": {
                "commit": "strictnocommit",
                "tree": "strictnotree01",
                "patch_truncated": false,
                "patch_path": "logs/deliveries/t-typed-strict.patch"
            }
        }),
        "typed-strict-absent",
    )
    .await;
    // 夹具配置之上翻严格闸（评审时读盘，后写生效）。
    write_board_config(
        &deps.home,
        r#"{"auto_accept":true,"review":{"require_delivery_evidence":true,"evidence_typed_required":true}}"#,
    );
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("严格闸 files 缺席降级闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::InReview,
        "严格闸开 + files 缺席 → FAIL → 转人工"
    );
    assert!(
        deps.store
            .list_comments(issue_id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("交付缺类型化证据块")),
        "缺口文案必须指明 typed 块缺席: {:?}",
        deps.store
            .list_comments(issue_id)
            .unwrap()
            .iter()
            .map(|c| &c.content)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn evidence_typed_absent_with_strict_off_passes() {
    use nemesis_board::IssueStatus;

    // 严格闸关（默认）+ files/checks 全缺席 = 旧形态照常放行（加法演进，
    // 存量行为不变的回归锁）。
    let (deps, _ws, issue_id) = evidence_gate_fixture(
        serde_json::json!({
            "node": "node-b",
            "delivery": {
                "commit": "legacycommit1",
                "tree": "legacytree01",
                "patch_truncated": false,
                "patch_path": "logs/deliveries/t-typed-legacy.patch"
            }
        }),
        "typed-legacy-ok",
    )
    .await;
    let reviewed = review_issue(&deps, issue_id, ReviewCtx::first_stage())
        .await
        .expect("严格闸关缺席放行闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue_id).unwrap().status,
        IssueStatus::Done,
        "默认闸关 + typed 块缺席 = 旧形态照常放行（烧入期零行为变化）"
    );
}

#[tokio::test]
async fn llm_parse_failure_unlimited_escalates_without_redispatch() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-parsefail-unlimited");
    write_board_config(&deps.home, r#"{"unlimited_mode":true}"#);
    let issue = delivered_issue(&deps.store, "无限模式解析失败", "t-llm-parsefail-1");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            // 恒乱码 → 解析失败 3 轮（非调用失败）→ 评审自身故障转人工。
            script: std::sync::Mutex::new(std::collections::VecDeque::new()),
            fallback: "不是 JSON 的评审输出".to_string(),
            on_call: None,
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("unlimited 解析失败按转人工闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "unlimited_mode 也不得因评审自身失败重派 worker（评审没出结论就没有重派授权）"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "评审自身失败 = 转人工：状态保持 in_review，不发车"
    );
    assert!(
        deps.store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("验收评审自身失败")
                && c.content.contains("连续 3 轮无法解析")),
        "必须落带失败原因的转人工评论"
    );
}

#[tokio::test]
async fn llm_need_evidence_selfcheck_dispatch_failure_falls_back() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("llm-selfcheck-fail");
    write_board_config(&deps.home, r#"{"review":{"selfcheck":true}}"#);
    let issue = delivered_issue(&deps.store, "取证派发失败兜底", "t-llm-b2b-1");
    // 空 RpcClient 无传输层 → call_with_timeout 必败 → 诚实兜底转人工。
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: r#"{"verdict":"UNSURE","reasons":["证据不足"],"gap":"缺运行输出","need_evidence":true,"evidence_request":"请贴运行输出"}"#.to_string(),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("取证失败兜底闭环");
    assert!(reviewed);
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("取证派发失败")),
        "必须有取证派发失败评论: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 兜底后按现有结论（UNSURE）转人工，不悬挂。
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("验收 agent 无法定案，请人工裁决")),
        "取证失败必须转人工兜底"
    );
    assert!(
        !deps.selfcheck.has_inflight(issue.id),
        "派发失败不得注册在途自检"
    );
}

// F6（2026-09-30）：AC 含 re: 交付文本锚点的单按需强制自检——全局
// board.review.selfcheck 关（默认）也触发；无锚点单维持默认关。
#[tokio::test]
async fn anchor_ac_forces_selfcheck_even_when_global_switch_off() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("f6-anchor-forced");
    // 不写 board config：review.selfcheck 默认关。
    // AC 带 [CHECK] re: 锚点，delivery 文本含 42 → 锚点过，进语义评审。
    let full = issue_in_review(
        &deps.store,
        "锚点单强制自检",
        "算式验证\n[CHECK] re:42",
        "## 结论\n交付完成：结果是 42",
    );
    seed_dispatch(&deps.store, "t-f6-anchor-1", full.id, "node-b");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: r#"{"verdict":"PASS","reasons":["锚点已过且结论明确"]}"#.to_string(),
        }),
    );

    let reviewed = review_issue(&deps, full.id, ReviewCtx::first_stage())
        .await
        .expect("F6 强制臂闭环");
    assert!(reviewed);
    let comments = deps.store.list_comments(full.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("取证派发失败")),
        "全局开关关时 re: 锚点单仍必须尝试自检取证（F6 强制臂）: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    // 取证派发失败兜底：Err 臂落「⛔ 取证派发失败」后按原 verdict 处置
    // （PASS + auto_accept 未开 → 停在 in_review 等人工，不悬挂）。
    assert_eq!(
        deps.store.get_issue(full.id).unwrap().status,
        IssueStatus::InReview
    );
    assert!(
        !deps.selfcheck.has_inflight(full.id),
        "派发失败不得注册在途自检"
    );

    // 对照：无 re: 锚点的 PASS 单，全局开关关 → 不尝试取证，PASS 正常处置。
    let plain = delivered_issue(&deps.store, "无锚点对照", "t-f6-plain-1");
    let reviewed2 = review_issue(&deps, plain.id, ReviewCtx::first_stage())
        .await
        .expect("对照单闭环");
    assert!(reviewed2);
    let plain_comments = deps.store.list_comments(plain.id).unwrap();
    assert!(
        !plain_comments.iter().any(|c| c.content.contains("取证")),
        "无锚点 + 全局关不得触发自检: {:?}",
        plain_comments
            .iter()
            .map(|c| &c.content)
            .collect::<Vec<_>>()
    );
    assert!(!deps.selfcheck.has_inflight(plain.id));
}

// ===== 降级交付游标不得被验收流程评论抢走（2026-10-08 NB-21 子单 22）=====
// worker 非结构化交付走 write_back 降级臂落 Comment 形态；F6 自检挂起评论
//（author=评审节点，同为 agent+Comment 形态）晚于交付写入时，二段验收的
// worker_report 游标曾被它抢走 → 锚点对「⏳ 验收暂缓…」文本判定必 FAIL，
// 重派/换节点均不可自愈（三连败转人工）。修复：fallback 限定作者 = 最新
// 派发行 worker_id（write_back 落库与派发行逐字同源）。
#[tokio::test]
async fn degraded_delivery_cursor_survives_review_meta_comments() {
    use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, _ws) = review_deps("degraded-cursor-meta");
    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "降级交付游标回归".to_string(),
            description: String::new(),
            priority: 2,
            acceptance_criteria: Some("算式验证\n[CHECK] re:42".to_string()),
            creator: Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let reviewer = Actor::agent("node-a");
    deps.store
        .transition_issue(issue.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    // 降级交付：write_back 无格式臂形态（ctype=Comment + 汇报前缀），
    // 作者 = 派发执行者 node-b（与派发行 worker_id 逐字同源）。
    deps.store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::agent("node-b"),
            content: "✅ worker 汇报完成：\n\n结果是 42".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    seed_dispatch(&deps.store, "t-degraded-cursor-1", issue.id, "node-b");
    // 验收流程评论晚于交付写入（= F6 挂起后的二段验收现场），走真实出口。
    post_review_comment(
        &deps.store,
        issue.id,
        &deps.cluster,
        "⏳ 验收暂缓（board.review.selfcheck）：评审 agent 需要更多证据（测试模拟 NB-21 现场文本）",
    )
    .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: r#"{"verdict":"PASS","reasons":["锚点已过且结论明确"]}"#.to_string(),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("二段评审闭环");
    assert!(reviewed);
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查通过")),
        "worker_report 游标必须保持 worker 交付（锚点对「结果是 42」判定）: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "验收流程评论不得冒充 worker 汇报抢游标: {:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn llm_experience_note_distilled_into_team_memory() {
    let (deps, _ws) = review_deps("llm-experience");
    write_board_config(&deps.home, r#"{"auto_accept":true}"#);
    let issue = delivered_issue(&deps.store, "经验蒸馏", "t-llm-exp-1");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: r#"{"verdict":"PASS","reasons":["对照成立"],"gap":"","experience":{"category":"调试","scope":"rust","content":"先看日志再改码"}}"#.to_string(),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("带经验槽位的评审闭环");
    assert!(reviewed);
    let mem = deps.store.list_team_memory(None, true).expect("查经验库");
    assert!(
        mem.iter()
            .any(|m| m.category == "调试" && m.content.contains("先看日志再改码")),
        "评审经验必须经 store_experience 入 team_memory: {:?}",
        mem.iter().map(|m| &m.category).collect::<Vec<_>>()
    );
}

// ===== review_parent_issue LLM 语义臂（PASS 收口 / FAIL 转人工 / 解析失败） =====

#[tokio::test]
async fn parent_llm_pass_auto_closes_and_settles() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("parent-llm-pass");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "父单语义收口", "", "（父单验收输入=子单汇总）");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("父单 PASS 闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::Done,
        "父单 PASS + auto_close_parent 必须落 done"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_auto_close")
    );
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("父单收口验收通过")),
        "必须落自动收口评论"
    );
}

#[tokio::test]
async fn parent_llm_fail_escalates_with_gap_comment() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("parent-llm-fail");
    // parent_remediation_max=0（关自动返工）= 本测钉「FAIL → 转人工」旧行为；
    // 默认开（2 轮）下的 FAIL 承接见 parent_llm_fail_auto_remediates_via_rework_child。
    write_board_config(
        &deps.home,
        r#"{"auto_close_parent":true,"review":{"parent_remediation_max":0}}"#,
    );
    let parent = issue_in_review(&deps.store, "父单语义未过", "", "（父单验收输入=子单汇总）");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "子单结论互相矛盾"),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("父单 FAIL 闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview,
        "父单 FAIL 转人工保持 in_review"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_escalate_human")
    );
    let comments = deps.store.list_comments(parent.id).unwrap();
    let esc = comments
        .iter()
        .find(|c| c.content.contains("父单收口验收未定案（FAIL）"))
        .expect("必须落父单转人工评论");
    assert!(
        esc.content.contains("差距：") && esc.content.contains("子单结论互相矛盾"),
        "父单 FAIL 评论必须带差距: {}",
        esc.content
    );
    assert!(
        esc.content.contains("自动返工未开启"),
        "承接被拒（配置关）必须在转人工评论注记原因: {}",
        esc.content
    );
    assert!(
        !deps
            .store
            .list_children(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.title.starts_with("返工：")),
        "配置关闭时不得创建返工子单"
    );
}

/// 父单收口 FAIL 自动返工承接（2026-10-16 NB-101「FAIL 弹回人工」根修）：
/// 差距具体（gap 非空）+ 默认配置（parent_remediation_max=2）→ 自动创建
/// 「返工：」子单（挂父单名下、差距原文进描述、继承项目/职能），父单保持
/// in_review 不弹人工，决策词 parent_auto_remediate，评论带返工单号与差距。
#[tokio::test]
async fn parent_llm_fail_auto_remediates_via_rework_child() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("parent-llm-remediate");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(
        &deps.store,
        "父单语义未过待返工",
        "",
        "（父单验收输入=子单汇总）",
    );
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "黑盒用例存在失败与阻塞未闭合"),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("父单 FAIL 承接闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview,
        "承接成功后父单保持 in_review（不转人工）"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_auto_remediate"),
        "承接路径决策词 = parent_auto_remediate"
    );
    let children = deps.store.list_children(parent.id).unwrap();
    let rem = children
        .iter()
        .find(|c| c.title.starts_with("返工："))
        .expect("必须自动创建返工子单");
    assert!(
        rem.title.contains("第 1/2 轮"),
        "返工单标题带轮次: {}",
        rem.title
    );
    assert!(
        rem.description.contains("黑盒用例存在失败与阻塞未闭合"),
        "差距原文必须进返工单描述: {}",
        rem.description
    );
    assert_eq!(
        rem.acceptance_criteria.as_deref(),
        Some(format!(
            "- 父单 {} 收口评审差距逐条闭合（差距清单见任务描述），每条附验证证据\n- 交付评论逐条列出：差距 → 修复动作 → 验证结果，不得有未闭合残留",
            parent.number
        ))
        .as_deref(),
        "返工单 AC = 差距闭合判据"
    );
    let comments = deps.store.list_comments(parent.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.content.contains("已自动创建返工子单"))
        .expect("必须落承接评论");
    assert!(
        note.content.contains(&rem.number) && note.content.contains("黑盒用例存在失败与阻塞未闭合"),
        "承接评论必须带返工单号与差距: {}",
        note.content
    );
}

/// 返工轮数预算（parent_remediation_max=2）：已有 2 个「返工：」子单（按
/// 尝试计轮，含 cancelled）→ 再 FAIL 不建第三单，落既有转人工（评论注记
/// 预算用满）。
#[tokio::test]
async fn parent_llm_fail_remediation_cap_exhausted_escalates() {
    use nemesis_board::NewIssue;
    let (deps, _ws) = review_deps("parent-llm-remediate-cap");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(
        &deps.store,
        "父单返工预算耗尽",
        "",
        "（父单验收输入=子单汇总）",
    );
    for round in 1..=2 {
        deps.store
            .create_issue(NewIssue {
                title: format!("返工：父单返工预算耗尽（第 {round}/2 轮）"),
                description: "历史返工单".to_string(),
                priority: 2,
                creator: nemesis_board::Actor::system("board-review"),
                parent_issue_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
    }
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "仍有差距"),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("预算耗尽 FAIL 闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.list_children(parent.id).unwrap().len(),
        2,
        "预算用满不得再建返工子单"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_escalate_human"),
        "预算用满落既有转人工"
    );
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("父单收口验收未定案（FAIL）")
                && c.content.contains("自动返工已用满轮数预算（2 轮）")),
        "转人工评论必须注记预算用满"
    );
}

#[tokio::test]
async fn parent_llm_parse_failure_posts_honest_comment() {
    let (deps, _ws) = review_deps("parent-llm-parsefail");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "父单评审失败", "", "（父单验收输入=子单汇总）");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "上游超时".to_string()
            )])),
            fallback: String::new(),
            on_call: None,
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("父单评审失败诚实闭环");
    assert!(reviewed);
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("父单收口验收无法完成")
                && c.content.contains("LLM 调用失败")
                && c.content.contains("上游超时")),
        "必须落带失败原因的父单评审失败评论"
    );
}

/// N4（§6.3 2026-10-12）：review_issue 父单分流——review.retrigger 对收口
/// 失败转人工（保持 in_review）的父单一键重评时，review_issue 必须识别
/// 「有子单」并分流 review_parent_issue（收口汇总语义），不得跑子单式
/// 评审（父单自身无交付汇报，跑出来的是无依据结论；PASS 臂还会错误
/// auto-done 父单绕过收口评审）。分流证据 = 决策词走 parent_escalate_human
/// （子单式评审 FAIL 无派发记录不会产出该决策词）。parent_remediation_max=0
/// （本测钉路由语义，不测承接——默认开时承接见 parent_llm_fail_auto_）。
#[tokio::test]
async fn review_issue_diverts_parent_with_children_to_parent_review() {
    use nemesis_board::{IssueStatus, NewIssue};
    let (deps, _ws) = review_deps("divert-parent");
    write_board_config(
        &deps.home,
        r#"{"auto_close_parent":true,"review":{"parent_remediation_max":0}}"#,
    );
    let parent = issue_in_review(
        &deps.store,
        "父单分流",
        "",
        "（父单不该被当成 worker 交付评审——本 Delivery 是干扰项）",
    );
    deps.store
        .create_issue(NewIssue {
            title: "子单占位".to_string(),
            priority: 2,
            creator: nemesis_board::Actor::agent("node-a"),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "子单结论缺口"),
        }),
    );

    let reviewed = review_issue(&deps, parent.id, ReviewCtx::first_stage())
        .await
        .expect("父单分流评审必须闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview,
        "父单 FAIL 转人工保持 in_review"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_escalate_human"),
        "必须走父单收口语义（parent_escalate_human），证明分流生效"
    );
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("父单收口验收未定案（FAIL）")),
        "必须落父单收口转人工评论（非子单式评审产物）"
    );
}

// ===== N2（2026-10-12）：评审自身失败自动重评（failure_retries） =====

/// N2 决策面共用：in_review 单 + 恒失败 LLM。
fn n2_fail_fixture(deps: &BoardReviewDeps, name: &str) -> nemesis_board::Issue {
    let issue = issue_in_review(&deps.store, name, "", "## 结论\n交付完成");
    attach_loop(
        deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "LLM 调用失败：连接被重置".to_string(),
            )])),
            fallback: String::new(),
            on_call: None,
        }),
    );
    issue
}

/// 预算内（failure_retries=1，ctx.retry=0）→ 落「自动重评」审计评论，
/// 不落「请人工裁决」（重评 spawn 的 60s 延迟任务随测试 runtime 结束取消）。
#[tokio::test]
async fn n2_review_failure_within_budget_schedules_retry() {
    let (deps, _ws) = review_deps("n2-retry");
    write_board_config(&deps.home, r#"{"review":{"failure_retries":1}}"#);
    let issue = n2_fail_fixture(&deps, "评审失败重评");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("重评调度闭环");
    assert!(reviewed);
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("自动重评（第 1/1 次）")),
        "必须落自动重评审计评论：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert!(
        !comments.iter().any(|c| c.content.contains("请人工裁决")),
        "预算内不得直接转人工"
    );
}

/// failure_retries=0（关）→ 旧行为 pin：直接转人工，不调度重评。
#[tokio::test]
async fn n2_review_failure_retries_zero_keeps_legacy_escalation() {
    let (deps, _ws) = review_deps("n2-off");
    write_board_config(&deps.home, r#"{"review":{"failure_retries":0}}"#);
    let issue = n2_fail_fixture(&deps, "评审失败旧行为");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("旧行为闭环");
    assert!(reviewed);
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("请人工裁决") && c.content.contains("review.retrigger")),
        "0=关必须走旧行为转人工"
    );
    assert!(
        !comments.iter().any(|c| c.content.contains("自动重评")),
        "0=关不得调度重评"
    );
}

/// 重评轮再失败（ctx.retry 已达预算）→ 转人工（预算耗尽），不再调度。
#[tokio::test]
async fn n2_review_failure_budget_exhausted_escalates() {
    let (deps, _ws) = review_deps("n2-exhausted");
    write_board_config(&deps.home, r#"{"review":{"failure_retries":1}}"#);
    let issue = n2_fail_fixture(&deps, "重评预算耗尽");

    let ctx = ReviewCtx {
        allow_selfcheck: true,
        evidence: None,
        retry: 1,
    };
    let reviewed = review_issue(&deps, issue.id, ctx).await.expect("闭环");
    assert!(reviewed);
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments.iter().any(|c| c.content.contains("请人工裁决")),
        "预算耗尽必须转人工"
    );
    assert!(
        !comments.iter().any(|c| c.content.contains("自动重评")),
        "预算耗尽不得再调度重评"
    );
}

// ===== review_project_completion LLM 语义臂（PASS 收口 / 解析失败全父单评论） =====

fn done_top_parent(deps: &BoardReviewDeps, pid: i64, title: &str) -> nemesis_board::Issue {
    use nemesis_board::{IssueStatus, NewIssue};
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: title.to_string(),
            creator: creator.clone(),
            project_id: Some(pid),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::Done, &creator)
        .unwrap();
    parent
}

#[tokio::test]
async fn project_llm_pass_completes_and_records_per_parent() {
    let (deps, _ws) = review_deps("project-llm-pass");
    write_board_config(&deps.home, r#"{"review":{"auto_close_project":true}}"#);
    let pid = deps
        .store
        .create_project("语义收口项目", "", None, "", "", None)
        .unwrap()
        .id;
    let parent = done_top_parent(&deps, pid, "语义收口父单");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("PASS", ""),
        }),
    );

    let reviewed = review_project_completion(&deps, pid)
        .await
        .expect("项目 PASS 闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "completed",
        "项目 PASS 必须落 completed"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("project_complete"),
        "项目级结论记在各顶层父单名下"
    );
}

#[tokio::test]
async fn project_llm_parse_failure_comments_all_parents() {
    let (deps, _ws) = review_deps("project-llm-parsefail");
    write_board_config(&deps.home, r#"{"review":{"auto_close_project":true}}"#);
    let pid = deps
        .store
        .create_project("评审失败项目", "", None, "", "", None)
        .unwrap()
        .id;
    let p1 = done_top_parent(&deps, pid, "失败项目父单A");
    let p2 = done_top_parent(&deps, pid, "失败项目父单B");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "全部检查员失联".to_string(),
            )])),
            fallback: String::new(),
            on_call: None,
        }),
    );

    let reviewed = review_project_completion(&deps, pid)
        .await
        .expect("项目评审失败诚实闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "active",
        "解析失败不得动项目状态"
    );
    for p in [p1.id, p2.id] {
        assert!(
            deps.store
                .list_comments(p)
                .unwrap()
                .iter()
                .any(|c| c.content.contains("收口验收无法完成")
                    && c.content.contains("全部检查员失联")),
            "每个顶层父单都必须收到转人工评论（issue {p}）"
        );
    }
}

// ===== replay_stuck_reviews 扫描主链（estop 停车作观测面） =====

#[tokio::test]
async fn replay_stuck_reviews_replays_all_three_kinds() {
    use nemesis_board::{IssueStatus, NewIssue};
    let (deps, _ws) = review_deps("replay-all-kinds");

    // ③ 项目先行建（project 表 id=1；与 issue 表 id 独立编号，须保证停车
    // 队列里 (kind,id) 三元组 id 互不碰撞才能逐一断言）。
    let pid = deps
        .store
        .create_project("重放项目", "", None, "", "", None)
        .unwrap()
        .id;
    let _parent_done_id = done_top_parent(&deps, pid, "重放项目父单").id;

    // 占位 issue（id=1，backlog 不参与重放），保证后续 id 与 project 错开。
    deps.store
        .create_issue(NewIssue {
            title: "占位".to_string(),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();

    // ① 叶子：in_review + 已交付（writeback 发生过）+ 从未评审。
    let leaf = delivered_issue(&deps.store, "重放叶子", "t-replay-leaf");

    // ② 父单：in_review + 全子单 done + 从未收口。
    let parent = issue_in_review(&deps.store, "重放父单", "", "（父单验收输入=子单汇总）");
    let creator = nemesis_board::Actor::agent("node-a");
    let child = deps
        .store
        .create_issue(NewIssue {
            title: "重放子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(child.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(child.id, IssueStatus::Done, &creator)
        .unwrap();

    deps.estop.trigger();
    replay_stuck_reviews(&deps, &[]);
    // spawn 是 fire-and-forget：等停车登记落地。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if deps.estop_parked.lock().unwrap().len() == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let parked = deps.estop_parked.lock().unwrap().clone();
    assert!(
        parked.contains(&(ParkedKind::Issue, leaf.id)),
        "叶子必须按 Issue 维度重放: {parked:?}"
    );
    assert!(
        parked.contains(&(ParkedKind::Parent, parent.id)),
        "全 done 父单必须按 Parent 维度重放: {parked:?}"
    );
    assert!(
        parked.contains(&(ParkedKind::Project, pid)),
        "全 done 项目必须按 Project 维度重放: {parked:?}"
    );
    for (kind, id) in &parked {
        if matches!(kind, ParkedKind::Project) {
            continue; // 项目维度停的是 project_id，无评论表
        }
        assert!(
            deps.store
                .list_comments(*id)
                .unwrap()
                .iter()
                .any(|c| c.content.contains("estop 急停中")),
            "重放条目 {id} 必须被 estop 保险丝拦下留痕"
        );
    }

    // skip 传刚唤醒条目：同态重扫不得双发（去重闸）。
    replay_stuck_reviews(
        &deps,
        &[
            (ParkedKind::Issue, leaf.id),
            (ParkedKind::Parent, parent.id),
            (ParkedKind::Project, pid),
        ],
    );
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(
        deps.estop_parked.lock().unwrap().len(),
        3,
        "skip 列表内的条目不得二次重放"
    );
}

#[tokio::test]
async fn replay_stuck_reviews_skips_concluded_and_undelivered() {
    let (deps, _ws) = review_deps("replay-guards");

    // 无 Delivery 的纯看板 in_review 单：不扫（人工挪入不自动验收）。
    let manual = issue_in_review_as(
        &deps.store,
        "人工挪入无交付",
        nemesis_board::Actor::agent("node-a"),
    );

    // 已有结论的叶子：sticky 不重放。
    let concluded = delivered_issue(&deps.store, "已有结论叶子", "t-replay-done");
    record_auto_decide(
        &deps,
        concluded.id,
        "suggest_manual",
        "PASS",
        serde_json::json!({ "reasons": "历史结论" }),
    );

    deps.estop.trigger();
    replay_stuck_reviews(&deps, &[]);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let parked = deps.estop_parked.lock().unwrap().clone();
    assert!(
        !parked.iter().any(|&(_, id)| id == manual.id),
        "无交付叶子不得重放: {parked:?}"
    );
    assert!(
        !parked.iter().any(|&(_, id)| id == concluded.id),
        "已有结论的单据 sticky 不重放: {parked:?}"
    );
    assert!(
        deps.store
            .list_comments(manual.id)
            .unwrap()
            .iter()
            .all(|c| !c.content.contains("estop 急停中")),
        "无交付叶子不应被 spawn"
    );
}

/// 4a（2026-09-30 17 场景观察点⑥根修）回归锁：writeback 评论三形态中，
/// 降级 ✅ Comment（parse_delivery_report 不中的无格式写回）与失败 ⛔
/// Comment（error 回调）形态的 in_review 单同样必须被重放——修复前
/// delivered 闸只认 Delivery，这两类单重启后永久卡死无自愈。
#[tokio::test]
async fn replay_stuck_reviews_covers_degraded_and_failed_writebacks() {
    use nemesis_board::{Actor, CommentType, NewComment};
    let (deps, _ws) = review_deps("replay-degraded-failed");

    // 降级形态：dispatch done + ✅ Comment（无 Delivery——write_back 对
    // 无结构化格式汇报的诚实降级写回形态）。
    let degraded = issue_in_review_as(&deps.store, "降级写回叶子", Actor::agent("node-a"));
    seed_dispatch(&deps.store, "t-replay-degraded", degraded.id, "node-b");
    deps.store
        .add_comment(NewComment {
            issue_id: degraded.id,
            author: Actor::agent("node-b"),
            content: "✅ worker 汇报完成：\n\n文本无结构化格式".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();

    // 失败形态：dispatch failed + ⛔ Comment（error 回调同样转 in_review
    // 进验收决策链——board_dispatch.rs 写回注释明示）。
    let failed = issue_in_review_as(&deps.store, "失败写回叶子", Actor::agent("node-a"));
    deps.store
        .insert_dispatch(
            "t-replay-failed",
            failed.id,
            "node-b",
            &Actor::agent("node-a"),
        )
        .unwrap();
    deps.store
        .finish_dispatch(
            "t-replay-failed",
            nemesis_board::models::dispatch_state::FAILED,
        )
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: failed.id,
            author: Actor::agent("node-b"),
            content: "⛔ worker 汇报失败：\n\nboom".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();

    deps.estop.trigger();
    replay_stuck_reviews(&deps, &[]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if deps.estop_parked.lock().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let parked = deps.estop_parked.lock().unwrap().clone();
    assert!(
        parked.contains(&(ParkedKind::Issue, degraded.id)),
        "降级写回叶子（dispatch done + ✅ Comment 无 Delivery）必须重放: {parked:?}"
    );
    assert!(
        parked.contains(&(ParkedKind::Issue, failed.id)),
        "失败写回叶子（dispatch failed + ⛔ Comment）必须重放: {parked:?}"
    );
}

// ===== spawn_selfcheck_second_stage：error 回报诚实转人工 =====

#[tokio::test]
async fn selfcheck_second_stage_error_escalates_honestly() {
    let (deps, _ws) = review_deps("b2b-error");
    let issue = delivered_issue(&deps.store, "取证回报失败", "t-b2b-err-1");

    spawn_selfcheck_second_stage(
        deps.clone(),
        issue.id,
        "error".to_string(),
        "worker 内部崩溃".to_string(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let found = deps
            .store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("自检取证回报失败"));
        if found || std::time::Instant::now() > deadline {
            assert!(found, "error 回报必须落诚实转人工评论");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    // 评审保持 in_review（人工裁决），未发生重派。
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        nemesis_board::IssueStatus::InReview
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 1);
}

#[tokio::test]
async fn selfcheck_suspension_comment_never_serves_as_worker_report() {
    use nemesis_board::{CommentType, IssueStatus};
    // 2026-10-08 Extended Tests T30/T-MRG in_review 停车根因回归锁：F6 锚
    // 点强制自检在二段评审前挂「⏳ 验收暂缓」（评审者自己 agent 署名），
    // 二段的交付回退（无结构化 Delivery → 最近 agent 评论）此前抓到这张
    // 比 worker 汇报更新的自述评论 → re: 交付文本锚点必假红 → FAIL → 重
    // 派循环 → 预算耗尽转人工。回退必须排除评审者自己，落在真实 worker
    // 汇报上。
    let (deps, _ws) = review_deps("selfcheck-poison");
    let reviewer = nemesis_board::Actor::agent("node-a"); // = deps.cluster.node_id()
    let issue = deps
        .store
        .create_issue(nemesis_board::NewIssue {
            title: "自检暂缓评论不充作 worker 汇报".into(),
            priority: 2,
            acceptance_criteria: Some("[CHECK] re:FILE_EDIT_DONE".into()),
            creator: reviewer.clone(),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    // 事故现场：worker 非结构化汇报 = 诚实降级的普通评论（不走 Delivery
    // 主通道，才进回退路径）。
    deps.store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: nemesis_board::Actor::agent("node-b"),
            content: "FILE_EDIT_DONE 已按任务要求完成文件编辑".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    // 复现生产时序：自检派发成功后、二段评审前，评审者先挂暂缓评论
    // （review_issue 原文形态，agent 署名=评审者自己，比 worker 汇报新）。
    deps.store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: reviewer,
            content: "⏳ 验收暂缓（board.review.selfcheck）：评审 agent 需要更多证据，已向执行 worker 发起取证（task t-poison-1）。\n\n取证请求：请回报证据\n\nworker 回报后自动进行二段验收。".into(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    // 一条已完结派发：锚点强制臂要求非空 dispatches；终态免重复派发闸。
    seed_dispatch(&deps.store, "t-poison-1", issue.id, "node-b");

    let provider = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: review_json("PASS", ""),
    });
    attach_loop(&deps, provider.clone());
    write_board_config(&deps.home, r#"{"auto_accept":true}"#);

    // 二段评审（取证回报 ok + 证据注入；与 cluster_init 自检回调同参形态）。
    spawn_selfcheck_second_stage(
        deps.clone(),
        issue.id,
        "ok".to_string(),
        "取证回报：文件已实际写入".to_string(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if deps.store.get_issue(issue.id).unwrap().status == IssueStatus::Done
            || std::time::Instant::now() > deadline
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::Done,
        "worker 汇报命中 re: 锚点 + PASS + auto_accept → done（回退抓错评论则 FAIL 停车）"
    );
    let prompts = provider.prompts.lock().unwrap().join("\n");
    // 暂缓评论合法保留在讨论线程段（上一轮评审意见在场是设计语义）——
    // 负断言只对「worker 汇报」槽，不对全 prompt。
    let report_slot = prompts
        .split("## worker 汇报")
        .nth(1)
        .expect("prompt 必含 worker 汇报段")
        .split("\n## ")
        .next()
        .expect("报告段非空")
        .to_string();
    assert!(
        report_slot.contains("FILE_EDIT_DONE 已按任务要求完成文件编辑"),
        "二段评审的报告槽必须是 worker 汇报: {report_slot}"
    );
    assert!(
        !report_slot.contains("验收暂缓"),
        "评审者自己的暂缓评论不得充作报告槽: {report_slot}"
    );
}

// ===== run_project_summary 成功路径（AI 生成件落盘） =====

#[tokio::test]
async fn run_project_summary_success_writes_summary_doc() {
    let (deps, _ws) = review_deps("summary-success");
    // 档案目录（脚手架保证目录存在）。
    let root = std::env::temp_dir().join(format!("nb-review-summary-ok-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    nemesis_board::archive::ensure_scaffold(&root, 1, "p", "active").unwrap();

    let pid = deps
        .store
        .create_project(
            "总结项目",
            "项目描述乙",
            None,
            "",
            "",
            Some(root.to_str().unwrap()),
        )
        .unwrap()
        .id;
    // active → in_progress → completed（状态机两跳，与自动流一致）。
    deps.store
        .update_project(
            pid,
            &nemesis_board::models::ProjectPatch {
                status: Some("in_progress".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    deps.store
        .update_project(
            pid,
            &nemesis_board::models::ProjectPatch {
                status: Some("completed".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
    done_top_parent(&deps, pid, "总结父单");

    let provider = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: "## 各任务做法\n按序完成。\n\n## 决策流摘要\n自动收口。\n\n## 最终结构\n单一父单。"
            .to_string(),
    });
    attach_loop(&deps, provider.clone());

    run_project_summary(&deps, pid)
        .await
        .expect("总结生成必须成功");
    // 事实清单（项目描述/任务行）必须进 prompt，供 LLM 依据撰写。
    let prompts = provider.prompts.lock().unwrap().join("\n");
    assert!(
        prompts.contains("项目描述乙") && prompts.contains("总结父单"),
        "事实清单必须进 prompt: {prompts}"
    );
    let target = root.join("docs").join("summary.md");
    let doc = std::fs::read_to_string(&target).expect("summary.md 必须落盘");
    assert!(doc.contains("AI 生成件"), "必须有 AI 生成件标注: {doc}");
    assert!(doc.contains("自动收口"), "LLM 正文必须原样写入");
    assert!(
        doc.contains("## 附录：档案结构") && doc.contains("archive/"),
        "必须附档案结构树: {doc}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// -------------------------------------------------------------------------
// Wave5 round2 batch4: 重派漏斗深水区 —— pick Err 停车 / 预算保险丝双臂
// （非 unlimited 停车转人工 / unlimited WARN 续派发车）/ D3 换节点发车
// 全链 / dispatch_issue_core 重复闸拒绝的诚实留痕 / 停滞 WARN 前置两轮
// 同差距 / 解析失败（评审 LLM 自身故障）unlimited 与护栏双臂 /
// load_board_flags fail-closed。
// -------------------------------------------------------------------------

/// 给 deps.home 写一份只含 board 段的最小 config.json（load_board_flags
/// 走 nemesis_config::load_config，缺省字段全 serde default）。
fn write_board_cfg(home: &std::path::Path, board: serde_json::Value) {
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({ "board": board }).to_string(),
    )
    .unwrap();
}

/// 必失败的 re: 锚点集 + 不含标志的交付（锚点短路 FAIL 的标准形态）。
const W5_FAIL_AC: &str = "[CHECK] re:验收完成标志XYZ\n[CHECK] re:交付完成";
const W5_FAIL_DELIVERY: &str = "## 结论\n交付完成";

/// 重派决策①：零派发历史 + FAIL → pick_redispatch_target 诚实 Err 臂
/// （⛔ 系统评论停车转人工，不发车不改状态）。
#[tokio::test]
async fn w5_redispatch_without_history_parks_with_pick_failure_comment() {
    use nemesis_board::{CommentType, IssueStatus};
    let (deps, _ws) = review_deps("w5-pick-err");
    let issue = issue_in_review(&deps.store, "无历史失败", W5_FAIL_AC, W5_FAIL_DELIVERY);

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("pick 失败必须诚实闭环不报错");
    assert!(reviewed, "停车也是闭环（Ok(true)）");

    let comments = deps.store.list_comments(issue.id).unwrap();
    let park = comments
        .iter()
        .find(|c| c.ctype == CommentType::System && c.content.contains("确定重派目标失败"))
        .expect("pick 失败必须落 ⛔ 系统评论");
    assert!(
        park.content.contains("无历史派发记录"),
        "错误原文透传: {}",
        park.content
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "停车保持 in_review"
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 0);
}

/// 重派决策②：预算保险丝（max_total_redispatch=1 < 全链 2 派）+ 护栏
/// 模式 → 🛑 停车转人工，不发车。
#[tokio::test]
async fn w5_redispatch_budget_breach_stops_for_human_when_not_unlimited() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("w5-budget-stop");
    write_board_cfg(
        &deps.home,
        serde_json::json!({ "budget": { "max_total_redispatch": 1 } }),
    );
    let issue = issue_in_review(&deps.store, "预算停", W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, "t-w5-bs-1", issue.id, "node-b");
    seed_dispatch(&deps.store, "t-w5-bs-2", issue.id, "node-b");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("预算停必须闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let stop = comments
        .iter()
        .find(|c| c.content.contains("自动重派预算超限"))
        .expect("🛑 预算超限评论在场");
    assert!(
        stop.content.contains("父单全链累计派发 2 次超过"),
        "超限项原文透传: {}",
        stop.content
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "护栏模式停车保持 in_review"
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 2);
}

/// 重派决策③：同一预算超限 + unlimited_mode → 只 WARN 继续照常发车
///（保险丝非护栏的边界一致），落第 3 条派发、单据转 in_progress。
#[tokio::test]
async fn w5_redispatch_budget_breach_warns_and_continues_when_unlimited() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("w5-budget-warn");
    write_board_cfg(
        &deps.home,
        serde_json::json!({ "unlimited_mode": true, "budget": { "max_total_redispatch": 1 } }),
    );
    let issue = issue_in_review(&deps.store, "预算warn续派", W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, "t-w5-bw-1", issue.id, "node-b");
    seed_dispatch(&deps.store, "t-w5-bw-2", issue.id, "node-b");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("unlimited 预算 warn 必须继续闭环");
    assert!(reviewed);

    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        3,
        "WARN 后照常发车"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress,
        "重派发车转 in_progress"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("自动重派预算超限")),
        "unlimited 走 warn 臂，不落 🛑 停车评论"
    );
}

/// 重派决策④：D3 换节点发车全链 —— node-c 连败 2 轮 + 在线 node-b →
/// 🔁 换节点评论 + 派发记账落到新 worker。
#[tokio::test]
async fn w5_redispatch_switch_comment_and_dispatch_to_fresh_node() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("w5-switch");
    // 在线候选（rpc_port 非 0 过 G14 闸；node-c 已是历史，ranked 次优 = node-b）。
    deps.cluster.handle_discovered_node(
        "node-b",
        "Node-B",
        vec!["127.0.0.1".to_string()],
        19011,
        "worker",
        "development",
        vec![],
        vec![],
        "standard",
    );
    let issue = issue_in_review(&deps.store, "连败换人发车", W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, "t-w5-sw-1", issue.id, "node-c");
    seed_dispatch(&deps.store, "t-w5-sw-2", issue.id, "node-c");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("换节点发车闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let sw = comments
        .iter()
        .find(|c| c.content.contains("换节点执行"))
        .expect("🔁 换节点评论在场");
    assert!(
        sw.content.contains("node-b"),
        "评论带新目标: {}",
        sw.content
    );
    let dispatches = deps.store.list_dispatches(issue.id).unwrap();
    assert_eq!(dispatches.len(), 3);
    assert_eq!(dispatches.last().unwrap().worker_id, "node-b");
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
    );
}

/// 重派决策⑤：活跃派发在途 → 闭环批 2026-10-13 的并发让位预检先命中 →
/// **静默让位不落评论**（「请人工派发」文案对瞬时竞态是误导——数秒后并发
/// 触发源自己完成派发，且 ⛔ 会打断 F2/停车去重锚定的 system 评论链），
/// 不发车不炸流程。旧契约「dispatch_issue_core 重复闸拒绝 → ⛔ 留痕」
/// 已被让位预检取代，此锁钉住新契约防回退。
#[tokio::test]
async fn w5_redispatch_active_dispatch_yields_silently() {
    use nemesis_board::{CommentType, IssueStatus};
    let (deps, _ws) = review_deps("w5-dispatch-err");
    let issue = issue_in_review(&deps.store, "派发失败留痕", W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, "t-w5-de-1", issue.id, "node-b");
    // 活跃（未完结）派发 → 让位预检必命中（先于 dispatch_issue_core）。
    deps.store
        .insert_dispatch(
            "t-w5-de-2",
            issue.id,
            "node-b",
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("重派让位必须诚实闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        !comments
            .iter()
            .any(|c| c.ctype == CommentType::System && c.content.contains("自动重派失败")),
        "让位臂不得落 ⛔ 评论（对瞬时态撒谎）：{:?}",
        comments
            .iter()
            .filter(|c| c.ctype == CommentType::System)
            .map(|c| &c.content)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        2,
        "不发新派发"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
}

/// 重派决策⑥：停滞可观测前置态 —— unlimited 两轮评审差距文本完全一致
/// （第二轮进 stale 分支），各落一条「## 差距」评论、每轮照常发车。
#[tokio::test]
async fn w5_redispatch_stale_gap_second_round_with_identical_gap_text() {
    use nemesis_board::IssueStatus;
    let (deps, _ws) = review_deps("w5-stale");
    write_board_cfg(&deps.home, serde_json::json!({ "unlimited_mode": true }));
    let issue = issue_in_review(&deps.store, "停滞可观测", W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, "t-w5-st-1", issue.id, "node-b");
    seed_dispatch(&deps.store, "t-w5-st-2", issue.id, "node-b");

    let reviewed1 = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("第一轮闭环");
    assert!(reviewed1);
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 3);

    // run 1 发车的派发行仍在途（state=dispatched）→ 置终态（模拟 worker
    // 回报完结），否则第二轮重派被 has_active_dispatch 闸诚实拒绝。
    let seeded = ["t-w5-st-1", "t-w5-st-2"];
    let inflight = deps
        .store
        .list_dispatches(issue.id)
        .unwrap()
        .into_iter()
        .find(|d| !seeded.contains(&d.task_id.as_str()))
        .expect("run 1 发车派发行在场");
    deps.store
        .finish_dispatch(
            &inflight.task_id,
            nemesis_board::models::dispatch_state::DONE,
        )
        .unwrap();

    // 派发把单转 in_progress → 拉回 in_review 触发第二轮同差距评审。
    deps.store
        .transition_issue(
            issue.id,
            IssueStatus::InReview,
            &nemesis_board::Actor::agent("node-a"),
        )
        .unwrap();
    let reviewed2 = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("第二轮闭环");
    assert!(reviewed2);
    // 重派落账带界收敛窗：insert 与 review_issue 同步域内（评审返回时行
    // 已在），但仪器化（llvm-cov）满载跑 2-3x 慢，会把 round-1 的 RPC
    // spawn 体（对不可达对端终结派发行 + ⛔ 评论）挤进断言窗口。2s 有界
    // 轮询只吸收调度抖动；真实拒绝仍会失败并带系统评论诊断。
    let mut dispatches = deps.store.list_dispatches(issue.id).unwrap();
    for _ in 0..20 {
        if dispatches.len() >= 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        dispatches = deps.store.list_dispatches(issue.id).unwrap();
    }
    assert_eq!(
        dispatches.len(),
        4,
        "第二轮重派必须发车（4 = 2 种子 + 两轮重派）。系统评论（⛔ 自动重派失败带底层拒绝原因）：{:?}",
        deps.store
            .list_comments(issue.id)
            .unwrap()
            .into_iter()
            .filter(|c| c.content.contains('⛔'))
            .map(|c| c.content)
            .collect::<Vec<_>>()
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    let gap_comments: Vec<_> = comments
        .iter()
        .filter(|c| c.content.contains("## 差距"))
        .collect();
    assert_eq!(gap_comments.len(), 2, "两轮各落一条差距评论");
}

/// 解析失败臂①：评审 LLM 一路输出非 JSON + unlimited → 评审自身故障 ≠
/// 质量结论：恒转人工（🤷 评论），**不重派 worker**——unlimited_mode 只
/// 治理真实 verdict 的重派（2026-10-09 web-FPS C1 无限重写循环根修）。
#[tokio::test]
async fn w5_parse_failed_unlimited_escalates_without_redispatch() {
    use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::AgentConfig;
    use nemesis_board::IssueStatus;

    struct JunkProvider;
    #[async_trait::async_trait]
    impl LlmProvider for JunkProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<nemesis_agent::types::ChatOptions>,
            _tools: Vec<nemesis_agent::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "这不是评审 JSON，只是一段散文。".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let (mut deps, _ws) = review_deps("w5-parse-unl");
    write_board_cfg(&deps.home, serde_json::json!({ "unlimited_mode": true }));
    let agent_loop = AgentLoop::new(Box::new(JunkProvider), AgentConfig::default());
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    let _ = deps.moderator_loop.set(Arc::new(agent_loop));

    // 无锚点（AC 空）→ 进语义项 → JunkProvider 连续解析失败。
    let issue = issue_in_review(&deps.store, "解析失败续派", "", "交付");
    seed_dispatch(&deps.store, "t-w5-pu-1", issue.id, "node-b");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("unlimited 解析失败按转人工闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.content.contains("验收评审自身失败"))
        .expect("🤷 评审自身失败转人工评论在场");
    assert!(
        note.content.contains("连续 3 轮无法解析"),
        "评论带失败原因分类: {}",
        note.content
    );
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        1,
        "unlimited_mode 评审自身失败不重派 worker（不发车）"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "转人工不发车：状态保持 in_review"
    );
}

/// 解析失败臂②：护栏模式（非 unlimited）→ 🤷 转人工评论，不发车。
#[tokio::test]
async fn w5_parse_failed_guardrail_escalates_to_human() {
    use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::AgentConfig;
    use nemesis_board::IssueStatus;

    struct JunkProvider;
    #[async_trait::async_trait]
    impl LlmProvider for JunkProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<nemesis_agent::types::ChatOptions>,
            _tools: Vec<nemesis_agent::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "同样不是评审 JSON。".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }

    let (mut deps, _ws) = review_deps("w5-parse-guard");
    // N2 默认 failure_retries=1：首轮解析失败落「自动重评」延迟链（60s 后
    // 异步重跑），不会同步出现转人工评论。本测试钉的是重评预算耗尽臂——
    // 关掉重评（0=关）直达转人工（与 n2_review_failure_retries_zero 同款
    // pin）。
    write_board_config(&deps.home, r#"{"review":{"failure_retries":0}}"#);
    let agent_loop = AgentLoop::new(Box::new(JunkProvider), AgentConfig::default());
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    let _ = deps.moderator_loop.set(Arc::new(agent_loop));

    let issue = issue_in_review(&deps.store, "解析失败转人工", "", "交付");

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("护栏解析失败必须转人工闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.content.contains("验收 agent 无法判定"))
        .expect("🤷 转人工评论在场");
    assert!(
        note.content.contains("验收评审自身失败"),
        "评论诚实归因评审系统: {}",
        note.content
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 0);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );
}

/// load_board_flags fail-closed：config.json 坏 JSON → 评审 Err 放弃
///（不拿默认值顶替用户配置）。
#[tokio::test]
async fn w5_review_issue_fails_closed_when_config_unreadable() {
    let (deps, _ws) = review_deps("w5-cfg-broken");
    std::fs::write(deps.home.join("config.json"), "{ not json").unwrap();
    let issue = issue_in_review(&deps.store, "配置坏", W5_FAIL_AC, W5_FAIL_DELIVERY);
    let err = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect_err("配置读失败必须 Err（fail-closed）");
    assert!(err.contains("config.json 读取失败"), "err={err}");
}

// ===========================================================================
// wave5 round2（2026-09-25）：review_parent_issue 前置跳过四臂 + 子单汇总/
// 项目目录实物证据/锚点 PASS 评论臂 + 锚点 FAIL 确定性短路（免 LLM 直达
// 转人工）、spawn_selfcheck_second_stage error 臂、spawn_project_review
// Err/Ok(false) spawn 臂、review_project_completion 空父单 + 交付摘要 +
// 决策注记段、run_project_summary 急停/状态/档案目录/空任务/空回复/成功
// 落盘全臂、spawn_project_summary 失败留痕端到端。
// ===========================================================================

mod w5r2parent {
    use super::*;
    use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};

    fn w5_deps(name: &str) -> (BoardReviewDeps, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("nb-w5r2parent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workspace = dir.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = Arc::new(
            nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"),
        );
        store.ensure_default_channels().unwrap();
        let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(
            nemesis_cluster::types::ClusterConfig {
                node_id: "node-a".to_string(),
                bind_address: "127.0.0.1:0".to_string(),
                peers: vec![],
                node_name: String::new(),
            },
        ));
        cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
        let deps = BoardReviewDeps {
            store,
            workspace: workspace.clone(),
            home: dir.clone(),
            moderator_loop: Arc::new(std::sync::OnceLock::new()),
            cluster,
            estop: Arc::new(nemesis_agent::estop::EstopState::new()),
            estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
            selfcheck: SelfcheckRegistry::new(),
            #[cfg(feature = "security")]
            security_slot: None,
        };
        (deps, workspace)
    }

    fn w5_issue(
        store: &nemesis_board::BoardStore,
        title: &str,
        parent: Option<i64>,
        project: Option<i64>,
        ac: &str,
    ) -> nemesis_board::Issue {
        store
            .create_issue(NewIssue {
                title: title.to_string(),
                priority: 2,
                acceptance_criteria: Some(ac.to_string()),
                creator: Actor::agent("node-a"),
                parent_issue_id: parent,
                project_id: project,
                ..Default::default()
            })
            .unwrap()
    }

    fn w5_in_review(store: &nemesis_board::BoardStore, id: i64) {
        let a = Actor::agent("node-a");
        store
            .transition_issue(id, IssueStatus::InProgress, &a)
            .unwrap();
        store
            .transition_issue(id, IssueStatus::InReview, &a)
            .unwrap();
    }

    fn w5_delivery(store: &nemesis_board::BoardStore, issue_id: i64, body: &str) {
        store
            .add_comment(NewComment {
                issue_id,
                author: Actor::agent("node-b"),
                content: body.to_string(),
                parent_id: None,
                ctype: CommentType::Delivery,
            })
            .unwrap();
    }

    /// ① 收口开关缺省关（无 config.json）→ flags 双保险臂 Ok(false)。
    #[tokio::test]
    async fn w5_parent_flags_off_skips() {
        let (deps, _ws) = w5_deps("flags-off");
        let parent = w5_issue(&deps.store, "父单-开关关", None, None, "");
        w5_in_review(&deps.store, parent.id);
        let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
        assert!(!reviewed, "auto_close_parent 缺省关必须跳过");
    }

    /// ② 父单不在 in_review（竞态让位人工）→ Ok(false)。
    #[tokio::test]
    async fn w5_parent_not_in_review_skips() {
        let (deps, _ws) = w5_deps("not-in-review");
        write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
        let parent = w5_issue(&deps.store, "父单-未送审", None, None, "");
        // 留在 backlog：不转 in_review。
        let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
        assert!(!reviewed, "非 in_review 必须让位人工");
    }

    /// ③ 子树存在 cancelled 单（SAN-08 隐藏缺口）→ 转人工 Ok(false)。
    #[tokio::test]
    async fn w5_parent_cancelled_subtree_skips() {
        let (deps, _ws) = w5_deps("cancelled-subtree");
        write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
        let parent = w5_issue(&deps.store, "父单-含缺口", None, None, "");
        w5_in_review(&deps.store, parent.id);
        let child = w5_issue(&deps.store, "被取消子单", Some(parent.id), None, "");
        deps.store
            .transition_issue(child.id, IssueStatus::Cancelled, &Actor::agent("node-a"))
            .unwrap();
        let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
        assert!(!reviewed, "cancelled 子树必须转人工");
    }

    /// ④ 全前置过 + 子单汇总（含/无 Delivery 两态）+ 项目目录实物证据
    /// （>200 文件诚实截断 + 内容节选）+ 锚点全过 PASS 评论 → 主 agent
    /// 未就绪 Ok(false)（无 LLM 也可达的全部前段）。
    #[tokio::test]
    async fn w5_parent_children_artifacts_anchor_pass_reaches_moderator_gate() {
        let (deps, ws) = w5_deps("artifacts-pass");
        write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);

        // 项目目录：210 个小文件（>200 触发清单截断）+ 1 个可读节选文本。
        let proj_dir = ws.join("proj-artifacts");
        std::fs::create_dir_all(&proj_dir).unwrap();
        for i in 0..210 {
            std::fs::write(proj_dir.join(format!("f{i:03}.txt")), "x").unwrap();
        }
        std::fs::write(proj_dir.join("report.md"), "产物齐全说明").unwrap();

        let project = deps
            .store
            .create_project(
                "实物证据项目",
                "",
                None,
                "",
                "",
                Some(proj_dir.to_str().unwrap()),
            )
            .unwrap();
        let parent = w5_issue(
            &deps.store,
            "父单-实物验收",
            None,
            Some(project.id),
            "[CHECK] re:子任务",
        );
        w5_in_review(&deps.store, parent.id);
        let c1 = w5_issue(&deps.store, "子单-有交付", Some(parent.id), None, "");
        w5_delivery(
            &deps.store,
            c1.id,
            "## 结论\n完成\n## 交付物清单\n无\n## 自检结果\n过\n",
        );
        let c2 = w5_issue(&deps.store, "子单-无交付", Some(parent.id), None, "");

        let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
        assert!(!reviewed, "无 moderator 必须在收口评审门前返回");
        let comments = deps.store.list_comments(parent.id).unwrap();
        assert!(
            comments
                .iter()
                .any(|c| c.content.contains("✅ 客观锚点检查通过")),
            "锚点全过必须落 PASS 评论: {:?}",
            comments.iter().map(|c| &c.content).collect::<Vec<_>>()
        );
        assert_eq!(
            deps.store.get_issue(c1.id).unwrap().status,
            IssueStatus::Backlog
        );
        assert_eq!(
            deps.store.get_issue(c2.id).unwrap().status,
            IssueStatus::Backlog
        );
    }

    /// ⑤ 锚点 FAIL → 确定性短路（跳过 LLM）→ 转人工评论 + parent_escalate
    /// _human 入账，状态保持 in_review。
    #[tokio::test]
    async fn w5_parent_anchor_fail_short_circuits_to_human() {
        let (deps, _ws) = w5_deps("anchor-fail");
        // parent_remediation_max=0：本测钉「锚点 FAIL → 转人工」语义（默认
        // 开启时 FAIL 带差距会先走自动返工承接，见 parent_llm_fail_auto_
        // remediates_via_rework_child）。
        write_board_config(
            &deps.home,
            r#"{"auto_close_parent":true,"review":{"parent_remediation_max":0}}"#,
        );
        let parent = w5_issue(
            &deps.store,
            "父单-锚点必败",
            None,
            None,
            "[CHECK] re:绝不存在于任何汇总的锚点词QQQ",
        );
        w5_in_review(&deps.store, parent.id);

        let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
        assert!(reviewed, "FAIL 短路也要闭环（转人工评论+入账）");
        assert_eq!(
            deps.store.get_issue(parent.id).unwrap().status,
            IssueStatus::InReview,
            "FAIL 保持 in_review 等人工"
        );
        let comments = deps.store.list_comments(parent.id).unwrap();
        assert!(
            comments
                .iter()
                .any(|c| c.content.contains("父单收口验收未定案") && c.content.contains("差距")),
            "必须落转人工评论（含差距段）: {:?}",
            comments.iter().map(|c| &c.content).collect::<Vec<_>>()
        );
        assert_eq!(
            decided_word(&deps.store, parent.id).as_deref(),
            Some("parent_escalate_human")
        );
    }

    /// ⑥ spawn_selfcheck_second_stage error 臂：worker 回报失败 → 诚实转
    /// 人工评论（fire-and-forget → sleep 等落地）。
    #[tokio::test]
    async fn w5_spawn_selfcheck_error_posts_manual_comment() {
        let (deps, _ws) = w5_deps("selfcheck-err");
        let issue = w5_issue(&deps.store, "取证失败单", None, None, "");
        spawn_selfcheck_second_stage(deps.clone(), issue.id, "error".into(), "worker 炸了".into());
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let comments = deps.store.list_comments(issue.id).unwrap();
        assert!(
            comments
                .iter()
                .any(|c| c.content.contains("自检取证回报失败")),
            "error 回报必须落人工裁决评论: {:?}",
            comments.iter().map(|c| &c.content).collect::<Vec<_>>()
        );
    }

    /// ⑦ spawn_project_review Err 臂：config.json 是目录 → load_board_flags
    /// 读失败 → Err warn（fire-and-forget 不炸）。
    #[tokio::test]
    async fn w5_spawn_project_review_err_arm_config_unreadable() {
        let (deps, _ws) = w5_deps("proj-err-arm");
        std::fs::remove_file(deps.home.join("config.json")).ok();
        std::fs::create_dir_all(deps.home.join("config.json")).unwrap();
        spawn_project_review(deps.clone(), 424242);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // 无 panic / 无窗口即为臂已走（Err 只 warn）。
    }

    /// ⑧ spawn_project_review Ok(false) 臂：项目已 completed → 诚实跳过。
    #[tokio::test]
    async fn w5_spawn_project_review_completed_project_skips() {
        let (deps, _ws) = w5_deps("proj-completed");
        write_board_config(
            &deps.home,
            r#"{"auto_review":true,"review":{"auto_close_project":true}}"#,
        );
        let pid = deps
            .store
            .create_project("已完成项目", "", None, "", "", None)
            .unwrap()
            .id;
        deps.store
            .update_project(
                pid,
                &nemesis_board::ProjectPatch {
                    status: Some("in_progress".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        deps.store
            .update_project(
                pid,
                &nemesis_board::ProjectPatch {
                    status: Some("completed".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        spawn_project_review(deps.clone(), pid);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(deps.store.get_project(pid).unwrap().status, "completed");
    }

    /// ⑨ review_project_completion：空父单臂 + 交付摘要段（父单 Delivery
    /// 在场）+ 子单决策注记段（auto_decide activity 带 decision 键）。
    #[tokio::test]
    async fn w5_project_completion_empty_parents_and_summary_segments() {
        // ⑨a 空父单 → 防御性复核跳过。
        let (deps, _ws) = w5_deps("proj-empty-parents");
        write_board_config(
            &deps.home,
            r#"{"auto_review":true,"review":{"auto_close_project":true}}"#,
        );
        let pid0 = deps
            .store
            .create_project("无单项目", "", None, "", "", None)
            .unwrap()
            .id;
        let reviewed = review_project_completion(&deps, pid0).await.unwrap();
        assert!(!reviewed, "空父单必须跳过");

        // ⑨b 全前置过 → 汇总段（父单 Delivery + 子单决策注记）→ 评审门前
        // Ok(false)。
        let (deps2, _ws2) = w5_deps("proj-summary-segments");
        write_board_config(
            &deps2.home,
            r#"{"auto_review":true,"review":{"auto_close_project":true}}"#,
        );
        let pid = deps2
            .store
            .create_project("汇总段项目", "", None, "", "", None)
            .unwrap()
            .id;
        let parent = w5_issue(&deps2.store, "已done父单", None, Some(pid), "");
        w5_in_review(&deps2.store, parent.id); // 先推到 in_review 便于落交付
        w5_delivery(&deps2.store, parent.id, "父单交付正文");
        deps2
            .store
            .transition_issue(parent.id, IssueStatus::Done, &Actor::agent("node-a"))
            .unwrap();
        let child = w5_issue(&deps2.store, "已done子单", Some(parent.id), Some(pid), "");
        deps2
            .store
            .transition_issue(child.id, IssueStatus::Done, &Actor::agent("node-a"))
            .unwrap();
        // 子单 auto_decide 活动带 decision 键 → verdict_note Some 臂。
        deps2
            .store
            .add_activity(
                child.id,
                &Actor::agent("node-a"),
                "auto_decide",
                Some(r#"{"decision":"accept"}"#),
            )
            .unwrap();

        let reviewed = review_project_completion(&deps2, pid).await.unwrap();
        assert!(!reviewed, "无 moderator 必须在评审门前 Ok(false)");
    }

    /// ⑩ run_project_summary 前置失败臂：急停 / 状态非 completed / 档案
    /// 目录不存在。
    #[tokio::test]
    async fn w5_run_project_summary_estop_status_and_bad_dir() {
        let (deps, _ws) = w5_deps("summary-guards");
        let pid = deps
            .store
            .create_project("守卫项目", "", None, "", "", None)
            .unwrap()
            .id;

        // 急停臂。
        deps.estop.trigger();
        let err = run_project_summary(&deps, pid).await.unwrap_err();
        assert!(err.contains("急停"), "急停必须拒跑: {err}");
        deps.estop.release();

        // 状态非 completed 臂（active 缺省）。
        let err = run_project_summary(&deps, pid).await.unwrap_err();
        assert!(err.contains("completed"), "非 completed 必须跳过: {err}");

        // completed + 目录不存在臂（project_archive_root !is_dir）。
        // ProjectPatch 无 directory 键——坏目录项目须建单时绑定。
        let pid_bad = deps
            .store
            .create_project(
                "坏目录项目",
                "",
                None,
                "",
                "",
                Some("Z:/definitely/not/here-nb-w5"),
            )
            .unwrap()
            .id;
        deps.store
            .update_project(
                pid_bad,
                &nemesis_board::ProjectPatch {
                    status: Some("in_progress".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        deps.store
            .update_project(
                pid_bad,
                &nemesis_board::ProjectPatch {
                    status: Some("completed".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let err = run_project_summary(&deps, pid_bad).await.unwrap_err();
        assert!(err.contains("档案目录不存在"), "坏目录必须 Err: {err}");
    }

    /// ⑪ run_project_summary 空任务臂 + LLM 空回复臂（moderator 在场）。
    #[tokio::test]
    async fn w5_run_project_summary_empty_issues_and_empty_reply() {
        // ⑪a 空任务：completed + 目录就绪 + loop 在场，但项目无单。
        let (deps, dir) = w5_deps("summary-empty-issues");
        let archive = dir.join("archive-a");
        std::fs::create_dir_all(&archive).unwrap();
        let pid = deps
            .store
            .create_project(
                "空任务项目",
                "",
                None,
                "",
                "",
                Some(archive.to_str().unwrap()),
            )
            .unwrap()
            .id;
        deps.store
            .update_project(
                pid,
                &nemesis_board::ProjectPatch {
                    status: Some("in_progress".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        deps.store
            .update_project(
                pid,
                &nemesis_board::ProjectPatch {
                    status: Some("completed".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        attach_loop(
            &deps,
            Arc::new(CapturingLlm {
                prompts: std::sync::Mutex::new(Vec::new()),
                reply: "总结正文".to_string(),
            }),
        );
        let err = run_project_summary(&deps, pid).await.unwrap_err();
        assert!(err.contains("项目无任务"), "空任务必须 Err: {err}");

        // ⑪b LLM 空回复：放一张父单再跑；moderator 换成空回复 loop
        //（OnceLock set-once → 必须新槽；同 store 复用项目行）。
        let parent = w5_issue(&deps.store, "唯一父单", None, Some(pid), "");
        w5_in_review(&deps.store, parent.id);
        deps.store
            .transition_issue(parent.id, IssueStatus::Done, &Actor::agent("node-a"))
            .unwrap();
        let deps2 = BoardReviewDeps {
            store: deps.store.clone(),
            workspace: deps.workspace.clone(),
            home: deps.home.clone(),
            moderator_loop: Arc::new(std::sync::OnceLock::new()),
            cluster: deps.cluster.clone(),
            estop: Arc::new(nemesis_agent::estop::EstopState::new()),
            estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
            selfcheck: SelfcheckRegistry::new(),
            #[cfg(feature = "security")]
            security_slot: None,
        };
        attach_loop(
            &deps2,
            Arc::new(CapturingLlm {
                prompts: std::sync::Mutex::new(Vec::new()),
                reply: String::new(),
            }),
        );
        // 真实契约（wave5_findings_B F-B7）：空回复在 loop 内被 turn_guard
        // 退化三判拦截并 nudge 重试，max_turns=1 无续轮 → 报兜底 Done 文本；
        // run_detached「Done 优先」恒 Ok(非空)——board_review.rs:2946 的
        // 「LLM 返回空内容」Err 臂结构性不可达。这里固化真实行为：兜底文本
        // 照常成文落盘，函数 Ok。
        run_project_summary(&deps2, pid)
            .await
            .expect("兜底 Done 文本必须照常成文");
        let summary = dir.join("archive-a").join("docs").join("summary.md");
        assert!(summary.exists(), "兜底总结必须落盘: {}", summary.display());
    }

    /// ⑫ run_project_summary 成功臂：summary.md 落盘（docs/）+ timeline。
    #[tokio::test]
    async fn w5_run_project_summary_success_writes_docs_summary() {
        let (deps, dir) = w5_deps("summary-success");
        let archive = dir.join("archive-b");
        std::fs::create_dir_all(&archive).unwrap();
        let pid = deps
            .store
            .create_project(
                "成功总结项目",
                "描述在",
                None,
                "",
                "",
                Some(archive.to_str().unwrap()),
            )
            .unwrap()
            .id;
        deps.store
            .update_project(
                pid,
                &nemesis_board::ProjectPatch {
                    status: Some("in_progress".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        deps.store
            .update_project(
                pid,
                &nemesis_board::ProjectPatch {
                    status: Some("completed".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let parent = w5_issue(&deps.store, "父单甲", None, Some(pid), "");
        deps.store
            .transition_issue(parent.id, IssueStatus::Done, &Actor::agent("node-a"))
            .unwrap();
        attach_loop(
            &deps,
            Arc::new(CapturingLlm {
                prompts: std::sync::Mutex::new(Vec::new()),
                reply: "项目按期收口，产物齐全。".to_string(),
            }),
        );

        run_project_summary(&deps, pid).await.expect("总结必须成功");
        let summary = archive.join("docs").join("summary.md");
        assert!(summary.exists(), "summary.md 必须落盘");
        let body = std::fs::read_to_string(&summary).unwrap();
        assert!(
            body.contains("AI 生成件") && body.contains("项目按期收口"),
            "总结必须含头注与 LLM 正文: {body}"
        );
        let timeline = std::fs::read_to_string(archive.join("timeline.jsonl")).unwrap_or_default();
        assert!(timeline.contains("summary"), "timeline 必须落 summary 事件");
    }

    /// ⑬ spawn_project_summary 失败留痕端到端（estop 臂 → fail note 落
    /// 顶层父单 System 评论）。
    #[tokio::test]
    async fn w5_spawn_project_summary_fail_note_end_to_end() {
        let (deps, _ws) = w5_deps("summary-spawn-fail");
        let pid = deps
            .store
            .create_project("派发总结项目", "", None, "", "", None)
            .unwrap()
            .id;
        let parent = w5_issue(&deps.store, "顶层父单", None, Some(pid), "");
        deps.estop.trigger(); // run_project_summary 立即 Err → fail note
        spawn_project_summary(deps.clone(), pid);
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let comments = deps.store.list_comments(parent.id).unwrap();
        assert!(
            comments.iter().any(
                |c| c.ctype == CommentType::System && c.content.contains("AI 收口总结生成失败")
            ),
            "失败必须落父单 System 评论: {:?}",
            comments.iter().map(|c| &c.content).collect::<Vec<_>>()
        );
    }
}

// ---------- is_system_terminal_text（压测①b：重试防污染） ----------

#[test]
fn system_terminal_text_matches_engine_done_forms() {
    // 与引擎 Done 构造点逐一对应（run_loop/recovery/tool_batch/turn_guard）。
    assert!(super::is_system_terminal_text(
        "已在定时任务预算 1 轮工具调用后暂停，已完成的工作已保存。定时任务未被删除，下次触发时会重新获得预算。"
    ));
    assert!(super::is_system_terminal_text(
        "已在 20 轮工具调用后暂停，已完成的工作已保存。发送下一条消息可继续，或调大 max_tool_iterations（设为 0 表示不限）。"
    ));
    assert!(super::is_system_terminal_text("已取消"));
    assert!(super::is_system_terminal_text(
        "⛔ 已急停 (E-STOP) — 已停止当前任务。发送 `nemesisbot estop --release` 恢复。"
    ));
    assert!(super::is_system_terminal_text(
        "⛔ 已急停 (E-STOP) — LLM 调用已中断。发送 `nemesisbot estop --release` 恢复。"
    ));
    assert!(super::is_system_terminal_text(
        "⛔ HOOK BLOCKED [layer:hook|policy:llm_hook] x — denied."
    ));
    assert!(super::is_system_terminal_text(
        "（模型多次未给出有效答复，已停止重试。请重试或换一种问法。）"
    ));
}

#[test]
fn system_terminal_text_does_not_flag_legit_review_output() {
    // 评审 JSON 正常内容（含偶现同词）不得误伤——否则真评审结论被当污染丢弃。
    let legit =
        r#"{"verdict":"pass","reasons":["交付齐全，其中已取消的旧任务不要求覆盖"],"gap":""}"#;
    assert!(!super::is_system_terminal_text(legit));
    assert!(!super::is_system_terminal_text(
        "{\"verdict\":\"fail\",\"reasons\":[],\"gap\":\"estop 按钮文案缺失\"}"
    ));
    // 空串/纯空白不是系统文本（交给 parse_review 走正常失败路径）。
    assert!(!super::is_system_terminal_text(""));
    assert!(!super::is_system_terminal_text("   \n"));
}

// ---------- 双根锚点核验（2026-10-09 NB-22 根修回归锁）----------
// 档案管线把 worker 产物合并在项目目录（主 workspace 根没有文件）——
// 挂项目单子的 file: 锚点必须能经项目目录 fallback 核验通过，否则合格
// 交付被锚点短路 FAIL 无限重派（NB-22 web-FPS C2 实证：5 锚点全败 os
// error 2 → 首派即重派）。

#[tokio::test]
async fn subissue_file_anchor_passes_via_project_dir_fallback() {
    use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, ws) = review_deps("anchor-dual-root");
    let reviewer = Actor::agent("node-a");

    // 项目目录（档案管线合并产物落点）带锚点文件；主 workspace 根刻意不写
    //（= NB-22 现场：worker 只在 exec 副本产出，合并只进项目目录）。
    let proj_dir = ws.parent().unwrap().join("anchor-dual-root-projdir");
    std::fs::create_dir_all(proj_dir.join("fps-game/src")).unwrap();
    std::fs::write(
        proj_dir.join("fps-game/src/weapons.js"),
        "export const WEAPON_DEFS = [];\n",
    )
    .unwrap();
    let project = deps
        .store
        .create_project("双根", "d", None, "", "", Some(proj_dir.to_str().unwrap()))
        .unwrap();

    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "C2 武器层".into(),
            description: String::new(),
            priority: 2,
            acceptance_criteria: Some(
                "[CHECK] file:fps-game/src/weapons.js exists\n[CHECK] file:fps-game/src/weapons.js contains:WEAPON_DEFS"
                    .into(),
            ),
            creator: reviewer.clone(),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InProgress, &reviewer)
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InReview, &reviewer)
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: issue.id,
            author: Actor::agent("node-b"),
            content: "## 结论\n交付完成，weapons.js 已落盘。".into(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    // 锚点经项目目录 fallback 全过 → 进 LLM 臂 → moderator_loop 空槽诚实
    // 跳过（Ok(false)）；若 fallback 失效则锚点短路 FAIL → 重派（Ok(true)）。
    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("评审必须闭环不报错");
    assert!(
        !reviewed,
        "锚点应经项目目录 fallback 通过并止于 LLM 未就绪跳过（非短路 FAIL）"
    );
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查通过")),
        "锚点 PASS 评论应落库（fallback 命中）"
    );
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("客观锚点检查失败")),
        "不得出现锚点失败短路"
    );
    let _ = std::fs::remove_dir_all(&proj_dir);
}

// ---------- M4-A 交付守卫（write_back_board_dispatch，2026-10-10） ----------

/// 守卫测试夹具：临时 store + issue + 一条 RUNNING 派发行。
fn wb_store(
    tag: &str,
) -> (
    std::path::PathBuf,
    std::sync::Arc<nemesis_board::BoardStore>,
    i64,
    String,
) {
    let dir = std::env::temp_dir().join(format!("nb-wb-guard-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store =
        std::sync::Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap());
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "守卫单".into(),
            ..Default::default()
        })
        .unwrap();
    let task_id = format!("task-{tag}-1");
    // 派发前置状态：单据在执行中（写回的 in_review 转移以 InProgress 为前提）。
    store
        .transition_issue(
            issue.id,
            nemesis_board::IssueStatus::InProgress,
            &nemesis_board::Actor::system("t"),
        )
        .unwrap();
    store
        .insert_dispatch(
            &task_id,
            issue.id,
            "node-w",
            &nemesis_board::Actor::system("t"),
        )
        .unwrap();
    (dir, store, issue.id, task_id)
}

/// M4-A：status 成功但回传是系统终态文本（急停）→ 交付守卫拦截，
/// 按任务失败处置（FAILED + ⛔ 评论 + fail_class 标记），单据仍进
/// in_review（失败走同一验收决策链，不是悄悄吞掉）。
#[test]
fn delivery_guard_blocks_system_terminal_text() {
    let (dir, store, issue_id, task_id) = wb_store("estop");
    let out = crate::commands::gateway::write_back_board_dispatch(
        &Some(store.clone()),
        &dir,
        &task_id,
        "ok",
        "(E-STOP) emergency stop triggered mid-task",
        "",
    );
    assert!(out.is_board_task);
    assert!(out.settled);
    assert_eq!(out.issue_for_review, Some(issue_id));
    let disp = store.get_dispatch(&task_id).unwrap().unwrap();
    assert_eq!(disp.state, nemesis_board::models::dispatch_state::FAILED);
    let issue = store.get_issue(issue_id).unwrap();
    assert_eq!(issue.status, nemesis_board::IssueStatus::InReview);
    let comments = store.list_comments(issue_id).unwrap();
    let guard = comments
        .iter()
        .find(|c| c.content.contains("交付守卫拦截"))
        .expect("guard comment must exist");
    assert!(
        guard.content.contains("E-STOP"),
        "原文应留痕：{}",
        guard.content
    );
    assert!(
        guard
            .content
            .contains("fail_class: delivery_guard_terminal_text"),
        "fail_class 应标记：{}",
        guard.content
    );
}

/// M4-A：空串回传同样触发守卫（worker 没说话不等于任务完成）。
#[test]
fn delivery_guard_blocks_empty_response() {
    let (dir, store, issue_id, task_id) = wb_store("empty");
    let out = crate::commands::gateway::write_back_board_dispatch(
        &Some(store.clone()),
        &dir,
        &task_id,
        "ok",
        "   ",
        "",
    );
    assert!(out.settled);
    let disp = store.get_dispatch(&task_id).unwrap().unwrap();
    assert_eq!(disp.state, nemesis_board::models::dispatch_state::FAILED);
    let comments = store.list_comments(issue_id).unwrap();
    assert!(comments.iter().any(|c| c.content.contains("交付守卫拦截")));
}

/// M4-A 对照组：status=error 时系统终态文本走既有失败路径（⛔ worker
/// 汇报失败），守卫不重复触发（守卫只在 status 成功臂生效）。
#[test]
fn error_status_bypasses_guard_uses_plain_failure_path() {
    let (dir, store, issue_id, task_id) = wb_store("err");
    let out = crate::commands::gateway::write_back_board_dispatch(
        &Some(store.clone()),
        &dir,
        &task_id,
        "error",
        "(E-STOP) estop text on the error path",
        "worker_aborted",
    );
    assert!(out.settled);
    let disp = store.get_dispatch(&task_id).unwrap().unwrap();
    assert_eq!(disp.state, nemesis_board::models::dispatch_state::FAILED);
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("⛔ worker 汇报失败")),
        "应走普通失败评论：{:?}",
        comments.iter().map(|c| &c.content).collect::<Vec<_>>()
    );
    assert!(
        !comments.iter().any(|c| c.content.contains("交付守卫拦截")),
        "error 臂不得触发交付守卫"
    );
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("fail_class: worker_aborted")),
        "入参 fail_class 应原样落评论"
    );
}

/// M4-A 对照组：正常交付（有内容、非系统终态）不触发守卫，DONE + 交付
/// 评论，in_review 照常。
#[test]
fn normal_delivery_skips_guard() {
    let (dir, store, issue_id, task_id) = wb_store("normal");
    let out = crate::commands::gateway::write_back_board_dispatch(
        &Some(store.clone()),
        &dir,
        &task_id,
        "ok",
        "任务已完成：修改了 config.rs 并通过全部测试。证据：测试输出 4 passed。",
        "",
    );
    assert!(out.settled);
    assert_eq!(out.issue_for_review, Some(issue_id));
    let disp = store.get_dispatch(&task_id).unwrap().unwrap();
    assert_eq!(disp.state, nemesis_board::models::dispatch_state::DONE);
    let comments = store.list_comments(issue_id).unwrap();
    assert!(
        !comments.iter().any(|c| c.content.contains("交付守卫拦截")),
        "正常交付不得触发守卫"
    );
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("✅ worker 汇报完成")),
        "无格式汇报诚实降级为普通完成评论"
    );
}

// ---------- M8 缺陷闸门（2026-10-10 机制加固）：扫描语义 + 父单/项目闸 ----------

/// 扫描纯语义：trim_start 后**行首** `[P0]`/`[P1]` 算声明（缩进也算——
/// trim_start 语义；正文提及、`[P2]` 非、`[FIXEDX]` 非）；`[FIXED]` FIFO
/// 只关最早一条在账缺陷，多余 FIXED 无在账 = 空操作。
#[test]
fn defect_scan_line_start_and_fifo_semantics() {
    use nemesis_board::{Actor, CommentType, NewComment, NewIssue};
    let dir = std::env::temp_dir().join(format!("nb-m8-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let issue = store
        .create_issue(NewIssue {
            title: "扫描单".into(),
            ..Default::default()
        })
        .unwrap();
    let comment = |content: &str, ctype: CommentType| {
        store
            .add_comment(NewComment {
                issue_id: issue.id,
                author: Actor::agent("w"),
                content: content.to_string(),
                parent_id: None,
                ctype,
            })
            .unwrap();
    };

    comment(
        "完成，遗留：\n[P0] 崩溃一\n缩进也算（trim_start 语义）：\n  [P1] 崩溃一乙\n正文提及 [P0] 不算行首\n[P2] 三档不算\n[FIXEDX] 非闭环标记",
        CommentType::Delivery,
    );
    let open = scan_unclosed_defects(&store, std::slice::from_ref(&issue)).unwrap();
    assert_eq!(open.len(), 2, "行首 P0 + 缩进 P1 入账，其余不算: {open:?}");
    assert_eq!(open[0].severity, "P0");
    assert_eq!(open[0].snippet, "崩溃一", "描述须去掉标记前缀");
    assert_eq!(open[1].severity, "P1");
    assert_eq!(open[1].snippet, "崩溃一乙");

    comment("[P1] 崩溃二", CommentType::Comment);
    let open = scan_unclosed_defects(&store, std::slice::from_ref(&issue)).unwrap();
    assert_eq!(open.len(), 3, "三条声明都在账: {open:?}");

    // FIFO：一条 FIXED 只关最早（崩溃一），其余保持在账。闭环只认
    // Delivery（D11-3：拆闸方向只走正式交付通道）。
    comment("[FIXED] 崩溃一已修复", CommentType::Delivery);
    let open = scan_unclosed_defects(&store, std::slice::from_ref(&issue)).unwrap();
    assert_eq!(open.len(), 2, "FIFO 只关最早一条: {open:?}");
    assert_eq!(open[0].snippet, "崩溃一乙", "剩余的应是后声明那些");
    assert_eq!(open[1].snippet, "崩溃二", "剩余的应是后声明那些");

    // D11-3 负向锁：非 Delivery（Discussion/Comment/system…）评论行首
    // [FIXED] 不闭环——任何能落讨论评论的路径不得借此清空在账缺陷。
    comment("[FIXED] 崩溃一乙讨论区闭环", CommentType::Comment);
    let open = scan_unclosed_defects(&store, std::slice::from_ref(&issue)).unwrap();
    assert_eq!(open.len(), 2, "Comment 形 [FIXED] 不得闭环: {open:?}");

    // 逐条闭环（Delivery）+ 多余 FIXED（无在账缺陷）= 空操作不炸。
    comment("[FIXED]", CommentType::Delivery);
    comment("[FIXED] 多余闭环", CommentType::Delivery);
    let open = scan_unclosed_defects(&store, &[issue]).unwrap();
    assert!(open.is_empty(), "全部闭环后应无在账: {open:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 父单缺陷闸门：子单交付行首 [P0] → 自动收口确定性转人工（不进语义评审
/// ——否则 loop 缺位臂会 Ok(false) 跳过而非转人工），评论留痕 + 审计入账。
#[tokio::test]
async fn parent_defect_gate_blocks_auto_close() {
    use nemesis_board::{CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, _ws) = review_deps("parent-defect-gate");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true}}"#,
    )
    .unwrap();
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "缺陷父单".to_string(),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(NewIssue {
            title: "缺陷子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InReview, &creator)
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: child.id,
            author: nemesis_board::Actor::agent("node-w"),
            content: "任务完成，但发现遗留问题：\n[P0] 登录接口在并发下崩溃（复现率 30%）"
                .to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    assert!(reviewed, "缺陷闸门转人工应闭环");
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview,
        "缺陷闸门转人工（保持 in_review）"
    );
    let comments = deps.store.list_comments(parent.id).unwrap();
    let gate = comments
        .iter()
        .find(|c| c.content.contains("[缺陷闸门]"))
        .expect("父单必须有缺陷闸门评论");
    assert!(
        gate.content.contains(&child.number),
        "评论须含声明单号 {}: {}",
        child.number,
        gate.content
    );
    assert!(
        gate.content.contains("登录接口在并发下崩溃"),
        "评论须含缺陷描述: {}",
        gate.content
    );
    let acts = deps.store.list_activity(parent.id).unwrap();
    assert!(
        acts.iter().any(|a| {
            a.action == "auto_decide"
                && a.details
                    .as_deref()
                    .is_some_and(|d| d.contains("parent_defect_gate"))
        }),
        "缺陷闸门决策必须入审计账"
    );
}

/// 对照组：声明已被行首 [FIXED] 闭环 → 闸门放行（无缺陷评论），落回既有
/// 语义臂（loop 缺位诚实跳过 Ok(false)），自动收口链恢复原状。
#[tokio::test]
async fn parent_defect_gate_fixed_recovers_auto_flow() {
    use nemesis_board::{CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, _ws) = review_deps("parent-defect-fixed");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true}}"#,
    )
    .unwrap();
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "已修复父单".to_string(),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(NewIssue {
            title: "已修复子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InReview, &creator)
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: child.id,
            author: nemesis_board::Actor::agent("node-w"),
            content: "[P0] 曾发现崩溃".to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: child.id,
            author: nemesis_board::Actor::agent("node-w"),
            content: "[FIXED] 崩溃已根修并补回归测试".to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    assert!(!reviewed, "闭环后闸门放行 → 落语义臂（loop 缺位跳过）");
    let comments = deps.store.list_comments(parent.id).unwrap();
    assert!(
        !comments.iter().any(|c| c.content.contains("[缺陷闸门]")),
        "闭环后不得再出缺陷闸门评论"
    );
    assert_eq!(
        deps.store.get_issue(parent.id).unwrap().status,
        IssueStatus::InReview
    );
}

/// 项目级缺陷闸门：父单全 done 但子树下有未闭环 [P1] → 项目收口确定性
/// 转人工，缺口评论落顶层父单（项目无 comment 载体），项目保持 active。
#[tokio::test]
async fn project_defect_gate_blocks_completion() {
    use nemesis_board::{CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, _ws) = review_deps("project-defect-gate");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"review":{"auto_close_project":true}}}"#,
    )
    .unwrap();
    let creator = nemesis_board::Actor::agent("node-a");
    let pid = deps
        .store
        .create_project("缺陷项目", "", None, "", "", None)
        .unwrap()
        .id;
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "缺陷项目父单".to_string(),
            creator: creator.clone(),
            project_id: Some(pid),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(NewIssue {
            title: "缺陷项目子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            project_id: Some(pid),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::Done, &creator)
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: child.id,
            author: nemesis_board::Actor::agent("node-w"),
            content: "[P1] 导出文件缺时间戳".to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = review_project_completion(&deps, pid).await.unwrap();
    assert!(reviewed, "项目缺陷闸门转人工应闭环");
    assert_eq!(
        deps.store.get_project(pid).unwrap().status,
        "active",
        "项目不得被收口"
    );
    let comments = deps.store.list_comments(parent.id).unwrap();
    let gate = comments
        .iter()
        .find(|c| c.content.contains("[缺陷闸门]"))
        .expect("父单必须有项目缺陷闸门评论");
    assert!(
        gate.content.contains(&child.number),
        "评论须含声明单号 {}: {}",
        child.number,
        gate.content
    );
    let acts = deps.store.list_activity(parent.id).unwrap();
    assert!(
        acts.iter().any(|a| {
            a.action == "auto_decide"
                && a.details
                    .as_deref()
                    .is_some_and(|d| d.contains("project_defect_gate"))
        }),
        "项目缺陷闸门决策必须入审计账"
    );
}

// ---- U-A（2026-10-12 停车现场工程）：停车档案落盘断言 ----
// 三个代表臂（UNSURE 闸 / token 预算 / 缺陷闸门）经完整 review 链跑通后，
// 断言 `<workspace>/board/handoffs/<NB-x>.json` 在场且字段正确——停车档案
// 是「继续 NB-x」LLM 接手链路的数据源，缺席 = 接手断链。

/// handoff JSON 读取助手（断言在场 + 反序列化）。
fn read_handoff_json(ws: &std::path::Path, number: &str) -> serde_json::Value {
    let path = nemesis_board::handoff::handoff_path(ws, number);
    assert!(path.is_file(), "停车档案必须落盘：{}", path.display());
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).expect("handoff JSON")
}

#[tokio::test]
async fn ua_unsure_park_writes_handoff_archive() {
    use nemesis_board::IssueStatus;
    let (deps, ws) = review_deps("ua-unsure");
    let issue = delivered_issue(&deps.store, "UNSURE 停车档案", "t-ua-unsure-1");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("UNSURE", "证据不足"),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("UNSURE 闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );

    let doc = read_handoff_json(&ws, &issue.number);
    assert_eq!(doc["schema"], 1);
    assert_eq!(doc["issue"], issue.number.as_str());
    // 单轮 UNSURE 走 EscalateHuman 通用臂：诚实分型 unsure（≠ streak 闸的
    // unsure_streak——默认阈值 2，一轮不构成 streak）。
    assert_eq!(doc["stop_kind"], "unsure");
    assert_eq!(doc["suggested_action"], "manual_check");
    assert!(
        doc["resume_entry"]
            .as_str()
            .unwrap()
            .contains("review.retrigger"),
        "resume 入口必须可执行: {}",
        doc["resume_entry"]
    );
    // 停车评论带档案注记（人可读轨指路机器可读轨）。
    let comments = deps.store.list_comments(issue.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.contains("停车现场已存档")),
        "转人工评论必须带停车档案注记"
    );
}

#[tokio::test]
async fn ua_budget_park_writes_handoff_archive() {
    use nemesis_board::IssueStatus;
    let (deps, ws) = review_deps("ua-budget");
    write_board_config(&deps.home, r#"{"max_redispatch":0}"#);
    let issue = delivered_issue(&deps.store, "预算停车档案", "t-ua-budget-1");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "缺口乙"),
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("FAIL+零预算闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview
    );

    let doc = read_handoff_json(&ws, &issue.number);
    // max_redispatch=0 的 FAIL 走 EscalateHuman 通用臂：诚实分型
    // redispatch_budget（≠ token 会话预算的 budget）。
    assert_eq!(doc["stop_kind"], "redispatch_budget");
    assert_eq!(doc["suggested_action"], "manual_check");
    assert!(
        doc["environment"].as_object().is_some(),
        "environment 快照必须在场: {}",
        doc["environment"]
    );
}

#[tokio::test]
async fn ua_defect_gate_park_writes_handoff_archive() {
    use nemesis_board::{CommentType, IssueStatus, NewComment, NewIssue};

    let (deps, ws) = review_deps("ua-defect-gate");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true}}"#,
    )
    .unwrap();
    let creator = nemesis_board::Actor::agent("node-a");
    let parent = deps
        .store
        .create_issue(NewIssue {
            title: "UA 缺陷父单".to_string(),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let child = deps
        .store
        .create_issue(NewIssue {
            title: "UA 缺陷子单".to_string(),
            creator: creator.clone(),
            parent_issue_id: Some(parent.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InProgress, &creator)
        .unwrap();
    deps.store
        .transition_issue(parent.id, IssueStatus::InReview, &creator)
        .unwrap();
    deps.store
        .add_comment(NewComment {
            issue_id: child.id,
            author: nemesis_board::Actor::agent("node-w"),
            content: "完成\n[P1] 内存泄漏未修".to_string(),
            parent_id: None,
            ctype: CommentType::Delivery,
        })
        .unwrap();

    let reviewed = review_parent_issue(&deps, parent.id).await.unwrap();
    assert!(reviewed);

    let doc = read_handoff_json(&ws, &parent.number);
    assert_eq!(doc["stop_kind"], "defect_gate");
    assert_eq!(doc["suggested_action"], "manual_check");
    assert_eq!(doc["environment"]["unclosed_defects"], 1);
}

// ---------- B3 禁令节预取（extract_ban_section / prefetch_ban_section）----------

#[test]
fn extract_ban_section_stops_at_next_section() {
    let text = "# 架构\n\nintro\n\n## 禁令与约束清单\nC1 禁止在接收层调用 poll()\nC2 禁止持锁跨 await\n\n## 其他节\nbody";
    let section = extract_ban_section(text).expect("有节必抽出");
    assert!(section.starts_with("## 禁令与约束清单"));
    assert!(section.contains("C1 禁止在接收层调用 poll()"));
    assert!(section.contains("C2 禁止持锁跨 await"));
    assert!(!section.contains("其他节"), "下一节头起整体截断");
    assert!(!section.contains("body"));
    // 尾部换行被 trim_end 收掉。
    assert!(!section.ends_with('\n'));
}

#[test]
fn extract_ban_section_extends_to_eof_when_last() {
    let text = "前文\n\n## 禁令与约束清单\nC1 禁止裸指针\n";
    let section = extract_ban_section(text).expect("文末节也抽出");
    assert!(section.contains("C1 禁止裸指针"));
}

#[test]
fn extract_ban_section_absent_header_is_none() {
    assert_eq!(extract_ban_section("# 架构\n只有普通内容"), None);
    assert_eq!(extract_ban_section(""), None);
    // 同形但非节头的文字不命中（find 的是整行节头子串——出现即命中，
    // 这里钉住「必须带 ## 前缀」的形态才抽得出）。
    assert!(extract_ban_section("## 禁令与约束清单").is_some());
}

#[test]
fn prefetch_ban_section_prefers_main_root_then_falls_back() {
    let ws = tempfile::tempdir().expect("ws");
    let proj = tempfile::tempdir().expect("proj");

    // 双根都缺 → None。
    assert_eq!(prefetch_ban_section(ws.path(), Some(proj.path())), None);

    // 仅 fallback 有 → 走 fallback（NB-22 双根形态：交付集合并在项目目录）。
    let proj_docs = proj.path().join("docs");
    std::fs::create_dir_all(&proj_docs).unwrap();
    std::fs::write(
        proj_docs.join("architecture.md"),
        "## 禁令与约束清单\nC9 禁止同步 IO\n",
    )
    .unwrap();
    assert_eq!(
        prefetch_ban_section(ws.path(), Some(proj.path())).expect("fallback 命中"),
        "## 禁令与约束清单\nC9 禁止同步 IO"
    );

    // 主根也有 → 主根优先（不是拼接、不是 fallback 抢答）。
    let ws_docs = ws.path().join("docs");
    std::fs::create_dir_all(&ws_docs).unwrap();
    std::fs::write(
        ws_docs.join("architecture.md"),
        "## 禁令与约束清单\nC1 主根禁令\n",
    )
    .unwrap();
    assert_eq!(
        prefetch_ban_section(ws.path(), Some(proj.path())).expect("主根命中"),
        "## 禁令与约束清单\nC1 主根禁令"
    );

    // 主根有文件但节缺席 → 穿透到 fallback（不是拿到文件就停）。
    std::fs::write(ws_docs.join("architecture.md"), "# 架构无禁令节\n").unwrap();
    assert_eq!(
        prefetch_ban_section(ws.path(), Some(proj.path())).expect("主根缺节穿透 fallback"),
        "## 禁令与约束清单\nC9 禁止同步 IO"
    );

    // fallback 槽位为 None + 主根缺 → None（无 panic）。
    std::fs::remove_file(ws_docs.join("architecture.md")).unwrap();
    assert_eq!(prefetch_ban_section(ws.path(), None), None);
}

// ---------- 取证软件化：执行档案直读取证（collect_execution_evidence）----------
//
// 纯函数（root/编号 → Option<String>），零 LLM 零 RPC；钉死：目录缺席 /
// 空轮目录 / 全空件 → None（诚实回落 LLM 自检）；最新一轮优先；raw.json
// 不作证据主体；输出有界。I/O 链路（selfcheck else 臂接线、二段验收）由
// cluster-uat 真流覆盖。

#[test]
fn execution_evidence_missing_dir_is_none() {
    let root = tempfile::tempdir().expect("root");
    assert_eq!(collect_execution_evidence(root.path(), "NB-404"), None);
}

#[test]
fn execution_evidence_empty_round_dir_is_none() {
    let root = tempfile::tempdir().expect("root");
    let round = root
        .path()
        .join("records")
        .join("NB-1")
        .join("execution")
        .join("20261015_100000000");
    std::fs::create_dir_all(&round).unwrap();
    assert_eq!(collect_execution_evidence(root.path(), "NB-1"), None);
}

#[test]
fn execution_evidence_only_empty_or_lost_files_is_none() {
    let root = tempfile::tempdir().expect("root");
    let round = root
        .path()
        .join("records")
        .join("NB-2")
        .join("execution")
        .join("20261015_100000000");
    std::fs::create_dir_all(&round).unwrap();
    std::fs::write(round.join("00.request.md"), "  \n\t").unwrap();
    std::fs::write(round.join("01.AI.Response.md"), "_NEMESIS_UNREADABLE_").unwrap();
    std::fs::write(round.join("01.AI.Response.raw.json"), "{\"x\":1}").unwrap();
    assert_eq!(collect_execution_evidence(root.path(), "NB-2"), None);
}

#[test]
fn execution_evidence_reads_newest_round_and_skips_raw_json() {
    let root = tempfile::tempdir().expect("root");
    let exec = root.path().join("records").join("NB-3").join("execution");
    let old = exec.join("20261015_100000000");
    let new = exec.join("20261015_110000000");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    std::fs::write(old.join("00.request.md"), "旧轮次任务请求").unwrap();
    std::fs::write(old.join("01.AI.Response.md"), "旧轮次回复").unwrap();
    std::fs::write(new.join("00.request.md"), "新轮次任务请求").unwrap();
    std::fs::write(new.join("02.AI.Response.md"), "新轮次最终回复").unwrap();
    std::fs::write(new.join("02.AI.Response.raw.json"), "{\"huge\":true}").unwrap();
    std::fs::write(new.join("manifest.json"), "{\"task_id\":\"t1\"}").unwrap();

    let ev = collect_execution_evidence(root.path(), "NB-3").expect("证据在场");
    assert!(ev.contains("20261015_110000000"), "取最新一轮: {ev}");
    assert!(ev.contains("新轮次最终回复"), "AI 回复头在场: {ev}");
    assert!(ev.contains("新轮次任务请求"), "任务请求头在场: {ev}");
    assert!(
        ev.contains("records/NB-3/execution/20261015_110000000/manifest.json"),
        "清单为档案根相对路径（board_evidence_read 可原样消费）: {ev}"
    );
    assert!(!ev.contains("旧轮次"), "旧轮次不得混入: {ev}");
    assert!(!ev.contains("\"huge\""), "raw.json 不作证据主体: {ev}");
}

#[test]
fn execution_evidence_bounded() {
    let root = tempfile::tempdir().expect("root");
    let round = root
        .path()
        .join("records")
        .join("NB-4")
        .join("execution")
        .join("20261015_100000000");
    std::fs::create_dir_all(&round).unwrap();
    std::fs::write(round.join("01.AI.Response.md"), "长".repeat(50_000)).unwrap();
    let ev = collect_execution_evidence(root.path(), "NB-4").expect("证据在场");
    assert!(
        ev.chars().count() <= 7600,
        "输出有界: {}",
        ev.chars().count()
    );
    assert!(ev.ends_with("..."), "截断带省略号注记");
}

// NB-105 真机熔断回归锁（2026-10-15）：评审员照清单逐个 board_evidence_read，
// 裸文件名清单让全部读取落空（「档案文件不存在或不可达」）→ UNSURE×3 转
// 人工、整条依赖链一宿不动。钉死：清单行 = 档案根相对路径，且该路径能
// 原样过取证工具读回原文（往返闭环）。
#[test]
fn execution_evidence_inventory_paths_roundtrip_through_evidence_tool() {
    let root = tempfile::tempdir().expect("root");
    let round = root
        .path()
        .join("records")
        .join("NB-5")
        .join("execution")
        .join("20261015_120000000");
    std::fs::create_dir_all(&round).unwrap();
    std::fs::write(round.join("manifest.json"), "{\"task_id\":\"t5\"}").unwrap();
    std::fs::write(round.join("00.request.md"), "任务请求全文").unwrap();

    let ev = collect_execution_evidence(root.path(), "NB-5").expect("证据在场");
    let line = ev
        .lines()
        .find(|l| l.contains("manifest.json"))
        .expect("清单含 manifest.json 行");
    let path = line
        .trim_start_matches("- ")
        .split("（")
        .next()
        .expect("路径段");
    assert_eq!(
        path, "records/NB-5/execution/20261015_120000000/manifest.json",
        "清单必须是档案根相对路径: {line}"
    );
    // 往返：清单路径直接过取证工具（与评审员同一解析单点）读取成功。
    let content = crate::board_evidence_tool::read_archive_file(root.path(), path, None, None)
        .expect("清单路径可读取");
    assert!(
        content.contains("\"task_id\":\"t5\""),
        "读回原文: {content}"
    );
}

/// 循环升级防抖（2026-10-16 用户裁决）：升级停车（最近 auto_decide =
/// parent_escalate_human）且其后无 admin 活动 = 人工未表态——钩子触发的
/// 复评必须跳过：不烧 LLM、不落新评论、决策词不被反复覆盖。NB-101 实证
/// 的 20:45/20:50/22:01 三连升级就是本测钉死的病灶。
#[tokio::test]
async fn parent_escalated_park_blocks_hook_rereview_until_admin() {
    let (deps, _ws) = review_deps("parent-escalate-sticky");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "升级停车父单", "", "（父单验收输入=子单汇总）");
    record_auto_decide(
        &deps,
        parent.id,
        "parent_escalate_human",
        "unsure",
        serde_json::json!({ "reasons": "证据不足" }),
    );
    let llm = Arc::new(CapturingLlm {
        prompts: std::sync::Mutex::new(Vec::new()),
        reply: review_json("FAIL", "不该被调用的差距"),
    });
    attach_loop(&deps, llm.clone());

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("防抖跳过是正常返回");
    assert!(!reviewed, "升级停车 + 人工未表态 = 跳过复评");
    assert!(
        llm.prompts
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty(),
        "防抖跳过不得烧 LLM"
    );
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_escalate_human"),
        "决策词不被钩子复评覆盖"
    );
    assert!(
        deps.store.list_children(parent.id).unwrap().is_empty(),
        "防抖跳过不得新建返工子单"
    );
}

/// 打回放行：升级停车后落一条 admin 活动（review.retrigger 的留痕评论
/// 即此形态）→ 防抖解除、复评真实执行（FAIL 回复 → 承接返工子单创建）。
#[tokio::test]
async fn parent_admin_activity_after_escalation_unblocks_rereview() {
    use nemesis_board::{Actor, CommentType, NewComment};
    let (deps, _ws) = review_deps("parent-escalate-reopen");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "打回重开父单", "", "（父单验收输入=子单汇总）");
    record_auto_decide(
        &deps,
        parent.id,
        "parent_escalate_human",
        "unsure",
        serde_json::json!({}),
    );
    deps.store
        .add_comment(NewComment {
            issue_id: parent.id,
            author: Actor::admin("dashboard"),
            content: "🔁 人工打回升级停车（review.retrigger）——自动验收重新开始，返工预算已重置"
                .to_string(),
            parent_id: None,
            ctype: CommentType::System,
        })
        .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "归档缺口"),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("打回后复评闭环");
    assert!(reviewed, "admin 活动晚于升级 = 复评放行");
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_auto_remediate"),
        "打回后评审真实执行（FAIL 承接）"
    );
    assert!(
        deps.store
            .list_children(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.title.starts_with("返工：")),
        "打回后 FAIL 承接返工子单"
    );
}

/// UNSURE 带可行动证据缺口（gap 空、reasons 非空）→ 自动承接返工（理由
/// 渲染进返工单描述），父单保持 in_review 不弹人工（2026-10-16 放宽：
/// 补证据 = 补差距，「无法核实」的归档/取证缺口是可派发的工作）。
#[tokio::test]
async fn parent_unsure_with_reasons_auto_remediates() {
    let (deps, _ws) = review_deps("parent-unsure-remediate");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "证据不足父单", "", "（父单验收输入=子单汇总）");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: r#"{"verdict":"UNSURE","reasons":["交付物未统一归档主工作区"],"gap":""}"#
                .to_string(),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("UNSURE 承接闭环");
    assert!(reviewed);
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_auto_remediate"),
        "UNSURE 带证据缺口 = 承接不弹人工"
    );
    let rem = deps
        .store
        .list_children(parent.id)
        .unwrap()
        .into_iter()
        .find(|c| c.title.starts_with("返工："))
        .expect("UNSURE 必须承接返工子单");
    assert!(
        rem.description.contains("交付物未统一归档主工作区"),
        "评审理由（证据缺口清单）必须进返工单描述: {}",
        rem.description
    );
}

/// UNSURE 双空（gap 与 reasons 双空 = 真无事可做）→ 才落转人工；升级评论
/// 必须说清楚（带打回重开指引——retrigger 打回后自动验收重新开始）。
#[tokio::test]
async fn parent_unsure_no_actionable_gap_escalates_with_reopen_hint() {
    let (deps, _ws) = review_deps("parent-unsure-escalate");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "无事可做父单", "", "（父单验收输入=子单汇总）");
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: r#"{"verdict":"UNSURE","reasons":[],"gap":""}"#.to_string(),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("双空 UNSURE 闭环");
    assert!(reviewed);
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_escalate_human"),
        "双空 UNSURE 落转人工（无人力可承接）"
    );
    assert!(
        deps.store
            .list_comments(parent.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("打回重开") && c.content.contains("review.retrigger")),
        "升级评论必须带打回重开指引"
    );
}

/// 打回重开（2026-10-16）：预算耗尽（2 个历史返工单）+ retrigger 写入
/// 纪元键 → 历史返工单不再计轮，复评 FAIL 重新承接「第 1/2 轮」——人工
/// 重新决策 = 自动验收重新开始，不再死锁在转人工。
#[tokio::test]
async fn parent_retrigger_resets_remediation_budget() {
    use nemesis_board::NewIssue;
    let (deps, _ws) = review_deps("parent-remediate-reset");
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
    let parent = issue_in_review(&deps.store, "预算重置父单", "", "（父单验收输入=子单汇总）");
    for round in 1..=2 {
        deps.store
            .create_issue(NewIssue {
                title: format!("返工：预算重置父单（第 {round}/2 轮）"),
                description: "历史返工单（纪元前）".to_string(),
                priority: 2,
                creator: nemesis_board::Actor::system("board-review"),
                parent_issue_id: Some(parent.id),
                ..Default::default()
            })
            .unwrap();
    }
    // retrigger 同款打回动作：写预算重置纪元（+1s 保证晚于历史子单
    // created_at，规避同秒写库时间戳打平）。
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        + 1;
    deps.store
        .set_meta(
            &format!("remediation_reset:{}", parent.id),
            &epoch.to_string(),
        )
        .unwrap();
    attach_loop(
        &deps,
        Arc::new(CapturingLlm {
            prompts: std::sync::Mutex::new(Vec::new()),
            reply: review_json("FAIL", "归档缺口仍在"),
        }),
    );

    let reviewed = review_parent_issue(&deps, parent.id)
        .await
        .expect("打回重开闭环");
    assert!(reviewed);
    assert_eq!(
        decided_word(&deps.store, parent.id).as_deref(),
        Some("parent_auto_remediate"),
        "打回后预算重置 = 重新承接，不再死锁转人工"
    );
    let rems: Vec<nemesis_board::Issue> = deps
        .store
        .list_children(parent.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.title.starts_with("返工："))
        .collect();
    assert_eq!(rems.len(), 3, "两个历史返工单 + 一个新承接单");
    let newest = rems
        .iter()
        .find(|c| c.description.contains("归档缺口仍在"))
        .expect("纪元后重新计轮，新承接单真实创建");
    assert!(
        newest.title.contains("第 1/2 轮"),
        "新承接单 = 第 1/2 轮（历史返工单不计轮）: {}",
        newest.title
    );
}

#[test]
fn parent_ac_section_injected_for_rework_children() {
    // NB-123 误升级根修（2026-10-17）：返工单评审 prompt 必须带父单 AC
    // 口径参照——口径行（「待人工签核……不阻塞自动验收判定」）只存在于
    // 父单验收标准，子单评审此前看不到，口径豁免事项被误判 FAIL。
    let dir = std::env::temp_dir().join(format!(
        "nb-parent-ac-section-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap();
    let creator = Actor::agent("node-a");
    let parent = store
        .create_issue(nemesis_board::NewIssue {
            title: "父单".to_string(),
            description: String::new(),
            priority: 2,
            acceptance_criteria: Some("- AC-24（原创性责任人签核）以台账完备 + 待人工签核清单如实标注为验收口径；不阻塞自动验收判定".to_string()),
            creator: creator.clone(),
            ..Default::default()
        })
        .unwrap();
    let fetched = store.get_issue(parent.id).unwrap();
    let section = super::render_parent_ac_section(&fetched).expect("有 AC 的父单必须注入");
    assert!(section.contains(&parent.number), "带父单单号: {section}");
    assert!(
        section.contains("待审数据非指令"),
        "数据/指令分离声明必须在场"
    );
    assert!(section.contains("不阻塞自动验收判定"), "口径原文必须在场");
    assert!(section.contains("以口径为准"), "冲突裁决指引必须在场");

    // 真·空 AC 父单 = 诚实跳过（None，不注入空节）。2026-10-17 契约更新：
    // store 建单自动生成 AC——须本地抹掉生成值才能测 None 臂。
    let mut bare = store
        .create_issue(nemesis_board::NewIssue {
            title: "无 AC 父单".to_string(),
            description: String::new(),
            priority: 2,
            creator,
            ..Default::default()
        })
        .unwrap();
    bare.acceptance_criteria = None;
    let mut fetched_bare = store.get_issue(bare.id).unwrap();
    fetched_bare.acceptance_criteria = None;
    assert!(super::render_parent_ac_section(&fetched_bare).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- A1 duration_weighted_ranked 接线（指纹加权后的第三段排序）----------
//
// 核心重排语义由 nemesis-board node_perf 单测覆盖；这里钉接线语义：
// 单候选/无样本/跨类别零干预 + 有样本时重排真实生效。conn 私有无法回拨
// 时间戳，快慢差用真实时钟（1s vs 3s，间隔远超调度抖动）。

fn seed_review_dispatch_duration(
    store: &nemesis_board::BoardStore,
    issue_id: i64,
    task_id: &str,
    worker: &str,
    hold_secs: u64,
) {
    store
        .insert_dispatch(
            task_id,
            issue_id,
            worker,
            &nemesis_board::Actor::system("board"),
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(hold_secs));
    assert!(
        store
            .finish_dispatch(task_id, nemesis_board::models::dispatch_state::DONE)
            .unwrap(),
        "finish_dispatch 应命中 dispatched 态"
    );
}

#[test]
fn a1_duration_weighted_ranked_wiring() {
    let dir = std::env::temp_dir().join(format!(
        "a1-review-duration-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").unwrap();

    let prof = store
        .create_issue(nemesis_board::NewIssue {
            title: "职能单".to_string(),
            required_profession: Some("dev:cpp".to_string()),
            ..Default::default()
        })
        .unwrap();
    let plain = store
        .create_issue(nemesis_board::NewIssue {
            title: "无职能单".to_string(),
            ..Default::default()
        })
        .unwrap();

    // 单候选透传 + 空仓零干预（原序返回）。
    assert_eq!(
        super::duration_weighted_ranked(&store, &plain, vec!["solo".into()]),
        vec!["solo".to_string()]
    );
    assert_eq!(
        super::duration_weighted_ranked(&store, &plain, vec!["b".into(), "a".into()]),
        vec!["b".to_string(), "a".to_string()]
    );

    // 真实样本：node-fast 3×1s（中位 1）vs node-slow 3×2s（中位 2），
    // 各达 MIN_WORKER_SAMPLES(=3) 才进 per-worker 预测面（欠采样者落
    // 类别全局中位数，1 样本/worker 会双双打平回原序）。
    for task in ["t-f1", "t-f2", "t-f3"] {
        seed_review_dispatch_duration(&store, prof.id, task, "node-fast", 1);
    }
    for task in ["t-s1", "t-s2", "t-s3"] {
        seed_review_dispatch_duration(&store, prof.id, task, "node-slow", 2);
    }

    // 有样本生效：慢者榜首 → 快者提前。
    let ranked = vec!["node-slow".to_string(), "node-fast".to_string()];
    assert_eq!(
        super::duration_weighted_ranked(&store, &prof, ranked.clone()),
        vec!["node-fast".to_string(), "node-slow".to_string()]
    );

    // 跨类别隔离：同批样本（dev:cpp）对无职能单（类别 "_"）零干预。
    assert_eq!(
        super::duration_weighted_ranked(&store, &plain, ranked),
        vec!["node-slow".to_string(), "node-fast".to_string()]
    );

    let _ = std::fs::remove_dir_all(&dir);
}
