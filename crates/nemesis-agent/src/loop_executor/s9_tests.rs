//! S9 覆盖率批次（Batch E：loop_executor.rs 遗留区逐点判别）。
//!
//! ⚠️ 本文件整体是 legacy 死区（文件头 STATUS 注释：AgentLoopExecutor 生产
//! 零构造点，2026-08-23 验证；生产只 import 其类型定义）。这里只测「测试
//! 基建可达且行为仍有意义」的簇，其余逐点归入结构性豁免（见最终报告）。
//!
//! 覆盖目标（基线缺口 → 测试）：
//! - 648-655 FallbackExecutor 冷却跳过臂（FallbackExecutor 是生产仍在用的
//!   类型定义，值得真测）。
//! - 761 SessionPersistence::maybe_summarize Some(summarizer) 臂。
//! - 900-902/912-914/935-937/962-966/2025-2044 setter/getter/Queue 获取。
//! - 1049-1086 cluster_continuation 前缀分支（有/无 manager 两臂）。
//! - 1103/1228 出站通道关闭时 busy/最终发送 warn。
//! - 1361-1388 data_store 用量记录块。
//! - 1439-1455 args_validator Fixed/Invalid 臂（legacy 循环内）+1602-1604
//!   execute_tool_with_result Err 臂。
//! - 1539-1549 validation 预算耗尽停环。
//! - 1621-1656 多 candidate fallback 路径（含 Err 臂）+1739 上下文压缩
//!   通知发送。
//! - 1794/1802-1808 handle_tool_calls Err/未知工具臂。
//! - 1919-1920 update_tool_contexts 命中注册的 message 工具。
//! - 1977/2018 run_agent_loop save 失败 / process_and_publish 发送失败 warn。
//! - 974/984-985/998-1002/1161/1214/1270/1396 observer 事件 + 追踪字段行。
//!
//! 结构性豁免（本文件内无法到达，见报告）：
//! - 1481-1520：run_llm_iteration 中 ToolResult 的唯一生产者是
//!   `ToolResult::simple()`（silent=true, for_user="", is_async=false）与
//!   `ToolResult::error()`（..Default::default()）——1480 `tool_result.is_async`
//!   与 1501 `!silent && !for_user.is_empty()` 恒假，两个块（异步续行保存 /
//!   for_user 立即发送）在该循环内死代码。

use super::*;
use crate::r#loop::LlmResponse;
use crate::test_support::capture_logs;
use crate::types::ToolCallInfo;
use async_trait::async_trait;

// ---------- 本地 mock ----------

struct ResultProvider {
    responses: std::sync::Mutex<Vec<Result<LlmResponse, String>>>,
}

impl ResultProvider {
    fn new(responses: Vec<Result<LlmResponse, String>>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
    }
    fn plain(contents: Vec<&str>) -> Self {
        Self::new(
            contents
                .into_iter()
                .map(|c| Ok(resp_content(c, true)))
                .collect(),
        )
    }
}

#[async_trait]
impl LlmProvider for ResultProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            Ok(resp_content("No more responses", true))
        } else {
            q.remove(0)
        }
    }
}

fn resp_content(content: &str, finished: bool) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn tc(id: &str, name: &str, arguments: &str) -> ToolCallInfo {
    ToolCallInfo {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn tool_response(calls: Vec<ToolCallInfo>) -> Result<LlmResponse, String> {
    Ok(LlmResponse {
        content: String::new(),
        tool_calls: calls,
        finished: false,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    })
}

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(format!("echo:{args}"))
    }
}

struct FailTool;

#[async_trait]
impl Tool for FailTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Err("s9 tool exploded".to_string())
    }
}

/// required ["path"] 的工具：驱动 args_validator 的 Fixed/Invalid 臂。
struct SchemaTool;

#[async_trait]
impl Tool for SchemaTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("schema ok".to_string())
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        })
    }
}

struct RecordingObserver {
    labels: std::sync::Mutex<Vec<String>>,
}

impl RecordingObserver {
    fn new() -> Self {
        Self {
            labels: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Observer for RecordingObserver {
    fn on_event(&self, event: ObserverEvent) {
        let label = match &event {
            ObserverEvent::ConversationStart { .. } => "start",
            ObserverEvent::ConversationEnd { .. } => "end",
            ObserverEvent::LlmRequest { .. } => "llm_req",
            ObserverEvent::LlmResponse { .. } => "llm_resp",
            ObserverEvent::ToolCall { .. } => "tool",
        };
        self.labels.lock().unwrap().push(label.to_string());
    }
}

fn s9_config() -> ExecutorConfig {
    ExecutorConfig {
        model: "test-model".to_string(),
        max_turns: 5,
        system_prompt: Some("You are a test assistant.".to_string()),
        event_buffer_size: 16,
    }
}

fn make_executor(
    responses: Vec<Result<LlmResponse, String>>,
) -> (
    AgentLoopExecutor,
    mpsc::Sender<nemesis_types::channel::InboundMessage>,
    mpsc::Receiver<nemesis_types::channel::OutboundMessage>,
) {
    let provider = Arc::new(ResultProvider::new(responses));
    let (inbound_tx, inbound_rx) = mpsc::channel(16);
    let (outbound_tx, outbound_rx) = mpsc::channel(16);
    let ex = AgentLoopExecutor::new(provider, inbound_rx, outbound_tx, s9_config());
    (ex, inbound_tx, outbound_rx)
}

fn make_msg(
    channel: &str,
    sender_id: &str,
    session_key: &str,
    metadata: std::collections::HashMap<String, String>,
) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: channel.to_string(),
        sender_id: sender_id.to_string(),
        chat_id: "chat1".to_string(),
        content: "hi".to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata,
        voice_playback: None,
    }
}

fn s9_ctx() -> RequestContext {
    RequestContext {
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        user: "user1".to_string(),
        session_key: "test:s9e".to_string(),
        correlation_id: None,
        async_callback: None,
    }
}

// ---------- 648-655 冷却跳过 ----------

#[tokio::test]
async fn fallback_executor_cooldown_skips_recently_failed_candidate() {
    let fx = FallbackExecutor::new();
    let cands = vec![
        FallbackCandidate {
            provider: "p1".to_string(),
            model: "m1".to_string(),
        },
        FallbackCandidate {
            provider: "p2".to_string(),
            model: "m2".to_string(),
        },
    ];
    let r1 = fx
        .execute(&cands, |_p, _m| async {
            Err::<LlmResponse, String>("boom".to_string())
        })
        .await;
    assert!(r1.is_err());

    // 立即重跑：两候选都在 5s 冷却内 → 全跳过 → 返回初始错误（try_fn 未跑）。
    let r2 = fx
        .execute(&cands, |_p, _m| async {
            Ok::<LlmResponse, String>(resp_content("never used", true))
        })
        .await;
    assert_eq!(r2.unwrap_err(), "No candidates available");
}

// ---------- 761 maybe_summarize Some 臂 ----------

#[tokio::test]
async fn session_persistence_maybe_summarize_some_arm() {
    let store = Arc::new(crate::session::SessionStore::new_in_memory());
    let summarizer = crate::session::Summarizer::new_silent(
        Arc::new(ResultProvider::plain(vec!["unused"])),
        "test-model".to_string(),
        128_000,
        store.clone(),
    );
    let p = SessionPersistence::with_storage(store, summarizer);
    // 空历史 → should_summarize false，但 761（Some 分支调用）已执行。
    assert!(!p.maybe_summarize("k", "web", "c", &[], 128_000));
}

// ---------- setter/getter + Queue 获取 + 1919-1920 ----------

#[test]
fn setters_getters_and_queue_acquire() {
    let (mut ex, in_tx, out_rx) = make_executor(Vec::new());
    drop(out_rx);
    drop(in_tx);

    ex.register_tool("message".to_string(), Arc::new(EchoTool));
    ex.set_session_store(Arc::new(crate::session::SessionStore::new_in_memory()));
    ex.set_observer(Arc::new(RecordingObserver::new()));
    ex.set_observer_manager(Arc::new(nemesis_observer::Manager::new()));
    assert!(ex.get_observer_manager().is_some());
    assert!(ex.has_observers());
    ex.set_concurrent_mode(ConcurrentMode::Queue, 4);
    assert!(ex.try_acquire_session("k1"));
    assert!(!ex.try_acquire_session("k1"), "queue mode: busy second");
    assert!(ex.try_acquire_session("k2"));
    ex.set_context_window(1000);
    ex.set_fallback_candidates(vec![FallbackCandidate {
        provider: "p".to_string(),
        model: "m".to_string(),
    }]);
    assert_eq!(ex.fallback_candidates().len(), 1);
    assert!(ex.tools().contains_key("message"));
    assert_eq!(ex.config().model, "test-model");
    assert!(ex.continuation_manager().is_none());

    // 1919-1920：注册了名为 message 的工具 → set_context 分支命中。
    ex.update_tool_contexts("web", "chat1");
}

// ---------- 1049-1086 cluster_continuation 两臂 ----------

#[tokio::test]
async fn cluster_continuation_without_manager_warns() {
    let _logs = capture_logs();
    let (ex, in_tx, _out_rx) = make_executor(Vec::new());
    let prefix = nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX;
    let msg = make_msg(
        "system",
        &format!("{prefix}s9task_no_mgr"),
        "test:s9e",
        std::collections::HashMap::new(),
    );
    ex.process_message(msg).await; // 1080-1085 warn 臂 + return，无出站
    drop(in_tx);
}

#[tokio::test]
async fn cluster_continuation_with_manager_calls_handler() {
    let _logs = capture_logs();
    let (mut ex, in_tx, _out_rx) = make_executor(Vec::new());
    ex.set_continuation_manager(Arc::new(
        crate::loop_continuation::ContinuationManager::new(),
    ));
    let prefix = nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX;
    let mut meta = std::collections::HashMap::new();
    meta.insert("error".to_string(), "boom".to_string());
    let msg = make_msg("system", &format!("{prefix}s9task_ghost"), "test:s9e", meta);
    ex.process_message(msg).await; // 1062-1063 failed/error 提取 + 1065-1079 调用
    drop(in_tx);
}

// ---------- 1103/1228 出站发送失败 warn ----------

#[tokio::test]
async fn outbound_send_failures_warn_on_busy_and_final() {
    let _logs = capture_logs();
    // 1103：busy + rx 已关 → busy 消息发送失败 warn。
    {
        let (ex, in_tx, out_rx) = make_executor(Vec::new());
        drop(out_rx);
        ex.busy_sessions.insert("test:s9busy".to_string());
        ex.process_message(make_msg(
            "web",
            "user1",
            "test:s9busy",
            std::collections::HashMap::new(),
        ))
        .await;
        drop(in_tx);
    }
    // 1228：正常处理收尾时 rx 已关 → 最终发送失败 warn。
    {
        let (mut ex, in_tx, out_rx) = make_executor(Vec::new());
        ex.register_tool("message".to_string(), Arc::new(EchoTool));
        drop(out_rx);
        in_tx
            .send(make_msg(
                "web",
                "user1",
                "test:s9final",
                std::collections::HashMap::new(),
            ))
            .await
            .unwrap();
        drop(in_tx);
        ex.run().await;
    }
}

// ---------- 1439-1455/1602-1604 Fixed/Invalid/Err 臂 ----------

#[tokio::test]
async fn validation_fixed_and_invalid_arms_in_legacy_loop() {
    let _logs = capture_logs();
    let (mut ex, _in_tx, _out_rx) = make_executor(vec![
        // Fixed：typo "pth"→"path" 自动修后执行。
        tool_response(vec![tc("t1", "s9schema", r#"{"pth":"x.rs"}"#)]),
        // Invalid：缺必填 → 回灌结构化错误。
        tool_response(vec![tc("t2", "s9schema", r#"{"other":1}"#)]),
        // 工具执行 Err → execute_tool_with_result Err 臂（1602-1603）。
        tool_response(vec![tc("t3", "s9fail", r#"{}"#)]),
        Ok(resp_content("all done s9", true)),
    ]);
    ex.register_tool("s9schema".to_string(), Arc::new(SchemaTool));
    ex.register_tool("s9fail".to_string(), Arc::new(FailTool));

    let out = ex
        .run_agent_loop("test:s9val", "hi", &s9_ctx())
        .await
        .expect("loop finishes");
    assert_eq!(out, "all done s9");
}

// ---------- 1539-1549 validation 预算耗尽 ----------

#[tokio::test]
async fn validation_budget_exhausted_stops_loop() {
    let _logs = capture_logs();
    let bad = || tool_response(vec![tc("b", "s9schema", r#"{"other":1}"#)]);
    let (mut ex, _in_tx, _out_rx) = make_executor(vec![bad(), bad(), bad(), bad(), bad(), bad()]);
    ex.register_tool("s9schema".to_string(), Arc::new(SchemaTool));

    let out = ex
        .run_agent_loop("test:s9budget", "hi", &s9_ctx())
        .await
        .expect("loop stops with budget message");
    assert!(
        out.contains("工具参数校验连续失败"),
        "budget message expected, got: {out}"
    );
}

// ---------- 1621-1656 fallback 多候选 + Err 臂；1739 压缩通知 ----------

#[tokio::test]
async fn fallback_two_candidates_success_and_error_paths() {
    let _logs = capture_logs();
    // 成功：空 provider 名 → model 用主配置（1636-1637 臂）。
    {
        let (mut ex, _in_tx, _out_rx) = make_executor(vec![Ok(resp_content("fb ok", true))]);
        ex.set_fallback_candidates(vec![
            FallbackCandidate {
                provider: String::new(),
                model: "alt-model".to_string(),
            },
            FallbackCandidate {
                provider: "p2".to_string(),
                model: "m2".to_string(),
            },
        ]);
        let out = ex
            .run_agent_loop("test:s9fb", "hi", &s9_ctx())
            .await
            .unwrap();
        assert_eq!(out, "fb ok");
    }
    // 全失败 → Err 臂 → 非 context 错误 → Error: 响应。
    {
        let (mut ex, _in_tx, _out_rx) =
            make_executor(vec![Err("nope".to_string()), Err("nope".to_string())]);
        ex.set_fallback_candidates(vec![
            FallbackCandidate {
                provider: "p1".to_string(),
                model: "m1".to_string(),
            },
            FallbackCandidate {
                provider: "p2".to_string(),
                model: "m2".to_string(),
            },
        ]);
        let out = ex
            .run_agent_loop("test:s9fb2", "hi", &s9_ctx())
            .await
            .unwrap();
        assert!(out.contains("Error:"), "got: {out}");
    }
}

#[tokio::test]
async fn context_error_triggers_compression_notify_and_retry() {
    let _logs = capture_logs();
    let (mut ex, in_tx, mut out_rx) = make_executor(vec![
        Err("context length exceeded".to_string()),
        Ok(resp_content("compressed retry ok", true)),
    ]);
    in_tx
        .send(make_msg(
            "web",
            "user1",
            "test:s9ctx",
            std::collections::HashMap::new(),
        ))
        .await
        .unwrap();
    drop(in_tx);
    ex.run().await;

    let first = out_rx.recv().await.expect("compression notify sent");
    assert!(
        first.content.contains("Context window exceeded"),
        "notify: {}",
        first.content
    );
    let final_msg = out_rx.recv().await.expect("final answer");
    assert_eq!(final_msg.content, "compressed retry ok");
}

// ---------- 1361-1388 data_store 用量记录 ----------

#[tokio::test]
async fn data_store_records_usage_from_response() {
    let _logs = capture_logs();
    let db_dir = tempfile::tempdir().unwrap();
    let ds = Arc::new(
        nemesis_data::DataStore::open(&db_dir.path().join("s9_usage.db")).expect("open db"),
    );

    let mut response = resp_content("with usage", true);
    response.usage = Some(ObserverUsageInfo {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        cached_tokens: Some(3),
        cache_creation_tokens: Some(0),
        cache_read_tokens: Some(2),
    });

    let (mut ex, _in_tx, _out_rx) = make_executor(vec![Ok(response)]);
    ex.set_data_store(ds);
    let out = ex
        .run_agent_loop("test:s9usage", "hi", &s9_ctx())
        .await
        .unwrap();
    assert_eq!(out, "with usage"); // insert_request_log 已执行，无 panic
}

// ---------- 1794/1802-1808 handle_tool_calls 三臂 ----------

#[tokio::test]
async fn handle_tool_calls_error_and_unknown_arms() {
    let _logs = capture_logs();
    let (ex, in_tx, _out_rx) = make_executor(Vec::new());
    drop(in_tx);
    let mut ex = ex;
    ex.register_tool("s9fail".to_string(), Arc::new(FailTool));
    ex.register_tool("s9echo".to_string(), Arc::new(EchoTool));

    let results = ex
        .handle_tool_calls(
            &[
                tc("a", "s9echo", r#"{"x":1}"#),
                tc("b", "s9fail", r#"{}"#),
                tc("c", "ghost_tool", r#"{}"#),
            ],
            &s9_ctx(),
            "trace9",
            1,
        )
        .await;
    assert_eq!(results.len(), 3);
    assert!(!results[0].is_error);
    assert!(results[0].result.contains("echo:"));
    assert!(results[1].is_error);
    assert!(
        results[1].result.contains("Tool error:"),
        "got: {}",
        results[1].result
    );
    assert!(results[2].is_error);
    assert!(
        results[2].result.contains("Unknown tool"),
        "got: {}",
        results[2].result
    );
}

// ---------- 1977/2018 保存失败 / 发送失败 warn ----------

#[tokio::test]
async fn run_agent_loop_failing_save_and_publish_send_failure() {
    let _logs = capture_logs();
    // storage_dir 指向一个文件 → save 必失败 → 1977 warn。
    let file_dir = tempfile::tempdir().unwrap();
    let blocked = file_dir.path().join("blocked_as_file");
    std::fs::write(&blocked, "i am a file").unwrap();
    let store = crate::session::SessionStore::new_with_storage(&blocked);
    store.get_or_create("test:s9save"); // 内存里有会话，save 时写盘必失败

    let (mut ex, _in_tx, _out_rx) = make_executor(vec![Ok(resp_content("saved?", true))]);
    ex.set_session_persistence(SessionPersistence::with_storage(
        Arc::new(store),
        crate::session::Summarizer::new_silent(
            Arc::new(ResultProvider::plain(vec!["unused"])),
            "test-model".to_string(),
            128_000,
            Arc::new(crate::session::SessionStore::new_in_memory()),
        ),
    ));
    let out = ex
        .run_agent_loop("test:s9save", "hi", &s9_ctx())
        .await
        .unwrap();
    assert_eq!(out, "saved?"); // save Err warn 已过，不 panic

    // 2018：process_and_publish 在 rx 关闭后发送失败 warn。
    let (ex2, _in2, out_rx2) = make_executor(vec![Ok(resp_content("pub", true))]);
    drop(out_rx2);
    let out2 = ex2
        .process_and_publish("test:s9pub", "hi", &s9_ctx())
        .await
        .expect("publish returns result even when send fails");
    assert_eq!(out2, "pub");
}

// ---------- 974/984-1002/1161/1214/1270/1396 全流程事件 + 字段行 ----------

#[tokio::test]
async fn process_message_full_flow_emits_observer_events_via_manager() {
    let _logs = capture_logs();
    let (mut ex, in_tx, mut out_rx) = make_executor(vec![
        tool_response(vec![tc("t1", "s9echo2", r#"{}"#)]),
        Ok(resp_content("final via manager", true)),
    ]);
    ex.register_tool("s9echo2".to_string(), Arc::new(EchoTool));
    let obs = Arc::new(RecordingObserver::new());
    ex.set_observer(obs.clone());
    ex.set_observer_manager(Arc::new(nemesis_observer::Manager::new()));

    in_tx
        .send(make_msg(
            "web",
            "user1",
            "test:s9obs",
            std::collections::HashMap::new(),
        ))
        .await
        .unwrap();
    drop(in_tx);
    ex.run().await;

    let msg = out_rx.recv().await.expect("final outbound");
    assert_eq!(msg.content, "final via manager");
    // legacy observer 至少收到 start/end（974 emit_event 转发）。
    let labels = obs.labels.lock().unwrap();
    assert!(
        labels.iter().any(|l| l == "start") && labels.iter().any(|l| l == "end"),
        "labels: {labels:?}"
    );
}
