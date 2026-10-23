//! §2 免打扰（2026-10-11 无人值守计划 Part 2）：wiring 注入/判定回环 +
//! check_escalation 消费面。nemesis-security 是本 crate 的 optional
//! 依赖（security feature）——整个文件随 feature 消失。
//!
//! 循环干预阶梯（2026-10-24）：`check_escalation` 的免打扰直批分支已删
//! ——escalation 终级改走无超时终级卡（`ask_doom_loop_terminal`），
//! bypass_auto_approve 任何自动化开关都压不掉（用户裁决第 5 点：烟雾
//! 报警器必须响）。本文件保留三个方向的钉子：
//! ① 免打扰开 ≠ escalation 被自动批准（终级卡照发/无 asker 照停车）；
//! ② `agents.doom_loop_approval` config 闸不再被 check_escalation 消费
//!    （J5 保留件的生产消费点已移除，闸留解析不留行为）；
//! ③ 阶梯 nudge L1/L2/L3 文案随累计次数升级（L0 原文案钉在
//!    turn_guard/tests.rs）。

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
        tool_fence_profession: None,
    }
}

/// 把 loop 的 config_path 钉到临时 config.json（`doom_loop_approval` 键
/// fresh-read 消费——2026-10-24 起仅 config_watch 解析器保留，无生产消费；
/// 本文件用它验证「闸不被 check_escalation 消费」）。
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
async fn escalation_auto_approve_does_not_bypass_terminal_card() {
    // 终极卡不可自动批（用户裁决第 5 点）：免打扰开 + asker 未装配 →
    // 问不到人 fail-closed 停车（旧行为「免打扰开=自动批准继续」已删）。
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    let state =
        std::sync::Arc::new(nemesis_security::approval_auto::ApprovalAutoApproveState::new(true));
    agent_loop.set_approval_auto_approve(state);

    let ctx = RequestContext::new("cluster", "chat1", "user1", "session1");
    let mut guard = escalating_guard();
    let verdict = agent_loop.check_escalation(&mut guard, &ctx).await;
    assert!(
        matches!(verdict, Some(AgentEvent::Done(_))),
        "免打扰开也压不掉终级卡：无 asker = 停车（不自动批准）"
    );
    assert!(
        guard.escalating_signature().is_some(),
        "停车 = 签名保留（无「清签名继续」分支）"
    );
}

#[tokio::test]
async fn escalation_ignores_doom_loop_config_gate() {
    // J5 不再被消费：`agents.doom_loop_approval=false` 的 config 闸对
    // 终级卡无效（阶梯默认全会话生效，用户裁决第 7 点）——配置关 +
    // asker 未装配 = 照旧停车，绝不因「闸关」短路出第二条路径。
    let dir = tempfile::tempdir().unwrap();
    pin_config_with_doom_approval(&dir, false);
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider), test_config());
    agent_loop.set_config_path(dir.path().join("config.json"));

    let ctx = RequestContext::new("cluster", "chat1", "user1", "session1");
    let mut guard = escalating_guard();
    let verdict = agent_loop.check_escalation(&mut guard, &ctx).await;
    assert!(
        matches!(verdict, Some(AgentEvent::Done(_))),
        "doom_loop_approval=false 不影响终级卡路径（fail-closed 停车）"
    );
}

/// 阶梯 nudge 随累计次数升级：同一签名 6 次=L1（换方法）、12 次=L2
/// （换路径/分解）、18 次=L3（绕过+记录缺口）——record_tool_outcome
/// 返回的 nudge 文案逐级换血（L0 原文案断言在 turn_guard/tests.rs）。
/// 隔次插入其他工具的成功破开 ④ 连败链（storm 文案优先级更高），让
/// ⑥ 累计计数的阶梯文案显形。
#[test]
fn escalation_nudge_ladder_text_advances_with_count() {
    let err = "Error: boom";
    let nudge_at = |count: u32| {
        let mut g = crate::turn_guard::TurnGuard::new();
        for _ in 0..(count - 1) {
            g.record_tool_outcome("exec", Some(err));
            g.record_tool_outcome("edit_file", None);
        }
        g.record_tool_outcome("exec", Some(err))
            .expect("nudge fires from 3")
    };
    assert!(
        nudge_at(6).contains("换一个工具、换一条命令或换一种完全不同的实现方式"),
        "6 次 = L1 换方法"
    );
    assert!(
        nudge_at(12).contains("换一条根本不同的实现路径，或把任务分解成更小的子步骤"),
        "12 次 = L2 换路径/分解"
    );
    let l3 = nudge_at(18);
    assert!(
        l3.contains("绕过该阻塞点完成其余能完成的工作"),
        "18 次 = L3 绕过"
    );
    assert!(l3.contains("不要再次重试同一调用"), "L3 禁止重试");
}
