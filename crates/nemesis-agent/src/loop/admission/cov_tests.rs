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
