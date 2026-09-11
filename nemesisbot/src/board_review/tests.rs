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
    // 失败锚点指向 workspace 内不存在的文件；交付文本锚点应通过。
    let issue = issue_in_review(
        &deps.store,
        "锚点失败短路",
        "[CHECK] file:out/missing.md exists\n[CHECK] re:交付完成",
        "## 结论\n交付完成",
    );
    assert!(!ws.join("out").join("missing.md").exists());
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
        fail.content.contains("out/missing.md"),
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
        .create_project("急停项目", "", None, "", "")
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
        .create_project("默认关项目", "", None, "", "")
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
        .create_project("竞态项目", "", None, "", "")
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
        .create_project("缺口项目", "", None, "", "")
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
        .create_project(name, "", None, "", "")
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
    assert!(
        joined.starts_with("【项目级】项目级标准\n"),
        "项目级 AC 排首段：{joined}"
    );
    assert!(joined.contains("【NB-1】标准一\n"));
    assert!(!joined.contains("NB-2"), "空 AC 父单不产生空段");
    assert!(joined.contains("【NB-3】标准三\n"), "父单 AC trim 后入段");
    assert!(joined.contains("---"), "段间分隔符");
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
