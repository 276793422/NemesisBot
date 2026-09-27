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
