//! P19（能力扩展 WS8）批内 steer 跳批测试。
//!
//! 覆盖三面：
//! 1. 单元面——`check_steer_skip_batch` 直调：peek 命中 → 剩余调用合成
//!    `Skipped due to queued user message.` tool 结果 + 事件流可见 +
//!    `repair_tool_message_pairs` 对落账后的历史零改动（消息对在落账时
//!    即完整合法，provider 序列化不炸）。
//! 2. 集成面——串行批（2 个非只读工具）：工具 A 执行中把 `!` steer 消息
//!    排进 inbox（模拟用户在工具执行窗口插队）→ 工具 B 不再执行、下次
//!    LLM 请求里 call_2 的 tool 结果 = Skipped 文本、steer 文本（剥 `!`）
//!    以 user 消息进同一请求、assistant(tool_calls) → tool 结果对完整。
//! 3. 并行面——U5 并行预计算批（全只读工具）：批在间隙检查可及前已全部
//!    执行完，steer 不跳批（跳掉已完成的工作只会制造未应答 tool_call），
//!    两个真实结果照常回灌，steer 由下一轮器官 2 正常认领。

use super::STEER_SKIP_TOOL_RESULT;
use crate::inbox::QueuedMessage;
use crate::r#loop::prelude::*;
use crate::r#loop::*;
use crate::types::repair_tool_message_pairs;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

// --- 测试件 ----------------------------------------------------------------

/// 捕获每次实际发出的请求消息 + 按序吐出脚本响应（耗尽后回落纯文本终答）。
struct CapturingProvider {
    captured: std::sync::Mutex<Vec<Vec<LlmMessage>>>,
    scripted: std::sync::Mutex<Vec<LlmResponse>>,
}

impl CapturingProvider {
    fn new(scripted: Vec<LlmResponse>) -> Self {
        Self {
            captured: std::sync::Mutex::new(Vec::new()),
            scripted: std::sync::Mutex::new(scripted),
        }
    }
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.captured.lock().unwrap().push(messages);
        let mut scripted = self.scripted.lock().unwrap();
        if scripted.is_empty() {
            Ok(LlmResponse {
                content: "final answer".to_string(),
                tool_calls: vec![],
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(scripted.remove(0))
        }
    }
}

/// 持共享句柄的转发 provider（测试断言用 captured 句柄）。
struct ArcCapturing(Arc<CapturingProvider>);

#[async_trait]
impl LlmProvider for ArcCapturing {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.0.chat(model, messages, options, tools).await
    }
}

/// 执行时把 `!` steer 消息排进 inbox 的工具——模拟「工具执行窗口内用户
/// 插队」。默认非只读（fail-closed）→ 强制串行批路径。
struct SteerEnqueueTool {
    inbox: crate::inbox::SharedInbox,
    session_key: String,
    steer_content: String,
    executed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for SteerEnqueueTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        self.executed.store(true, AtomicOrdering::SeqCst);
        self.inbox.enqueue(
            &self.session_key,
            QueuedMessage {
                msg: nemesis_types::channel::InboundMessage {
                    channel: "web".to_string(),
                    sender_id: "tester".to_string(),
                    chat_id: "c1".to_string(),
                    content: self.steer_content.clone(),
                    media: vec![],
                    session_key: self.session_key.clone(),
                    correlation_id: String::new(),
                    metadata: Default::default(),
                    voice_playback: None,
                },
                timestamp: String::new(),
            },
        );
        Ok("STEER_TOOL_OK".to_string())
    }
}

/// 普通探测工具：记录是否真的被派发（被跳过 = flag 保持 false）。
struct ProbeTool {
    marker: String,
    executed: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for ProbeTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        self.executed.store(true, AtomicOrdering::SeqCst);
        Ok(self.marker.clone())
    }
}

/// 只读探测工具（U5 并行路径准入形态）。
struct ReadOnlyProbeTool {
    marker: String,
    executed: Arc<AtomicBool>,
    /// 执行时可选地把 steer 排进 inbox（模拟并行窗口内到达）。
    inbox: Option<crate::inbox::SharedInbox>,
    session_key: String,
}

#[async_trait]
impl Tool for ReadOnlyProbeTool {
    async fn execute(&self, _args: &str, _ctx: &RequestContext) -> Result<String, String> {
        self.executed.store(true, AtomicOrdering::SeqCst);
        if let Some(ref inbox) = self.inbox {
            inbox.enqueue(
                &self.session_key,
                QueuedMessage {
                    msg: nemesis_types::channel::InboundMessage {
                        channel: "web".to_string(),
                        sender_id: "tester".to_string(),
                        chat_id: "c1".to_string(),
                        content: "! 并行窗口内到达".to_string(),
                        media: vec![],
                        session_key: self.session_key.clone(),
                        correlation_id: String::new(),
                        metadata: Default::default(),
                        voice_playback: None,
                    },
                    timestamp: String::new(),
                },
            );
        }
        Ok(self.marker.clone())
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

fn tool_call(id: &str, name: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: "{}".to_string(),
    }
}

fn scripted_tool_resp(calls: Vec<crate::types::ToolCallInfo>) -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: calls,
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn steer_mode_loop(provider: Box<dyn LlmProvider>) -> AgentLoop {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    AgentLoop::new_bus(
        provider,
        AgentConfig::default(),
        tx,
        ConcurrentMode::Steer,
        4,
        0,
    )
}

fn inbound(session: &str, content: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "tester".to_string(),
        chat_id: "c1".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

// --- 1. 单元面：check_steer_skip_batch 直调 --------------------------------

/// peek 命中 steer：batch_idx 起全部调用合成 Skipped 结果；落账后的历史
/// 过 repair_tool_message_pairs 零改动（消息对已完整，无需修复）。
#[tokio::test]
async fn steer_skip_unit_synthesizes_results_and_keeps_pairs_legal() {
    let provider = CapturingProvider::new(vec![]);
    let lp = steer_mode_loop(Box::new(provider));
    let key = "agent:p19-unit";
    // 预置 steer 在队列里（下一轮 organ 2 会认领；这里只关心跳批裁决）。
    lp.inbox.enqueue(
        key,
        QueuedMessage {
            msg: inbound(key, "! 先停下"),
            timestamp: String::new(),
        },
    );

    let instance = AgentInstance::new(AgentConfig::default());
    let calls = vec![tool_call("call_1", "tool_a"), tool_call("call_2", "tool_b")];
    instance.add_assistant_message("", calls.clone(), None);

    let ctx = RequestContext::new("web", "c1", "tester", key);
    let mut events = Vec::new();
    let fired = lp.check_steer_skip_batch(&instance, &ctx, &calls, 0, false, &mut events);
    assert!(fired, "steer pending → 必须触发跳批");

    // 历史落账：两个 tool 结果，均为 Skipped 文本，tool_call_id 对位。
    let history = instance.get_history();
    let tool_turns: Vec<_> = history.iter().filter(|t| t.role == "tool").collect();
    assert_eq!(tool_turns.len(), 2);
    for (turn, call) in tool_turns.iter().zip(calls.iter()) {
        assert_eq!(turn.tool_call_id.as_deref(), Some(call.id.as_str()));
        assert_eq!(turn.content, STEER_SKIP_TOOL_RESULT);
    }

    // 事件流：两个 ToolResult 可见（前端卡片能展示跳过原因），非错误。
    assert_eq!(events.len(), 2);
    for ev in &events {
        match ev {
            AgentEvent::ToolResult(r) => {
                assert_eq!(r.result, STEER_SKIP_TOOL_RESULT);
                assert!(!r.is_error);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    // 消息对合法性：repair 对当前历史零改动（无孤儿/无缺答/无重复）。
    let mut projected = history.clone();
    repair_tool_message_pairs(&mut projected);
    assert_eq!(
        projected.len(),
        history.len(),
        "repair 必须零改动——消息对在落账时即完整"
    );

    // skip_cancel_estop=true（U5 并行预计算路径）→ 检查惰性。
    let mut events2 = Vec::new();
    let fired2 = lp.check_steer_skip_batch(&instance, &ctx, &calls, 0, true, &mut events2);
    assert!(!fired2, "并行预计算路径不做批内跳批");
    assert!(events2.is_empty());
    // 该分支不落账（调用数不变）。
    assert_eq!(
        instance
            .get_history()
            .iter()
            .filter(|t| t.role == "tool")
            .count(),
        2
    );
}

// --- 2. 集成面：串行批中注入 steer -----------------------------------------

/// 工具 A 执行中排进 steer → 工具 B 不执行（Skipped 合成）+ steer 文本
/// （剥 `!`）进下一次 LLM 请求 + 请求里消息对完整合法。
#[tokio::test]
async fn steer_mid_serial_batch_skips_remaining_and_injects_next_round() {
    let provider = Arc::new(CapturingProvider::new(vec![scripted_tool_resp(vec![
        tool_call("call_1", "steer_enqueue"),
        tool_call("call_2", "probe_b"),
    ])]));
    let mut lp = steer_mode_loop(Box::new(ArcCapturing(provider.clone())));
    let key = "agent:p19-serial";

    let inbox_handle = lp.inbox.clone();
    let a_executed = Arc::new(AtomicBool::new(false));
    let b_executed = Arc::new(AtomicBool::new(false));
    lp.register_tool(
        "steer_enqueue".to_string(),
        Box::new(SteerEnqueueTool {
            inbox: inbox_handle,
            session_key: key.to_string(),
            steer_content: "! 停一下，先别删".to_string(),
            executed: a_executed.clone(),
        }),
    );
    lp.register_tool(
        "probe_b".to_string(),
        Box::new(ProbeTool {
            marker: "B_RAN".to_string(),
            executed: b_executed.clone(),
        }),
    );

    let (_id, resp, _err) = lp.process_inbound_message(&inbound(key, "do things")).await;
    assert_eq!(resp, "final answer", "跳批后 steer 进下一轮，正常终答");

    // 工具 A 真执行了；工具 B 被跳（从未派发）。
    assert!(a_executed.load(AtomicOrdering::SeqCst), "工具 A 应已执行");
    assert!(
        !b_executed.load(AtomicOrdering::SeqCst),
        "工具 B 必须被跳过（从未派发）"
    );

    // 第二次请求：call_2 = Skipped 文本；steer 文本（剥 `!`）以 user 进场。
    let captured = provider.captured.lock().unwrap();
    assert_eq!(captured.len(), 2, "两次 LLM 调用：批前 + 跳批 steer 注入后");
    let req2 = &captured[1];

    let skipped_result = req2.iter().any(|m| {
        m.role == "tool"
            && m.tool_call_id.as_deref() == Some("call_2")
            && m.content == STEER_SKIP_TOOL_RESULT
    });
    assert!(skipped_result, "call_2 的 tool 结果必须是 Skipped 文本");

    let real_result = req2.iter().any(|m| {
        m.role == "tool"
            && m.tool_call_id.as_deref() == Some("call_1")
            && m.content.contains("STEER_TOOL_OK")
    });
    assert!(real_result, "call_1 的真实结果保留");

    let steer_in = req2
        .iter()
        .any(|m| m.role == "user" && m.content.contains("停一下，先别删"));
    assert!(
        steer_in,
        "steer 文本（剥 ! 前缀）必须进下一次请求的 user 消息"
    );
    let no_marker_leak = req2
        .iter()
        .all(|m| !(m.role == "user" && m.content.starts_with('!')));
    assert!(no_marker_leak, "路由标记 ! 不得泄漏进请求正文");

    // 消息对合法性（provider 序列化面）：assistant 的每个 tool_call id
    // 恰好被一个 tool 消息应答，且 tool 消息都紧跟在其 assistant 之后。
    let assistant_idx = req2
        .iter()
        .position(|m| m.role == "assistant" && m.tool_calls.is_some())
        .expect("req2 必须含带 tool_calls 的 assistant 消息");
    let call_ids: Vec<String> = req2[assistant_idx]
        .tool_calls
        .as_ref()
        .unwrap()
        .iter()
        .map(|tc| tc.id.clone())
        .collect();
    assert_eq!(call_ids.len(), 2);
    let mut answered: Vec<String> = Vec::new();
    for m in req2.iter().skip(assistant_idx + 1) {
        if m.role == "assistant" {
            break;
        }
        if m.role == "tool"
            && let Some(ref id) = m.tool_call_id
        {
            answered.push(id.clone());
        }
    }
    assert_eq!(
        answered.len(),
        call_ids.len(),
        "每个 tool_call 恰好一个结果：{answered:?} vs {call_ids:?}"
    );
    for id in &call_ids {
        assert_eq!(
            answered.iter().filter(|a| *a == id).count(),
            1,
            "call {id} 必须恰好应答一次"
        );
    }

    // 队列已排空（steer 被器官 2 认领消费）。
    assert_eq!(lp.inbox.pending(key), (0, 0));
}

// --- 3. 并行面：U5 预计算批不跳批 ------------------------------------------

/// 全只读双工具批走 U5 并行预计算：批内 steer 到达不跳批（两者都已执行，
/// 真实结果照常回灌），steer 由下一轮器官 2 正常认领。
#[tokio::test]
async fn steer_during_parallel_batch_does_not_skip_completed_work() {
    let provider = Arc::new(CapturingProvider::new(vec![scripted_tool_resp(vec![
        tool_call("call_1", "ro_a"),
        tool_call("call_2", "ro_b"),
    ])]));
    let mut lp = steer_mode_loop(Box::new(ArcCapturing(provider.clone())));
    let key = "agent:p19-parallel";

    let a_executed = Arc::new(AtomicBool::new(false));
    let b_executed = Arc::new(AtomicBool::new(false));
    // 只读工具 A 执行时把 steer 排进 inbox——模拟并行窗口内到达。
    lp.register_tool(
        "ro_a".to_string(),
        Box::new(ReadOnlyProbeTool {
            marker: "A_RAN".to_string(),
            executed: a_executed.clone(),
            inbox: Some(lp.inbox.clone()),
            session_key: key.to_string(),
        }),
    );
    lp.register_tool(
        "ro_b".to_string(),
        Box::new(ReadOnlyProbeTool {
            marker: "B_RAN".to_string(),
            executed: b_executed.clone(),
            inbox: None,
            session_key: key.to_string(),
        }),
    );

    let (_id, resp, _err) = lp
        .process_inbound_message(&inbound(key, "do parallel things"))
        .await;
    assert_eq!(resp, "final answer");

    // 两个工具都真实执行了（并行批不做批内跳批）。
    assert!(a_executed.load(AtomicOrdering::SeqCst));
    assert!(b_executed.load(AtomicOrdering::SeqCst));

    let captured = provider.captured.lock().unwrap();
    assert_eq!(captured.len(), 2);
    let req2 = &captured[1];
    // call_2 的结果是真实 marker，不是 Skipped。
    let real_b = req2.iter().any(|m| {
        m.role == "tool"
            && m.tool_call_id.as_deref() == Some("call_2")
            && m.content.contains("B_RAN")
    });
    assert!(real_b, "并行批的 call_2 必须回真实结果");
    let no_skip = req2
        .iter()
        .all(|m| !(m.role == "tool" && m.content.contains("Skipped due to queued")));
    assert!(no_skip, "并行批不得出现 Skipped 合成结果");
    // steer 照常被下一轮器官 2 认领（剥 `!` 后进场）。
    assert!(
        req2.iter()
            .any(|m| m.role == "user" && m.content.contains("并行窗口内到达"))
    );
    assert_eq!(lp.inbox.pending(key), (0, 0));
}
