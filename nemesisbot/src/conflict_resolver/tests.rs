//! P5/F5 冲突硬解执行体单测：产出契约校验矩阵（范围钉死/二进制择边/
//! 锁文件理由/覆盖完备/围栏容错）+ prompt 静态断言 + 接触时刻表常量。

use super::*;

pub(crate) fn conflict_text(path: &str) -> ConflictFile {
    ConflictFile {
        path: path.into(),
        binary: false,
        ours: Some(b"ours line\n".to_vec()),
        theirs: Some(b"theirs line\n".to_vec()),
        ancestor: Some(b"base line\n".to_vec()),
    }
}

fn conflict_binary(path: &str) -> ConflictFile {
    ConflictFile {
        path: path.into(),
        binary: true,
        ours: Some(vec![0x01, 0x00, 0x02]),
        theirs: Some(vec![0x03, 0x00, 0x04]),
        ancestor: Some(vec![0x00, 0x00]),
    }
}

#[test]
fn parse_ok_full_coverage() {
    let conflicts = [conflict_text("src/a.rs"), conflict_text("src/b.rs")];
    let raw = r#"{"resolutions": [
        {"path": "src/a.rs", "action": "merge", "content": "merged line\n", "reason": "两侧都保留"},
        {"path": "src/b.rs", "action": "theirs", "reason": "对方改动更完整"}
    ]}"#;
    let out = parse_resolutions(raw, &conflicts).expect("合法产出必须通过");
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].action, ResolutionAction::Merge);
    assert_eq!(out[0].content.as_deref(), Some(b"merged line\n".as_slice()));
    assert_eq!(out[1].action, ResolutionAction::Theirs);
    assert!(
        out[1].content.is_none(),
        "择边不带 content（落盘时从冲突三阶段取）"
    );
}

#[test]
fn parse_ok_strips_code_fence_and_prose() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = "好的，以下是解决方案：\n```json\n{\"resolutions\": [{\"path\": \"src/a.rs\", \"action\": \"ours\", \"reason\": \"我方为准\"}]}\n```\n请查收。";
    let out = parse_resolutions(raw, &conflicts).expect("围栏+前后杂文必须容错");
    assert_eq!(out.len(), 1);
}

#[test]
fn parse_reject_out_of_scope_path() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = r#"{"resolutions": [{"path": "src/evil.rs", "action": "merge", "content": "x", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("越界"), "必须拒绝范围外文件：{err}");
}

#[test]
fn parse_reject_binary_merge() {
    let conflicts = [conflict_binary("assets/logo.bin")];
    let raw = r#"{"resolutions": [{"path": "assets/logo.bin", "action": "merge", "content": "abc", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("二进制"), "二进制必须择边：{err}");
}

#[test]
fn parse_reject_lockfile_reason_without_regen() {
    let conflicts = [conflict_text("Cargo.lock")];
    let raw =
        r#"{"resolutions": [{"path": "Cargo.lock", "action": "ours", "reason": "保留我方版本"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("建议重新生成"), "锁文件理由闸：{err}");
}

#[test]
fn parse_ok_lockfile_reason_with_regen() {
    let conflicts = [conflict_text("package-lock.json")];
    let raw = r#"{"resolutions": [{"path": "package-lock.json", "action": "theirs", "reason": "采信对方，建议重新生成以保证一致性"}]}"#;
    parse_resolutions(raw, &conflicts).expect("锁文件理由含建议重新生成必须通过");
}

#[test]
fn parse_reject_incomplete_coverage() {
    let conflicts = [conflict_text("src/a.rs"), conflict_text("src/b.rs")];
    let raw = r#"{"resolutions": [{"path": "src/a.rs", "action": "ours", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(
        err.contains("缺少冲突文件处置"),
        "漏一个冲突文件就是假绿：{err}"
    );
}

#[test]
fn parse_reject_duplicate_path() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = r#"{"resolutions": [
        {"path": "src/a.rs", "action": "ours", "reason": "r"},
        {"path": "src/a.rs", "action": "theirs", "reason": "r"}
    ]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("重复"), "重复处置必须拒绝：{err}");
}

#[test]
fn parse_reject_side_absent_choice() {
    // 我方侧无文件（对方新增、我方删除型冲突）：择边 ours 不可表达（删除）。
    let conflicts = [ConflictFile {
        path: "src/new.rs".into(),
        binary: false,
        ours: None,
        theirs: Some(b"new\n".to_vec()),
        ancestor: None,
    }];
    let raw = r#"{"resolutions": [{"path": "src/new.rs", "action": "ours", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("不能择边 ours"), "缺席侧择边必须拒绝：{err}");
}

#[test]
fn parse_reject_merge_without_content() {
    let conflicts = [conflict_text("src/a.rs")];
    let raw = r#"{"resolutions": [{"path": "src/a.rs", "action": "merge", "reason": "r"}]}"#;
    let err = parse_resolutions(raw, &conflicts).unwrap_err();
    assert!(err.contains("content"), "merge 缺内容必须拒绝：{err}");
}

#[test]
fn parse_reject_garbage() {
    let conflicts = [conflict_text("src/a.rs")];
    assert!(parse_resolutions("完全不是 JSON", &conflicts).is_err());
    assert!(parse_resolutions("{\"resolutions\": \"not-array\"}", &conflicts).is_err());
}

#[test]
fn system_prompt_pins_scope_and_lockfile_rule() {
    // 静态断言：范围钉死 + 锁文件规则 + 二进制择边必须写在 prompt 里
    //（prompt 是行为契约的一部分，改丢规则在这里红）。
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("只允许处置列出的冲突文件"));
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("建议重新生成"));
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("择边"));
    assert!(CONFLICT_RESOLVER_SYSTEM_PROMPT.contains("resolutions"));
}

#[test]
fn probe_schedule_is_t0_60_120() {
    // 三轮接触时刻表钉死（goal 钦定 t0/+60s/+120s；具名常量防手滑改掉）。
    assert_eq!(PROBE_SCHEDULE_SECS, &[0, 60, 120]);
}

#[test]
fn user_prompt_carries_intent_and_three_stages() {
    // 本地构造 Issue（只需 title/description/project_id 参与 prompt 组装）。
    let issue = nemesis_board::Issue {
        id: 9,
        number: "NB-9".into(),
        title: "修登录 bug".into(),
        description: "把登录超时改成 30s".into(),
        status: nemesis_board::IssueStatus::InProgress,
        priority: 2,
        assignee: None,
        assignee_id: None,
        creator: nemesis_board::Actor::system("test"),
        parent_issue_id: None,
        project_id: Some(1),
        due_date: None,
        position: 0,
        acceptance_criteria: None,
        origin: None,
        required_role: None,
        required_tags: vec![],
        hidden: false,
        created_at: 0,
        updated_at: 0,
    };
    let conflicts = [
        conflict_text("src/login.rs"),
        conflict_binary("assets/logo.bin"),
    ];
    let p = build_user_prompt(&issue, "task-1", &conflicts);
    assert!(p.contains("修登录 bug"), "任务意图（标题）必须在场");
    assert!(p.contains("把登录超时改成 30s"), "任务说明必须在场");
    assert!(p.contains("src/login.rs"), "冲突路径必须在场");
    assert!(p.contains("ours line"), "我方内容必须在场");
    assert!(p.contains("theirs line"), "对方内容必须在场");
    assert!(p.contains("base line"), "基线内容必须在场");
    assert!(p.contains("assets/logo.bin"), "二进制冲突路径必须在场");
    assert!(
        p.contains("二进制文件"),
        "二进制择边提示语义在场（二进制条目渲染）"
    );
}

// ===========================================================================
// Coverage 追加（2026-09-24）：parse_resolutions 校验臂补全 / override_content
// 择边装配矩阵 / build_user_prompt 截断与缺侧渲染 / run_resolver 三道前置
// 闸（estop / 主 agent 未就绪 / mini 档）→ fallback_freeze / 重派链三道
// 发车前闸（estop / 旗标读取失败 / 预算超限）。
// 豁免（见交付报告）：硬解 LLM 循环与重派接触循环——run_detached 需真
// AgentLoop 全链，probe 循环按 t0/+60s/+120s 真睡眠 180s + 帧级真探，
// 均非进程内单测形态。
// ===========================================================================

use std::sync::Arc;

use nemesis_board::models::NewIssue;

/// 最小评审依赖（同 board_review::tests::review_deps 形态；不装 rpc client
/// ——probe_peer 无客户端即秒 False，测试不碰网络）。
pub(crate) fn resolver_deps(name: &str) -> crate::board_review::BoardReviewDeps {
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;

    let dir = std::env::temp_dir().join(format!(
        "nb-conflict-resolver-{}-{name}",
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
    crate::board_review::BoardReviewDeps {
        store,
        workspace,
        home: dir,
        moderator_loop: Arc::new(std::sync::OnceLock::new()),
        cluster,
        estop: Arc::new(nemesis_agent::estop::EstopState::new()),
        estop_parked: Arc::new(std::sync::Mutex::new(Vec::new())),
        selfcheck: crate::board_review::SelfcheckRegistry::new(),
    }
}

/// 建一个绑定项目的 issue（fallback_freeze 冻结要有 project_id 才有落点）。
fn issue_with_project(store: &nemesis_board::BoardStore, title: &str) -> nemesis_board::Issue {
    let project = store.create_project("p", "d", None, "", "", None).unwrap();
    store
        .create_issue(NewIssue {
            title: title.to_string(),
            creator: Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap()
}

fn parked_comment(store: &nemesis_board::BoardStore, issue_id: i64) -> Option<String> {
    store
        .list_comments(issue_id)
        .unwrap()
        .into_iter()
        .map(|c| c.content)
        .find(|c| c.contains("AI 硬解未果，项目已冻结待人工解"))
}

fn fallback_audit_exists(store: &nemesis_board::BoardStore, issue_id: i64) -> bool {
    store
        .list_activity(issue_id)
        .unwrap()
        .iter()
        .any(|a| a.details.as_deref().unwrap_or("").contains("auto_fallback"))
}

/// run_resolver 第一闸：estop 挂起 → 不碰 LLM 直接回落 human 档。
#[tokio::test]
async fn run_resolver_estop_falls_back_to_freeze() {
    let deps = resolver_deps("rr-estop");
    let issue = issue_with_project(deps.store.as_ref(), "estop 硬解闸");
    deps.estop.trigger();

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-re".into(),
        vec![],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(
        deps.store.get_project(pid).unwrap().conflict_frozen,
        "必须置位项目冻结"
    );
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("急停中，冲突硬解挂起转人工"), "{note}");
    assert!(fallback_audit_exists(deps.store.as_ref(), issue.id));
}

/// run_resolver 第二闸：主 agent 未装配 → 诚实转人工（不伪造硬解）。
#[tokio::test]
async fn run_resolver_without_moderator_loop_falls_back() {
    let deps = resolver_deps("rr-noagent");
    let issue = issue_with_project(deps.store.as_ref(), "无 agent 硬解闸");

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rn".into(),
        vec![],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("主 agent 未就绪"), "{note}");
}

/// run_resolver 第三闸：mini 档模型拒绝硬解（F5 造假重灾区防线）。
#[tokio::test]
async fn run_resolver_rejects_mini_tier() {
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

    let mut deps = resolver_deps("rr-mini");
    let issue = issue_with_project(deps.store.as_ref(), "mini 档硬解闸");
    let agent_loop = AgentLoop::new(Box::new(NullProvider), AgentConfig::default());
    agent_loop.set_tier(nemesis_types::capability::ModelTier::Mini);
    deps.moderator_loop = Arc::new(std::sync::OnceLock::new());
    let _ = deps.moderator_loop.set(Arc::new(agent_loop));

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rm".into(),
        vec![],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("mini"), "{note}");
}

/// spawn_conflict_resolver 包装（fire-and-forget）：异步接管后同样走闸。
#[tokio::test]
async fn spawn_conflict_resolver_runs_gate_asynchronously() {
    let deps = resolver_deps("spawn-gate");
    let issue = issue_with_project(deps.store.as_ref(), "spawn 包装闸");
    deps.estop.trigger();
    let pid = issue.project_id.unwrap();

    super::spawn_conflict_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-sp".into(),
        vec![],
    );

    // 异步任务落 freeze 后置位（轮询上限 5s，防悬挂）。
    let mut frozen = false;
    for _ in 0..50 {
        if deps.store.get_project(pid).unwrap().conflict_frozen {
            frozen = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(frozen, "spawn 包装必须在超时内完成回落冻结");
    assert!(parked_comment(deps.store.as_ref(), issue.id).is_some());
}

/// redispatch 链第一闸：estop 挂起 → 不发车不接触，直接转人工。
#[tokio::test]
async fn redispatch_estop_freezes_before_probe_loop() {
    let deps = resolver_deps("rd-estop");
    let issue = issue_with_project(deps.store.as_ref(), "重派 estop 闸");
    deps.estop.trigger();

    super::redispatch_after_failed_resolve(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rde".into(),
        "硬解失败",
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("急停中，冲突重派挂起转人工"), "{note}");
}

/// redispatch 链第二闸：board 旗标读取失败 → fail-closed 转人工（不拿
/// 默认值顶替用户配置）。
#[tokio::test]
async fn redispatch_flag_read_failure_freezes() {
    let deps = resolver_deps("rd-flagerr");
    let issue = issue_with_project(deps.store.as_ref(), "旗标读取失败闸");
    // config.json 是目录 → 读取必败（fail-closed 形态）。
    std::fs::create_dir_all(deps.home.join("config.json")).unwrap();

    super::redispatch_after_failed_resolve(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rdf".into(),
        "硬解失败",
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("board 旗标读取失败"), "{note}");
}

/// redispatch 链第三闸：预算保险丝打满 → 转人工（wall_clock 维度；
/// created_at=0 构造超期存活）。
#[tokio::test]
async fn redispatch_budget_breach_freezes() {
    let deps = resolver_deps("rd-budget");
    let issue = issue_with_project(deps.store.as_ref(), "预算超限闸");
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board": {"budget": {"wall_clock_budget_secs": 1}}}"#,
    )
    .unwrap();
    let mut issue = issue;
    issue.created_at = 0; // 存活 ≈ 现在-纪元 ≫ 1s

    super::redispatch_after_failed_resolve(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rdb".into(),
        "硬解失败",
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(
        note.contains("重派预算超限") && note.contains("wall_clock_budget_secs"),
        "{note}"
    );
}

/// fallback_freeze 直调：无项目 issue（project_id None）→ 冻结让位跳过，
/// 审计与停车照落（不炸）。
#[test]
fn fallback_freeze_without_project_still_audits_and_parks() {
    let deps = resolver_deps("ff-noproj");
    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "无项目硬解失败".to_string(),
            creator: Actor::agent("node-a"),
            ..Default::default()
        })
        .unwrap();

    super::fallback_freeze(&deps, &issue, "task-ff", "测试原因");

    assert!(
        fallback_audit_exists(deps.store.as_ref(), issue.id),
        "审计必须照落"
    );
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("停车评论必须照落");
    assert!(note.contains("测试原因"), "{note}");
}

/// override_content 择边装配矩阵：merge 用产出内容 / ours-theirs 取冲突
/// 三阶段对应侧 blob；越界路径、缺 content、缺侧均防御性拒绝。
#[test]
fn override_content_matrix() {
    let conflicts = [
        conflict_text("src/a.rs"),
        ConflictFile {
            path: "gone-ours.rs".into(),
            binary: false,
            ours: None,
            theirs: Some(b"t\n".to_vec()),
            ancestor: None,
        },
        ConflictFile {
            path: "gone-theirs.rs".into(),
            binary: false,
            ours: Some(b"o\n".to_vec()),
            theirs: None,
            ancestor: None,
        },
    ];

    // merge：产出内容原样。
    let r = Resolution {
        path: "src/a.rs".into(),
        action: ResolutionAction::Merge,
        content: Some(b"merged\n".to_vec()),
        reason: "r".into(),
    };
    assert_eq!(
        override_content(&r, &conflicts).unwrap(),
        ("src/a.rs".to_string(), b"merged\n".to_vec())
    );

    // ours / theirs：从冲突三阶段取对应侧。
    let r = Resolution {
        path: "src/a.rs".into(),
        action: ResolutionAction::Ours,
        content: None,
        reason: "r".into(),
    };
    assert_eq!(
        override_content(&r, &conflicts).unwrap().1,
        b"ours line\n".to_vec()
    );
    let r = Resolution {
        path: "src/a.rs".into(),
        action: ResolutionAction::Theirs,
        content: None,
        reason: "r".into(),
    };
    assert_eq!(
        override_content(&r, &conflicts).unwrap().1,
        b"theirs line\n".to_vec()
    );

    // 防御兜底四臂：越界 / 缺 content / 我方缺侧 / 对方缺侧。
    let r = Resolution {
        path: "src/evil.rs".into(),
        action: ResolutionAction::Merge,
        content: Some(b"x".to_vec()),
        reason: "r".into(),
    };
    assert!(
        override_content(&r, &conflicts)
            .unwrap_err()
            .contains("不在冲突集")
    );
    let r = Resolution {
        path: "src/a.rs".into(),
        action: ResolutionAction::Merge,
        content: None,
        reason: "r".into(),
    };
    assert!(
        override_content(&r, &conflicts)
            .unwrap_err()
            .contains("缺 content")
    );
    let r = Resolution {
        path: "gone-ours.rs".into(),
        action: ResolutionAction::Ours,
        content: None,
        reason: "r".into(),
    };
    assert!(
        override_content(&r, &conflicts)
            .unwrap_err()
            .contains("我方侧无文件")
    );
    let r = Resolution {
        path: "gone-theirs.rs".into(),
        action: ResolutionAction::Theirs,
        content: None,
        reason: "r".into(),
    };
    assert!(
        override_content(&r, &conflicts)
            .unwrap_err()
            .contains("对方侧无文件")
    );
}

/// ResolutionAction::parse 三态 + trim + 未知值。
#[test]
fn resolution_action_parse_matrix() {
    assert_eq!(
        ResolutionAction::parse("merge"),
        Some(ResolutionAction::Merge)
    );
    assert_eq!(
        ResolutionAction::parse(" ours "),
        Some(ResolutionAction::Ours)
    );
    assert_eq!(
        ResolutionAction::parse("theirs"),
        Some(ResolutionAction::Theirs)
    );
    assert_eq!(ResolutionAction::parse("both"), None);
    assert_eq!(ResolutionAction::parse(""), None);
}

/// parse_resolutions 校验臂补全：缺 path / 缺 action / 非法 action / 缺
/// reason / 对方缺侧择边 / 无 JSON 对象两态 / JSON 解析失败。
#[test]
fn parse_reject_arm_completion() {
    let conflicts = [conflict_text("src/a.rs")];

    // 缺 path（第 1 条）。
    let err = parse_resolutions(
        r#"{"resolutions": [{"action": "ours", "reason": "r"}]}"#,
        &conflicts,
    )
    .unwrap_err();
    assert!(err.contains("缺 path"), "{err}");

    // path 空白等价缺 path。
    let err = parse_resolutions(
        r#"{"resolutions": [{"path": "  ", "action": "ours", "reason": "r"}]}"#,
        &conflicts,
    )
    .unwrap_err();
    assert!(err.contains("缺 path"), "{err}");

    // 缺 action。
    let err = parse_resolutions(
        r#"{"resolutions": [{"path": "src/a.rs", "reason": "r"}]}"#,
        &conflicts,
    )
    .unwrap_err();
    assert!(err.contains("缺 action"), "{err}");

    // action 非法。
    let err = parse_resolutions(
        r#"{"resolutions": [{"path": "src/a.rs", "action": "both", "reason": "r"}]}"#,
        &conflicts,
    )
    .unwrap_err();
    assert!(err.contains("action 非法"), "{err}");

    // 缺 reason / 空白 reason。
    for raw in [
        r#"{"resolutions": [{"path": "src/a.rs", "action": "ours"}]}"#,
        r#"{"resolutions": [{"path": "src/a.rs", "action": "ours", "reason": "   "}]}"#,
    ] {
        let err = parse_resolutions(raw, &conflicts).unwrap_err();
        assert!(err.contains("缺 reason"), "{err}");
    }

    // 锁文件按路径末段识别（嵌套目录也命中）。
    let err = parse_resolutions(
        r#"{"resolutions": [{"path": "sub/dir/Cargo.lock", "action": "ours", "reason": "r"}]}"#,
        &[conflict_text("sub/dir/Cargo.lock")],
    )
    .unwrap_err();
    assert!(err.contains("建议重新生成"), "{err}");

    // 对方缺侧（我方删除型冲突）：theirs 不可表达。
    let del = [ConflictFile {
        path: "src/del.rs".into(),
        binary: false,
        ours: Some(b"o\n".to_vec()),
        theirs: None,
        ancestor: Some(b"a\n".to_vec()),
    }];
    let err = parse_resolutions(
        r#"{"resolutions": [{"path": "src/del.rs", "action": "theirs", "reason": "r"}]}"#,
        &del,
    )
    .unwrap_err();
    assert!(err.contains("不能择边 theirs"), "{err}");

    // 无 '{' / 无 '}'（防御提取两态）。
    assert!(parse_resolutions("完全没有花括号", &conflicts).is_err());
    assert!(parse_resolutions(r#"{"resolutions": ["#, &conflicts).is_err());

    // 花括号内不是 JSON。
    let err = parse_resolutions("{不是json}", &conflicts).unwrap_err();
    assert!(err.contains("JSON 解析失败"), "{err}");
}

/// build_user_prompt 渲染臂补全：缺侧（新增/删除型）注记 / 超长侧截断 /
/// 验收标准在场 / 空说明不渲染任务说明。
#[test]
fn user_prompt_absent_side_truncation_and_criteria() {
    let issue = nemesis_board::Issue {
        id: 3,
        number: "NB-3".into(),
        title: "缺侧与截断".into(),
        description: "  ".into(),
        status: nemesis_board::IssueStatus::InProgress,
        priority: 2,
        assignee: None,
        assignee_id: None,
        creator: Actor::system("test"),
        parent_issue_id: None,
        project_id: Some(1),
        due_date: None,
        position: 0,
        acceptance_criteria: Some("构建通过".into()),
        origin: None,
        required_role: None,
        required_tags: vec![],
        hidden: false,
        created_at: 0,
        updated_at: 0,
    };
    // 我方缺侧（对方新增型）+ 超长基线（>16KiB 截断）。
    let big = vec![b'x'; 16 * 1024 + 7];
    let conflicts = [
        ConflictFile {
            path: "src/new.rs".into(),
            binary: false,
            ours: None,
            theirs: Some(b"new\n".to_vec()),
            ancestor: None,
        },
        ConflictFile {
            path: "src/big.rs".into(),
            binary: false,
            ours: Some(big.clone()),
            theirs: Some(b"t\n".to_vec()),
            ancestor: Some(big),
        },
    ];
    let p = build_user_prompt(&issue, "task-up", &conflicts);
    assert!(
        p.contains("该侧无此文件（新增/删除型冲突）"),
        "缺侧注记必须在场: {p}"
    );
    assert!(p.contains("已截断"), "超长侧截断注记必须在场");
    assert!(
        p.contains("验收标准") && p.contains("构建通过"),
        "验收标准在场"
    );
    assert!(!p.contains("任务说明"), "空白说明不渲染任务说明段");
}

// ===========================================================================
// 硬解 LLM 主链（2026-09-25 覆盖补齐）：脚本化 provider 装进 moderator_loop
// （同 board_review::tests::attach_loop 形态——run_detached 纯文本单轮在
// 进程内可跑），打通产出→落盘 commit→审计→收口与机械失败→重派闸全链。
// ===========================================================================

use nemesis_board::IssueStatus;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 脚本化 provider（带调用计数 + 副作用钩子）：Err 项 = 调用失败；耗尽后
/// 回落 fallback 文本。
pub(crate) struct ResolverLlm {
    pub(crate) script: std::sync::Mutex<std::collections::VecDeque<Result<String, String>>>,
    pub(crate) fallback: String,
    pub(crate) calls: AtomicUsize,
    pub(crate) on_call: Option<Box<dyn Fn() + Send + Sync>>,
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ResolverLlm {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = &self.on_call {
            hook();
        }
        let next = self
            .script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front();
        let raw = match next {
            Some(Ok(t)) => t,
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

/// 委托壳：AgentLoop::new 收 Box<dyn LlmProvider>，测试侧保留 Arc 句柄读
/// 调用计数（同 board_review::tests::SharedLlm 形态）。
struct SharedResolverLlm(Arc<ResolverLlm>);

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for SharedResolverLlm {
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

/// 把 provider 装进 moderator_loop，返回 Arc 句柄（读调用计数）。
pub(crate) fn attach_resolver_llm(
    deps: &crate::board_review::BoardReviewDeps,
    provider: Arc<ResolverLlm>,
) {
    let _ = deps
        .moderator_loop
        .set(Arc::new(nemesis_agent::r#loop::AgentLoop::new(
            Box::new(SharedResolverLlm(provider)),
            nemesis_agent::types::AgentConfig::default(),
        )));
}

fn resolutions_json(path: &str, content: &str, reason: &str) -> String {
    format!(
        r#"{{"resolutions":[{{"path":"{path}","action":"merge","content":"{content}","reason":"{reason}"}}]}}"#
    )
}

/// 带 git 档案目录的项目 + in_progress 单（finish_merged_review 可合法
/// 转入 in_review）。
pub(crate) fn issue_on_git_project(
    deps: &crate::board_review::BoardReviewDeps,
    name: &str,
) -> (nemesis_board::Issue, std::path::PathBuf) {
    use nemesis_board::IssueStatus;
    let root = std::env::temp_dir().join(format!("nb-conflict-root-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    nemesis_board::archive::ensure_scaffold(&root, 1, "p", "active").unwrap();
    nemesis_board::git_repo::ensure_repo(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"base line\n").unwrap();
    nemesis_board::git_repo::commit_worktree(&root, "baseline")
        .unwrap()
        .expect("baseline commit 必须成功");

    let project = deps
        .store
        .create_project("硬解项目", "d", None, "", "", Some(root.to_str().unwrap()))
        .unwrap();
    let issue = deps
        .store
        .create_issue(NewIssue {
            title: format!("{name}硬解单"),
            creator: Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();
    (issue, root)
}

#[tokio::test]
async fn run_resolver_hard_resolve_success_commits_and_audits() {
    let deps = resolver_deps("rr-success");
    let (issue, root) = issue_on_git_project(&deps, "success");

    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Ok(
                resolutions_json("a.txt", "merged line\\n", "两侧合并保留"),
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rs".into(),
        vec![conflict_text("a.txt")],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(
        !deps.store.get_project(pid).unwrap().conflict_frozen,
        "硬解成功不得冻结项目"
    );
    // 审计：conflict_auto_resolve 单一漏斗。
    let audit = deps
        .store
        .list_activity(issue.id)
        .unwrap()
        .into_iter()
        .find(|a| {
            a.details
                .as_deref()
                .map(|d| d.contains("conflict_auto_resolve"))
                .unwrap_or(false)
        })
        .expect("必须落 conflict_auto_resolve 审计");
    assert!(
        audit.details.as_deref().unwrap_or("").contains("task-rs"),
        "审计必须带 task_id: {:?}",
        audit.details
    );
    // 落盘：HEAD 树里 a.txt = 产出内容（经 export_head_tree 客观核验）。
    let exported = root.join("export-head");
    let (_n, _bytes) = nemesis_board::git_repo::export_head_tree(&root, &exported).unwrap();
    let merged = std::fs::read(exported.join("a.txt")).expect("HEAD 树必须含 a.txt");
    assert_eq!(merged, b"merged line\n".to_vec(), "HEAD 必须是硬解产出内容");
    // 收口：评论 + 转 in_review（评审触发因 MERGE_DEPS 未装配而天然跳过）。
    assert!(
        deps.store
            .list_comments(issue.id)
            .unwrap()
            .iter()
            .any(|c| c.content.contains("合并冲突已由 AI 硬解")
                && c.content.contains("两侧合并保留")),
        "必须落硬解评论（带解题理由）"
    );
    assert_eq!(
        deps.store.get_issue(issue.id).unwrap().status,
        IssueStatus::InReview,
        "硬解收口必须转 in_review"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn run_resolver_llm_call_failure_goes_to_redispatch_gate() {
    let deps = resolver_deps("rr-callfail");
    let (issue, root) = issue_on_git_project(&deps, "callfail");

    // LLM 调用失败（不消耗解析轮）→ 机械失败 → 重派链首闸 estop（on_call
    // 触发急停）→ 冻结兜底（不真发车不真接触）。
    let estop = deps.estop.clone();
    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "上游失联".to_string()
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: Some(Box::new(move || estop.trigger())),
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rcf".into(),
        vec![conflict_text("a.txt")],
    )
    .await;

    assert_eq!(
        deps.store.list_dispatches(issue.id).unwrap().len(),
        0,
        "estop 闸必须拦下重派发车"
    );
    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("急停中，冲突重派挂起转人工"), "{note}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn run_resolver_three_rounds_parse_failure_then_freezes() {
    let deps = resolver_deps("rr-parsefail");
    let (issue, root) = issue_on_git_project(&deps, "parsefail");

    // 恒乱码 → 3 轮解析失败（错误反馈回灌）→ 机械失败 → estop 兜底。
    let estop = deps.estop.clone();
    let provider = Arc::new(ResolverLlm {
        script: std::sync::Mutex::new(std::collections::VecDeque::new()),
        fallback: "全是乱码没有 JSON".to_string(),
        calls: AtomicUsize::new(0),
        on_call: Some(Box::new(move || estop.trigger())),
    });
    attach_resolver_llm(&deps, provider.clone());

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rpf".into(),
        vec![conflict_text("a.txt")],
    )
    .await;

    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        RESOLVE_MAX_ATTEMPTS as usize,
        "解析失败必须回灌重试满 3 轮"
    );
    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    assert!(parked_comment(deps.store.as_ref(), issue.id).is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn run_resolver_project_root_missing_freezes_honestly() {
    let deps = resolver_deps("rr-noroot");
    // 项目无档案目录 → 硬解产出无处落盘 → 诚实冻结（不伪造成功）。
    let project = deps
        .store
        .create_project("无档案项目", "d", None, "", "", None)
        .unwrap();
    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "无档案硬解单".to_string(),
            creator: Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();

    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Ok(
                resolutions_json("a.txt", "x", "理由"),
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rnr".into(),
        vec![conflict_text("a.txt")],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("项目档案目录缺失"), "{note}");
}

#[tokio::test]
async fn run_resolver_commit_failure_redispatch_gate_freezes() {
    let deps = resolver_deps("rr-commitfail");
    // 目录存在但不是 git 仓库 → commit_resolution 失败 → 机械失败重派链
    // → estop 兜底（on_call 触发）。
    let root = std::env::temp_dir().join(format!("nb-conflict-nogit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let project = deps
        .store
        .create_project(
            "无仓库项目",
            "d",
            None,
            "",
            "",
            Some(root.to_str().unwrap()),
        )
        .unwrap();
    let issue = deps
        .store
        .create_issue(NewIssue {
            title: "落盘失败硬解单".to_string(),
            creator: Actor::agent("node-a"),
            project_id: Some(project.id),
            ..Default::default()
        })
        .unwrap();
    deps.store
        .transition_issue(issue.id, IssueStatus::InProgress, &Actor::agent("node-a"))
        .unwrap();

    let estop = deps.estop.clone();
    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Ok(
                resolutions_json("a.txt", "x", "理由"),
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: Some(Box::new(move || estop.trigger())),
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-rcf2".into(),
        vec![conflict_text("a.txt")],
    )
    .await;

    assert!(
        deps.store.list_dispatches(issue.id).unwrap().is_empty(),
        "estop 闸必须拦下重派发车"
    );
    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(note.contains("急停中，冲突重派挂起转人工"), "{note}");
    let _ = std::fs::remove_dir_all(&root);
}

// ===========================================================================
// wave5 round2 batch-3（2026-09-25）：硬解收尾的三条快闸——E1 预算保险丝
// 打满转人工、board 旗标读取失败 fail-closed、ours/theirs 择边审计 arm。
// （estop/无 agent/mini 档三闸既有测试已盖，不重复。）
// ===========================================================================

/// E1 预算保险丝：父单全链累计派发数打满 → 冲突重派前就被拦停转人工。
/// LLM 调用失败（不消耗解析轮）→ 机械失败进重派链 → 旗标 Ok 但预算超限
/// → fallback_freeze（不真接触不真发车——接触时刻表 60s/120s 在测试域
/// 结构性不可等）。
#[tokio::test]
async fn w5_resolver_budget_breach_freezes_before_redispatch() {
    let deps = resolver_deps("rr-budget");
    let (issue, root) = issue_on_git_project(&deps, "budget");
    // board.budget.max_total_redispatch = 1，本单已有 2 条派发记录 → 超限。
    std::fs::write(
        deps.home.join("config.json"),
        r#"{"board":{"budget":{"max_total_redispatch":1}}}"#,
    )
    .unwrap();
    let a = nemesis_board::Actor::agent("node-a");
    deps.store
        .insert_dispatch("task-w5b1", issue.id, "node-b", &a)
        .unwrap();
    deps.store
        .insert_dispatch("task-w5b2", issue.id, "node-c", &a)
        .unwrap();

    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "上游失联".to_string()
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-w5b".into(),
        vec![],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(
        deps.store.get_project(pid).unwrap().conflict_frozen,
        "预算超限必须冻结项目"
    );
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(
        note.contains("重派预算超限"),
        "停车理由必须点名预算超限: {note}"
    );
    assert!(fallback_audit_exists(deps.store.as_ref(), issue.id));
    let _ = std::fs::remove_dir_all(&root);
}

/// board 旗标读取失败（config.json 非法 JSON）→ fail-closed 转人工，
/// 不拿默认值顶替用户配置。
#[tokio::test]
async fn w5_resolver_flags_read_failure_falls_back() {
    let deps = resolver_deps("rr-flagfail");
    let (issue, root) = issue_on_git_project(&deps, "flagfail");
    std::fs::write(deps.home.join("config.json"), "{oops 非法 JSON").unwrap();

    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "上游失联".to_string()
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-w5b".into(),
        vec![],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(deps.store.get_project(pid).unwrap().conflict_frozen);
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(
        note.contains("board 旗标读取失败"),
        "停车理由必须点名旗标读取失败: {note}"
    );
    assert!(fallback_audit_exists(deps.store.as_ref(), issue.id));
    let _ = std::fs::remove_dir_all(&root);
}

/// 硬解成功的审计 json 里 ours/theirs 两个择边 arm（既有 success 测试只走
/// merge arm）：两文件各择一边（同路径多条会被 parse 以「重复处置」拒绝，
/// 故分置 a.txt/b.txt），审计详情原样记档，HEAD 内容各取所择侧。
#[tokio::test]
async fn w5_resolver_ours_and_theirs_actions_audited() {
    let deps = resolver_deps("rr-sides");
    let (issue, root) = issue_on_git_project(&deps, "sides");

    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Ok(
                r#"{"resolutions":[
                    {"path":"a.txt","action":"ours","reason":"我方为准"},
                    {"path":"b.txt","action":"theirs","reason":"对方改动更完整"}
                ]}"#
                .to_string(),
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-w5b".into(),
        vec![conflict_text("a.txt"), conflict_text("b.txt")],
    )
    .await;

    let audit = deps
        .store
        .list_activity(issue.id)
        .unwrap()
        .into_iter()
        .find(|a| {
            a.details
                .as_deref()
                .map(|d| d.contains("conflict_auto_resolve"))
                .unwrap_or(false)
        })
        .expect("必须落 conflict_auto_resolve 审计");
    let details = audit.details.as_deref().unwrap_or("");
    assert!(
        details.contains("\"ours\""),
        "审计必须记 ours 择边: {details}"
    );
    assert!(
        details.contains("\"theirs\""),
        "审计必须记 theirs 择边: {details}"
    );
    let exported = root.join("export-head-w5b");
    let (_n, _bytes) = nemesis_board::git_repo::export_head_tree(&root, &exported).unwrap();
    let a_head = std::fs::read(exported.join("a.txt")).expect("HEAD 树必须含 a.txt");
    assert_eq!(a_head, b"ours line\n", "ours 择边 = 冲突我方侧内容");
    let b_head = std::fs::read(exported.join("b.txt")).expect("HEAD 树必须含 b.txt");
    assert_eq!(b_head, b"theirs line\n", "theirs 择边 = 冲突对方侧内容");
    let _ = std::fs::remove_dir_all(&root);
}

/// 机械失败（LLM Err）+ 旗标/预算双过 + 三轮接触 t0 即全空 → 无未用候选
/// 回落原 worker 照发。测试沙箱无 RPC client：发车深达 insert_dispatch
/// （dispatched/assigned 审计在案）后在传输层诚实失败 → 发车失败臂冻结
/// 转人工。盖 probe 循环 t0 轮、offline 分支、switched=false 回落臂、
/// 发车 Err 臂。
// multi_thread：全量并行下 BASELINE_PUSHER 全局可能已被某个 gateway boot
// 测试装配（进程级 OnceLock，装配与否取决于测试调度顺序——F-B8 同族），
// 装配态下发车会走 push_dispatch_baseline 的 block_in_place 桥，current_thread
// runtime 直接 panic。multi_thread 下两条序都合法。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w5_resolver_offline_fallback_redispatches_original_worker() {
    let deps = resolver_deps("rr-fallback");
    let (issue, root) = issue_on_git_project(&deps, "fallback");

    attach_resolver_llm(
        &deps,
        Arc::new(ResolverLlm {
            script: std::sync::Mutex::new(std::collections::VecDeque::from([Err(
                "上游失联".to_string()
            )])),
            fallback: String::new(),
            calls: AtomicUsize::new(0),
            on_call: None,
        }),
    );

    super::run_resolver(
        deps.clone(),
        issue.clone(),
        "node-b".into(),
        "task-w5b".into(),
        vec![],
    )
    .await;

    let pid = issue.project_id.unwrap();
    assert!(
        deps.store.get_project(pid).unwrap().conflict_frozen,
        "传输层发车失败必须冻结转人工"
    );
    // 冻结理由只锚「发车失败」阶段（不锚具体传输错误文案）：BASELINE_PUSHER
    // 装配序决定失败发生在基线下发还是 peer_chat 提交，文案随之不同。
    let note = parked_comment(deps.store.as_ref(), issue.id).expect("必须有停车评论");
    assert!(
        note.contains("冲突重派发车失败"),
        "停车理由必须点名发车失败: {note}"
    );
    let acts = deps.store.list_activity(issue.id).unwrap();
    let fallback = acts
        .iter()
        .find(|a| {
            a.details
                .as_deref()
                .map(|d| d.contains("conflict_redispatch") && d.contains("回落原 worker"))
                .unwrap_or(false)
        })
        .expect("审计必须记回落决定");
    assert!(fallback.action == "auto_decide");
    let _ = std::fs::remove_dir_all(&root);
}
