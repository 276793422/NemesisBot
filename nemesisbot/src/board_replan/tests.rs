//! AOR 批三 5.3：重规划器编排层单测。夹具刻意不带模型（moderator_loop
//! 留空）：全部闸跳过臂 / 策略闸 / 执行链 / sweep 候选扫描不触碰 LLM 照
//! 样闭环；`run_replan_llm` 起草本体由 cluster-uat 端到端覆盖。纯逻辑层
//! （parse_replan 等）测试在 `nemesis-board/src/replan/tests.rs`。

use super::*;

use nemesis_board::replan::{PROPOSAL_COMMENT_PREFIX, ReplanAction, ReplanOp, ReplanOutput};
use nemesis_board::{Actor, CommentType, IssueStatus, NewComment, NewIssue};

// ---------- 夹具 ----------

fn replan_deps(name: &str) -> (BoardReviewDeps, std::path::PathBuf) {
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;

    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-replan-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let store =
        Arc::new(nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"));
    store.ensure_default_channels().unwrap();
    // 离线 Cluster（内存态，无 socket；不 seed worker 节点——本文件全部
    // 断言不依赖派发成功，confirm_plan 的派发失败面被其内部 catch）。
    let cluster = Arc::new(Cluster::new(ClusterConfig {
        node_id: "node-a".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    }));
    cluster.set_rpc_client(Arc::new(nemesis_cluster::rpc::client::RpcClient::new()));
    let deps = BoardReviewDeps {
        store,
        workspace,
        home: dir.clone(),
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: crate::board_review::SelfcheckRegistry::new(),
        #[cfg(feature = "security")]
        security_slot: None,
    };
    (deps, dir)
}

fn mk_issue(
    store: &nemesis_board::BoardStore,
    title: &str,
    parent: Option<i64>,
    creator: &Actor,
) -> nemesis_board::Issue {
    store
        .create_issue(NewIssue {
            title: title.to_string(),
            description: "重规划测试夹具".to_string(),
            priority: 2,
            creator: creator.clone(),
            parent_issue_id: parent,
            ..Default::default()
        })
        .unwrap()
}

fn write_board_config(dir: &std::path::Path, board: serde_json::Value) {
    std::fs::write(
        dir.join("config.json"),
        serde_json::json!({ "board": board }).to_string(),
    )
    .unwrap();
}

fn action(op: ReplanOp, number: &str) -> ReplanAction {
    ReplanAction {
        op: Some(op),
        number: number.to_string(),
        evidence: "NB-x 失败评论 fail_class: llm_timeout".to_string(),
        ..Default::default()
    }
}

/// 直接落 system 评论（绕开 add_comment 的 author 语义检查——测试素材）。
fn sys_comment(store: &nemesis_board::BoardStore, issue_id: i64, content: &str) {
    store
        .add_comment(NewComment {
            issue_id,
            author: Actor::system("board"),
            content: content.to_string(),
            parent_id: None,
            ctype: CommentType::System,
        })
        .unwrap();
}

// ---------- 策略闸（evaluate_policy_gates，纯函数）----------

#[test]
fn policy_gates_gate_a_rejects_add_over_budget() {
    let (deps, _dir) = replan_deps("gate-a");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let child = mk_issue(&deps.store, "子单", Some(root.id), &creator);
    let children = vec![child];
    let out = ReplanOutput {
        actions: vec![action(ReplanOp::Add, ""), action(ReplanOp::Add, "")],
        ..Default::default()
    };
    let cfg = nemesis_config::BoardReplanConfig {
        max_new_subissues: 1,
        allow_cancel_active: true,
        ..Default::default()
    };
    let v = evaluate_policy_gates(&root, &children, &out, &cfg);
    assert_eq!(v.len(), 1, "只应报闸 A：{v:?}");
    assert!(v[0].contains("闸 A"), "{}", v[0]);
    assert!(v[0].contains("2 条超过上限 1"), "{}", v[0]);
}

#[test]
fn policy_gates_gate_b_requires_allow_cancel_active() {
    let (deps, _dir) = replan_deps("gate-b");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let child = mk_issue(&deps.store, "在途子单", Some(root.id), &creator);
    let children = vec![child];
    let out = ReplanOutput {
        actions: vec![action(ReplanOp::Cancel, &children[0].number)],
        ..Default::default()
    };
    let cfg_off = nemesis_config::BoardReplanConfig::default();
    let v = evaluate_policy_gates(&root, &children, &out, &cfg_off);
    assert_eq!(v.len(), 1, "allow_cancel_active=false 应报闸 B：{v:?}");
    assert!(v[0].contains("闸 B"), "{}", v[0]);
    assert!(v[0].contains("allow_cancel_active=true"), "{}", v[0]);

    let cfg_on = nemesis_config::BoardReplanConfig {
        allow_cancel_active: true,
        ..Default::default()
    };
    assert!(
        evaluate_policy_gates(&root, &children, &out, &cfg_on).is_empty(),
        "显式放行后同计划应全过"
    );
}

#[test]
fn policy_gates_gate_c_rejects_out_of_tree_numbers() {
    let (deps, _dir) = replan_deps("gate-c");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let child = mk_issue(&deps.store, "子单", Some(root.id), &creator);
    let children = vec![child.clone()];
    // 他树编号（合法存在于看板但不是本树直接子单）+ 不存在编号。
    let foreign = mk_issue(&deps.store, "他树子单", None, &creator);
    let out = ReplanOutput {
        actions: vec![
            action(ReplanOp::Cancel, &foreign.number),
            action(ReplanOp::Modify, "NB-999"),
        ],
        ..Default::default()
    };
    let cfg = nemesis_config::BoardReplanConfig {
        allow_cancel_active: true,
        ..Default::default()
    };
    let v = evaluate_policy_gates(&root, &children, &out, &cfg);
    assert_eq!(v.len(), 2, "两个越界编号都应报闸 C：{v:?}");
    assert!(v.iter().all(|x| x.contains("闸 C")), "{v:?}");
    assert!(v.iter().all(|x| x.contains(&root.number)), "{v:?}");
}

#[test]
fn policy_gates_terminal_cancel_is_exempt_and_clean_plan_passes() {
    let (deps, _dir) = replan_deps("gate-clean");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let done_child = mk_issue(&deps.store, "已完成子单", Some(root.id), &creator);
    deps.store
        .transition_issue(done_child.id, IssueStatus::Done, &Actor::system("board"))
        .unwrap();
    let children = vec![deps.store.get_issue(done_child.id).unwrap()];
    // 取消已终态子单 + allow_cancel_active=false：不算违反（执行时按
    // no-op 跳过）；混一个界内 modify 全过。
    let out = ReplanOutput {
        actions: vec![
            action(ReplanOp::Cancel, &children[0].number),
            ReplanAction {
                op: Some(ReplanOp::Modify),
                number: children[0].number.clone(),
                title: "新标题".to_string(),
                evidence: "e".to_string(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let cfg = nemesis_config::BoardReplanConfig::default();
    assert!(
        evaluate_policy_gates(&root, &children, &out, &cfg).is_empty(),
        "终态取消豁免 + 界内修改应全过"
    );
}

// ---------- 建议在声状态（proposal_state，去重闸 + 版本号）----------

#[test]
fn proposal_state_tracks_dedup_and_version() {
    let (deps, _dir) = replan_deps("proposal-state");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);

    let s = proposal_state(&deps.store, root.id).unwrap();
    assert!(!s.pending, "无评论 = 不在声");
    assert_eq!(s.version, 1, "首版 = 1");

    sys_comment(
        &deps.store,
        root.id,
        &format!("{PROPOSAL_COMMENT_PREFIX}计划 v1（t）"),
    );
    let s = proposal_state(&deps.store, root.id).unwrap();
    assert!(s.pending, "建议比人工评论新 = 在声待表态");
    assert_eq!(s.version, 2, "既有建议 1 条 → 下一版 = 2");

    deps.store
        .add_comment(NewComment {
            issue_id: root.id,
            author: Actor::admin("user-1"),
            content: "同意取消 NB-3".to_string(),
            parent_id: None,
            ctype: CommentType::Comment,
        })
        .unwrap();
    let s = proposal_state(&deps.store, root.id).unwrap();
    assert!(!s.pending, "人工评论在后 = 接管，去重闸放行");
    assert_eq!(s.version, 2, "人工表态不推版本");

    sys_comment(
        &deps.store,
        root.id,
        &format!("{BLOCKED_COMMENT_PREFIX}计划 v2 被拒"),
    );
    let s = proposal_state(&deps.store, root.id).unwrap();
    assert!(s.pending, "闸拒绝评论也是「重规划在声」，等人工接管");
    assert_eq!(s.version, 2, "拒绝评论不计入建议版本");
}

// ---------- 起草闸跳过臂（draft_replan_proposal，Ok(false) 路径）----------

#[tokio::test]
async fn draft_skips_when_estop_engaged() {
    let (deps, _dir) = replan_deps("draft-estop");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    deps.estop.trigger();
    let r = draft_replan_proposal(&deps, root.id, "unknown_block_timeout")
        .await
        .expect("estop 跳过不是错误");
    assert!(!r, "estop 生效中应跳过起草");
}

#[tokio::test]
async fn draft_skips_when_replan_disabled() {
    let (deps, dir) = replan_deps("draft-disabled");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    write_board_config(&dir, serde_json::json!({ "replan": { "enabled": false } }));
    let r = draft_replan_proposal(&deps, root.id, "sweep_freeze")
        .await
        .expect("开关关是跳过不是错误");
    assert!(!r, "board.replan.enabled=false 应跳过起草");
    assert!(
        deps.store.list_comments(root.id).unwrap().is_empty(),
        "静默关 = 不落任何评论"
    );
}

#[tokio::test]
async fn draft_skips_when_config_unreadable() {
    let (deps, dir) = replan_deps("draft-config-broken");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    // home 指向不存在路径 → load_board_flags fail-closed = 关。
    let deps = BoardReviewDeps {
        home: dir.join("no-such-home"),
        ..deps
    };
    let r = draft_replan_proposal(&deps, root.id, "parent_review_fail")
        .await
        .expect("配置读取失败是跳过不是错误");
    assert!(!r, "fail-closed 读取失败应跳过起草");
}

#[tokio::test]
async fn draft_skips_when_moderator_missing() {
    let (deps, dir) = replan_deps("draft-no-moderator");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    write_board_config(
        &dir,
        serde_json::json!({ "autonomy": "autonomous", "replan": { "enabled": true } }),
    );
    let r = draft_replan_proposal(&deps, root.id, "capability_fail")
        .await
        .expect("评审引擎未就绪是跳过不是错误");
    assert!(!r, "moderator 槽空应诚实跳过（含 autonomous 模式）");
}

#[test]
fn spawn_skips_without_runtime() {
    // 无活动 tokio runtime 的同步直调（sweep_freeze_park 先例）诚实跳过，
    // 不 spawn 不 panic。
    let (deps, dir) = replan_deps("spawn-no-runtime");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    spawn_replan_draft(deps.clone(), root.id, "budget_breach");
    std::fs::remove_dir_all(&dir).ok();
}

// ---------- autonomous 执行链（autonomous_execute，LLM 无关直驱）----------

#[test]
fn execute_blocked_writes_audit_comment_and_notification() {
    let (deps, _dir) = replan_deps("exec-blocked");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let child = mk_issue(&deps.store, "在途子单", Some(root.id), &creator);
    let children = vec![child.clone()];
    let out = ReplanOutput {
        actions: vec![action(ReplanOp::Cancel, &child.number)],
        ..Default::default()
    };
    let cfg = nemesis_config::BoardReplanConfig::default(); // allow_cancel_active=false
    autonomous_execute(&deps, &root, &children, &out, 1, "capability_fail", &cfg)
        .expect("闸拒绝是诚实闭环不是错误");

    // 子单不动（宁可停不可闯）。
    assert_eq!(
        deps.store.get_issue(child.id).unwrap().status,
        IssueStatus::Backlog,
        "闸拒绝后子单状态不得变化"
    );
    // 🛑 审计评论落根单。
    let comments = deps.store.list_comments(root.id).unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.content.starts_with(BLOCKED_COMMENT_PREFIX)),
        "应有策略闸拒绝审计评论"
    );
    // 无差异评论（未执行不留执行差异）。
    assert!(
        !comments
            .iter()
            .any(|c| c.content.contains("重规划执行差异")),
        "未执行不得落执行差异评论"
    );
    // 站内通知（创建者收件）。
    let notes = deps
        .store
        .list_notifications("admin", Some("user-1"), false, 50)
        .unwrap();
    assert!(
        notes.iter().any(
            |n| n.kind == nemesis_board::notification_kind::REPLAN_BLOCKED
                && n.issue_id == Some(root.id)
        ),
        "创建者应收到 replan_blocked 通知：{notes:?}"
    );
}

#[test]
fn execute_cancels_modifies_adds_and_writes_diff() {
    let (deps, _dir) = replan_deps("exec-happy");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let child_a = mk_issue(&deps.store, "被替代子单", Some(root.id), &creator);
    let child_b = mk_issue(&deps.store, "待改子单", Some(root.id), &creator);
    let child_done = mk_issue(&deps.store, "已完成子单", Some(root.id), &creator);
    deps.store
        .transition_issue(child_done.id, IssueStatus::Done, &Actor::system("board"))
        .unwrap();
    let children = vec![
        child_a.clone(),
        child_b.clone(),
        deps.store.get_issue(child_done.id).unwrap(),
    ];
    let out = ReplanOutput {
        analysis: "取消替代/修标题/补缺口".to_string(),
        actions: vec![
            action(ReplanOp::Cancel, &child_a.number),
            action(ReplanOp::Cancel, &child_done.number), // 终态 → no-op 跳过
            ReplanAction {
                op: Some(ReplanOp::Modify),
                number: child_b.number.clone(),
                title: "改后的标题".to_string(),
                evidence: "e".to_string(),
                ..Default::default()
            },
            ReplanAction {
                op: Some(ReplanOp::Add),
                title: "补救子单".to_string(),
                acceptance_criteria: "- [DONE] 补上".to_string(),
                evidence: "unknown: 依赖版本未定".to_string(),
                ..Default::default()
            },
        ],
    };
    let cfg = nemesis_config::BoardReplanConfig {
        allow_cancel_active: true,
        ..Default::default()
    };
    autonomous_execute(&deps, &root, &children, &out, 2, "fail_class_freeze", &cfg)
        .expect("执行链应闭环");

    // cancel：非终态取消落 Cancelled；终态取消按 no-op 跳过保 Done。
    assert_eq!(
        deps.store.get_issue(child_a.id).unwrap().status,
        IssueStatus::Cancelled,
        "非终态子单应被取消"
    );
    assert_eq!(
        deps.store.get_issue(child_done.id).unwrap().status,
        IssueStatus::Done,
        "终态子单不得被重复取消"
    );
    // 取消证据评论落子单。
    assert!(
        deps.store
            .list_comments(child_a.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("重规划取消")),
        "被取消子单应留取消证据评论"
    );
    // modify。
    assert_eq!(
        deps.store.get_issue(child_b.id).unwrap().title,
        "改后的标题",
        "modify 应落库"
    );
    // add：confirm_plan 单一入口，新子单挂根单。
    let kids = deps.store.list_children(root.id).unwrap();
    assert_eq!(
        kids.len(),
        4,
        "根单下应有 3 旧 + 1 新子单（新增经 confirm_plan 落库）：{:?}",
        kids.iter().map(|k| &k.title).collect::<Vec<_>>()
    );
    assert!(
        kids.iter().any(|k| k.title == "补救子单"),
        "新增子单应带建议标题"
    );
    // 差异评论逐段诚实记账。
    let diff = deps
        .store
        .list_comments(root.id)
        .unwrap()
        .into_iter()
        .find(|c| c.content.contains("重规划执行差异"))
        .expect("应落执行差异评论");
    assert!(
        diff.content
            .contains(&format!("- 取消：{}", child_a.number))
    );
    assert!(
        diff.content
            .contains(&format!("- 修改：{}", child_b.number))
    );
    assert!(diff.content.contains("- 新增子单：1 条"));
    assert!(
        diff.content
            .contains(&format!("{}（已终态", child_done.number)),
        "终态取消应记入跳过段：{}",
        diff.content
    );
}

// ---------- 触发点④ 候选扫描（sweep_unknown_block_candidates）----------

/// 在指定子单上落一条未裁决 unknown 批评论（system + `unknown: ` 行）。
fn plant_unknown(store: &nemesis_board::BoardStore, issue_id: i64, item: &str) -> i64 {
    sys_comment(store, issue_id, &format!("unknown: {item}\n"));
    store
        .list_comments(issue_id)
        .unwrap()
        .into_iter()
        .find(|c| c.content.trim_start().starts_with("unknown:"))
        .unwrap()
        .id
}

#[test]
fn sweep_candidates_respect_timeout_estop_and_state() {
    let (deps, dir) = replan_deps("sweep-matrix");
    let creator = Actor::admin("user-1");
    let db = dir.join("board.db");

    // aged：全终态子树 + 未裁决 unknown（回龄 7200s）→ 候选。
    let aged_root = mk_issue(&deps.store, "aged", None, &creator);
    let aged_child = mk_issue(&deps.store, "aged 子", Some(aged_root.id), &creator);
    deps.store
        .transition_issue(aged_child.id, IssueStatus::Done, &Actor::system("board"))
        .unwrap();
    let unknown_id = plant_unknown(&deps.store, aged_child.id, "依赖服务版本未定");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE comment SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![now - 7200, unknown_id],
        )
        .unwrap();
    }

    // fresh：同形态但 unknown 是刚落的（age≈0 < timeout）→ 不候选。
    let fresh_root = mk_issue(&deps.store, "fresh", None, &creator);
    let fresh_child = mk_issue(&deps.store, "fresh 子", Some(fresh_root.id), &creator);
    deps.store
        .transition_issue(
            fresh_child.id,
            IssueStatus::Cancelled,
            &Actor::system("board"),
        )
        .unwrap();
    plant_unknown(&deps.store, fresh_child.id, "刚发现的 unknown");

    // inflight：子单非终态（有事件链兜底）→ 不候选。
    let inflight_root = mk_issue(&deps.store, "inflight", None, &creator);
    mk_issue(&deps.store, "在途子", Some(inflight_root.id), &creator);

    // nounknown：全终态但无 unknown 批 → 不候选。
    let clean_root = mk_issue(&deps.store, "clean", None, &creator);
    let clean_child = mk_issue(&deps.store, "clean 子", Some(clean_root.id), &creator);
    deps.store
        .transition_issue(clean_child.id, IssueStatus::Done, &Actor::system("board"))
        .unwrap();

    let candidates = sweep_unknown_block_candidates(&deps, 3600);
    assert_eq!(
        candidates,
        vec![aged_root.id],
        "只有 aged 形态应入选：{candidates:?}"
    );

    // timeout=0 恒空；estop 生效恒空。
    assert!(sweep_unknown_block_candidates(&deps, 0).is_empty(), "0=关");
    deps.estop.trigger();
    assert!(
        sweep_unknown_block_candidates(&deps, 3600).is_empty(),
        "estop 生效中不扫描"
    );
}

// ---------- 证据材料组装（build_replan_user_prompt）----------

#[test]
fn prompt_carries_evidence_materials() {
    let (deps, _dir) = replan_deps("prompt");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根任务", None, &creator);
    deps.store
        .update_issue(
            root.id,
            &nemesis_board::IssuePatch {
                acceptance_criteria: Some("[TOUCH] src/a.rs\n[CHECK] re: 交付验收".to_string()),
                ..Default::default()
            },
            &Actor::admin("user-1"),
        )
        .unwrap();
    let child_a = mk_issue(&deps.store, "失败子单", Some(root.id), &creator);
    let child_b = mk_issue(&deps.store, "冲突子单", Some(root.id), &creator);
    for cid in [child_a.id, child_b.id] {
        deps.store
            .update_issue(
                cid,
                &nemesis_board::IssuePatch {
                    acceptance_criteria: Some("[TOUCH] src/a.rs".to_string()),
                    ..Default::default()
                },
                &Actor::admin("user-1"),
            )
            .unwrap();
    }
    sys_comment(
        &deps.store,
        child_a.id,
        "⛔ 任务失败：超时\nfail_class: llm_timeout",
    );
    sys_comment(
        &deps.store,
        child_a.id,
        "评审结论：FAIL\n## 差距\n- 缺少错误分支处理",
    );
    // 从 store 重取（mk_issue 返回的是 update 前的旧结构体，ac 需要新值）。
    let children = vec![
        deps.store.get_issue(child_a.id).unwrap(),
        deps.store.get_issue(child_b.id).unwrap(),
    ];

    let p = build_replan_user_prompt(
        &deps.store.get_issue(root.id).unwrap(),
        &children,
        &deps.store,
        "sweep_freeze（触发单 NB-2）",
    );
    assert!(p.contains("触发源：sweep_freeze（触发单 NB-2）"), "{p}");
    assert!(p.contains("## 根任务背景"));
    assert!(p.contains("## 子单现状"));
    assert!(
        p.contains("最近失败分类：llm_timeout"),
        "fail_class 行应进材料"
    );
    assert!(p.contains("缺少错误分支处理"), "差距段应取「## 差距」尾段");
    assert!(p.contains("[TOUCH] 声明：src/a.rs"), "[TOUCH] 声明应进材料");
    assert!(
        p.contains("[TOUCH] 资源冲突") && p.contains("src/a.rs："),
        "跨单重复声明应进冲突段：{p}"
    );
}

// ---------- 空 / 无改动执行面 ----------

#[test]
fn execute_empty_plan_writes_honest_noop_diff() {
    let (deps, _dir) = replan_deps("exec-empty");
    let creator = Actor::admin("user-1");
    let root = mk_issue(&deps.store, "根单", None, &creator);
    let child = mk_issue(&deps.store, "子单", Some(root.id), &creator);
    let children = vec![child.clone()];
    let out = ReplanOutput::default(); // 无建议动作
    let cfg = nemesis_config::BoardReplanConfig::default();
    autonomous_execute(&deps, &root, &children, &out, 3, "parent_review_fail", &cfg)
        .expect("空计划执行应闭环");
    let diff = deps
        .store
        .list_comments(root.id)
        .unwrap()
        .into_iter()
        .find(|c| c.content.contains("重规划执行差异"))
        .expect("空计划也应落差异评论（诚实记账）");
    assert!(diff.content.contains("无实际改动"), "{}", diff.content);
    assert_eq!(
        deps.store.get_issue(child.id).unwrap().status,
        IssueStatus::Backlog,
        "空计划不得改动子单"
    );
}
