//! N2 (2026-10-13 机会项): present_plan 计划审批工具单元测试。
//!
//! 覆盖：注册门控（handle Some→注册 / None→不注册）、schema（plan 必填 /
//! title 可选）、批准臂（mode 翻 Build + ModeChanged 事件 + verdict 落
//! Approved）、驳回臂（verdict 落 Rejected，mode 不动）、超时臂（verdict
//! 落 Timeout）、broker 槽空诚实报错、build 模式直呼拒绝、缺 plan 字段
//! 校验、verdict take 单次消费。

use super::{
    PlanPresentHandle, PlanPresentTool, QuestionBrokerSlot, SharedToolConfig, register_shared_tools,
};
use crate::context::RequestContext;
use crate::r#loop::Tool;
use nemesis_types::agent::{QuestionAsker, QuestionOutcome, QuestionRequest};
use std::sync::Arc;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "agent:main:session:pp")
}

fn empty_slot() -> QuestionBrokerSlot {
    Arc::new(parking_lot::RwLock::new(None))
}

fn slot_with(broker: Arc<dyn QuestionAsker>) -> QuestionBrokerSlot {
    Arc::new(parking_lot::RwLock::new(Some(broker)))
}

/// 回放预设结局的 mock broker（question_tool_tests 同款形态）。
struct MockBroker {
    outcome: QuestionOutcome,
    seen: parking_lot::Mutex<Vec<QuestionRequest>>,
}

impl MockBroker {
    fn answered(items: &[&str]) -> Self {
        Self {
            outcome: QuestionOutcome::Answered(items.iter().map(|s| s.to_string()).collect()),
            seen: parking_lot::Mutex::new(Vec::new()),
        }
    }
    fn timeout() -> Self {
        Self {
            outcome: QuestionOutcome::Timeout,
            seen: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl QuestionAsker for MockBroker {
    fn ask(&self, request: QuestionRequest) -> Result<QuestionOutcome, String> {
        self.seen.lock().push(request);
        Ok(self.outcome.clone())
    }
}

/// 装配一个 plan 模式句柄（mode 初始 Plan / verdict 空）；返回句柄与
/// ModeChanged 事件的接收端（徽标广播断言用）。
fn handle_with(
    broker: QuestionBrokerSlot,
) -> (
    Arc<PlanPresentHandle>,
    tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) {
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    let handle = Arc::new(PlanPresentHandle {
        broker,
        mode: Arc::new(parking_lot::RwLock::new(crate::types::AgentMode::Plan)),
        agent_event_tx: Some(tx),
        verdict: Arc::new(parking_lot::Mutex::new(None)),
    });
    (handle, rx)
}

// ---------------------------------------------------------------------------
// 注册门控
// ---------------------------------------------------------------------------

#[test]
fn register_with_handle_inserts_present_plan() {
    let cfg = SharedToolConfig {
        plan_present: Some(handle_with(empty_slot()).0),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(
        tools.contains_key("present_plan"),
        "handle wired → present_plan registered"
    );
}

#[test]
fn register_without_handle_omits_present_plan() {
    let cfg = SharedToolConfig::default();
    let tools = register_shared_tools(&cfg);
    assert!(!tools.contains_key("present_plan"));
}

// ---------------------------------------------------------------------------
// schema
// ---------------------------------------------------------------------------

#[test]
fn schema_requires_plan_title_optional() {
    let tool = PlanPresentTool::new(handle_with(empty_slot()).0);
    let schema = tool.parameters();
    assert_eq!(schema["required"][0], "plan");
    assert_eq!(schema["properties"]["plan"]["type"], "string");
    assert_eq!(schema["properties"]["title"]["type"], "string");
    let desc = tool.description();
    assert!(
        desc.contains("plan mode") && desc.to_lowercase().contains("approval"),
        "description 应说明 plan 模式审批语义: {desc}"
    );
}

// ---------------------------------------------------------------------------
// execute：三臂 + 校验
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approve_flips_mode_and_sets_verdict() {
    let (handle, mut rx) = handle_with(slot_with(Arc::new(MockBroker::answered(&[
        super::PLAN_PRESENT_OPTION_APPROVE,
    ]))));
    let tool = PlanPresentTool::new(handle.clone());
    let out = tool
        .execute(r#"{"plan": "1. do x\n2. do y"}"#, &ctx())
        .await
        .expect("approve 臂必须 Ok");
    assert!(out.contains("✅"), "批准回灌: {out}");
    assert_eq!(*handle.mode.read(), crate::types::AgentMode::Build);
    assert_eq!(
        *handle.verdict.lock(),
        Some(super::PlanPresentVerdict::Approved)
    );
    let ev = rx.recv().await.expect("ModeChanged 必须广播");
    match &ev {
        nemesis_types::agent::AgentEvent::ModeChanged { mode, .. } => {
            assert_eq!(mode, "build", "批准必须广播 build 翻转: {mode}");
        }
        other => panic!("必须是 ModeChanged，实际 {other:?}"),
    }
}

#[tokio::test]
async fn reject_sets_verdict_and_keeps_plan_mode() {
    let (handle, _rx) = handle_with(slot_with(Arc::new(MockBroker::answered(&[
        super::PLAN_PRESENT_OPTION_REJECT,
    ]))));
    let tool = PlanPresentTool::new(handle.clone());
    let out = tool
        .execute(r#"{"plan": "draft"}"#, &ctx())
        .await
        .expect("驳回臂也 Ok（verdict 走批次尾消费）");
    assert!(out.contains("❌"), "驳回回灌: {out}");
    assert_eq!(*handle.mode.read(), crate::types::AgentMode::Plan);
    assert_eq!(
        *handle.verdict.lock(),
        Some(super::PlanPresentVerdict::Rejected)
    );
}

#[tokio::test]
async fn timeout_sets_timeout_verdict() {
    let (handle, _rx) = handle_with(slot_with(Arc::new(MockBroker::timeout())));
    let tool = PlanPresentTool::new(handle.clone());
    let out = tool
        .execute(r#"{"plan": "draft"}"#, &ctx())
        .await
        .expect("超时臂 Ok");
    assert!(out.contains("⏱"), "超时回灌: {out}");
    assert_eq!(*handle.mode.read(), crate::types::AgentMode::Plan);
    assert_eq!(
        *handle.verdict.lock(),
        Some(super::PlanPresentVerdict::Timeout)
    );
}

#[tokio::test]
async fn empty_broker_is_honest_error() {
    let (handle, _rx) = handle_with(empty_slot());
    let tool = PlanPresentTool::new(handle);
    let err = tool
        .execute(r#"{"plan": "draft"}"#, &ctx())
        .await
        .expect_err("无 broker 必须诚实报错");
    assert!(err.contains("present_plan unavailable"), "got: {err}");
}

#[tokio::test]
async fn build_mode_call_is_rejected() {
    let (handle, _rx) = handle_with(slot_with(Arc::new(MockBroker::timeout())));
    *handle.mode.write() = crate::types::AgentMode::Build;
    let tool = PlanPresentTool::new(handle);
    let err = tool
        .execute(r#"{"plan": "draft"}"#, &ctx())
        .await
        .expect_err("build 模式直呼必须拒绝");
    assert!(err.contains("already in build mode"), "got: {err}");
}

#[tokio::test]
async fn missing_plan_field_is_validation_error() {
    let (handle, _rx) = handle_with(slot_with(Arc::new(MockBroker::timeout())));
    let tool = PlanPresentTool::new(handle);
    let err = tool
        .execute(r#"{"title": "only title"}"#, &ctx())
        .await
        .expect_err("缺 plan 必须校验失败");
    assert!(err.contains("plan"), "got: {err}");
    let err2 = tool
        .execute("not json", &ctx())
        .await
        .expect_err("非 JSON 必须校验失败");
    assert!(err2.contains("Invalid JSON"), "got: {err2}");
}

#[test]
fn verdict_take_is_single_consumption() {
    let (handle, _rx) = handle_with(empty_slot());
    *handle.verdict.lock() = Some(super::PlanPresentVerdict::Rejected);
    assert_eq!(
        handle.verdict.lock().take(),
        Some(super::PlanPresentVerdict::Rejected)
    );
    assert_eq!(handle.verdict.lock().take(), None, "take 后槽必须清空");
}
