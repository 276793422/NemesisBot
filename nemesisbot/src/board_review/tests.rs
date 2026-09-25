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

    let deps = BoardReviewDeps {
        store,
        workspace: workspace.clone(),
        home: dir,
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: SelfcheckRegistry::new(),
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
    // serde(default)，最小配置即可）。
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"auto_close_parent":true}}"#,
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
    // 本机 worker + 轮数 >1 → 只读取证。
    assert_eq!(
        pick_review_tool_mode(Some("node-a"), "node-a", 3),
        ReviewToolMode::ReadOnly { max_turns: 3 }
    );
    // 远端 worker：工件不在本机，一律纯文本（防张冠李戴读错工作区）。
    assert_eq!(
        pick_review_tool_mode(Some("node-b"), "node-a", 3),
        ReviewToolMode::NoTools
    );
    // 无派发历史（如人工转入 in_review）→ 纯文本。
    assert_eq!(
        pick_review_tool_mode(None, "node-a", 3),
        ReviewToolMode::NoTools
    );
    // max_turns=1：与历史行为字节等价（即便 worker 是本机也不开工具）。
    assert_eq!(
        pick_review_tool_mode(Some("node-a"), "node-a", 1),
        ReviewToolMode::NoTools
    );
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
    let (deps, _ws) = review_deps("d3-fallback");
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
    // AC 缺失 → 诚实占位，不出现空段。
    let bare = store
        .create_issue(NewIssue {
            title: "无 AC".to_string(),
            creator: nemesis_board::Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();
    let prompt = build_selfcheck_prompt(&bare, "贴日志");
    assert!(prompt.contains("（未提供）"), "无 AC 必须占位: {prompt}");
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
    let parents = vec![
        parent_with_ac("NB-1", Some("标准一")),
        parent_with_ac("NB-2", None),
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
    let dir =
        std::env::temp_dir().join(format!("nemesis-board-review-audit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    let issue = store
        .create_issue(NewIssue {
            title: "词表断言".into(),
            ..Default::default()
        })
        .unwrap();

    record_auto_decide(
        &store,
        issue.id,
        "node-review",
        "auto_accept",
        "PASS",
        serde_json::json!({ "gap": "", "note": "锚点全过" }),
    );
    // 失败静默（warn）不炸验收：不存在的 issue_id 不 panic。
    record_auto_decide(
        &store,
        999_999,
        "node-review",
        "redispatch",
        "FAIL",
        serde_json::json!({}),
    );

    let rows = store
        .list_recent_activity(500, Some("auto_decide"))
        .unwrap();
    assert_eq!(rows.len(), 1, "失败调用不落行");
    let row = &rows[0];
    assert_eq!(row.activity.action, "auto_decide");
    assert_eq!(row.activity.actor.kind, "agent");
    assert_eq!(row.activity.actor.id, "node-review");
    let details: serde_json::Value =
        serde_json::from_str(row.activity.details.as_deref().unwrap()).unwrap();
    assert_eq!(details["decision"], "auto_accept");
    assert_eq!(details["verdict"], "PASS");
    assert_eq!(details["note"], "锚点全过");
    assert!(details.get("gap").is_some(), "extra 键合并进 details");
    let _ = std::fs::remove_dir_all(&dir);
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
    let p1 = mk("父单1", 7);
    let p2 = mk("父单2", 7);
    // 无项目级结论 → 重放。
    assert!(!super::project_review_already_concluded(
        &store,
        &[p1, p2],
        7
    ));
    // 父单2 带项目 7 的收口结论（escalate sticky 同样算）→ 不重放。
    store
        .add_activity(
            p2,
            &actor,
            "auto_decide",
            Some(r#"{"decision":"project_escalate_human","verdict":"fail","project_id":7}"#),
        )
        .unwrap();
    assert!(super::project_review_already_concluded(
        &store,
        &[p1, p2],
        7
    ));
    // 结论挂在其他项目名下（project_id=9）→ 项目 7 仍视为未结论。
    let p3 = mk("父单3", 9);
    store
        .add_activity(
            p3,
            &actor,
            "auto_decide",
            Some(r#"{"decision":"project_complete","verdict":"pass","project_id":9}"#),
        )
        .unwrap();
    assert!(!super::project_review_already_concluded(&store, &[p1], 7));
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
    // 大文本（> 8KB）：列清单不附内容。
    let big = "x".repeat(9 * 1024);
    std::fs::write(root.join("big.txt"), &big).unwrap();
    // 二进制扩展名：列清单不附内容。
    std::fs::write(root.join("model.bin"), b"\x00\x01\x02").unwrap();

    let out = super::render_project_artifacts_evidence(&root);
    assert!(
        out.contains("big.txt") && out.contains("9216 字节"),
        "{out}"
    );
    assert!(!out.contains("xxxxx"), "大文件不附内容节选");
    assert!(out.contains("model.bin"), "二进制也在清单");
    assert!(out.contains("未附内容节选"), "诚实披露哪些文件没有节选");
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
    let node = deps.cluster.node_id();

    // 单据仍在 in_review → 结论有效。
    assert!(
        super::review_still_relevant(
            &deps.store,
            issue.id,
            node,
            ReviewVerdict::Pass.as_str(),
            "子单验收"
        ),
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
        !super::review_still_relevant(
            &deps.store,
            issue.id,
            node,
            ReviewVerdict::Pass.as_str(),
            "子单验收"
        ),
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

#[tokio::test]
async fn llm_parse_failure_unlimited_redispatches_with_note() {
    let (deps, _ws) = review_deps("llm-parsefail-unlimited");
    write_board_config(&deps.home, r#"{"unlimited_mode":true}"#);
    let issue = delivered_issue(&deps.store, "无限模式解析失败", "t-llm-parsefail-1");
    attach_loop(
        &deps,
        Arc::new(ScriptedLlm {
            // 恒乱码 → 解析失败 3 轮（非调用失败）→ unlimited 继续重派。
            script: std::sync::Mutex::new(std::collections::VecDeque::new()),
            fallback: "不是 JSON 的评审输出".to_string(),
            on_call: None,
        }),
    );

    let reviewed = review_issue(&deps, issue.id, ReviewCtx::first_stage())
        .await
        .expect("unlimited 解析失败按重派闭环");
    assert!(reviewed);
    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        2,
        "unlimited_mode 解析失败继续重派"
    );
    assert!(
        deps.store.list_comments(issue.id).unwrap().iter().any(|c| c
            .content
            .contains("验收评审未出结论（unlimited_mode 继续重派")
            && c.content.contains("连续 3 轮无法解析")),
        "必须落带失败原因的继续重派评论"
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
    write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
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
        &deps.store,
        concluded.id,
        "node-a",
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

/// 重派决策⑤：活跃派发在途 → dispatch_issue_core 重复闸拒绝 →
/// ⛔ 系统评论诚实留痕，不发车不炸流程。
#[tokio::test]
async fn w5_redispatch_dispatch_failure_leaves_honest_system_comment() {
    use nemesis_board::{CommentType, IssueStatus};
    let (deps, _ws) = review_deps("w5-dispatch-err");
    let issue = issue_in_review(&deps.store, "派发失败留痕", W5_FAIL_AC, W5_FAIL_DELIVERY);
    seed_dispatch(&deps.store, "t-w5-de-1", issue.id, "node-b");
    // 活跃（未完结）派发 → has_active_dispatch 闸必拒。
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
        .expect("重派失败必须诚实闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.ctype == CommentType::System && c.content.contains("自动重派失败"))
        .expect("⛔ 重派失败系统评论在场");
    assert!(
        note.content.contains("已有进行中的派发"),
        "拒绝原因透传: {}",
        note.content
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 2);
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

/// 解析失败臂①：评审 LLM 一路输出非 JSON + unlimited → 按 UNSURE 语义
/// 带说明继续重派（🤷 评论 + 发车），不转人工。
#[tokio::test]
async fn w5_parse_failed_unlimited_redispatches_with_honest_note() {
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
        .expect("unlimited 解析失败必须续派闭环");
    assert!(reviewed);

    let comments = deps.store.list_comments(issue.id).unwrap();
    let note = comments
        .iter()
        .find(|c| c.content.contains("验收评审未出结论"))
        .expect("🤷 解析失败续派评论在场");
    assert!(
        note.content.contains("unlimited_mode 继续重派"),
        "评论带 unlimited 语义: {}",
        note.content
    );
    assert_eq!(deps.store.list_dispatches(issue.id).unwrap().len(), 2);
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InProgress
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
        write_board_config(&deps.home, r#"{"auto_close_parent":true}"#);
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
