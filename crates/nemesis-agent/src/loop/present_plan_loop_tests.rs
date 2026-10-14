//! N2 (2026-10-13 机会项): present_plan 循环级测试。
//!
//! 覆盖：真实 run_with_trace 链路下——批准臂（mode 翻 Build + 循环继续 +
//! 后续轮可达）、驳回臂（结构性停轮：LLM 拿不到下一轮 + Done 文案 +
//! mode 仍 Plan + verdict 消费后清空）、超时臂（同驳回停轮语义）、
//! 供给双闸（build 隐藏 / plan 白名单供给）、PLAN_MODE_TOOLS 白名单收录。
//!
//! 自带迷你 fixture（本仓测试文件独立惯例，模块私有不可共享）。

use super::*;
use async_trait::async_trait;
use std::sync::Arc as StdArc;

use crate::loop_tools::{
    PLAN_PRESENT_OPTION_APPROVE, PLAN_PRESENT_OPTION_REJECT, PlanPresentHandle,
};
use nemesis_types::agent::{QuestionAsker, QuestionOutcome, QuestionRequest};

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

/// 固定回放脚本 provider：逐条回放响应，记录调用次数。
struct ScriptedProvider {
    responses: std::sync::Mutex<std::vec::IntoIter<LlmResponse>>,
    calls: StdArc<std::sync::Mutex<usize>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<LlmResponse>, calls: StdArc<std::sync::Mutex<usize>>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses.into_iter()),
            calls,
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        *self.calls.lock().unwrap() += 1;
        self.responses
            .lock()
            .unwrap()
            .next()
            .ok_or_else(|| "script exhausted".to_string())
    }
}

fn text_response(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        finish_reason: None,
        escalated_to: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn present_plan_call_response() -> LlmResponse {
    LlmResponse {
        content: String::new(),
        tool_calls: vec![ToolCallInfo {
            id: "pp-1".to_string(),
            name: "present_plan".to_string(),
            arguments: r#"{"plan": "1. step one\n2. step two", "title": "测试计划"}"#.to_string(),
        }],
        finished: false,
        reasoning_content: None,
        usage: None,
        finish_reason: None,
        escalated_to: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 回放预设结局的 mock broker。
struct MockBroker {
    outcome: QuestionOutcome,
}

impl QuestionAsker for MockBroker {
    fn ask(&self, _request: QuestionRequest) -> Result<QuestionOutcome, String> {
        Ok(self.outcome.clone())
    }
}

fn pp_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 6,
        tools: vec!["present_plan".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 装配：plan 模式 loop + present_plan 工具（handle 双侧注入）+ 事件接收端。
struct PpHarness {
    loop_handle: AgentLoop,
    shared: StdArc<PlanPresentHandle>,
    events: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    provider_calls: StdArc<std::sync::Mutex<usize>>,
}

async fn pp_harness(broker_outcome: QuestionOutcome, after: Vec<LlmResponse>) -> PpHarness {
    let (etx, erx) = tokio::sync::broadcast::channel(32);
    let provider_calls: StdArc<std::sync::Mutex<usize>> = StdArc::new(std::sync::Mutex::new(0));
    let mut responses = vec![present_plan_call_response()];
    responses.extend(after);
    let provider = ScriptedProvider::new(responses, provider_calls.clone());
    let config = pp_config();
    let mut al = AgentLoop::new(Box::new(provider), config.clone());

    let shared: StdArc<PlanPresentHandle> = StdArc::new(PlanPresentHandle {
        broker: StdArc::new(parking_lot::RwLock::new(Some(StdArc::new(MockBroker {
            outcome: broker_outcome,
        })
            as StdArc<dyn QuestionAsker>))),
        mode: al.mode_arc(),
        agent_event_tx: Some(etx),
        verdicts: StdArc::new(parking_lot::Mutex::new(std::collections::HashMap::new())),
    });
    al.set_plan_present_handle(shared.clone());
    al.register_tool(
        "present_plan".to_string(),
        Box::new(crate::loop_tools::PlanPresentTool::new(shared.clone())),
    );
    al.set_mode_with_event(
        crate::types::AgentMode::Plan,
        "agent:main:session:pp",
        "web:pp",
    );

    PpHarness {
        loop_handle: al,
        shared,
        events: erx,
        provider_calls,
    }
}

async fn run_turn(h: &PpHarness) -> Vec<AgentEvent> {
    let config = pp_config();
    let instance = AgentInstance::new(config);
    let context = RequestContext::new("web", "pp-chat", "pp-user", "agent:main:session:pp");
    let token = tokio_util::sync::CancellationToken::new();
    h.loop_handle
        .run_with_trace(
            &instance,
            "please plan",
            &context,
            "pp-trace",
            false,
            &token,
            None,
            &[],
        )
        .await
}

fn done_text(events: &[crate::types::AgentEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        AgentEvent::Done(msg) => Some(msg.clone()),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// 三臂循环级
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approve_continues_loop_and_flips_build() {
    let mut h = pp_harness(
        QuestionOutcome::Answered(vec![PLAN_PRESENT_OPTION_APPROVE.to_string()]),
        vec![text_response("PLAN_APPROVED_FINAL")],
    )
    .await;
    let events = run_turn(&h).await;

    // 循环继续：第二轮 LLM 可达并产出终稿。
    assert_eq!(*h.provider_calls.lock().unwrap(), 2, "批准后循环必须继续");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m.contains("PLAN_APPROVED_FINAL"))),
        "终稿必须发布: {events:?}"
    );
    // mode 已翻 build。
    assert_eq!(h.loop_handle.mode(), crate::types::AgentMode::Build);
    // verdict 已被批次尾消费（remove 清空）。
    assert!(
        h.shared
            .verdicts
            .lock()
            .get("agent:main:session:pp")
            .is_none()
    );
    // ModeChanged 广播（build）。
    let mut saw_build = false;
    while let Ok(ev) = h.events.try_recv() {
        if let nemesis_types::agent::AgentEvent::ModeChanged { mode, .. } = ev {
            saw_build |= mode == "build";
        }
    }
    assert!(saw_build, "批准必须广播 build ModeChanged");
}

#[tokio::test]
async fn reject_stops_turn_structurally() {
    let h = pp_harness(
        QuestionOutcome::Answered(vec![PLAN_PRESENT_OPTION_REJECT.to_string()]),
        // 若停轮失效模型会拿到下一轮——这条脚本故意只有一条且内容可识别。
        vec![text_response("SHOULD_NOT_BE_REACHED")],
    )
    .await;
    let events = run_turn(&h).await;

    // 结构性停轮：LLM 只被调用一次（工具轮后不再有下一轮）。
    assert_eq!(
        *h.provider_calls.lock().unwrap(),
        1,
        "驳回后必须结构性停轮（LLM 不得再被调用）"
    );
    let done = done_text(&events).expect("停轮必须有 Done");
    assert!(done.contains("❌") && done.contains("驳回"), "done: {done}");
    // mode 不动（仍 plan）。
    assert_eq!(h.loop_handle.mode(), crate::types::AgentMode::Plan);
    // verdict 消费后清空（不留跨轮脏态）。
    assert!(
        h.shared
            .verdicts
            .lock()
            .get("agent:main:session:pp")
            .is_none()
    );
}

#[tokio::test]
async fn timeout_stops_turn_like_reject() {
    let h = pp_harness(
        QuestionOutcome::Timeout,
        vec![text_response("SHOULD_NOT_BE_REACHED")],
    )
    .await;
    let events = run_turn(&h).await;

    assert_eq!(*h.provider_calls.lock().unwrap(), 1, "超时同驳回停轮");
    let done = done_text(&events).expect("停轮必须有 Done");
    assert!(done.contains("⏱") && done.contains("超时"), "done: {done}");
    assert_eq!(h.loop_handle.mode(), crate::types::AgentMode::Plan);
    assert!(
        h.shared
            .verdicts
            .lock()
            .get("agent:main:session:pp")
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// 供给双闸
// ---------------------------------------------------------------------------

#[tokio::test]
async fn supply_gate_hides_in_build_and_shows_in_plan() {
    let h = pp_harness(QuestionOutcome::Timeout, vec![]).await;
    // 初始 plan：供给。
    let names: Vec<String> = h
        .loop_handle
        .build_tool_defs()
        .iter()
        .map(|d| d.function.name.clone())
        .collect();
    assert!(
        names.contains(&"present_plan".to_string()),
        "plan 模式必须供给 present_plan: {names:?}"
    );

    // build：隐藏。
    h.loop_handle.set_mode_with_event(
        crate::types::AgentMode::Build,
        "agent:main:session:pp",
        "web:pp",
    );
    let names: Vec<String> = h
        .loop_handle
        .build_tool_defs()
        .iter()
        .map(|d| d.function.name.clone())
        .collect();
    assert!(
        !names.contains(&"present_plan".to_string()),
        "build 模式必须隐藏 present_plan: {names:?}"
    );
}

#[test]
fn plan_mode_whitelist_contains_present_plan() {
    // 供给白名单收录（否则 plan 模式下该工具被白名单过滤掉）。
    assert!(
        AgentLoop::PLAN_MODE_TOOLS.contains(&"present_plan"),
        "PLAN_MODE_TOOLS 必须收录 present_plan"
    );
}
