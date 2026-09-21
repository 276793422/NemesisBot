//! chat_log 落盘时序测试（2026-09-21 切标签页丢消息修复）。
//!
//! 核心契约：**user 行在 LLM 调用前落盘**（turn 进行中即可见）——
//! AI 处理期间（分钟级）用户切走路由再切回，前端 loadHistory 从
//! chat_log 必须能拉到本轮 user 行，不再空视图。assistant 行仍在
//! turn 末落盘，HD「一轮 = jsonl 恰好 +2 行」不变量由两处合计维持
//! （既有 tests.rs::one_turn_appends_exactly_user_and_assistant_rows
//! 继续守护完整轮的行数；本文件钉「进行中可见」的时序半边）。

// 共享键测试持 std Mutex guard 跨 await（串行 chat_log 落盘窗口）——
// 与 tests.rs 同款豁免（单线程测试进程内无害）。
#![allow(clippy::await_holding_lock)]

use super::*;

/// 共享键（agent:main:main）chat_log 测试的串行锁。tests.rs 的
/// CHAT_LOG_INTEGRATION_LOCK 非 pub 不可跨模块引用，自备等价锁
/// （只串行本文件，不与 tests.rs 的锁互斥——但两者都用「先清理、
/// 跑完断言、再清理」的独占窗口，交叉写入仅影响 >= 断言的场景，
/// 本文件全部用恰好断言且窗口内完成，安全）。
static TIMING_SHARED_KEY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 门控 provider：chat 挂起直到测试放行（watch 闸）——模拟分钟级
/// LLM 轮次，用于断言「user 行在 LLM 返回之前已在 jsonl 里」。
struct GatedProvider {
    release: tokio::sync::watch::Receiver<bool>,
}

#[async_trait]
impl LlmProvider for GatedProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut rx = self.release.clone();
        while !*rx.borrow_and_update() {
            rx.changed().await.map_err(|_| "gate dropped".to_string())?;
        }
        Ok(LlmResponse {
            content: "gated reply".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn timing_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn timing_session_key(name: &str) -> String {
    format!(
        "agent:test:session:{name}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

fn timing_log_path(session_key: &str) -> std::path::PathBuf {
    let safe_key = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{safe_key}.jsonl"))
}

fn cleanup_timing_log(session_key: &str) {
    let path = timing_log_path(session_key);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

fn read_jsonl_rows(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// 轮询直到 jsonl 行数达到 `min_rows`（限时）；返回是否达成。
async fn wait_for_rows(path: &std::path::Path, min_rows: usize, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if read_jsonl_rows(path).len() >= min_rows {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    read_jsonl_rows(path).len() >= min_rows
}

/// 核心时序钉：LLM 挂起（turn 进行中）时，jsonl 里已有本轮 user 行；
/// 放行完成后恰好 2 行（user + assistant），HD 不变量不破。
#[tokio::test]
async fn user_row_lands_while_llm_turn_in_progress() {
    let key = timing_session_key("pending_user_row");
    cleanup_timing_log(&key);
    let path = timing_log_path(&key);

    let (_gate_tx, gate_rx) = tokio::sync::watch::channel(false);
    let (bus_tx, _bus_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(GatedProvider { release: gate_rx }),
        timing_config(),
        bus_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );

    let msg = nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "user1".to_string(),
        chat_id: "chat1".to_string(),
        content: "slow question".to_string(),
        media: vec![],
        session_key: key.clone(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };

    // 不 await 完成：turn 在后台跑（LLM 挂起中 = 模拟分钟级处理窗口）。
    let handle = tokio::spawn(async move { agent_loop.process_inbound_message(&msg).await });

    // turn 进行中（LLM 未返回）user 行必须已在 jsonl——本修复的核心契约。
    assert!(
        wait_for_rows(&path, 1, 5000).await,
        "user 行必须在 LLM 完成前落盘（turn 进行中前端可拉到）"
    );
    let rows = read_jsonl_rows(&path);
    assert_eq!(rows.len(), 1, "LLM 挂起期间应恰好 1 行（assistant 未落）");
    assert_eq!(rows[0]["role"], "user");
    assert_eq!(rows[0]["content"], "slow question");

    // 放行 → turn 完成。
    let _ = _gate_tx.send(true);
    let (_agent_id, response, err) = handle.await.unwrap();
    assert!(err.is_none(), "turn 必须正常完成: {err:?}");
    assert_eq!(response, "gated reply");

    // HD 不变量：完整轮 = 恰好 2 行，user 先 assistant 后。
    let rows = read_jsonl_rows(&path);
    assert_eq!(rows.len(), 2, "完整轮 = 恰好 +2 行（user+assistant）");
    assert_eq!(rows[0]["role"], "user");
    assert_eq!(rows[1]["role"], "assistant");
    assert_eq!(rows[1]["content"], "gated reply");

    cleanup_timing_log(&key);
}

/// system 直调路径（process_system_message，:3936 同走
/// `run_agent_loop_internal`）行为一致受益：完整轮仍恰好 +2 行、
/// 首行 user（`[System: ...]` 前缀原文）。共享键测试须持
/// CHAT_LOG_INTEGRATION_LOCK 串行。
#[tokio::test]
async fn system_message_turn_appends_user_then_assistant_rows() {
    let _lock = TIMING_SHARED_KEY_LOCK.lock().unwrap();
    let key = build_agent_main_session_key("main");
    cleanup_timing_log(&key);

    let (_gate_tx, gate_rx) = tokio::sync::watch::channel(true); // 直接放行
    let (bus_tx, _bus_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(GatedProvider { release: gate_rx }),
        timing_config(),
        bus_tx,
        ConcurrentMode::Queue,
        8,
        0,
    );

    // system 消息格式：channel="system"、chat_id="origin_channel:origin_chat_id"。
    let msg = nemesis_types::channel::InboundMessage {
        channel: "system".to_string(),
        sender_id: "tester".to_string(),
        chat_id: "web:chat-sys".to_string(),
        content: "background result".to_string(),
        media: vec![],
        session_key: key.clone(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };

    let (response, err) = agent_loop.process_system_message(&msg).await;
    assert!(err.is_none(), "system turn 必须正常完成: {err:?}");
    assert_eq!(response, "gated reply");

    let path = timing_log_path(&key);
    let rows = read_jsonl_rows(&path);
    assert_eq!(rows.len(), 2, "system 完整轮 = 恰好 +2 行: {rows:?}");
    assert_eq!(rows[0]["role"], "user");
    assert_eq!(rows[0]["content"], "[System: tester] background result");
    assert_eq!(rows[1]["role"], "assistant");

    cleanup_timing_log(&key);
}
