//! 凭据免打扰 auto 臂（2026-11-23 ABCD 批 A）：dispatch 凭据闸三臂矩阵。
//!
//! 判定链（tool_dispatch.rs 凭据闸块）：出域硬拒 → 域内预授权 →
//! **auto 臂（凭据开关 ∨ 全局免打扰，任一开即免卡直放，前置于审批通道
//! 判定——无人值守/headless 同样生效）→ 冷却窗 → 人工卡**。本文件钉：
//! ① 凭据开关开 → 不出卡、工具照常执行、真值注入照常、统计 auto 打点
//!   （source=credential_switch）——审批通道假件计数恒 0 证明卡路径
//!   未被触达；
//! ② 凭据开关未注入 + 全局免打扰开 → 同样免卡（approvals_dnd 臂；
//!   无统计载体 = 不打点，诚实边界）；
//! ③ 两开关全关 → 走人工卡路径（审批管理器计数 =1），批准后照常执行
//!   ——auto 臂缺席时行为与落地前零漂移。
//!
//! fixture 自持（兄弟测试模块不互 import）；全局 vault resolver 触槽
//! 互斥复用 VAULT_SLOT_LOCK（BUG-20261110-049 家规）。无 config.vault.json
//! = NoBindings → 抵达卡臂分支（域绑定臂由 credential_domain_tests 钉）。

use super::credential_injection_tests::{MockLlmProvider, VAULT_SLOT_LOCK, tool_call_response};
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const SECRET: &str = "GATE-AUTO-TEST-SECRET-DO-NOT-LOG";

fn auto_test_config(tools: &[&str]) -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: tools.iter().map(|t| t.to_string()).collect(),
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 捕获 execute 实参的工具（credential 槽，字符串形态）。
struct CapturingCredTool {
    captured: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl Tool for CapturingCredTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        self.captured.lock().unwrap().push(args.to_string());
        Ok("executed".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "target": {"type": "string"},
                "credential": {"type": "string"}
            }
        })
    }
    fn credential_arg_keys(&self) -> &[&str] {
        &["credential"]
    }
}

fn resolver_for(alias: &str) {
    let alias = alias.to_string();
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(move |a| {
        if a == alias {
            Ok(SECRET.to_string())
        } else {
            Err(format!("别名不存在: {a}"))
        }
    }));
}

fn done_response() -> LlmResponse {
    LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: "done".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn events_text(events: &[AgentEvent]) -> String {
    let mut dump = String::new();
    for e in events {
        match e {
            AgentEvent::ToolResult(r) => dump.push_str(&r.result),
            AgentEvent::Message(m) | AgentEvent::Error(m) | AgentEvent::Done(m) => dump.push_str(m),
            AgentEvent::ToolCall(_) => {}
        }
        dump.push('\n');
    }
    dump
}

/// 出卡计数假件：每次凭据卡请求 +1，裁决恒批准（若 auto 臂误失效，
/// 计数 > 0 = 走了卡路径，断言红）。
struct CountingCardManager(Arc<AtomicU64>);

impl nemesis_security::auditor::ApprovalManager for CountingCardManager {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(nemesis_security::auditor::ApprovalVerdict::approved())
    }
}

/// 规则层全关 + default allow 的插件（凭据闸消费其 auditor 出卡通道）。
fn gate_plugin() -> Arc<nemesis_security::pipeline::SecurityPlugin> {
    Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            command_guard_enabled: false,
            injection_enabled: false,
            credential_enabled: false,
            dlp_enabled: false,
            ssrf_enabled: false,
            default_action: "allow".to_string(),
            ..Default::default()
        },
    ))
}

/// 装配 + 驱动一轮：无 config.vault.json（NoBindings）→ 卡臂分支；
/// 返回 (事件流文本, 捕获实参, 出卡计数, tempdir 句柄)。
async fn drive_round(
    cred_auto: Option<Arc<nemesis_security::credential_auto::CredentialAutoApproveState>>,
    dnd_enabled: bool,
) -> (
    String,
    Arc<Mutex<Vec<String>>>,
    Arc<AtomicU64>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), r#"{"agents": {}}"#).unwrap();

    let plugin = gate_plugin();
    let card_count = Arc::new(AtomicU64::new(0));
    plugin
        .auditor()
        .set_approval_manager(Arc::new(CountingCardManager(card_count.clone())));
    if dnd_enabled {
        plugin.auditor().set_approval_auto_approve(Arc::new(
            nemesis_security::approval_auto::ApprovalAutoApproveState::new(true),
        ));
    }

    let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let mut agent_loop = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![
            tool_call_response("credtool", r#"{"credential":"vault:corp"}"#),
            done_response(),
        ])),
        auto_test_config(&["credtool"]),
    );
    agent_loop.register_tool(
        "credtool".to_string(),
        Box::new(CapturingCredTool {
            captured: captured.clone(),
        }),
    );
    agent_loop.set_security_plugin(plugin);
    agent_loop.set_config_path(dir.path().join("config.json"));
    if let Some(st) = cred_auto {
        agent_loop.set_credential_auto_approve(st);
    }
    let instance = AgentInstance::new(auto_test_config(&["credtool"]));
    let context = RequestContext::new("web", "chat1", "user1", "session1");
    let events = agent_loop.run(&instance, "go", &context).await;
    (events_text(&events), captured, card_count, dir)
}

/// ① 凭据开关开 → 免卡直放：工具执行 + 真值注入 + 统计 auto 打点
/// （source=credential_switch）；出卡计数恒 0。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn credential_switch_on_auto_approves_without_card() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let stats = Arc::new(nemesis_security::credential_auto::CredentialAutoApproveState::new(true));

    let (dump, captured, cards, _dir) = drive_round(Some(stats.clone()), false).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "免卡直放必须照常执行");
    assert!(cap[0].contains(SECRET), "真值注入照常: {:?}", cap[0]);
    drop(cap);
    assert_eq!(
        cards.load(Ordering::SeqCst),
        0,
        "auto 臂必须先于卡路径（审批通道在场也不出卡）"
    );
    assert!(
        !dump.contains("USER REJECTED"),
        "免卡形态不得有拒绝串: {dump}"
    );
    let (auto, _pre, _den) = stats.counts();
    assert_eq!(auto, 1, "auto 计数打点");
    let recs = stats.records_snapshot();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].outcome, "auto");
    assert_eq!(recs[0].source, "credential_switch");
    assert_eq!(recs[0].tool, "credtool");
    assert_eq!(recs[0].aliases, "corp");
    assert!(
        !serde_json::to_string(&recs[0]).unwrap().contains(SECRET),
        "统计记录不得携带真值"
    );
}

/// ② 凭据开关未注入 + 全局免打扰开 → approvals_dnd 臂免卡（原始投诉
/// 形态的根治面：只开全局免打扰也覆盖凭据卡）。无统计载体 = 不打点。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn global_dnd_alone_auto_approves_credential_card() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");

    let (dump, captured, cards, _dir) = drive_round(None, true).await;
    nemesis_config::clear_global_vault_resolver();

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "全局免打扰下凭据卡必须免卡直放");
    assert!(cap[0].contains(SECRET));
    drop(cap);
    assert_eq!(cards.load(Ordering::SeqCst), 0, "不出卡");
    assert!(!dump.contains("USER REJECTED"), "{dump}");
}

/// ③ 两开关全关 → 人工卡路径照常（批准后执行；出卡计数 =1）——auto
/// 臂缺席零漂移锚。
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn both_switches_off_takes_card_path() {
    let _g = VAULT_SLOT_LOCK.lock();
    resolver_for("corp");
    let stats = Arc::new(nemesis_security::credential_auto::CredentialAutoApproveState::new(false));

    let (dump, captured, cards, _dir) = drive_round(Some(stats.clone()), false).await;
    nemesis_config::clear_global_vault_resolver();

    assert_eq!(
        cards.load(Ordering::SeqCst),
        1,
        "开关全关必须走人工卡路径（假件批准 → 执行）"
    );
    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1, "卡批准后照常执行");
    assert!(cap[0].contains(SECRET));
    drop(cap);
    assert!(!dump.contains("USER REJECTED"), "批准形态无拒绝串: {dump}");
    let (auto, _pre, _den) = stats.counts();
    assert_eq!(auto, 0, "人工批准臂不打 auto 计数");
    let recs = stats.records_snapshot();
    assert_eq!(recs.len(), 1, "人工批准臂打 approved 记录");
    assert_eq!(recs[0].outcome, "approved");
    assert_eq!(recs[0].source, "user");
}
