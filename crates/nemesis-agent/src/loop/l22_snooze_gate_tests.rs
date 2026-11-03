// L22（2026-11-03 snooze 排队）：gate_inbound 暂停闸三臂 + 管理动词穿透。
//
// 拍板「暂停期入站消息=排队（恢复后投递）」的闸面回归锁：
// ① 用户消息：排队（.pending 落盘）+ 💤 Immediate 回执 + jsonl system 单行
//    （不成对——user 行由重放物化）；
// ② cron tick：静默排队（Coalesced，零 jsonl 落盘零回执）；
// ③ 超上限：诚实拒收（persist_busy_refusal 成对留痕 user+assistant）；
// ④ 管理动词（slash）在闸上方短路——暂停不拦管理操作；
// ⑤ 未暂停会话正常准入，零 pending 文件。
//
// harness 仿 f1_plan_mode_tests（MockProvider + AgentLoop::new + gate 直调）；
// 预置键 `agent:test/l22gate:N` 被 route_message 原样采纳（is_preset_session_key）
// ——键可预测，chat_log 侧 unique key + delete 清理（本 crate 测试惯例）。

use super::*;
use nemesis_types::channel::InboundMessage;

/// 恒定回复的迷你 provider（f1 同形状——gate 测试不触 LLM，仅装配需要）。
struct L22GateMockProvider;

#[async_trait]
impl LlmProvider for L22GateMockProvider {
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

fn l22_gate_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(L22GateMockProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
            tool_fence_profession: None,
        },
    )
}

/// unique 会话键（nanos 防撞）+ 预置前缀（route_message 原样采纳）。
fn l22_key(tag: &str) -> String {
    format!(
        "agent:test/l22gate:{tag}:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn gate_msg(session_key: &str, content: &str) -> InboundMessage {
    InboundMessage {
        channel: "web".to_string(),
        sender_id: "l22user".to_string(),
        chat_id: "l22chat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

fn jsonl_roles(key: &str) -> Vec<(String, String)> {
    let (rows, _, _, _) = crate::chat_log::read_chat_log(key, 100_000, None);
    rows.iter()
        .map(|r| {
            (
                r.get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                r.get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn snoozed_user_message_queues_with_receipt() {
    let key = l22_key("user");
    let al = l22_gate_loop();
    crate::chat_log::write_session_snooze(&key, "2099-01-01T00:00:00+08:00", None);

    match al.gate_inbound(&gate_msg(&key, "帮我看看这个")) {
        GateOutcome::Immediate { agent_id, response } => {
            assert!(!agent_id.is_empty(), "agent_id 由 route_message 现取");
            assert!(
                response.contains("💤") && response.contains("排队"),
                "回执须明示暂停中已排队，实际: {response}"
            );
            assert!(
                response.contains("第 1 条"),
                "回执带排队序号，实际: {response}"
            );
        }
        other => panic!("暂停中用户消息必须排队回执 Immediate，实际 {other:?}"),
    }

    // pending 落盘 + 行内键烧入。
    let queued = crate::chat_log::take_session_pending(&key);
    assert_eq!(queued.len(), 1, "消息必须落 pending");
    assert_eq!(queued[0].content, "帮我看看这个");
    assert_eq!(queued[0].session_key, key, "裁决键必须烧入行内");

    // jsonl 回执：system 单行（不成对——没有 user 行）。
    let roles = jsonl_roles(&key);
    assert_eq!(roles.len(), 1, "排队回执只落 system 单行，实际: {roles:?}");
    assert_eq!(roles[0].0, "system");
    assert!(roles[0].1.contains("💤"), "回执行含 💤 文案: {roles:?}");

    crate::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn snoozed_cron_tick_queues_silently() {
    let key = l22_key("cron");
    let al = l22_gate_loop();
    crate::chat_log::write_session_snooze(&key, "2099-01-01T00:00:00+08:00", None);

    let mut msg = gate_msg(&key, "定时任务心跳");
    msg.metadata
        .insert("cron_job_id".to_string(), "job-l22".to_string());

    match al.gate_inbound(&msg) {
        GateOutcome::Coalesced { cron_job_id } => {
            assert_eq!(cron_job_id, "job-l22");
        }
        other => panic!("暂停中 cron tick 必须 Coalesced 静默排队，实际 {other:?}"),
    }

    // pending 落盘（唤醒后投递），但零 jsonl 落盘（cron 无接收方）。
    let queued = crate::chat_log::take_session_pending(&key);
    assert_eq!(queued.len(), 1);
    let roles = jsonl_roles(&key);
    assert!(
        roles.is_empty(),
        "cron 静默排队不得落回执行，实际: {roles:?}"
    );

    crate::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn snoozed_overflow_refuses_with_paired_refusal_rows() {
    let key = l22_key("cap");
    let al = l22_gate_loop();
    crate::chat_log::write_session_snooze(&key, "2099-01-01T00:00:00+08:00", None);

    // 直接灌满 pending（绕过 gate，隔离闸的排队臂）。
    for i in 0..crate::chat_log::SESSION_PENDING_CAP {
        crate::chat_log::append_session_pending(&key, &gate_msg(&key, &format!("m{i}")))
            .expect("灌满过程不应失败");
    }

    // 第 65 条走 gate：超限诚实拒收。
    match al.gate_inbound(&gate_msg(&key, "overflow")) {
        GateOutcome::Immediate { response, .. } => {
            assert!(
                response.contains("排队不可用") || response.contains("未接收"),
                "超限必须诚实拒收，实际: {response}"
            );
        }
        other => panic!("超限必须 Immediate 拒收回执，实际 {other:?}"),
    }

    // pending 不被挤掉（拒收不丢旧条）。
    assert_eq!(
        crate::chat_log::session_pending_count(&key),
        crate::chat_log::SESSION_PENDING_CAP,
        "拒收不得挤掉已排队消息"
    );

    // jsonl：persist_busy_refusal 成对留痕（user + assistant）。
    let roles = jsonl_roles(&key);
    assert_eq!(roles.len(), 2, "拒收须成对留痕，实际: {roles:?}");
    assert_eq!(roles[0].0, "user");
    assert_eq!(roles[0].1, "overflow");
    assert_eq!(roles[1].0, "assistant");
    assert!(roles[1].1.contains("未接收"), "assistant 行为拒收文案");

    crate::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn snoozed_slash_verbs_bypass_queue() {
    let key = l22_key("slash");
    let al = l22_gate_loop();
    crate::chat_log::write_session_snooze(&key, "2099-01-01T00:00:00+08:00", None);

    // slash 在闸上方短路（handle_command_with_context）：暂停不拦管理动词。
    match al.gate_inbound(&gate_msg(&key, "/build")) {
        GateOutcome::Immediate { response, .. } => {
            assert!(
                !response.contains("排队"),
                "slash 回执不是排队回执，实际: {response}"
            );
        }
        other => panic!("/build 必须在闸上方短路，实际 {other:?}"),
    }
    assert_eq!(
        crate::chat_log::session_pending_count(&key),
        0,
        "slash 不得进 pending"
    );
    let roles = jsonl_roles(&key);
    assert!(roles.is_empty(), "slash 短路零 jsonl 落盘: {roles:?}");

    crate::chat_log::delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// inject_inbound：snooze replay hook 的重注入入口
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inject_inbound_unwired_false_and_wired_delivers() {
    let al = l22_gate_loop();
    // 未接线（standalone）：false——调用方把余量写回 pending，sweeper 重试。
    assert!(!al.inject_inbound(gate_msg("agent:test/l22gate:inj", "x")));

    // 接线后：true + receiver 端收到（与 drain 重注入同一通道）。
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    al.set_reinject_tx(tx);
    assert!(al.inject_inbound(gate_msg("agent:test/l22gate:inj", "hello")));
    let got = rx.try_recv().expect("重注入消息应进通道");
    assert_eq!(got.content, "hello");
    assert_eq!(got.session_key, "agent:test/l22gate:inj");
}

#[tokio::test]
async fn not_snoozed_session_admits_without_pending() {
    let key = l22_key("clean");
    let al = l22_gate_loop();

    // 未暂停：正常准入（Admitted），零 pending 文件。
    let outcome = al.gate_inbound(&gate_msg(&key, "正常消息"));
    assert!(
        matches!(outcome, GateOutcome::Admitted(_)),
        "未暂停会话必须 Admitted，实际 {outcome:?}"
    );
    assert_eq!(crate::chat_log::session_pending_count(&key), 0);
    assert!(!crate::chat_log::pending_path(&key).exists());

    // 暂停→排队→摘标记→重放再入闸：不再排队（豁免只来自先摘标记的顺序
    // 纪律）。重放消息与新消息同权过完整 gate——本测前一条 Admitted 已把
    // 会话置 busy（try_acquire_session，无 turn 结束不释放），重放撞 busy
    // 闸即「穿过 snooze 闸」的证明；核心断言是**不得再进排队臂**。
    crate::chat_log::write_session_snooze(&key, "2099-01-01T00:00:00+08:00", None);
    assert!(matches!(
        al.gate_inbound(&gate_msg(&key, "暂停期消息")),
        GateOutcome::Immediate { .. }
    ));
    crate::chat_log::clear_session_snooze(&key);
    let queued = crate::chat_log::take_session_pending(&key);
    assert_eq!(queued.len(), 1);
    let outcome = al.gate_inbound(&queued.into_iter().next().unwrap());
    match outcome {
        GateOutcome::Admitted(_) => {}
        GateOutcome::Immediate { response, .. } => {
            assert!(
                !response.contains("排队"),
                "摘标记后重放不得再排队（先摘再放的顺序纪律），实际: {response}"
            );
            assert!(
                response.contains("processing a previous request"),
                "非排队臂只能是 busy 闸照常（同权），实际: {response}"
            );
        }
        other => panic!("重放不得 Coalesced，实际 {other:?}"),
    }
    assert_eq!(
        crate::chat_log::session_pending_count(&key),
        0,
        "重放消息不得再落 pending"
    );

    crate::chat_log::delete_chat_log(&key);
}
