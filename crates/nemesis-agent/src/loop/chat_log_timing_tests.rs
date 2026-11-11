//! chat_log 落盘时序测试（2026-09-21 切标签页丢消息修复）。
//!
//! 核心契约：**user 行在 LLM 调用前落盘**（turn 进行中即可）——
//! AI 处理期间（分钟级）用户切走路由再切回，前端 loadHistory 从
//! chat_log 必须能拉到本轮 user 行，不再空视图。assistant 行仍在
//! turn 末落盘，HD「一轮 = jsonl 恰好 +2 行」不变量由两处合计维持
//! （既有 tests.rs::one_turn_appends_exactly_user_and_assistant_rows
//! 继续守护完整轮的行数；本文件钉「进行中可见」的时序半边）。

// （2026-11-09 串扰根治）共享键测试不再持文件级锁：进程单例 home 下的
// agent_main_main.jsonl 是全部默认路由 bus 测试（s9_tests ~30+ 例、本文件
// system 直调例）的公共落盘，无法独占——此前自备的 TIMING_SHARED_KEY_LOCK
// 与 tests.rs 的 CHAT_LOG_INTEGRATION_LOCK 互不互斥、更拦不住不带锁的 s9
// 写方，「恰好 2 行」文件全局断言在并发下天然脆弱（组合过滤跑挂三连复现
// 实证，干净树同样复挂=预先存在）。现断言契约收窄到「本 turn 自己的行」：
// 内容掺 run 级 nonce，跨次运行残留也不误配；顺序/前缀/角色配对契约
// 原样保留。

use super::*;

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
            finish_reason: None,
            escalated_to: None,
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
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
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

/// 未知窗口模型的首个 turn 会落一条「压缩已停用」系统
/// 注记，破坏本文件「完整轮恰好 +2 行」类精确断言。夹具统一给 test-model
/// 钉已知窗口（config.json 唯一依据 fresh-read）→ 自动压缩路径惰性
/// （信号远低于阈值），本文件继续只测 turn 持久化时序本身； 注记行为
/// 由 w1_w2_note_tests 专测。
fn pin_known_window(al: &AgentLoop) {
    let cfg_path = std::env::temp_dir().join(format!(
        "nemesis_test_known_window_{}.json",
        std::process::id()
    ));
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [
                {"model": "test-model", "model_name": "test-model", "context_window": 128000}
            ]
        })
        .to_string(),
    )
    .unwrap();
    al.set_config_path(cfg_path);
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
    pin_known_window(&agent_loop);

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
    let (_agent_id, response, err, _) = handle.await.unwrap();
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

/// system 直调路径（process_system_message，3936 同走
/// `run_agent_loop_internal`）行为一致受益：完整轮仍恰好 +2 行、
/// 首行 user（`[System:...]` 前缀原文）。该路径在 commands.rs:47 强制
/// 使用主会话键（忽略 msg.session_key）→ 落共享文件，断言按本 turn
/// 自己的行（nonce 过滤）收窄，不依赖文件全局行数。
#[tokio::test]
async fn system_message_turn_appends_user_then_assistant_rows() {
    let key = build_agent_main_session_key("main");
    // best-effort 清理：并发写方持句柄时删除失败不碍事（nonce 断言免疫）。
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
    pin_known_window(&agent_loop);

    // system 消息格式：channel="system"、chat_id="origin_channel:origin_chat_id"。
    // 内容掺 run 级 nonce：共享文件里本 turn 的行跨次运行也不误配。
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let content = format!("background result {nonce}");
    let msg = nemesis_types::channel::InboundMessage {
        channel: "system".to_string(),
        sender_id: "tester".to_string(),
        chat_id: "web:chat-sys".to_string(),
        content: content.clone(),
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
    let mine: Vec<&serde_json::Value> = rows
        .iter()
        .filter(|r| {
            r["content"].as_str() == Some(format!("[System: tester] {content}").as_str())
                || r["content"].as_str() == Some("gated reply")
        })
        .collect();
    assert_eq!(
        mine.len(),
        2,
        "system 完整轮 = 本 turn 恰好 +2 行: {rows:?}"
    );
    assert_eq!(mine[0]["role"], "user");
    assert_eq!(
        mine[0]["content"],
        format!("[System: tester] {content}"),
        "首行必须带 [System:] 前缀原文"
    );
    assert_eq!(mine[1]["role"], "assistant");
    assert_eq!(mine[1]["content"], "gated reply");

    cleanup_timing_log(&key);
}
