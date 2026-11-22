// loop/admission.rs 覆盖率补充测试（(b) 审批卡回执语法 / 会话闸队列
// 账目 / inbox_status 模式快照）。
//
// 声明挂 admission.rs 的 cfg(test) 块——SessionBusyState 字段为
// admission 模块私有，构造需在本模块的后代中进行。

use super::*;

// ---------------------------------------------------------------------------
// parse_approval_reply：/approve /deny 语法决策表
// ---------------------------------------------------------------------------

#[test]
fn approval_reply_table() {
    assert_eq!(
        parse_approval_reply("/approve deadbeef"),
        Some("✓ 已收到审批回复（同意 deadbeef），结果另行通知。".to_string())
    );
    assert_eq!(
        parse_approval_reply("  /deny 1a2b3c  "),
        Some("✓ 已收到审批回复（拒绝 1a2b3c），结果另行通知。".to_string())
    );
    // trim 后命中（前导空白）。
    assert!(parse_approval_reply("\t/approve abc123\n").is_some());

    // 非审批语法 → None 落普通轮。
    assert_eq!(parse_approval_reply("普通消息"), None);
    assert_eq!(parse_approval_reply("/approve"), None, "裸命令无 id");
    assert_eq!(
        parse_approval_reply("/approved deadbeef"),
        None,
        "前缀必须完整词"
    );
    // id 形态校验：空 / 超长 / 非字母数字。
    assert_eq!(parse_approval_reply("/approve    "), None);
    assert_eq!(
        parse_approval_reply(&format!("/approve {}", "a".repeat(65))),
        None
    );
    assert_eq!(parse_approval_reply("/approve abc;drop"), None);
    assert_eq!(parse_approval_reply("/approve 有中文"), None);
}

/// 闸门 Immediate 短路：审批回执不进 LLM（MockLlmProvider 恒不消耗）。
#[tokio::test]
async fn gate_short_circuits_approval_replies() {
    struct UnusedLlmProvider;
    #[async_trait]
    impl LlmProvider for UnusedLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("approval reply must not reach the LLM");
        }
    }

    let al = AgentLoop::new(Box::new(UnusedLlmProvider), test_config());
    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "covuser".to_string(),
        chat_id: "covchat".to_string(),
        content: "/approve deadbeef".to_string(),
        media: vec![],
        session_key: "agent:main:session:covadm".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
        msg_class: nemesis_types::channel::MsgClass::User,
    };
    let (agent_id, response, err, _) = al.process_inbound_message(&msg).await;
    assert!(err.is_none());
    assert!(agent_id.is_empty(), "Immediate short-circuit: no agent");
    assert!(response.contains("同意 deadbeef"), "got: {response}");
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

// ---------------------------------------------------------------------------
// 会话闸：队列账目（release 在 queue_length>0 时保持 busy）
// ---------------------------------------------------------------------------

struct CovQuietProvider;

#[async_trait]
impl LlmProvider for CovQuietProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[test]
fn release_session_drains_queue_before_releasing() {
    let al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    let key = "agent:main:session:covqueue";

    // 无人认领：release 是 no-op false（busy 本就 false）。
    assert!(!al.is_session_busy(key));
    assert!(!al.release_session(key));

    // 预置：busy + 排队 2。
    al.session_busy.lock().insert(
        key.to_string(),
        SessionBusyState {
            busy: true,
            queue_length: 2,
        },
    );
    assert!(al.is_session_busy(key));
    assert_eq!(al.get_session_busy_state(key), (true, 2));

    // 前两次 release：有排队 → 保持 busy（返回 true）。
    assert!(al.release_session(key), "queue remaining → stays busy");
    assert_eq!(al.session_queue_length(key), 1);
    assert!(
        al.release_session(key),
        "last queued → still busy this round"
    );
    assert_eq!(al.get_session_busy_state(key), (true, 0));

    // 队列空 → release 真正解锁。
    assert!(!al.release_session(key));
    assert!(!al.is_session_busy(key));
    assert_eq!(al.get_session_busy_state(key), (false, 0));

    // try_acquire 正常路径：空闲获取 → 二次拒绝 → 释放后再获取。
    assert!(al.try_acquire_session(key));
    assert!(!al.try_acquire_session(key), "busy → reject");
    assert!(
        !al.release_session(key),
        "no queue → plain release returns false"
    );
    assert!(al.try_acquire_session(key), "acquirable again");
    al.release_session(key);
}

// ---------------------------------------------------------------------------
// BUG-20261113-170：完全释放摘除条目（24/7 会话数无界累积根修）。
// 排队路径条目保留；全释放后 map 回空；缺席键读路径观测等价
// （旧行为留 busy=false 死条目，读路径 (false,0) 不可区分）。
// ---------------------------------------------------------------------------

#[test]
fn release_session_evicts_entry_on_full_release() {
    let al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    let key = "agent:main:session:evict";

    // busy + 排队：release 只消耗队列，条目保留（排队期间闸语义需要它）。
    al.session_busy.lock().insert(
        key.to_string(),
        SessionBusyState {
            busy: true,
            queue_length: 1,
        },
    );
    assert!(al.release_session(key), "queue remaining → stays busy");
    assert_eq!(al.session_busy.lock().len(), 1, "排队未排空前条目保留");

    // 队列已空：完全释放 → 条目摘除，map 回空。
    assert!(!al.release_session(key));
    assert!(
        al.session_busy.lock().is_empty(),
        "完全释放后条目必须摘除（无界累积根修）"
    );
    // 缺席键读路径观测等价。
    assert_eq!(al.get_session_busy_state(key), (false, 0));
    assert_eq!(al.session_queue_length(key), 0);
    assert!(!al.is_session_busy(key));

    // 摘除后可正常重新认领；再完全释放再摘。
    assert!(al.try_acquire_session(key));
    assert!(!al.release_session(key));
    assert!(al.session_busy.lock().is_empty());

    // 重复 release（条目已不在）：no-op false，不复活条目。
    assert!(!al.release_session(key));
    assert!(al.session_busy.lock().is_empty());
}

// ---------------------------------------------------------------------------
// inbox_status：三模式快照（dashboard 可见性）
// ---------------------------------------------------------------------------

#[test]
fn inbox_status_reflects_mode_and_busy() {
    let mut al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    let key = "agent:main:session:covinbox";

    for (mode, expected) in [
        (ConcurrentMode::Reject, "reject"),
        (ConcurrentMode::Queue, "queue"),
        (ConcurrentMode::Steer, "steer"),
    ] {
        al.concurrent_mode = mode;
        let status = al.inbox_status(key);
        assert_eq!(status.mode, expected);
        assert!(!status.busy);
        assert!(status.capacity > 0);
        assert_eq!((status.next_turn, status.next_step), (0, 0));
    }

    // busy 反映。
    assert!(al.try_acquire_session(key));
    assert!(al.inbox_status(key).busy);
    al.release_session(key);
}

// ---------------------------------------------------------------------------
// 空闲态 `!` steer 信号剥除（Steer 模式专属，第三条
// 时序补齐——busy 态 claim/transfer 两条已由 inbox 单一规则覆盖）。
// ---------------------------------------------------------------------------

/// 捕获每次 LLM 请求最后一条 user 消息内容的 provider（模型所见断言面）。
struct F2CapturingProvider {
    seen: std::sync::Arc<parking_lot::Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmProvider for F2CapturingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        if let Some(m) = messages.iter().rev().find(|m| m.role == "user") {
            self.seen.lock().push(m.content.clone());
        }
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn f2_msg(content: &str, key: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "f2user".to_string(),
        chat_id: "f2chat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
        msg_class: nemesis_types::channel::MsgClass::User,
    }
}

/// 断言辅助：跑一条空闲直进消息，返回（模型所见 user 内容, jsonl user 行
/// 内容）。chat_log 走唯一键 + 结尾清理（ws9_lineage_tests 同款纪律）。
async fn f2_run_and_observe(
    al: &AgentLoop,
    seen: &std::sync::Arc<parking_lot::Mutex<Vec<String>>>,
    content: &str,
    key: &str,
) -> (String, serde_json::Value) {
    use crate::chat_log::{delete_chat_log, read_chat_log};
    delete_chat_log(key);
    let (_, resp, err, _) = al.process_inbound_message(&f2_msg(content, key)).await;
    assert!(err.is_none(), "turn must succeed: resp={resp}");
    let seen_content = seen
        .lock()
        .last()
        .cloned()
        .expect("LLM must see exactly one user message");
    let (rows, _, _, _) = read_chat_log(key, 100, None);
    let user_row = rows
        .iter()
        .rev()
        .find(|r| r["role"] == "user")
        .expect("user row persisted (B1 early persist)")
        .clone();
    delete_chat_log(key);
    (seen_content, user_row)
}

/// Steer 模式空闲：`! 前缀` 剥除后才进模型与 早落盘（与 busy 态同形
/// ——标记是路由信号不是内容，模型/历史/上下文三面一致）。
#[tokio::test]
async fn f2_steer_mode_idle_admission_strips_marker() {
    let seen: std::sync::Arc<parking_lot::Mutex<Vec<String>>> = Default::default();
    let mut al = AgentLoop::new(
        Box::new(F2CapturingProvider { seen: seen.clone() }),
        test_config(),
    );
    al.concurrent_mode = ConcurrentMode::Steer;
    let (model_saw, user_row) = f2_run_and_observe(
        &al,
        &seen,
        "! 紧急：先跑测试",
        "agent:main:session:f2steer1",
    )
    .await;
    assert_eq!(
        model_saw, "紧急：先跑测试",
        "模型所见必须无标记（与 busy claim 剥除同形）"
    );
    assert_eq!(
        user_row["content"], "紧急：先跑测试",
        "B1 早落盘的 user 行同为无标记（剥除在落盘之前）"
    );
}

/// `!!` 转义：剥一位 → 字面 `!` 进模型（shell 惯例；与 busy 态 claim 的
/// 单剥规则同源，转义在两条时序下行为一致）。
#[tokio::test]
async fn f2_steer_mode_idle_double_bang_escapes_to_literal() {
    let seen: std::sync::Arc<parking_lot::Mutex<Vec<String>>> = Default::default();
    let mut al = AgentLoop::new(
        Box::new(F2CapturingProvider { seen: seen.clone() }),
        test_config(),
    );
    al.concurrent_mode = ConcurrentMode::Steer;
    let (model_saw, user_row) = f2_run_and_observe(
        &al,
        &seen,
        "!! 字面感叹号开头的内容",
        "agent:main:session:f2steer2",
    )
    .await;
    assert_eq!(model_saw, "! 字面感叹号开头的内容");
    assert_eq!(user_row["content"], "! 字面感叹号开头的内容");
}

/// Queue 模式空闲：`!` 从来不是信号（busy 时也不剥），保持字面量——
/// mode-aware 剥除的另一面（语义随模式走，不随到达时机走）。
#[tokio::test]
async fn f2_queue_mode_idle_keeps_marker_literal() {
    let seen: std::sync::Arc<parking_lot::Mutex<Vec<String>>> = Default::default();
    let mut al = AgentLoop::new(
        Box::new(F2CapturingProvider { seen: seen.clone() }),
        test_config(),
    );
    al.concurrent_mode = ConcurrentMode::Queue;
    let (model_saw, user_row) = f2_run_and_observe(
        &al,
        &seen,
        "! Queue 模式下保持原样",
        "agent:main:session:f2queue1",
    )
    .await;
    assert_eq!(model_saw, "! Queue 模式下保持原样");
    assert_eq!(user_row["content"], "! Queue 模式下保持原样");
}

// ---------------------------------------------------------------------------
// （cron 合闸）：busy 闸丢弃同 job 重复 tick
// ---------------------------------------------------------------------------

fn cron_tick(job: &str, key: &str, content: &str) -> nemesis_types::channel::InboundMessage {
    let mut m = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: format!("cron:{job}"),
        chat_id: "web:x".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
        msg_class: nemesis_types::channel::MsgClass::Instruction(
            nemesis_types::channel::InstructionKind::CronTick,
        ),
    };
    m.metadata
        .insert("cron_job_id".to_string(), job.to_string());
    m
}

#[test]
fn cron_tick_coalesced_when_same_job_already_queued() {
    let mut al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    al.concurrent_mode = ConcurrentMode::Queue;
    let key = "agent:main:session:croncoalesce";

    // busy：占住会话，后续消息全走入队路径。
    assert!(al.try_acquire_session(key));

    // tick 1：正常入队（指令注入静默排队——§9 政策表：⏳ 回执是用户
    // 消息语义，不作用内部指令；排队事实由 claim 后真轮呈现）。
    let GateOutcome::InstructionQueued { session_key } =
        al.gate_inbound(&cron_tick("job-a", key, "tick1"))
    else {
        panic!("expected instruction queued");
    };
    assert_eq!(session_key, key);

    // tick 2 同 job：合闸丢弃（Coalesced，无回执）。
    let GateOutcome::Coalesced { cron_job_id } = al.gate_inbound(&cron_tick("job-a", key, "tick2"))
    else {
        panic!("expected coalesced");
    };
    assert_eq!(cron_job_id, "job-a");
    assert_eq!(al.inbox.pending(key), (1, 0), "队列仍只有 tick1 一条");

    // 不同 job 不受影响：照常静默入队。
    let GateOutcome::InstructionQueued { .. } = al.gate_inbound(&cron_tick("job-b", key, "tick3"))
    else {
        panic!("expected queued for other job");
    };
    assert_eq!(al.inbox.pending(key), (2, 0));

    // 用户消息（User class）永不合闸（Queue 模式 ⏳ 排队回执语义保留）。
    let user = nemesis_types::channel::InboundMessage::user_ingress(
        "web",
        "covuser",
        "web:x",
        "普通用户消息",
    )
    .with_session_key(key);
    let GateOutcome::Immediate { response, .. } = al.gate_inbound(&user) else {
        panic!("expected queued for user message");
    };
    assert!(response.contains("已排队"), "response={response}");
    assert_eq!(al.inbox.pending(key), (3, 0));

    // 合闸上界：再来的 job-a tick 全部丢弃，队列恒 ≤ 1 条/job。
    for i in 0..5 {
        let GateOutcome::Coalesced { .. } =
            al.gate_inbound(&cron_tick("job-a", key, &format!("burst{i}")))
        else {
            panic!("expected coalesced for burst{i}");
        };
    }
    assert_eq!(al.inbox.pending(key), (3, 0), "burst 未改变队列");
}

// ---------------------------------------------------------------------------
// busy 判定与入队之间的竞态孤儿 kick
// ---------------------------------------------------------------------------

/// 合法交织的坏终态 + kick 行为：回合持闸期间消息进 busy 分支，回合在本闸
/// try_acquire 与入队之间完成（release + 空手 drain——两个 worker 并行），
/// 消息入队后没有任何在途回合会来排它（drain 只挂在回合/维护/历史闸尾巴）
/// ——滞留到该会话下一条消息到来（顺序倒挂），无下一条则无限期滞留。
/// kick = 入队后复查 busy，已空闲则就地走 drain 同款排空（reinject 回流，
/// 泵按正常闸门重新准入）。busy 时 kick 必须是 no-op（正常路径由在途回合
/// 尾巴排空）。
///
/// 诚实边界：交织窗口在 gate_inbound 单次调用内部（纯同步区间），无法从
/// 外部确定性注入——本测试按同一批原语的合法调用序构造坏终态，再驱动
/// 生产 helper 本体；两处 Queued* 臂的接线为可见的两行调用。
#[tokio::test]
async fn busy_race_orphan_kick_reinjects_parked_head() {
    struct UnusedLlmProvider;
    #[async_trait]
    impl LlmProvider for UnusedLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("kick 路径不得触碰 LLM");
        }
    }

    let al = AgentLoop::new(Box::new(UnusedLlmProvider), test_config());
    let (rein_tx, mut rein_rx) = tokio::sync::mpsc::channel(4);
    al.set_reinject_tx(rein_tx);
    let key = "agent:main:session:covkick";

    // ① 坏终态（合法原语序）：turn 持闸 → 完成（release + 空手 drain）→
    //    消息入队（此刻已无在途回合）。
    assert!(al.try_acquire_session(key));
    al.release_session(key);
    let queued = crate::inbox::QueuedMessage {
        msg: nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: "covuser".to_string(),
            chat_id: "covchat".to_string(),
            content: "孤儿消息".to_string(),
            media: vec![],
            session_key: key.to_string(),
            correlation_id: String::new(),
            metadata: Default::default(),
            voice_playback: None,
            msg_class: nemesis_types::channel::MsgClass::User,
        },
        timestamp: chrono::Local::now().to_rfc3339(),
        priority: crate::inbox::MessagePriority::Normal,
    };
    assert_eq!(
        al.inbox.enqueue_for_mode(key, queued, false),
        crate::inbox::EnqueueOutcome::QueuedForNextTurn
    );
    assert_eq!(al.inbox.pending(key), (1, 0));
    assert!(
        !al.is_session_busy(key),
        "precondition: interleaved turn already finished"
    );

    // ② kick：队列清空，队首经 reinject 通道回流。
    al.kick_orphaned_queue_if_idle(key);
    assert_eq!(al.inbox.pending(key), (0, 0), "orphan must be drained");
    let reinjected = tokio::time::timeout(std::time::Duration::from_secs(2), rein_rx.recv())
        .await
        .expect("kick must re-inject the parked head")
        .expect("channel open");
    assert_eq!(reinjected.content, "孤儿消息");
    assert_eq!(reinjected.session_key, key, "会话键归一随队首回流");

    // ③ 正常路径守卫：busy 会话不 kick（在途回合的尾巴负责）。
    assert!(al.try_acquire_session(key));
    let queued2 = crate::inbox::QueuedMessage {
        msg: nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: "covuser".to_string(),
            chat_id: "covchat".to_string(),
            content: "第二条".to_string(),
            media: vec![],
            session_key: key.to_string(),
            correlation_id: String::new(),
            metadata: Default::default(),
            voice_playback: None,
            msg_class: nemesis_types::channel::MsgClass::User,
        },
        timestamp: chrono::Local::now().to_rfc3339(),
        priority: crate::inbox::MessagePriority::Normal,
    };
    let _ = al.inbox.enqueue_for_mode(key, queued2, false);
    al.kick_orphaned_queue_if_idle(key);
    assert_eq!(al.inbox.pending(key), (1, 0), "busy session must not kick");
}

// ---------------------------------------------------------------------------
// system_notice_of：系统通知标记推导（2026-11-16 系统注入消息渲染成用户
// 气泡 BUG）——sender 推导 / metadata 覆盖 / 真人消息不命中。
// ---------------------------------------------------------------------------

#[test]
fn system_notice_of_derivation_table() {
    let mk =
        |sender: &str, metadata: HashMap<String, String>| nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: sender.to_string(),
            chat_id: "c".to_string(),
            content: "x".to_string(),
            media: vec![],
            session_key: "s".to_string(),
            correlation_id: String::new(),
            metadata,
            voice_playback: None,
            msg_class: nemesis_types::channel::MsgClass::User,
        };

    // ① 后台任务完成唤醒形态：sender=system、无 metadata → 推导命中。
    let wake = mk("system", HashMap::new());
    assert_eq!(super::system_notice_of(&wake), Some("system"));

    // ② Turn WAL 续轮形态：sender=system + 自有 metadata（不含
    // system_notice）→ 仍推导命中（自有键不受影响）。
    let wal = mk(
        "system",
        [("turn_wal_resume".to_string(), "1".to_string())]
            .into_iter()
            .collect(),
    );
    assert_eq!(super::system_notice_of(&wal), Some("system"));

    // ③ metadata 显式值优先（注入点可指定更细类别）。
    let explicit = mk(
        "system",
        [("system_notice".to_string(), "background_wake".to_string())]
            .into_iter()
            .collect(),
    );
    assert_eq!(super::system_notice_of(&explicit), Some("background_wake"));

    // ④ 真人消息：web 通道真实会话 id → None（不写键，用户气泡旧行为）。
    let human = mk("web-session-abc", HashMap::new());
    assert_eq!(super::system_notice_of(&human), None);

    // ⑤ cron 投递形态：sender=cron:{id} → None（cron 行已有 cron_job_id
    // meta，前端按 cronJobId 渲染，不走本标记）。
    let cron = mk("cron:job-7", HashMap::new());
    assert_eq!(super::system_notice_of(&cron), None);

    // ⑥ 集群桥接远端用户：sender 为远端真实 id → None。
    let remote = mk("remote-user-9", HashMap::new());
    assert_eq!(super::system_notice_of(&remote), None);
}

// ---------------------------------------------------------------------------
// 幻影通告根治回归锁（2026-11-21）：busy 闸通告分支——盖章通告 stash 旁路
// 三模式；指令类注入静默排队（政策臂）；空闲照常 Admitted 开真轮；
// busy-race 孤儿 kick；turn 收口合成通告轮。
// 事故形态：目标机 14 条后台构建通告连环排队/软拒/claim 后物化 user 行 +
// 幻影 LLM 轮（usage 97→109）。任何一条断言红 = 幻影路径复活。
// ---------------------------------------------------------------------------

/// 盖章通告消息构造（与 nemesisbot post_agent::system_notice_inbound 同形
/// ——msg_class=Notice 是闸门识别的唯一真相源；kind 必须是合法 NoticeKind
/// slug）。
fn notice_msg(kind: &str, key: &str, content: &str) -> nemesis_types::channel::InboundMessage {
    let mut m = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "system".to_string(),
        chat_id: "web:notice".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
        msg_class: nemesis_types::channel::MsgClass::Notice(
            nemesis_types::channel::NoticeKind::from_slug(kind)
                .unwrap_or_else(|| panic!("invalid NoticeKind slug in test: {kind}")),
        ),
    };
    m.metadata
        .insert("system_notice".to_string(), kind.to_string());
    m
}

/// 三模式 busy 全部 stash（Reject 弹回 / Queue 排队 / Steer 插话——通告都
/// 不再走用户消息语义；队列恒空 = 零回执零占位零弹回）。
#[test]
fn notice_stash_bypasses_all_three_busy_modes() {
    let mut al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    let key = "agent:main:session:noticemode";
    assert!(al.try_acquire_session(key), "precondition: busy session");

    for mode in [
        ConcurrentMode::Reject,
        ConcurrentMode::Queue,
        ConcurrentMode::Steer,
    ] {
        al.concurrent_mode = mode;
        let GateOutcome::NoticeStashed { session_key } =
            al.gate_inbound(&notice_msg("events", key, "Background job #1 finished"))
        else {
            panic!("mode {mode:?}: stamped notice must stash, not queue/bounce");
        };
        assert_eq!(session_key, key);
        assert_eq!(
            al.inbox.pending(key),
            (0, 0),
            "通告不占用户队列位（占位 = 幻影物化路径复发）"
        );
    }
    assert!(al.inbox.has_notices(key), "三模式 stash 共 3 条在库");
}

/// 指令类注入（WAL 续轮 / goals 缺口 / 看板派发）busy 时静默排队：政策臂
/// 按类裁决（零回执零占位零弹回）；cron tick 按 job_id 合闸（重复 tick 只
/// 留首条）。指令不进通告 stash、不发回执（排队事实由 claim 后真轮呈现）。
#[test]
fn instruction_class_busy_queues_silently() {
    let mut al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    al.concurrent_mode = ConcurrentMode::Queue;
    let key = "agent:main:session:noticeinstr";

    assert!(al.try_acquire_session(key));
    let mk_instruction = |kind| nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "system".to_string(),
        chat_id: "web:x".to_string(),
        content: "继续执行".to_string(),
        media: vec![],
        session_key: key.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
        msg_class: nemesis_types::channel::MsgClass::Instruction(kind),
    };
    for kind in [
        nemesis_types::channel::InstructionKind::WalResume,
        nemesis_types::channel::InstructionKind::GoalRound,
        nemesis_types::channel::InstructionKind::BoardDispatch,
    ] {
        let GateOutcome::InstructionQueued { session_key } = al.gate_inbound(&mk_instruction(kind))
        else {
            panic!("instruction must queue silently via policy arm");
        };
        assert_eq!(session_key, key);
    }
    // cron tick：同 job 重复 tick 合闸（Coalesced，只留首条）。
    let mut cron = mk_instruction(nemesis_types::channel::InstructionKind::CronTick);
    cron.metadata
        .insert("cron_job_id".to_string(), "job-7".to_string());
    let GateOutcome::InstructionQueued { .. } = al.gate_inbound(&cron) else {
        panic!("first cron tick queues");
    };
    let GateOutcome::Coalesced { cron_job_id } = al.gate_inbound(&cron) else {
        panic!("duplicate cron tick must coalesce");
    };
    assert_eq!(cron_job_id, "job-7");
    // 队列只有 4 条指令（cron 合闸只留 1 条）。
    assert_eq!(al.inbox.pending(key), (4, 0), "四条全在队列（cron 已合闸）");
    assert!(!al.inbox.has_notices(key), "指令不进通告 stash");
}

/// Reject 模式指令静默排队（§9 政策表「Reject 不弹回不留痕」收编锁）：
/// 政策臂在模式分派之前——Reject 的弹回+成对留痕（persist_busy_refusal
/// 幻影双行）结构性作用不到指令类。本锁钉住「Reject × Instruction 零
/// Immediate 零 jsonl 行」契约（验收 §14-9 的 Reject 幻影双行面）。
#[test]
fn instruction_reject_mode_queues_silently_without_rows() {
    use crate::chat_log::{delete_chat_log, read_chat_log};
    let mut al = AgentLoop::new(Box::new(CovQuietProvider), test_config());
    al.concurrent_mode = ConcurrentMode::Reject;
    let key = "agent:main:session:instrrej";
    delete_chat_log(key);

    assert!(al.try_acquire_session(key), "precondition: busy session");
    let instr = nemesis_types::channel::InboundMessage::instruction(
        nemesis_types::channel::InstructionKind::GoalRound,
        "web",
        "web:x",
        key,
        "目标缺口：补齐验收锚点",
    );
    let GateOutcome::InstructionQueued { session_key } = al.gate_inbound(&instr) else {
        panic!("Reject mode must NOT bounce instructions (silent queue per policy)");
    };
    assert_eq!(session_key, key);

    // 零弹回零留痕：无 persist_busy_refusal 成对行，队列持有该指令。
    let (rows, _, _, _) = read_chat_log(key, 100, None);
    assert!(
        rows.is_empty(),
        "Reject × Instruction 不得落弹回双行（幻影双行收编），实际: {rows:?}"
    );
    assert_eq!(al.inbox.pending(key), (1, 0), "指令在 next_turn 队等待");
    delete_chat_log(key);
}

/// 空闲盖章通告：照常 Admitted 开真轮（通知条形态——信息不丢，模型可
/// 跟进）；jsonl user 行带 system_notice 键（前端通知条渲染，非用户气泡）。
#[tokio::test]
async fn idle_notice_admitted_opens_real_turn_with_notice_row() {
    use crate::chat_log::{delete_chat_log, read_chat_log};
    let seen: std::sync::Arc<parking_lot::Mutex<Vec<String>>> = Default::default();
    let al = AgentLoop::new(
        Box::new(F2CapturingProvider { seen: seen.clone() }),
        test_config(),
    );
    let key = "agent:main:session:noticeidle";
    delete_chat_log(key);

    let (_, resp, err, _) = al
        .process_inbound_message(&notice_msg("events", key, "Background job #2 finished"))
        .await;
    assert!(err.is_none(), "idle notice turn must succeed: resp={resp}");
    // 模型照常拿到通告内容（信息不丢）。
    let model_saw = seen.lock().last().cloned().expect("LLM saw the notice");
    assert!(model_saw.contains("Background job #2 finished"));
    // user 行落盘带 system_notice 键（通知条渲染）——role 仍是 user。
    let (rows, _, _, _) = read_chat_log(key, 100, None);
    let user_row = rows
        .iter()
        .rev()
        .find(|r| r["role"] == "user")
        .expect("user row persisted");
    assert_eq!(
        user_row["system_notice"], "events",
        "通知条标记随行（前端渲染为通知条而非用户气泡）"
    );
    delete_chat_log(key);
    // turn 收尾释放：会话可重新认领（无 stash 残留——空闲路径不经 stash）。
    assert!(!al.is_session_busy(key));
    assert!(!al.inbox.has_notices(key));
}

/// busy-race 孤儿通告 kick（kick_orphaned_queue_if_idle 同款机理）：
/// stash 后复查 busy——回合已收尾即就地合成通告轮重注入（构造消息带
/// system_notice=merged 章 + 框注）；busy 会话不 kick（正常路径守卫）。
#[tokio::test]
async fn notice_busy_race_orphan_kick_reinjects_merged() {
    struct UnusedLlmProvider;
    #[async_trait]
    impl LlmProvider for UnusedLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("kick 路径不得触碰 LLM");
        }
    }

    let al = AgentLoop::new(Box::new(UnusedLlmProvider), test_config());
    let (rein_tx, mut rein_rx) = tokio::sync::mpsc::channel(4);
    al.set_reinject_tx(rein_tx);
    let key = "agent:main:session:noticekick";

    // ① 坏终态（合法原语序）：release 先行（无在途回合）→ stash 落库
    //    （等价于 gate 判 busy 后回合恰完成的窗口终态）→ kick。
    al.release_session(key); // no-op busy=false，构造「无在途回合」状态
    al.inbox.stash_notice(
        key,
        crate::inbox::NoticeEntry {
            kind: nemesis_types::channel::NoticeKind::Events,
            channel: "web".to_string(),
            chat_id: "web:notice".to_string(),
            content: "Background job #3 finished".to_string(),
        },
    );
    assert!(!al.is_session_busy(key), "precondition: no in-flight turn");
    al.kick_orphaned_notices_if_idle(key);
    assert!(!al.inbox.has_notices(key), "orphan notices must be kicked");
    let reinjected = tokio::time::timeout(std::time::Duration::from_secs(2), rein_rx.recv())
        .await
        .expect("kick must re-inject the merged notice")
        .expect("channel open");
    assert_eq!(reinjected.session_key, key);
    assert_eq!(
        reinjected.msg_class,
        nemesis_types::channel::MsgClass::Notice(nemesis_types::channel::NoticeKind::Merged),
        "合成通告轮 = Notice(Merged) 类（空闲准入=通知条；再 busy=re-stash 闭环）"
    );
    assert!(
        reinjected.content.contains("[系统通告]") && reinjected.content.contains("#3 finished"),
        "框注 + 原文随行"
    );

    // ② 正常路径守卫：busy 会话不 kick（在途回合的边界/尾巴负责 drain）。
    assert!(al.try_acquire_session(key));
    al.inbox.stash_notice(
        key,
        crate::inbox::NoticeEntry {
            kind: nemesis_types::channel::NoticeKind::Events,
            channel: "web".to_string(),
            chat_id: "web:notice".to_string(),
            content: "Background job #4 finished".to_string(),
        },
    );
    al.kick_orphaned_notices_if_idle(key);
    assert!(al.inbox.has_notices(key), "busy session must not kick");
    al.release_session(key);
}

/// turn 收口通告 drain：busy 末段（最后一个 claim 边界之后）到达的 stash
/// 残留 → drain_next_turn_queue 合成一条合并通告轮经 reinject 回流；
/// 多条合并为一条（count/路由=最后一条来源）；模式无关（Reject 无队列
/// drain 也合成）；与队列头部 drain 互不吞并。
#[tokio::test]
async fn turn_end_drain_synthesizes_merged_notice_turn() {
    struct UnusedLlmProvider;
    #[async_trait]
    impl LlmProvider for UnusedLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("drain 收口路径不得触碰 LLM（合成消息走泵重新准入）");
        }
    }

    let mut al = AgentLoop::new(Box::new(UnusedLlmProvider), test_config());
    al.concurrent_mode = ConcurrentMode::Reject;
    let (rein_tx, mut rein_rx) = tokio::sync::mpsc::channel(4);
    al.set_reinject_tx(rein_tx);
    let key = "agent:main:session:noticedrain";

    // 两条通告 stash（跨通道来源）→ turn 收口 drain。
    al.inbox.stash_notice(
        key,
        crate::inbox::NoticeEntry {
            kind: nemesis_types::channel::NoticeKind::Loop,
            channel: "web".to_string(),
            chat_id: "web:early".to_string(),
            content: "job A finished".to_string(),
        },
    );
    al.inbox.stash_notice(
        key,
        crate::inbox::NoticeEntry {
            kind: nemesis_types::channel::NoticeKind::Loop,
            channel: "rpc".to_string(),
            chat_id: "rpc:late".to_string(),
            content: "job B finished".to_string(),
        },
    );
    al.drain_next_turn_queue(key).await;

    // 合并成一条合成通告轮（Reject 模式无队列头部 drain，恰验证通告轮
    // 独立于模式）。
    let reinjected = tokio::time::timeout(std::time::Duration::from_secs(2), rein_rx.recv())
        .await
        .expect("turn-end drain must synthesize the notice turn")
        .expect("channel open");
    assert_eq!(reinjected.session_key, key);
    assert_eq!(reinjected.channel, "rpc", "路由=最后一条通告来源");
    assert_eq!(reinjected.chat_id, "rpc:late");
    assert!(reinjected.content.contains("job A finished"));
    assert!(reinjected.content.contains("job B finished"), "多条合并");
    assert!(
        matches!(
            &reinjected.msg_class,
            nemesis_types::channel::MsgClass::Notice(nemesis_types::channel::NoticeKind::Merged)
        ),
        "合成通告轮 class=Notice(Merged)（构造器烤死，旧 merged 章退役），实际 {:?}",
        reinjected.msg_class
    );
    // stash 已空；无第二条合成消息（通道空）。
    assert!(!al.inbox.has_notices(key));
    assert!(
        rein_rx.try_recv().is_err(),
        "两条通告只合成一条（不逐条物化——逐条=幻影复发）"
    );
}

// —— C1 绊线 + estop×class 兼容锁（msg_class 单轨 §10-9/§11，2026-11-21）——

/// C1 绊线是**观察面**（不改路由）：Notice 指向不存在的会话 → gate 照常
/// 走 idle 准入臂（Admitted），默认 warn 态不 panic。孤儿注入的物化风险
/// 由 warn 观测 + 生产装配纪律双面收敛；panic 态走 env
/// （NEMESIS_BUS_SEMANTIC_ASSERT，同 bus 检测网键），进程级 env 置换会
/// 与并行测试竞速，不在套件内驱动（检测逻辑本身由本锁覆盖，处置分叉
/// 只是 matches! 分支）。本锁钉住「绊线不得改变路由语义」契约。
#[tokio::test]
async fn c1_tripwire_is_observe_only_routes_unchanged() {
    struct UnusedLlmProvider;
    #[async_trait]
    impl LlmProvider for UnusedLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("本测试不驱动真回合");
        }
    }
    let al = AgentLoop::new(Box::new(UnusedLlmProvider), test_config());
    // 会话 jsonl 刻意不存在（孤儿注入形态——C1 第 2 条命中）。
    let key = format!(
        "agent:main:session:c1orphan-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let notice = nemesis_types::channel::InboundMessage::notice(
        nemesis_types::channel::NoticeKind::Events,
        "web",
        "web:c1",
        key.clone(),
        "orphan notice",
    );
    let outcome = al.gate_inbound(&notice);
    assert!(
        matches!(outcome, GateOutcome::Admitted(_)),
        "绊线必须观察不拦截（Admitted 照常 mint），实际: {outcome:?}"
    );
    al.release_session(&key);
    crate::chat_log::delete_chat_log(&key);
}

/// estop×class 兼容（§11 estop 锁）：急停期间闸门 class 臂照常工作——
/// busy 会话的通告照常 stash、指令照常静默排队（闸门机制不咨询 estop；
/// 零轮次不变量由 run-loop 顶部臂保证——estop_engaged_stops_loop_before_llm
/// 已锁，此处锁「class 臂不被 estop 冻结」的另一面：release 后积压指令/
/// 通告照常可交付，无丢失）。
#[tokio::test]
async fn estop_engaged_class_arms_still_stash_and_queue() {
    use crate::estop::EstopState;
    struct UnusedLlmProvider;
    #[async_trait]
    impl LlmProvider for UnusedLlmProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            panic!("本测试不驱动真回合");
        }
    }
    let al = AgentLoop::new(Box::new(UnusedLlmProvider), test_config());
    let estop = std::sync::Arc::new(EstopState::new());
    al.set_estop(estop.clone());
    estop.trigger();
    let key = "agent:main:session:c1estop";
    // 先建会话 jsonl（C1 绊线第 2 条要求目标会话存在——本锁测 estop 面，
    // 不测绊线）。
    crate::chat_log::delete_chat_log(key);
    crate::chat_log::append_chat_log(key, "system", "seed");

    // busy 持闸 + estop 中：通告 stash、指令排队都照常（class 臂不冻结）。
    assert!(al.try_acquire_session(key));
    let stashed = al.gate_inbound(&notice_msg("events", key, "bg done"));
    assert!(
        matches!(stashed, GateOutcome::NoticeStashed { .. }),
        "estop 中通告照常 stash，实际: {stashed:?}"
    );
    let queued = al.gate_inbound(&cron_tick("job-e", key, "tick"));
    assert!(
        matches!(queued, GateOutcome::InstructionQueued { .. }),
        "estop 中指令照常静默排队，实际: {queued:?}"
    );
    al.release_session(key);

    // release 后积压可交付（无丢失——A3/队列语义在 estop 两侧一致）。
    assert_eq!(al.inbox.pending(key), (1, 0), "指令条目在队");
    assert!(al.inbox.has_notices(key), "通告在 stash");
    al.inbox.clear(key);
    crate::chat_log::delete_chat_log(key);
}
