//! tool_dispatch.rs 安全管线后续层覆盖（Wave6B）：limits 超限审批直通车
//! 三分支（批准放行 / 用户拒绝 / 无通道 fail-closed）+ guardian 语义二审
//! 的升级与故障姿态矩阵（flagged×审批三分支 / judge Err × failure_policy
//! allow·ask·deny）。judge allow / mode-off / flagged 无通道三臂已由
//! loop/tests.rs 的 guardian e2e 钉死，本文件不重复。
//!
//! fixture 自带（兄弟测试模块不能互相 import）。安全层涉及
//! `tokio::task::block_in_place`，一律 `multi_thread` flavor。

#![cfg(feature = "security")]

use super::*;
use crate::test_support::capture_logs;

// ---------------------------------------------------------------------------
// 迷你 fixture
// ---------------------------------------------------------------------------

struct SecCovProvider;

#[async_trait]
impl LlmProvider for SecCovProvider {
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

/// 固定 Ok 文本的工具。
struct OkTool(&'static str);

#[async_trait]
impl Tool for OkTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(self.0.to_string())
    }
}

/// 声明限额类别的工具（类别由构造注入——静态共享计数需隔离类别名）。
struct LimitedTool(&'static str);

#[async_trait]
impl Tool for LimitedTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("limited_ok".to_string())
    }
    fn limit_categories(&self) -> &[&str] {
        std::slice::from_ref(&self.0)
    }
}

fn sec_call(name: &str, args: &str) -> ToolCallInfo {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    ToolCallInfo {
        id: format!("seccov-{n}"),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

fn sec_ctx() -> RequestContext {
    RequestContext::new("web", "chat-1", "covuser", "agent:main:session:seccov")
}

fn sec_loop() -> AgentLoop {
    AgentLoop::new(
        Box::new(SecCovProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: None,
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    )
}

/// 全关规则层 + default allow 的插件（guardian/limits 是唯一活动闸）。
#[cfg(feature = "security")]
fn sec_plugin() -> Arc<nemesis_security::pipeline::SecurityPlugin> {
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

// ---------------------------------------------------------------------------
// limits 超限审批直通车
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
mod limits_cov {
    use super::*;

    /// limits 静态规则/计数表跨测试共享——整组串行 + 每测后清场。
    /// tokio Mutex：guard 需要跨 handle_tool_call 的 await 持有（clippy
    /// await_holding_lock 只认 std 版）。
    static LIMITS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn set_rule(category: &str, max: u32) {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            category.to_string(),
            super::super::limits::LimitRule {
                max,
                window_secs: 60,
            },
        );
        super::super::limits::set_rules(m);
    }

    /// 超限 → 审批批准 → 计数 + 放行执行（Ok(approved) 臂）。
    #[cfg(feature = "security")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn limit_overrun_approved_by_user_executes() {
        let _logs = capture_logs();
        let _guard = LIMITS_LOCK.lock().await;
        let category: &'static str = "cov_lim_approved";
        set_rule(category, 1);

        let plugin = sec_plugin();
        plugin
            .auditor()
            .set_approval_manager(Arc::new(super::FakeApprovalManager::approved()));
        let mut agent_loop = sec_loop();
        agent_loop.set_security_plugin(plugin);
        agent_loop.register_tool("cov_limited_a".to_string(), Box::new(LimitedTool(category)));
        let ctx = sec_ctx();

        // 第 1 次：限内直接放行（并计数到 max）。
        let out = agent_loop
            .handle_tool_call(&sec_call("cov_limited_a", "{}"), &ctx)
            .await;
        assert_eq!(out, "limited_ok", "限内首调必须直通: {out}");

        // 第 2 次：超限 → 审批批准 → 放行执行。
        let out = agent_loop
            .handle_tool_call(&sec_call("cov_limited_a", "{}"), &ctx)
            .await;
        assert_eq!(out, "limited_ok", "批准后必须放行执行: {out}");

        super::super::limits::clear_all();
    }

    /// 超限 → 用户拒绝（带备注）→ 结构化拒绝回灌（Ok(rejected) 臂）。
    #[cfg(feature = "security")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn limit_overrun_rejected_by_user_denies() {
        let _logs = capture_logs();
        let _guard = LIMITS_LOCK.lock().await;
        let category: &'static str = "cov_lim_rejected";
        set_rule(category, 1);

        let plugin = sec_plugin();
        plugin
            .auditor()
            .set_approval_manager(Arc::new(super::FakeApprovalManager::rejected()));
        let mut agent_loop = sec_loop();
        agent_loop.set_security_plugin(plugin);
        agent_loop.register_tool("cov_limited_b".to_string(), Box::new(LimitedTool(category)));
        let ctx = sec_ctx();

        let _ = agent_loop
            .handle_tool_call(&sec_call("cov_limited_b", "{}"), &ctx)
            .await;
        let out = agent_loop
            .handle_tool_call(&sec_call("cov_limited_b", "{}"), &ctx)
            .await;
        assert!(
            out.contains("RATE LIMIT — USER REJECTED"),
            "拒绝必须回灌结构化限流文案: {out}"
        );
        assert!(out.contains("covw6 note"), "用户备注必须带上: {out}");
        assert!(out.contains("Do NOT retry"), "必须明示不要原样重试: {out}");

        super::super::limits::clear_all();
    }

    /// 超限 → 无审批通道 → fail-closed 拒绝（Err 臂）。
    #[cfg(feature = "security")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn limit_overrun_without_channel_fails_closed() {
        let _logs = capture_logs();
        let _guard = LIMITS_LOCK.lock().await;
        let category: &'static str = "cov_lim_nochan";
        set_rule(category, 1);

        // 插件在、审计器在、但没挂审批管理器 → request_limit_approval Err。
        let plugin = sec_plugin();
        let mut agent_loop = sec_loop();
        agent_loop.set_security_plugin(plugin);
        agent_loop.register_tool("cov_limited_c".to_string(), Box::new(LimitedTool(category)));
        let ctx = sec_ctx();

        let _ = agent_loop
            .handle_tool_call(&sec_call("cov_limited_c", "{}"), &ctx)
            .await;
        let out = agent_loop
            .handle_tool_call(&sec_call("cov_limited_c", "{}"), &ctx)
            .await;
        assert!(
            out.contains("RATE LIMIT — NO APPROVAL CHANNEL"),
            "无通道必须 fail-closed: {out}"
        );

        super::super::limits::clear_all();
    }
}

// ---------------------------------------------------------------------------
// guardian 语义二审：升级与故障姿态矩阵
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
enum ApproveBehavior {
    Approve,
    Reject,
    Fail,
}

/// 审批管理器假件（is_running=true，裁决按构造而定）。
#[cfg(feature = "security")]
struct FakeApprovalManager(ApproveBehavior);

#[cfg(feature = "security")]
impl FakeApprovalManager {
    fn approved() -> Self {
        Self(ApproveBehavior::Approve)
    }
    fn rejected() -> Self {
        Self(ApproveBehavior::Reject)
    }
}

#[cfg(feature = "security")]
impl nemesis_security::auditor::ApprovalManager for FakeApprovalManager {
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
        match self.0 {
            ApproveBehavior::Approve => Ok(nemesis_security::auditor::ApprovalVerdict::approved()),
            ApproveBehavior::Reject => Ok(nemesis_security::auditor::ApprovalVerdict {
                approved: false,
                note: Some("covw6 note".to_string()),
            }),
            ApproveBehavior::Fail => Err("covw6: no approval channel".to_string()),
        }
    }
}

#[cfg(feature = "security")]
enum JudgeBehavior {
    Flag,
    Fail,
}

/// 固定裁决的 guardian judge 假件。
#[cfg(feature = "security")]
struct FakeJudge(JudgeBehavior);

#[cfg(feature = "security")]
#[async_trait]
impl nemesis_security::guardian::LlmJudge for FakeJudge {
    async fn judge(
        &self,
        _req: &nemesis_security::guardian::JudgeRequest,
    ) -> Result<nemesis_security::guardian::JudgeVerdict, String> {
        match self.0 {
            JudgeBehavior::Flag => Ok(nemesis_security::guardian::JudgeVerdict {
                intent: "wipes the disk".to_string(),
                matches_rules: true,
                risk_level: "critical".to_string(),
                recommendation: "deny".to_string(),
                rationale: "covw6: destructive without authorization".to_string(),
            }),
            JudgeBehavior::Fail => Err("covw6: judge exploded".to_string()),
        }
    }
}

/// 装配：critical 模式 + judge + 可选审批管理器 + failure policy。
#[cfg(feature = "security")]
fn guardian_setup(
    agent_loop: &mut AgentLoop,
    judge: JudgeBehavior,
    manager: Option<FakeApprovalManager>,
    failure_policy: &str,
) {
    let plugin = sec_plugin();
    plugin.set_guardian_mode("critical");
    plugin.set_guardian_failure_policy(failure_policy);
    plugin.set_judge(Arc::new(FakeJudge(judge)));
    if let Some(mgr) = manager {
        plugin.auditor().set_approval_manager(Arc::new(mgr));
    }
    agent_loop.set_security_plugin(plugin);
}

/// CRITICAL 工具调用（规则层全关，唯一闸 = guardian/limits）。
#[cfg(feature = "security")]
fn critical_call() -> ToolCallInfo {
    sec_call("shell", r#"{"command":"rm -rf /"}"#)
}

/// judge flag → 审批**批准** → info + 放行执行（升级批准臂）。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_flagged_user_approved_executes() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(
        &mut agent_loop,
        JudgeBehavior::Flag,
        Some(FakeApprovalManager::approved()),
        "",
    );
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert_eq!(out, "executed_ok", "用户批准后必须放行执行（不拦）: {out}");
}

/// judge flag → 审批**拒绝**（带备注）→ 结构化拒绝（升级拒绝臂）。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_flagged_user_rejected_denies() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(
        &mut agent_loop,
        JudgeBehavior::Flag,
        Some(FakeApprovalManager::rejected()),
        "",
    );
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert!(
        out.contains("GUARDIAN FLAGGED — USER REJECTED"),
        "拒绝必须回灌结构化文案: {out}"
    );
    assert!(out.contains(": covw6 note"), "用户备注必须拼进文案: {out}");
    assert!(out.contains("Do NOT retry"), "必须明示不要原样重试: {out}");
}

/// judge Err + failure_policy=allow → 显式放行执行。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_failure_policy_allow_proceeds() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(&mut agent_loop, JudgeBehavior::Fail, None, "allow");
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert_eq!(out, "executed_ok", "allow 姿态必须放行: {out}");
}

/// judge Err + failure_policy=ask + 批准 → 放行执行。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_failure_policy_ask_approved_executes() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(
        &mut agent_loop,
        JudgeBehavior::Fail,
        Some(FakeApprovalManager::approved()),
        "ask",
    );
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert_eq!(out, "executed_ok", "ask + 批准必须放行: {out}");
}

/// judge Err + failure_policy=ask + 拒绝 → GUARDIAN UNAVAILABLE — USER REJECTED。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_failure_policy_ask_rejected_denies() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(
        &mut agent_loop,
        JudgeBehavior::Fail,
        Some(FakeApprovalManager::rejected()),
        "ask",
    );
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert!(
        out.contains("GUARDIAN UNAVAILABLE — USER REJECTED"),
        "ask + 拒绝必须回灌结构化文案: {out}"
    );
}

/// judge Err + failure_policy=ask + 审批通道故障（Err）→ fail-closed 拒绝。
/// （无管理器与通道故障同落 Err 臂；这里走 Err 形态覆盖 Fail 变体。）
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_failure_policy_ask_without_channel_fails_closed() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(
        &mut agent_loop,
        JudgeBehavior::Fail,
        Some(FakeApprovalManager(ApproveBehavior::Fail)),
        "ask",
    );
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert!(
        out.contains("GUARDIAN UNAVAILABLE [layer:guardian|policy:guardian_failure_policy=ask]"),
        "ask + 无通道必须 fail-closed: {out}"
    );
    assert!(out.contains("judge exploded"), "故障原因必须带上: {out}");
}

/// judge Err + failure_policy=deny → 按策略拒绝。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_failure_policy_deny_denies() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(&mut agent_loop, JudgeBehavior::Fail, None, "deny");
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert!(
        out.contains("GUARDIAN UNAVAILABLE — DENIED BY POLICY"),
        "deny 姿态必须拒绝: {out}"
    );
    assert!(
        out.contains("guardian_failure_policy=deny"),
        "文案必须携带策略名: {out}"
    );
}

/// judge Err + failure_policy=未知非空值 → 从严按 deny 处理（other 臂，
/// 复核 2026-09-16 的 fail-closed 加固语义）。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_failure_policy_unknown_value_treated_as_deny() {
    let _logs = capture_logs();
    let mut agent_loop = sec_loop();
    guardian_setup(&mut agent_loop, JudgeBehavior::Fail, None, "bogus-policy");
    agent_loop.register_tool("shell".to_string(), Box::new(OkTool("executed_ok")));
    let out = agent_loop
        .handle_tool_call(&critical_call(), &sec_ctx())
        .await;
    assert!(
        out.contains("GUARDIAN UNAVAILABLE — DENIED BY POLICY"),
        "未知策略值必须从严拒绝: {out}"
    );
    assert!(
        out.contains("guardian_failure_policy=bogus-policy"),
        "文案必须携带原始策略值: {out}"
    );
}
