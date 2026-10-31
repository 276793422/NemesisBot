// loop/admission.rs 覆盖率补充测试（K4(b) 审批卡回执语法 / 会话闸队列
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
    };
    let (agent_id, response, err) = al.process_inbound_message(&msg).await;
    assert!(err.is_none());
    assert!(agent_id.is_empty(), "Immediate short-circuit: no agent");
    assert!(response.contains("同意 deadbeef"), "got: {response}");
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
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
// inbox_status：三模式快照（U7 dashboard 可见性）
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
// F2（2026-09-27）：空闲态 `!` steer 信号剥除（Steer 模式专属，第三条
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
    let (_, resp, err) = al.process_inbound_message(&f2_msg(content, key)).await;
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

/// Steer 模式空闲：`! 前缀` 剥除后才进模型与 B1 早落盘（与 busy 态同形
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
// C4（2026-10-24 cron 合闸）：busy 闸丢弃同 job 重复 tick
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

    // tick 1：正常入队（排队回执）。
    let GateOutcome::Immediate { response, .. } =
        al.gate_inbound(&cron_tick("job-a", key, "tick1"))
    else {
        panic!("expected immediate queue receipt");
    };
    assert!(response.contains("已排队"), "response={response}");

    // tick 2 同 job：合闸丢弃（Coalesced，无回执）。
    let GateOutcome::Coalesced { cron_job_id } = al.gate_inbound(&cron_tick("job-a", key, "tick2"))
    else {
        panic!("expected coalesced");
    };
    assert_eq!(cron_job_id, "job-a");
    assert_eq!(al.inbox.pending(key), (1, 0), "队列仍只有 tick1 一条");

    // 不同 job 不受影响：照常入队。
    let GateOutcome::Immediate { .. } = al.gate_inbound(&cron_tick("job-b", key, "tick3")) else {
        panic!("expected queued for other job");
    };
    assert_eq!(al.inbox.pending(key), (2, 0));

    // 用户消息（无 cron_job_id）永不合闸。
    let mut user = cron_tick("job-a", key, "普通用户消息");
    user.metadata.remove("cron_job_id");
    user.sender_id = String::new();
    let GateOutcome::Immediate { .. } = al.gate_inbound(&user) else {
        panic!("expected queued for user message");
    };
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
// R294（BUG-20261031-236）：busy 判定与入队之间的竞态孤儿 kick
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
        },
        timestamp: chrono::Local::now().to_rfc3339(),
        priority: crate::inbox::MessagePriority::Normal,
    };
    let _ = al.inbox.enqueue_for_mode(key, queued2, false);
    al.kick_orphaned_queue_if_idle(key);
    assert_eq!(al.inbox.pending(key), (1, 0), "busy session must not kick");
}
