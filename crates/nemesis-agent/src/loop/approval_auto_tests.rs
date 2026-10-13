//! §2 免打扰（2026-10-11 无人值守计划 Part 2）：wiring 注入/判定回环 +
//! check_escalation 自动批准臂。nemesis-security 是本 crate 的 optional
//! 依赖（security feature）——整个文件随 feature 消失。

use super::*;
use async_trait::async_trait;

// ---------- 本地 mock（与 loop/tests.rs 同构，模块私有不可共享） ----------

struct MockLlmProvider;

#[async_trait]
impl LlmProvider for MockLlmProvider {
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

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

/// 把 loop 的 config_path 钉到临时 config.json（`doom_loop_approval` 键
/// fresh-read 消费——读不到/键缺失按关处理）。
fn pin_config_with_doom_approval(dir: &tempfile::TempDir, enabled: bool) {
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::json!({ "agents": { "doom_loop_approval": enabled } }).to_string(),
    )
    .unwrap();
}

/// 驱动 TurnGuard 到硬停阈值（同签名连败 ≥ ALTERNATING_LOOP_HARD_STOP）。
fn escalating_guard() -> crate::turn_guard::TurnGuard {
    let mut g = crate::turn_guard::TurnGuard::new();
    for _ in 0..crate::turn_guard::ALTERNATING_LOOP_HARD_STOP {
        g.record_tool_outcome("exec", Some("Error: boom"));
    }
    assert!(
        g.escalating_signature().is_some(),
        "setup: guard must be at hard-stop"
    );
    g
}

#[test]
fn wiring_round_trip_defaults_off() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    assert!(
        !agent_loop.approval_auto_enabled(),
        "未注入 = 关（现行为零变化）"
    );
    let state =
        std::sync::Arc::new(nemesis_security::approval_auto::ApprovalAutoApproveState::new(false));
    agent_loop.set_approval_auto_approve(state.clone());
    assert!(!agent_loop.approval_auto_enabled(), "注入但未开 = 关");
    state.set_enabled(true);
    assert!(
        agent_loop.approval_auto_enabled(),
        "热开即时判定（同一 Arc 双端可见）"
    );
}

#[tokio::test]
async fn escalation_auto_approve_clears_signature_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    pin_config_with_doom_approval(&dir, true);
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.set_config_path(dir.path().join("config.json"));
    let state =
        std::sync::Arc::new(nemesis_security::approval_auto::ApprovalAutoApproveState::new(true));
    agent_loop.set_approval_auto_approve(state);

    let ctx = RequestContext::new("cluster", "chat1", "user1", "session1");
    let mut guard = escalating_guard();
    let verdict = agent_loop.check_escalation(&mut guard, &ctx).await;
    assert!(
        verdict.is_none(),
        "免打扰开：escalation 必须自动批准继续（不发卡不停车）"
    );
    assert!(
        guard.escalating_signature().is_none(),
        "批准 = 清签名计数（继续跑）"
    );
}

#[tokio::test]
async fn escalation_auto_off_still_stops_without_asker() {
    let dir = tempfile::tempdir().unwrap();
    pin_config_with_doom_approval(&dir, true);
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.set_config_path(dir.path().join("config.json"));
    // 注入但关闭（+ asker 未装配）：ask 通路 unwrap_or(false) → 停轮现行为。
    let state =
        std::sync::Arc::new(nemesis_security::approval_auto::ApprovalAutoApproveState::new(false));
    agent_loop.set_approval_auto_approve(state);

    let ctx = RequestContext::new("cluster", "chat1", "user1", "session1");
    let mut guard = escalating_guard();
    let verdict = agent_loop.check_escalation(&mut guard, &ctx).await;
    assert!(
        matches!(verdict, Some(AgentEvent::Done(_))),
        "免打扰关：escalation 照旧停轮"
    );
}

#[tokio::test]
async fn escalation_auto_approve_respects_doom_loop_config_gate() {
    // 极性钉死：自动批准不越过 `agents.doom_loop_approval` 配置闸——
    // 配置关 = escalation 一律停轮（审批化整体未启用）。
    let dir = tempfile::tempdir().unwrap();
    pin_config_with_doom_approval(&dir, false);
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.set_config_path(dir.path().join("config.json"));
    let state =
        std::sync::Arc::new(nemesis_security::approval_auto::ApprovalAutoApproveState::new(true));
    agent_loop.set_approval_auto_approve(state);

    let ctx = RequestContext::new("cluster", "chat1", "user1", "session1");
    let mut guard = escalating_guard();
    let verdict = agent_loop.check_escalation(&mut guard, &ctx).await;
    assert!(
        matches!(verdict, Some(AgentEvent::Done(_))),
        "doom_loop_approval=false 时免打扰不得放行"
    );
}
