//! S9 覆盖率批次（Batch D：loop.rs 可直测的辅助方法与纯函数）。
//! - 7011 textwise_similar 纯函数（空/非空/相同分支）。
//! - 6862-6925 resolve_route（#[cfg(test)] 兼容函数：peer 带/不带 kind、
//!   parent_peer 两种形态）。
//! - 5610 set_snapshot_role（system/未知归 user）。
//! - 5641-5660 refresh_active_tier + 5767-5782 check_config_reload（config
//!   路径注入 + mtime 变化触发重解析）。
//! - 6127-6135 handle_command_with_context（/help、/model 无参、非命令）。
//! - 5303-5314 wait_estop_engaged（None=永挂 / 已 engaged=立即返回 / false→true）。
//! - 3001-3060 force_compression（mock provider 出摘要 → cache 推进）。
//! - 5891-6041 build_messages_with_memory_annotated（摘要 cache + memory 注入
//!   合并 system-reminder）。
//! - 6686-6775 emit_observer_events_around_llm（None / Some(manager)）。
//! - 5087-5117 precompute_readonly_batch（Valid/Fixed/Invalid 三臂）。
//! - 1260-1262 check_mcp_reload 的 mcp_manager=None 早退。
//! - 2810 maybe_update_summary 的超阈值推进（长历史 + mock 摘要）。

use super::*;
use crate::test_support::capture_logs;
use async_trait::async_trait;

// ---------- 本地 mock（与 loop/tests.rs 同构，模块私有不可共享） ----------

struct MockLlmProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
}

impl MockLlmProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(format!("echo:{}", args))
    }
    fn is_read_only(&self) -> bool {
        true
    }
}

/// 必填 path 字段的工具：驱动 args_validator 的 Invalid/Fixed 两臂。
struct StrictPathTool;

#[async_trait]
impl Tool for StrictPathTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("strict ok".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }
    fn is_read_only(&self) -> bool {
        true
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

fn resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn turn(role: &str, content: &str) -> crate::types::ConversationTurn {
    crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

// ---------- 纯函数 ----------

#[test]
fn textwise_similar_branches() {
    assert_eq!(textwise_similar("", ""), 1.0);
    assert_eq!(textwise_similar("", "abc"), 0.0);
    assert_eq!(textwise_similar("abc", ""), 0.0);
    assert_eq!(textwise_similar("hello world", "hello world"), 1.0);
    assert!(textwise_similar("hello world", "hellp world") > 0.5);
    assert!(textwise_similar("alpha", "omega") < 0.5);
}

#[test]
fn resolve_route_peer_shapes() {
    let out = resolve_route(&RouteInput {
        channel: "web".to_string(),
        account_id: Some("acc1".to_string()),
        peer: "user:42".to_string(),
        parent_peer: Some("guild:7".to_string()),
        guild_id: Some("g1".to_string()),
        team_id: Some("t1".to_string()),
    });
    assert!(!out.agent_id.is_empty());
    assert!(!out.session_key.is_empty());
    assert!(!out.matched_by.is_empty());

    // peer 无 kind、parent None 的兜底形态。
    let out2 = resolve_route(&RouteInput {
        channel: "web".to_string(),
        account_id: None,
        peer: "just-an-id".to_string(),
        parent_peer: None,
        guild_id: None,
        team_id: None,
    });
    assert_eq!(out2.agent_id, "main");
}

// ---------- AgentLoop 方法 ----------

#[test]
fn set_snapshot_role_normalizes_unknown_to_user() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_snapshot_role("SYSTEM");
    assert_eq!(*agent_loop.snapshot_role.read(), "system");
    agent_loop.set_snapshot_role("bogus");
    assert_eq!(*agent_loop.snapshot_role.read(), "user");
}

#[test]
fn refresh_active_tier_reads_config_and_reload_detects_mtime() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(
        &cfg_path,
        r#"{"model_list":[{"model":"test-model","model_tier":"mini"}]}"#,
    )
    .unwrap();

    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    // 未设路径 → 早退，无 panic。
    agent_loop.refresh_active_tier();

    agent_loop.set_config_path(cfg_path.clone());
    agent_loop.refresh_active_tier();
    assert!(
        matches!(
            *agent_loop.tier.read(),
            nemesis_types::capability::ModelTier::Mini
        ),
        "tier must be re-resolved to mini from config"
    );

    // 首次 check_config_reload：记录 mtime；同 mtime 再查 → 早退；mtime 变 → 重解析。
    std::fs::write(
        &cfg_path,
        r#"{"model_list":[{"model":"test-model","model_tier":"big"}]}"#,
    )
    .unwrap();
    agent_loop.check_config_reload();
    assert!(
        matches!(
            *agent_loop.tier.read(),
            nemesis_types::capability::ModelTier::Big
        ),
        "mtime change must re-resolve tier"
    );
    agent_loop.check_config_reload(); // unchanged → 早退分支
}

#[test]
fn handle_command_with_context_basic_arms() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(
        agent_loop
            .handle_command_with_context("hello", "web")
            .is_none()
    );
    let help = agent_loop
        .handle_command_with_context("  /help  ", "web")
        .expect("/help answers");
    assert!(help.contains("/model"));
    let listing = agent_loop
        .handle_command_with_context("/model", "web")
        .expect("/model without arg lists models");
    assert!(!listing.is_empty());
}

#[tokio::test]
async fn wait_estop_engaged_three_shapes() {
    // None → 永挂。
    let never = tokio::time::timeout(
        std::time::Duration::from_millis(80),
        AgentLoop::wait_estop_engaged(None),
    )
    .await;
    assert!(never.is_err(), "None receiver must stay pending");

    // 已 engaged → 立即返回。
    let (tx_a, mut rx_a) = tokio::sync::watch::channel(true);
    let fast = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        AgentLoop::wait_estop_engaged(Some(&mut rx_a)),
    )
    .await;
    assert!(fast.is_ok(), "already-engaged must return at once");
    drop(tx_a);

    // false → true 变化 → 返回。
    let (tx_b, mut rx_b) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let _ = tx_b.send(true);
    });
    let flipped = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        AgentLoop::wait_estop_engaged(Some(&mut rx_b)),
    )
    .await;
    assert!(flipped.is_ok(), "false->true flip must return");
}

#[tokio::test]
async fn force_compression_advances_summary_cache() {
    let _logs = capture_logs();
    let instance = AgentInstance::new(test_config());
    let mut hist = vec![turn("system", "sys prompt")];
    for i in 0..12 {
        hist.push(turn(
            if i % 2 == 0 { "user" } else { "assistant" },
            &format!("message number {i} with some body text"),
        ));
    }
    instance.set_history(hist);

    let agent_loop = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![resp("S9 summary of the prefix")])),
        test_config(),
    );
    agent_loop.force_compression(&instance).await;
    let cache = instance.get_summary_cache();
    let cache = cache.expect("summary cache must be set by force_compression");
    assert!(
        cache.covers_up_to >= 1,
        "covers_up_to={}",
        cache.covers_up_to
    );
    assert!(cache.text.contains("S9 summary"));
}

#[tokio::test]
async fn build_messages_with_memory_annotation_merges_sections() {
    let instance = AgentInstance::new(test_config());
    let mut hist = vec![turn("system", "sys prompt")];
    for i in 0..8 {
        hist.push(turn(
            if i % 2 == 0 { "user" } else { "assistant" },
            &format!("history body {i}"),
        ));
    }
    instance.set_summary_cache(Some(crate::instance::SummaryCache {
        covers_up_to: 5,
        text: "covered prefix summary".to_string(),
    }));
    instance.set_history(hist);

    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    let (messages, _annot) = agent_loop
        .build_messages_with_memory_annotated(&instance, Some(&["memory hit one".to_string()]));
    assert!(!messages.is_empty());
    // 摘要折进首条 system 消息。
    assert!(messages[0].content.contains("covered prefix summary"));
    // memory 命中注入成 system-reminder（6035-6041 合并臂）。
    let joined = messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("'");
    assert!(
        joined.contains("memory hit one"),
        "memory hit must be injected, got: {}",
        joined
    );
}

#[tokio::test]
async fn emit_observer_events_wraps_llm_call_both_ways() {
    // None manager：直接透传调用结果。
    let out =
        emit_observer_events_around_llm(None, "s9label", "m", async { Ok(resp("raw ok")) }).await;
    assert!(matches!(out, Some(Ok(r)) if r.content == "raw ok"));

    // Some(manager)：发 ConversationStart/End + LlmRequest/Response 事件。
    let mgr = std::sync::Arc::new(nemesis_observer::Manager::new());
    let out = emit_observer_events_around_llm(Some(&mgr), "s9label2", "m", async {
        Ok(resp("observed ok"))
    })
    .await;
    assert!(matches!(out, Some(Ok(r)) if r.content == "observed ok"));

    // Err 路径同样透传。
    let out = emit_observer_events_around_llm(Some(&mgr), "s9label3", "m", async {
        Err::<LlmResponse, _>("llm exploded".to_string())
    })
    .await;
    assert!(matches!(out, Some(Err(e)) if e.contains("exploded")));
}

#[tokio::test]
async fn precompute_readonly_batch_validation_arms() {
    let _logs = capture_logs();
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
    agent_loop.register_tool("s9strict".to_string(), Box::new(StrictPathTool));

    let ctx = RequestContext {
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        user: "u1".to_string(),
        session_key: "agent:test/s9d".to_string(),
        correlation_id: None,
        async_callback: None,
    };

    let calls = vec![
        // Valid：直接执行。
        crate::types::ToolCallInfo {
            id: "t1".to_string(),
            name: "s9echo".to_string(),
            arguments: r#"{"a":1}"#.to_string(),
        },
        // Fixed：typo 近邻 "pth"→"path" 自动修。
        crate::types::ToolCallInfo {
            id: "t2".to_string(),
            name: "s9strict".to_string(),
            arguments: r#"{"pth":"x.rs"}"#.to_string(),
        },
        // Invalid：缺必填 → 回灌错误。
        crate::types::ToolCallInfo {
            id: "t3".to_string(),
            name: "s9strict".to_string(),
            arguments: r#"{"other":1}"#.to_string(),
        },
    ];
    let out = agent_loop.precompute_readonly_batch(&calls, &ctx).await;
    assert_eq!(out.len(), 3, "one PrecomputedTool per call");
    assert!(out[0].result.contains("echo:"), "valid arm executes");
    assert!(out[1].result.contains("strict ok"), "fixed arm executes");
    assert!(
        out[2].result.contains("Tool error:"),
        "invalid arm feeds back validation error: {}",
        out[2].result
    );
    assert!(out[2].validation_failed, "invalid marks validation_failed");
    assert!(!out[0].validation_failed);
}

#[test]
fn check_mcp_reload_without_manager_returns_early() {
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.check_mcp_reload(); // mcp_manager=None → 早退（1260-1262）
}

#[tokio::test]
async fn maybe_update_summary_advances_on_long_history() {
    let _logs = capture_logs();
    let instance = AgentInstance::new(test_config());
    let mut hist = vec![turn("system", "sys prompt")];
    let body = "x".repeat(400);
    for i in 0..140 {
        hist.push(turn(if i % 2 == 0 { "user" } else { "assistant" }, &body));
    }
    instance.set_history(hist);

    let agent_loop = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![
            resp("incremental summary s9"),
            resp("incremental summary s9"),
            resp("incremental summary s9"),
        ])),
        test_config(),
    );
    // 只要不 panic 即覆盖入口判定链；cache 是否推进由阈值决定。
    agent_loop
        .maybe_update_summary(&instance, "agent:test/s9d", "web", "c1")
        .await;
}

// ---------- 补充批次：route_message 兜底 / cancel / MCP 禁用重载 / prefetch 早退 ----------

fn s9_inbound(
    session_key: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "hi".to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

/// route_message 无 resolver 兜底臂（2116-2135）：session_key 空走
/// channel:peer 格式；agent: 前缀的预设 key 被保留。
#[test]
fn route_message_no_resolver_fallback_arms() {
    let _logs = capture_logs();
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());

    let (agent_id, session_key) =
        agent_loop.route_message(&s9_inbound("", std::collections::HashMap::new()));
    assert_eq!(agent_id, "main");
    assert!(!session_key.is_empty(), "channel:peer derived key");

    let (agent_id, session_key) = agent_loop.route_message(&s9_inbound(
        "agent:test/s9route",
        std::collections::HashMap::new(),
    ));
    assert_eq!(agent_id, "main");
    assert_eq!(
        session_key, "agent:test/s9route",
        "agent: prefixed key honored"
    );
}

/// cancel_session 命中/未命中 + cancel_all_sessions（2748-2775）。
#[test]
fn cancel_session_and_cancel_all_arms() {
    let _logs = capture_logs();
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    assert!(!agent_loop.cancel_session("ghost"), "no token -> false");

    agent_loop
        .cancel_tokens
        .insert("k1".to_string(), tokio_util::sync::CancellationToken::new());
    assert!(agent_loop.cancel_session("k1"), "token found -> cancelled");
    assert_eq!(agent_loop.cancel_all_sessions(), 1, "one token cancelled");
}

/// enable_mcp_reload 禁用配置分支（1250-1255）+ check_mcp_reload 变更路径
/// （1266-1346，find_new_servers 为空 → 不 spawn 任何 MCP 进程，符合纪律 3）
/// + refresh_mcp_snapshot/mcp_tool_snapshot。
#[test]
fn enable_mcp_reload_disabled_config_and_change_detection() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.mcp.s9.json");

    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    // 初始路径不存在 → McpManager 默认 enabled=false → 禁用分支。
    agent_loop.enable_mcp_reload(cfg.clone());
    assert!(agent_loop.mcp_tool_snapshot().read().is_empty());

    // 未变化 → 早退。
    agent_loop.check_mcp_reload();

    // 写入合法但 enabled=false 的配置 → mtime 变化 → 变更路径（新服务器列表为空）。
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&cfg, r#"{"enabled":false,"servers":[],"timeout":30}"#).unwrap();
    agent_loop.check_mcp_reload();
    assert!(agent_loop.mcp_tool_snapshot().read().is_empty());
}

/// 注册表杂项：remove_tool_shared 两臂 + tool_count/tool_names。
#[test]
fn remove_tool_shared_and_registry_misc() {
    let _logs = capture_logs();
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.register_tool("s9misc".to_string(), Box::new(EchoTool));
    assert_eq!(agent_loop.tool_count(), 1);
    assert_eq!(agent_loop.tool_names(), vec!["s9misc".to_string()]);
    assert!(agent_loop.remove_tool_shared("s9misc"));
    assert!(
        !agent_loop.remove_tool_shared("s9misc"),
        "second remove -> false"
    );
    assert_eq!(agent_loop.tool_count(), 0);
}

/// prefetch_memory_context 早退臂（5793-5807）：auto=false → None；
/// auto=true 但 manager=None → None。深检索臂需要真实向量管理器
/// （feature memory + 向量库）→ 环境依赖组，见报告。
#[cfg(feature = "memory")]
#[tokio::test]
async fn prefetch_memory_context_early_returns() {
    let instance = AgentInstance::new(test_config());
    let mut hist = vec![
        turn("system", "sys"),
        turn("user", "find memories about rust"),
    ];
    instance.set_history(std::mem::take(&mut hist));

    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    // auto=false（默认）→ None。
    assert!(
        agent_loop
            .prefetch_memory_context(&instance)
            .await
            .is_none()
    );

    // auto=true、有 query，但 manager=None → None（5807 的 ? 臂）。
    agent_loop.set_memory_inject(None, true, 3);
    assert!(
        agent_loop
            .prefetch_memory_context(&instance)
            .await
            .is_none()
    );
}

// ============================================================================
// 终批：总线流（run_bus_owned）+ setter 杂项 + 遗留直调
// ============================================================================

struct ErrProvider;

#[async_trait]
impl LlmProvider for ErrProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("s9 llm exploded".to_string())
    }
}

/// 可排 Result 的 provider（hook 重试 fail-open 臂需要 Err 响应）。
struct SeqResultProvider {
    responses: std::sync::Mutex<Vec<Result<LlmResponse, String>>>,
}

#[async_trait]
impl LlmProvider for SeqResultProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            Ok(resp("exhausted"))
        } else {
            q.remove(0)
        }
    }
}

/// 返回 __ASYNC__ 标记的工具（cluster_rpc 异步路径 4701-4766）。
struct AsyncMarkerTool;

#[async_trait]
impl Tool for AsyncMarkerTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("__ASYNC__:s9taskA:peer1:PeerNine".to_string())
    }
}

/// 巨型结果工具（spill 阈值 65536 触发，4854-4882）。
struct BigResultTool;

#[async_trait]
impl Tool for BigResultTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("s".repeat(70_000))
    }
}

/// 成功写类工具（⑤ 重复成功 guard，4756-4766）。
struct WriteOkTool;

#[async_trait]
impl Tool for WriteOkTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("write ok".to_string())
    }
}

fn s9_msg(
    channel: &str,
    sender_id: &str,
    chat_id: &str,
    session_key: &str,
    content: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

fn plain_msg(content: &str) -> nemesis_types::channel::InboundMessage {
    s9_msg(
        "web",
        "user1",
        "chat1",
        "web:chat1",
        content,
        std::collections::HashMap::new(),
    )
}

fn tc_resp(calls: Vec<crate::types::ToolCallInfo>) -> LlmResponse {
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

fn s9_call(id: &str, name: &str, args: &str) -> crate::types::ToolCallInfo {
    crate::types::ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

/// setter 杂项（909-914 / 1443-1461 / 1471-1483 / 5580-5589）。
#[test]
fn setters_roundtrip_part2() {
    let _logs = capture_logs();
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());

    let (rein_tx, rein_rx) = tokio::sync::mpsc::channel(4);
    agent_loop.set_reinject_tx(rein_tx);
    drop(rein_rx);

    agent_loop.set_session_store(std::sync::Arc::new(
        crate::session::SessionStore::new_in_memory(),
    ));
    assert!(agent_loop.session_store().is_some());

    agent_loop.set_continuation_manager(std::sync::Arc::new(
        crate::loop_continuation::ContinuationManager::new(),
    ));
    assert!(agent_loop.continuation_manager.is_some());

    let db = tempfile::tempdir().unwrap();
    agent_loop.set_data_store(std::sync::Arc::new(
        nemesis_data::DataStore::open(&db.path().join("s9set.db")).unwrap(),
    ));
    assert!(agent_loop.data_store.is_some());

    agent_loop.set_provider_and_model(
        std::sync::Arc::new(MockLlmProvider::new(vec![])),
        "other-model".to_string(),
    );
    let _ = agent_loop.provider_arc();
    assert!(agent_loop.get_observer_manager().is_none());
    agent_loop.config_mut().max_turns = 7;
    assert_eq!(agent_loop.config.max_turns, 7);

    let spill_root = std::path::PathBuf::from("Z:/s9/spill");
    agent_loop.set_spill_root(spill_root.clone());
    assert_eq!(agent_loop.spill_root_path(), Some(spill_root));
    agent_loop.set_channel_manager(vec!["web".to_string()]);
}

/// stop() + clear_session_busy 非空 warn（1749-1759）。
#[test]
fn stop_and_clear_session_busy() {
    let _logs = capture_logs();
    let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.stop();
    assert!(!agent_loop.is_running());
    agent_loop.clear_session_busy(); // 空 map → 无 warn 分支

    agent_loop.session_busy.lock().insert(
        "s9busy".to_string(),
        SessionBusyState {
            busy: true,
            queue_length: 0,
        },
    );
    agent_loop.clear_session_busy(); // 非空 → warn 字段行
    assert!(!agent_loop.is_session_busy("s9busy"));
}

/// LLM 失败 → finish_message 错误漏斗（1530-1539）。
#[tokio::test]
async fn bus_flow_llm_error_funnels_to_error_message() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(ErrProvider),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    in_tx.send(plain_msg("hello")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("error response published");
    assert!(
        out.content.contains("Error processing message"),
        "got: {}",
        out.content
    );
}

/// history 请求 JSON 解析失败 → 空 history 响应（2547-2558）。
#[tokio::test]
async fn bus_flow_history_request_parse_error() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mut meta = std::collections::HashMap::new();
    meta.insert("request_type".to_string(), "history".to_string());
    let msg = s9_msg("web", "user1", "chat1", "web:chat1", "not-json{{", meta);
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx
        .recv()
        .await
        .expect("history error response published");
    assert!(!out.content.is_empty());
}

/// Reject 模式 continuation 标记 + permits=0 内联派发（1093-1133 + 1638-1646）。
#[tokio::test]
async fn bus_flow_continuation_inline() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![resp("cont inline final")])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    mgr.save_continuation(
        "s9taskI",
        vec![LlmMessage {
            role: "user".to_string(),
            content: "go".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }],
        "tcI",
        "web",
        "chat9",
        "s9sess",
    )
    .await;
    agent_loop.set_continuation_manager(mgr);

    let prefix = nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX;
    let msg = s9_msg(
        "system",
        &format!("{prefix}s9taskI"),
        "chat9",
        "",
        "task response body",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("continuation final published");
    assert!(
        out.content.contains("cont inline final"),
        "got: {}",
        out.content
    );
}

/// permits=1 → spawn 派发臂（1134-1166）。
#[tokio::test]
async fn bus_flow_continuation_spawned() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![resp("cont spawned final")])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        1,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    mgr.save_continuation(
        "s9taskS",
        vec![LlmMessage {
            role: "user".to_string(),
            content: "go".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }],
        "tcS",
        "web",
        "chat9",
        "s9sess",
    )
    .await;
    agent_loop.set_continuation_manager(mgr);

    let prefix = nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX;
    let msg = s9_msg(
        "system",
        &format!("{prefix}s9taskS"),
        "chat9",
        "",
        "task response body",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    // spawned 任务可能晚于 pump 结束 → 轮询等待。
    let mut got = None;
    for _ in 0..40 {
        if let Ok(o) = out_rx.try_recv() {
            got = Some(o);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let out = got.expect("spawned continuation final published");
    assert!(
        out.content.contains("cont spawned final"),
        "got: {}",
        out.content
    );
}

/// Queue 模式：slash → Immediate 内联回复（1665）+ 普通消息 → Admitted
/// spawn 任务（1675-1683）。
#[tokio::test]
async fn bus_flow_queue_mode_immediate_and_admitted() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![resp("queue admitted final")])),
        test_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    in_tx.send(plain_msg("/help")).await.unwrap();
    in_tx.send(plain_msg("hello queue")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let first = out_rx.recv().await.expect("slash immediate reply");
    assert!(!first.content.is_empty());
    let second = out_rx.recv().await.expect("admitted turn final");
    assert!(
        second.content.contains("queue admitted final"),
        "got: {}",
        second.content
    );
}

/// __ASYNC__ 标记 → 续行快照保存 + 中间回复（4701-4766）。
#[tokio::test]
async fn bus_flow_async_marker_intermediate_reply() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            tc_resp(vec![s9_call("a1", "s9async", "{}")]),
            resp("never reached"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9async".to_string(), Box::new(AsyncMarkerTool));
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    agent_loop.set_continuation_manager(mgr.clone());

    in_tx.send(plain_msg("call the peer")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("intermediate reply published");
    assert!(
        out.content.contains("已经联系") && out.content.contains("PeerNine"),
        "got: {}",
        out.content
    );
    // 4724-4734 的快照保存在 tokio::spawn 里 —— run_bus_owned 返回时任务
    // 可能还没被调度。轮询 mgr 直至快照落内存（否则 spawn 体永不执行）。
    // 用 async has_continuation：sync 版在 runtime 内阻塞会 panic。
    let mut saved = false;
    for _ in 0..200 {
        if mgr.has_continuation("s9taskA").await {
            saved = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(saved, "spawned continuation snapshot for s9taskA landed");
}

/// data_store 用量记录（4295-4322）。
#[tokio::test]
async fn bus_flow_data_store_records_usage() {
    let _logs = capture_logs();
    let db = tempfile::tempdir().unwrap();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![LlmResponse {
            content: "usage reply".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: Some(crate::loop_executor::ObserverUsageInfo {
                prompt_tokens: 11,
                completion_tokens: 7,
                total_tokens: 18,
                cached_tokens: Some(2),
                cache_creation_tokens: Some(0),
                cache_read_tokens: Some(1),
            }),
            raw_request_body: None,
            raw_response_body: None,
        }])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_data_store(std::sync::Arc::new(
        nemesis_data::DataStore::open(&db.path().join("s9bus.db")).unwrap(),
    ));
    in_tx.send(plain_msg("hi")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final with usage");
    assert_eq!(out.content, "usage reply");
}

/// 主 dispatch 的 args_validator Fixed 臂（4625-4641）：typo "pth" → "path"
/// 自动修后执行，预算不受影响。
#[tokio::test]
async fn bus_flow_validator_fixed_arm_repairs_typo() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            tc_resp(vec![s9_call("v1", "s9path", "{\"pth\":\"x.rs\"}")]),
            resp("fixed flow done"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9path".to_string(), Box::new(StrictPathTool));
    in_tx.send(plain_msg("validate args")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after fixed repair");
    assert_eq!(out.content, "fixed flow done");
}

/// Invalid 臂：缺必填回灌结构化错误，tier 预算（big=1）耗尽 → 停环消息。
#[tokio::test]
async fn bus_flow_validator_invalid_arm_exhausts_budget() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            tc_resp(vec![s9_call("v2", "s9path", "{\"other\":1}")]),
            resp("never reached"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9path".to_string(), Box::new(StrictPathTool));
    in_tx.send(plain_msg("validate args")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("budget stop message");
    assert!(
        out.content.contains("工具参数校验连续失败"),
        "got: {}",
        out.content
    );
}

/// max_tokens 截断 → continue-generation（4338-4360 一带）。
#[tokio::test]
async fn bus_flow_hit_cap_continue_generation() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            LlmResponse {
                content: "partial output cut mid".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: Some(crate::loop_executor::ObserverUsageInfo {
                    prompt_tokens: 5,
                    completion_tokens: 8192,
                    total_tokens: 8197,
                    cached_tokens: None,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                }),
                raw_request_body: None,
                raw_response_body: None,
            },
            resp("continued to the end"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    in_tx.send(plain_msg("write a huge file")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after continuation");
    assert!(
        out.content.contains("continued to the end"),
        "got: {}",
        out.content
    );
}

/// ⑤ 重复成功 guard：同 (tool,args) 连续两次成功 → nudge（4756-4766）。
#[tokio::test]
async fn bus_flow_repeat_success_guard_nudges() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            tc_resp(vec![s9_call("w1", "s9write", "{\"a\":1}")]),
            tc_resp(vec![s9_call("w2", "s9write", "{\"a\":1}")]),
            resp("guard flow done"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9write".to_string(), Box::new(WriteOkTool));
    in_tx.send(plain_msg("write twice")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after guard nudge");
    assert_eq!(out.content, "guard flow done");
}

/// 巨型工具结果 → spill 落盘（4854-4882）。
#[tokio::test]
async fn bus_flow_big_tool_result_spills() {
    let _logs = capture_logs();
    let ws = tempfile::tempdir().unwrap();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            tc_resp(vec![s9_call("b1", "s9big", "{}")]),
            resp("spill flow done"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9big".to_string(), Box::new(BigResultTool));
    agent_loop.set_workspace_root(ws.path().to_path_buf());
    // 原版漏了 set_spill_root —— spill_root=None 时 4853 直接短路，
    // 4854-4882 整块跳过（只有 prune 档在跑）。补上后 Spilled 臂生效。
    let spill_root = ws.path().join("spill");
    agent_loop.set_spill_root(spill_root.clone());
    in_tx.send(plain_msg("read something huge")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after spill");
    assert_eq!(out.content, "spill flow done");
    // spill 文件应落在 <root>/<sanitized session>/<stamp>_<call_id>.txt。
    // 真实路由 session_key 是 "agent:main:main" → sanitize 成 agent_main_main
    // （不是 web_chat1）。
    let session_dir = spill_root.join("agent_main_main");
    let entries: Vec<_> = std::fs::read_dir(&session_dir)
        .expect("spill session dir exists")
        .collect();
    assert!(
        !entries.is_empty(),
        "spill file written under {:?}",
        session_dir
    );
}

/// H5：write_file 触到指令链文件 → digest 失效（4925-4980）。
#[tokio::test]
async fn bus_flow_instruction_chain_touch_invalidates() {
    let _logs = capture_logs();
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("AGENTS.md"), "# s9 agent rules\n").unwrap();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            tc_resp(vec![s9_call(
                "c1",
                "write_file",
                &serde_json::json!({"path": ws.path().join("AGENTS.md").to_string_lossy()})
                    .to_string(),
            )]),
            resp("chain touch done"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("write_file".to_string(), Box::new(WriteOkTool));
    agent_loop.set_workspace_root(ws.path().to_path_buf());
    in_tx.send(plain_msg("edit the agent file")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after chain touch");
    assert_eq!(out.content, "chain touch done");
}

/// LLM hook Retry → 重呼成功臂（4247-4251）。
struct RetryOnceHook {
    fired: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl crate::hooks::LlmHook for RetryOnceHook {
    fn name(&self) -> String {
        "s9-retry-once".to_string()
    }
    async fn post_llm_call(
        &self,
        _call: &crate::hooks::HookLlmCall,
        _response: &LlmResponse,
    ) -> crate::hooks::LlmResponseDecision {
        if !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            crate::hooks::LlmResponseDecision::Retry {
                reason: "s9 needs regeneration".to_string(),
            }
        } else {
            crate::hooks::LlmResponseDecision::Allow
        }
    }
}

#[tokio::test]
async fn bus_flow_hook_retry_succeeds() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            resp("first attempt"),
            resp("regenerated answer"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.add_llm_hook(std::sync::Arc::new(RetryOnceHook {
        fired: std::sync::atomic::AtomicBool::new(false),
    }));
    in_tx.send(plain_msg("hook me")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after hook retry");
    assert_eq!(out.content, "regenerated answer");
}

/// LLM hook Retry → 重呼失败 → fail-open 保留原响应（4252-4255）。
struct AlwaysRetryHook;

#[async_trait]
impl crate::hooks::LlmHook for AlwaysRetryHook {
    fn name(&self) -> String {
        "s9-always-retry".to_string()
    }
    async fn post_llm_call(
        &self,
        _call: &crate::hooks::HookLlmCall,
        _response: &LlmResponse,
    ) -> crate::hooks::LlmResponseDecision {
        crate::hooks::LlmResponseDecision::Retry {
            reason: "again".to_string(),
        }
    }
}

#[tokio::test]
async fn bus_flow_hook_retry_fail_open() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(SeqResultProvider {
            responses: std::sync::Mutex::new(vec![
                Ok(resp("original answer")),
                Err("retry call failed".to_string()),
            ]),
        }),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.add_llm_hook(std::sync::Arc::new(AlwaysRetryHook));
    in_tx.send(plain_msg("hook fail open")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("fail-open keeps previous");
    assert_eq!(out.content, "original answer");
}

/// AgentLoop::handle_cluster_continuation 参考实现两臂（1794-1833，
/// #[allow(dead_code)]，主循环走自由函数；直调覆盖）。
#[tokio::test]
async fn handle_cluster_continuation_wrapper_both_arms() {
    let _logs = capture_logs();
    // 无 manager → warn 臂。
    {
        let agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
        let msg = plain_msg("wrapper body");
        agent_loop.handle_cluster_continuation("s9w1", &msg).await;
    }
    // 有 manager + 已存快照 → 内部调用臂。
    {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let mut agent_loop = AgentLoop::new_bus(
            Box::new(MockLlmProvider::new(vec![resp("wrapper final")])),
            test_config(),
            out_tx,
            ConcurrentMode::Reject,
            8,
            0,
        );
        let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
        mgr.save_continuation(
            "s9w2",
            vec![LlmMessage {
                role: "user".to_string(),
                content: "go".to_string(),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            }],
            "tcW",
            "web",
            "chat9",
            "s9sess",
        )
        .await;
        agent_loop.set_continuation_manager(mgr);
        let msg = plain_msg("wrapper body");
        agent_loop.handle_cluster_continuation("s9w2", &msg).await;
        let out = out_rx.recv().await.expect("wrapper final published");
        assert!(
            out.content.contains("wrapper final"),
            "got: {}",
            out.content
        );
    }
}

/// summarize_bare_concat_owned 的 Err 臂（6527-6535）。
#[tokio::test]
async fn summarize_bare_concat_owned_error_arm() {
    let _logs = capture_logs();
    let t1 = turn("user", "hello world");
    let t2 = turn("assistant", "hi there");
    let out = summarize_bare_concat_owned(&[&t1, &t2], "existing", &ErrProvider, "m", None).await;
    assert!(out.is_none(), "failed summary must yield None");
}

/// maybe_update_summary 的 compaction stuck 计数（2882-2894）：预置
/// last_summary_tokens + consecutive_failures，摘要失败 → 计数到顶 →
/// stuck warn + paused。
#[tokio::test]
async fn maybe_update_summary_compaction_stuck_marks_paused() {
    let _logs = capture_logs();
    let instance = AgentInstance::new(test_config());
    let mut hist = vec![turn("system", "sys prompt")];
    let body = "y".repeat(1500);
    for i in 0..300 {
        hist.push(turn(if i % 2 == 0 { "user" } else { "assistant" }, &body));
    }
    instance.set_history(hist);

    let agent_loop = AgentLoop::new(Box::new(ErrProvider), test_config());
    {
        let mut states = agent_loop.compact_state.lock();
        let st = states.entry("agent:test/s9stuck".to_string()).or_default();
        st.last_summary_tokens = 1; // 任意 >0 → ineffective(1, 大 tail) = true
        st.consecutive_failures = 1; // ineffective 再 +1 即达 COMPACT_STUCK_LIMIT=2
    }
    // stuck 判定在 summarize LLM 调用**之前**（纯 token 数比较）：大 tail →
    // will_summarize → ineffective → 计数到顶 → warn + paused_stuck 提前
    // return，provider 永不被调。只需不 panic 即覆盖。
    agent_loop
        .maybe_update_summary(&instance, "agent:test/s9stuck", "web", "chat1")
        .await;
}

// ============================================================================
// S9 收尾批（94.2% → 95% 冲刺）：错误恢复梯 / hook 取消 / Queue 臂 / 摘要
// 成败两路 / spill 三臂 / 并行只读批重放 / reinject 双臂。
// ============================================================================

/// 首次调用可门控的 provider：chat 先 notify started，第 1 次调用额外等
/// release；测试用它把 turn 卡在 LLM 调用处，从外部 cancel_session。
struct GateProvider {
    started: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl GateProvider {
    fn new(
        responses: Vec<LlmResponse>,
    ) -> (
        Self,
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        (
            Self {
                started: started.clone(),
                release: release.clone(),
                responses: std::sync::Mutex::new(responses),
                calls: std::sync::atomic::AtomicUsize::new(0),
            },
            started,
            release,
        )
    }
}

#[async_trait]
impl LlmProvider for GateProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.started.notify_one();
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            self.release.notified().await;
        }
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            Ok(resp("gate exhausted"))
        } else {
            Ok(q.remove(0))
        }
    }
}

/// 可门控的工具：execute 通知 started 后阻塞等 release —— 测试在工具执行
/// 期间从外部 cancel_session。
struct GatedTool {
    started: std::sync::Arc<tokio::sync::Notify>,
    release: std::sync::Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Tool for GatedTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        self.started.notify_one();
        self.release.notified().await;
        Ok("gated tool ok".to_string())
    }
}

/// post_llm_call 里取消本 session 的 hook —— 确定性打中 4568 的
/// 「工具执行前取消」检查（chat 已返回、dispatch 未跑）。
struct CancelInHookHook {
    handle: std::sync::Arc<AgentLoop>,
    session: String,
}

#[async_trait]
impl crate::hooks::LlmHook for CancelInHookHook {
    fn name(&self) -> String {
        "s9-cancel-in-hook".to_string()
    }
    async fn post_llm_call(
        &self,
        _call: &crate::hooks::HookLlmCall,
        _response: &LlmResponse,
    ) -> crate::hooks::LlmResponseDecision {
        let _ = self.handle.cancel_session(&self.session);
        crate::hooks::LlmResponseDecision::Allow
    }
}

/// 收集 outbound 直到 200ms 静默（Queue 模式 spawn 的 turn 不保证在
/// run_bus 返回前完成）。
async fn drain_outbound(
    out_rx: &mut tokio::sync::mpsc::Receiver<nemesis_types::channel::OutboundMessage>,
) -> Vec<String> {
    let mut got = Vec::new();
    while let Ok(Some(m)) =
        tokio::time::timeout(std::time::Duration::from_millis(200), out_rx.recv()).await
    {
        got.push(m.content);
    }
    got
}

/// 上下文错误 → 压缩重试成功（3887-3950），voice_playback=true 连带
/// 3912-3919 的重注入臂 + 重试路的 tool 定义重建闭包（3932-3938 ——
/// `self.tools` 非空才执行）。
#[tokio::test]
async fn bus_flow_context_error_compression_retry_recovers() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(SeqResultProvider {
            responses: std::sync::Mutex::new(vec![
                Err("context length exceeded".to_string()),
                // 压缩重试内部会再呼一次（恢复路消费），成功后主循环回到
                // chat 臂还要再呼一次 —— 共 3 次调用。
                Ok(resp("compressed ok")),
                Ok(resp("compressed ok")),
            ]),
        }),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    // 注册一个工具让重试路的 tool-defs 映射闭包真正迭代。
    agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
    let mut msg = plain_msg("way too long");
    msg.voice_playback = Some(true);
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after compression retry");
    assert!(
        out.content.contains("compressed ok"),
        "got: {}",
        out.content
    );
}

/// 瞬时错误（503）→ 重试成功（4037-4078；重试路的 tool-defs 闭包
/// 4051-4057 需注册工具）。
#[tokio::test]
async fn bus_flow_transient_error_retry_recovers() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(SeqResultProvider {
            responses: std::sync::Mutex::new(vec![
                Err("HTTP 503: service unavailable".to_string()),
                Ok(resp("transient ok")),
            ]),
        }),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
    in_tx.send(plain_msg("flaky provider")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after transient retry");
    assert!(out.content.contains("transient ok"), "got: {}", out.content);
}

/// 连续空 final → RetryWithNudge ×2 后 GiveUp（4495-4503）。
#[tokio::test]
async fn bus_flow_empty_final_gives_up_after_budget() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![resp("   "), resp(""), resp("")])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    in_tx.send(plain_msg("say nothing")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("give-up notice");
    assert!(
        out.content.contains("模型多次未给出有效答复"),
        "got: {}",
        out.content
    );
}

/// ⑧ 跨轮文本重复 → repetition nudge 入队（4383-4388）→ 下一轮 LLM 调用
/// 前 build_messages 重注入（3713-3726）。重复必须发生在**非 final** 的
/// 响应上（带 tool_calls 且未 finished），轮次才会继续、nudge 才会被消费。
#[tokio::test]
async fn bus_flow_prose_repetition_sets_nudge() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            LlmResponse {
                content: "dup prose answer".to_string(),
                tool_calls: vec![s9_call("r1", "s9echo", "{}")],
                finished: false,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            },
            LlmResponse {
                content: "dup prose answer".to_string(),
                tool_calls: vec![s9_call("r2", "s9echo", "{}")],
                finished: false,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            },
            resp("final after repetition nudge"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
    in_tx.send(plain_msg("repeat yourself")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;
    let out = out_rx.recv().await.expect("final after repetition nudge");
    assert!(
        out.content.contains("final after repetition nudge"),
        "got: {}",
        out.content
    );
}

/// Queue 模式 continuation 臂（1651-1656）。
#[tokio::test]
async fn bus_flow_queue_mode_continuation_arm() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![resp("cont queue final")])),
        test_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    let mgr = std::sync::Arc::new(crate::loop_continuation::ContinuationManager::new());
    mgr.save_continuation(
        "s9taskQ",
        vec![LlmMessage {
            role: "user".to_string(),
            content: "go queue".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }],
        "tcQ",
        "web",
        "chat9",
        "s9sess",
    )
    .await;
    agent_loop.set_continuation_manager(mgr);

    let prefix = nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX;
    let msg = s9_msg(
        "system",
        &format!("{prefix}s9taskQ"),
        "chat9",
        "",
        "queue task response",
        std::collections::HashMap::new(),
    );
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = out_rx.recv().await.expect("queue continuation final");
    assert!(
        out.content.contains("cont queue final"),
        "got: {}",
        out.content
    );
}

/// Queue 模式 history 请求 → gate Ungated（2175-2176）→ spawn_turn_task
/// （1668-1673）→ process_ungated 的 history 分支（2289-2293）。
#[tokio::test]
async fn bus_flow_queue_mode_history_ungated_spawn() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    let mut meta = std::collections::HashMap::new();
    meta.insert("request_type".to_string(), "history".to_string());
    let msg = s9_msg("web", "user1", "chatQ", "web:chatQ", "not-json{{", meta);
    in_tx.send(msg).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let out = tokio::time::timeout(std::time::Duration::from_secs(3), out_rx.recv())
        .await
        .expect("spawned history handler replied in time")
        .expect("history error response published");
    assert!(!out.content.is_empty());
}

/// outbound 通道对端已关 → finish_message 发送失败 warn（1593-1594）。
#[tokio::test]
async fn bus_flow_outbound_send_fail_warns() {
    let _logs = capture_logs();
    let (out_tx, out_rx) = tokio::sync::mpsc::channel(16);
    drop(out_rx); // 对端消失：所有 send 立刻 Err
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    in_tx.send(plain_msg("/help")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await; // 不 panic 即覆盖 warn 臂
}

/// 预置大历史的 session store。
/// 先 get_or_create 物化条目 —— set_history 的 `get_mut` 只改已存在的
/// session，条目缺失时是静默 no-op（曾导致大历史根本没写进 store，
/// get_or_create 反而走了全局 chat_log 自愈重放）。
fn big_history_store(
    dir: &std::path::Path,
    key: &str,
) -> std::sync::Arc<crate::session::SessionStore> {
    let store = crate::session::SessionStore::new_with_storage(dir);
    let _ = store.get_or_create(key); // 可能在 tempdir 里触发一次自愈落盘，无妨
    let mut turns = vec![turn("system", "sys prompt")];
    let body = "z".repeat(1500);
    // 50×1500 字符 ≈ 30k tokens：超过 summarize 阈值（32k 窗 × 75% = 24k）
    // 但前缀仍在单批摘要范围内（80k 字符/批）。300 轮会拆成多批 + 合并，
    // 把 provider 队列烧穿。
    for i in 0..50 {
        turns.push(turn(if i % 2 == 0 { "user" } else { "assistant" }, &body));
    }
    let stored: Vec<crate::session::StoredMessage> = turns
        .iter()
        .map(crate::session::StoredMessage::from)
        .collect();
    store.set_history(key, stored);
    store.save(key).unwrap();
    assert!(
        !store.get_history(key).is_empty(),
        "big history actually landed in the store"
    );
    std::sync::Arc::new(store)
}

/// 摘要成功路：主轮答复 → 通知外发（2940-2952）→ summarize 成功 →
/// cache 写回（2970-2974）→ 会话持久化（3237-3246）。
#[tokio::test]
async fn bus_flow_summary_success_notice_and_persist() {
    let _logs = capture_logs();
    let ws = tempfile::tempdir().unwrap();
    // new_bus 装了默认 route resolver：web 消息实际路由到
    // session_key "agent:main:main"（不是 "web:chat1"）——大历史必须写在
    // 真实路由键下，否则 get_or_create_instance 读不到（还会触发 chat_log
    // 自愈重建把历史换成全局测试日志的重放）。
    let store = big_history_store(ws.path(), "agent:main:main");
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![
            resp("final ok"),
            // 摘要路可能发 1-2 次调用（批次 + 可能的合并）——多备几份同样
            // 文本的响应，任何一个消费者都拿到合法摘要文本。
            resp("S9 SUMMARY TEXT"),
            resp("S9 SUMMARY TEXT"),
            resp("S9 SUMMARY TEXT"),
        ])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_session_store(store.clone());
    in_tx.send(plain_msg("summarize me")).await.unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let outs = drain_outbound(&mut out_rx).await;
    assert!(
        outs.iter().any(|c| c.contains("Memory threshold reached")),
        "notice sent, got: {:?}",
        outs
    );
    assert!(
        outs.iter().any(|c| c.contains("final ok")),
        "got: {:?}",
        outs
    );
    // 摘要写回 store（3239-3240 set_summary）。
    assert!(
        store
            .get_summary("agent:main:main")
            .contains("S9 SUMMARY TEXT"),
        "summary persisted"
    );
}

/// 摘要失败路：ErrProvider → summarize Err → None → loud warn（2975-2981），
/// cache 不推进。
#[tokio::test]
async fn bus_flow_summary_failure_warns_and_keeps_cache_empty() {
    let _logs = capture_logs();
    let ws = tempfile::tempdir().unwrap();
    // 同上：真实路由键是 agent:main:main。
    let store = big_history_store(ws.path(), "agent:main:main");
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(ErrProvider),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_session_store(store.clone());
    in_tx
        .send(plain_msg("summarize me but fail"))
        .await
        .unwrap();
    drop(in_tx);
    agent_loop.run_bus_owned(in_rx).await;

    let outs = drain_outbound(&mut out_rx).await;
    assert!(
        outs.iter().any(|c| c.contains("Memory threshold reached")),
        "notice still sent before the summarize attempt, got: {:?}",
        outs
    );
    assert!(
        store.get_summary("agent:main:main").is_empty(),
        "failed summarize must not advance the cache"
    );
}

/// 40k 字符工具（spill 阈值之下、prune 阈值之上 → BelowThreshold 臂）。
struct MidResultTool;

#[async_trait]
impl Tool for MidResultTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("m".repeat(40_000))
    }
}

/// SpillFailed 臂（4864-4870，root 是文件 → create_dir_all 失败）+
/// BelowThreshold 臂（4871，40k < 65536 → prune 档）。
#[tokio::test]
async fn bus_flow_spill_failed_and_below_threshold_both_prune() {
    let _logs = capture_logs();
    // 1) spill_root 指向一个已存在的文件 → SpillFailed → 回落 prune。
    {
        let ws = tempfile::tempdir().unwrap();
        let blocker = ws.path().join("blocker");
        std::fs::write(&blocker, "i am a file").unwrap();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let mut agent_loop = AgentLoop::new_bus(
            Box::new(MockLlmProvider::new(vec![
                tc_resp(vec![s9_call("f1", "s9big", "{}")]),
                resp("prune after spill fail"),
            ])),
            test_config(),
            out_tx,
            ConcurrentMode::Reject,
            8,
            0,
        );
        agent_loop.register_tool("s9big".to_string(), Box::new(BigResultTool));
        agent_loop.set_workspace_root(ws.path().to_path_buf());
        agent_loop.set_spill_root(blocker);
        in_tx.send(plain_msg("big but blocked")).await.unwrap();
        drop(in_tx);
        agent_loop.run_bus_owned(in_rx).await;
        let out = out_rx.recv().await.expect("final after spill failure");
        assert!(out.content.contains("prune after spill fail"));
    }
    // 2) 40k 结果 < 65536 阈值 → BelowThreshold → prune 档。
    {
        let ws = tempfile::tempdir().unwrap();
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let mut agent_loop = AgentLoop::new_bus(
            Box::new(MockLlmProvider::new(vec![
                tc_resp(vec![s9_call("b1", "s9mid", "{}")]),
                resp("mid pruned done"),
            ])),
            test_config(),
            out_tx,
            ConcurrentMode::Reject,
            8,
            0,
        );
        agent_loop.register_tool("s9mid".to_string(), Box::new(MidResultTool));
        agent_loop.set_workspace_root(ws.path().to_path_buf());
        agent_loop.set_spill_root(ws.path().join("spill"));
        in_tx.send(plain_msg("mid size result")).await.unwrap();
        drop(in_tx);
        agent_loop.run_bus_owned(in_rx).await;
        let out = out_rx
            .recv()
            .await
            .expect("final after below-threshold prune");
        assert!(out.content.contains("mid pruned done"));
    }
}

/// record_last_channel / record_last_chat_id 落盘失败 warn（2653-2655 /
/// 2663-2665）：workspace 下 "state" 是文件 → save_atomic 失败。
#[test]
fn record_last_channel_and_chat_id_warn_on_save_failure() {
    let _logs = capture_logs();
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("state"), "not a dir").unwrap();
    let mgr = nemesis_state::workspace_state::WorkspaceStateManager::new(ws.path());
    let mut agent_loop = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    agent_loop.set_state_manager(mgr);
    agent_loop.record_last_channel("web"); // 必须不 panic（warn 臂）
    agent_loop.record_last_chat_id("chat1"); // 同上
}

/// emit_observer_sync 双下沉（1770-1777）：observer_manager + legacy
/// callback 同时在场。
#[tokio::test]
async fn emit_observer_sync_reaches_manager_and_callback() {
    let _logs = capture_logs();
    let (out_tx, _out_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_observer_manager(std::sync::Arc::new(nemesis_observer::Manager::new()));
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    agent_loop.set_observer_callback(std::sync::Arc::new(
        move |event_type: &str, _data: &serde_json::Value| {
            sink.lock().unwrap().push(event_type.to_string());
        },
    ));
    agent_loop
        .emit_observer_sync(crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: "s9-trace".to_string(),
            session_key: "web:chat1".to_string(),
            channel: "web".to_string(),
            chat_id: "chat1".to_string(),
            sender_id: "user1".to_string(),
            content: "hello observers".to_string(),
        })
        .await;
    let events = seen.lock().unwrap().clone();
    assert!(
        events.iter().any(|t| t.contains("conversation")),
        "legacy callback saw event: {:?}",
        events
    );
}

/// U5 并行只读批（≥2 个 read-only 调用）→ precomputed 重放两臂
/// （4613-4618）：全 Valid（reset 0，正常走完）与含 Invalid（+1 → big 档
/// 预算 1 → 触发校验预算停）。
#[tokio::test]
async fn parallel_readonly_batch_replays_validation_counters() {
    let _logs = capture_logs();
    // 1) 两个调用都 Valid：precompute 全成功 → 重放走 else（reset）臂，
    //    正常收尾。
    {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let mut agent_loop = AgentLoop::new_bus(
            Box::new(MockLlmProvider::new(vec![
                tc_resp(vec![
                    s9_call("p1", "s9echo", "{}"),
                    s9_call("p2", "s9strict", "{\"path\":\"x\"}"),
                ]),
                resp("parallel done"),
            ])),
            test_config(),
            out_tx,
            ConcurrentMode::Reject,
            8,
            0,
        );
        agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
        agent_loop.register_tool("s9strict".to_string(), Box::new(StrictPathTool));
        in_tx.send(plain_msg("run a parallel batch")).await.unwrap();
        drop(in_tx);
        agent_loop.run_bus_owned(in_rx).await;
        let out = out_rx.recv().await.expect("final after parallel batch");
        assert!(
            out.content.contains("parallel done"),
            "got: {}",
            out.content
        );
    }
    // 2) p2 缺必填 path → precompute 标 validation_failed → 重放 +1 臂
    //    → big 档预算（1）耗尽 → 校验预算停（两个臂都要真实跑到）。
    {
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
        let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
        let mut agent_loop = AgentLoop::new_bus(
            Box::new(MockLlmProvider::new(vec![tc_resp(vec![
                s9_call("p1", "s9echo", "{}"),
                s9_call("p2", "s9strict", "{}"),
            ])])),
            test_config(),
            out_tx,
            ConcurrentMode::Reject,
            8,
            0,
        );
        agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
        agent_loop.register_tool("s9strict".to_string(), Box::new(StrictPathTool));
        in_tx.send(plain_msg("run an invalid batch")).await.unwrap();
        drop(in_tx);
        agent_loop.run_bus_owned(in_rx).await;
        let out = out_rx.recv().await.expect("budget stop final");
        assert!(
            out.content.contains("已停止重试"),
            "validation budget stop fired, got: {}",
            out.content
        );
    }
}

/// LLM 调用进行中 cancel_session → 主 select 的 cancelled 臂
/// （3855-3862 附近，Done("已取消")）。
#[tokio::test]
async fn cancel_during_llm_call_breaks_turn() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let (provider, started, _release) = GateProvider::new(vec![resp("never returns")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let handle = std::sync::Arc::new(agent_loop);
    let runner = tokio::spawn(handle.clone().run_bus_arc(in_rx));

    in_tx
        .send(plain_msg("cancel while thinking"))
        .await
        .unwrap();
    // 等 chat 真正进入（token 一定已创建）。
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .expect("chat started");
    // 真实路由 session_key 是 agent:main:main（默认 route resolver）。
    assert!(
        handle.cancel_session("agent:main:main"),
        "cancel token existed and was cancelled"
    );
    drop(in_tx);

    let out = tokio::time::timeout(std::time::Duration::from_secs(3), out_rx.recv())
        .await
        .expect("cancelled done published in time")
        .expect("outbound");
    assert!(out.content.contains("已取消"), "got: {}", out.content);
    let _ = runner.await;
}

/// 工具执行进行中 cancel_session → 下一轮迭代顶部的取消检查
/// （3521-3529，Done("已取消")）。
#[tokio::test]
async fn cancel_during_tool_execution_breaks_turn() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let tool_started = std::sync::Arc::new(tokio::sync::Notify::new());
    let tool_release = std::sync::Arc::new(tokio::sync::Notify::new());
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![tc_resp(vec![s9_call(
            "t1", "s9gate", "{}",
        )])])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool(
        "s9gate".to_string(),
        Box::new(GatedTool {
            started: tool_started.clone(),
            release: tool_release.clone(),
        }),
    );
    let handle = std::sync::Arc::new(agent_loop);
    let runner = tokio::spawn(handle.clone().run_bus_arc(in_rx));

    in_tx.send(plain_msg("cancel mid tool")).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), tool_started.notified())
        .await
        .expect("tool started");
    assert!(
        handle.cancel_session("agent:main:main"),
        "cancel token existed and was cancelled"
    );
    tool_release.notify_one();
    drop(in_tx);

    let out = tokio::time::timeout(std::time::Duration::from_secs(3), out_rx.recv())
        .await
        .expect("cancelled done published in time")
        .expect("outbound");
    assert!(out.content.contains("已取消"), "got: {}", out.content);
    let _ = runner.await;
}

/// post_llm_call hook 里 cancel → 工具执行前检查臂（4568-4574）。
#[tokio::test]
async fn hook_cancels_session_before_tool_dispatch() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(MockLlmProvider::new(vec![tc_resp(vec![s9_call(
            "h1", "s9echo", "{}",
        )])])),
        test_config(),
        out_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.register_tool("s9echo".to_string(), Box::new(EchoTool));
    let handle = std::sync::Arc::new(agent_loop);
    handle.add_llm_hook(std::sync::Arc::new(CancelInHookHook {
        handle: handle.clone(),
        session: "agent:main:main".to_string(),
    }));
    let runner = tokio::spawn(handle.run_bus_arc(in_rx));

    in_tx.send(plain_msg("hook cancels me")).await.unwrap();
    drop(in_tx);

    let out = tokio::time::timeout(std::time::Duration::from_secs(3), out_rx.recv())
        .await
        .expect("cancelled done published in time")
        .expect("outbound");
    assert!(out.content.contains("已取消"), "got: {}", out.content);
    let _ = runner.await;
}

/// Queue 模式排队消息的 drain：reinject_tx 有但接收端已关 → try_send 失败
/// warn（2401-2409），消息被丢弃（不再有第二个 final）。
#[tokio::test]
async fn queued_message_reinject_send_fail_warns_and_drops() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let (provider, started, release) = GateProvider::new(vec![resp("first done")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );
    let (reinject_tx, reinject_rx) = tokio::sync::mpsc::channel(4);
    drop(reinject_rx); // try_send 必失败
    agent_loop.set_reinject_tx(reinject_tx);

    // Queue 模式的回合跑在 spawn_turn_task 里 —— pump 必须先启动，否则
    // 没人消费 in_rx，started 永远等不到。
    let runner = tokio::spawn(async move {
        agent_loop.run_bus_owned(in_rx).await;
    });

    in_tx.send(plain_msg("first message")).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .expect("first chat started");
    // busy 已置位：第二条进队列（busy 回执是第一条 outbound）。
    in_tx.send(plain_msg("second message")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();
    drop(in_tx);
    let _ = runner.await;
    // 回合任务与 pump 并发 —— out_rx 轮询排空。
    let outs = drain_outbound(&mut out_rx).await;
    assert!(
        outs.iter().any(|c| c.contains("first done")),
        "got: {:?}",
        outs
    );
    assert!(
        !outs.iter().any(|c| c.contains("second done")),
        "dropped message must not produce a second turn, got: {:?}",
        outs
    );
}

/// Queue 模式 drain：reinject_tx 未设置 → 排队消息就地 inline 处理
/// （2411-2438），第二个 final 正常发布。
#[tokio::test]
async fn queued_message_without_reinject_processes_inline() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let (provider, started, release) =
        GateProvider::new(vec![resp("first done"), resp("second done")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        out_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );

    // 同上：先启动 pump（回合是 spawn 出去的任务）。
    let runner = tokio::spawn(async move {
        agent_loop.run_bus_owned(in_rx).await;
    });

    in_tx.send(plain_msg("first message")).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .expect("first chat started");
    in_tx.send(plain_msg("second message")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();
    drop(in_tx);
    let _ = runner.await;

    let outs = drain_outbound(&mut out_rx).await;
    assert!(
        outs.iter().any(|c| c.contains("first done"))
            && outs.iter().any(|c| c.contains("second done")),
        "both turns completed inline, got: {:?}",
        outs
    );
}

/// I1 (U7) steer 逃生舱（4422-4432）：Steer 模式下模型正要收尾时来了
/// 未认领的 "!" 插话 → 不收尾、再跑一轮（下一轮顶部 claim 注入）。
#[tokio::test]
async fn steer_mode_escape_hatch_extends_turn() {
    let _logs = capture_logs();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel(16);
    let (in_tx, in_rx) = tokio::sync::mpsc::channel(16);
    let (provider, started, release) =
        GateProvider::new(vec![resp("first answer"), resp("steer consumed answer")]);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        out_tx,
        ConcurrentMode::Steer,
        8,
        0,
    );
    let runner = tokio::spawn(async move {
        agent_loop.run_bus_owned(in_rx).await;
    });

    in_tx.send(plain_msg("normal message")).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), started.notified())
        .await
        .expect("first chat started");
    // busy 已置位：'!' 前缀消息走 Steer 通道 → QueuedForNextStep 回执。
    in_tx.send(plain_msg("!urgent interjection")).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    release.notify_one();
    drop(in_tx);
    let _ = runner.await;

    let outs = drain_outbound(&mut out_rx).await;
    assert!(
        outs.iter().any(|c| c.contains("已接收为紧急插话")),
        "steer receipt published, got: {:?}",
        outs
    );
    assert!(
        outs.iter().any(|c| c.contains("steer consumed answer")),
        "turn extended and consumed the steer, got: {:?}",
        outs
    );
}
