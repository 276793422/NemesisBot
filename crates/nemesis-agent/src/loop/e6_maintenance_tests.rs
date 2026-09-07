//! E6 (devtool-upgrade 阶段 2)：手动会话维护（`/compact` / `/clear`）测试。
//! - `parse_maintenance_command` 决策表（/compact /clear /带参 /非命令）。
//! - `compact_session`：空会话 Err；mock 摘要成功 → 回执 + store 持久化
//!   （summary + covers 先于 history，回合末持久化同序）；摘要 LLM 失败 →
//!   Err 且 covers/历史不动（2026-08-25 静默失忆修复同一契约）。
//! - `clear_session`：store + chat_log 双清，key 保留（后续 compact 空会话）。
//! - gate 集成：busy 闸回执；串行端到端（Maintenance 短路 → store 副作用）。
//! - `BUILTIN_SLASH_COMMANDS` 防回归（自定义命令表不得遮蔽维护命令）。

use super::*;

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

/// 摘要 LLM 硬失败：驱动 compact_session 的「摘要生成失败」臂。
struct FailingLlmProvider;

#[async_trait]
impl LlmProvider for FailingLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("e6 simulated llm failure".to_string())
    }
}

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        // None：AgentInstance::new 会把 system_prompt 注入为 history[0] 的
        // system turn——这里要让「store N 条 == instance 历史 N 条」的算术
        // 透明可断言，不带隐式 +1。
        system_prompt: None,
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

fn inbound_msg(content: &str, session_key: &str) -> nemesis_types::channel::InboundMessage {
    nemesis_types::channel::InboundMessage {
        channel: "web".to_string(),
        sender_id: "e6user".to_string(),
        chat_id: "e6chat".to_string(),
        content: content.to_string(),
        media: vec![],
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

/// 进程内唯一 key（时间戳纳秒 + 标签），chat_log 落盘测试互不干扰。
fn unique_key(tag: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("agent:main:session:e6_{}_{}", tag, nanos)
}

/// store 预填 N 条消息（add_message 需先 get_or_create 建条目，否则静默丢弃）。
fn populate(store: &crate::session::SessionStore, key: &str, n: usize) {
    store.get_or_create(key);
    for i in 0..n {
        store.add_message(key, "user", &format!("e6 msg {i}"));
    }
}

// ---------- parse_maintenance_command ----------

#[test]
fn parse_compact_and_clear() {
    let (kind, receipt) = parse_maintenance_command("/compact").unwrap();
    assert_eq!(kind, SessionMaintenance::Compact);
    assert!(receipt.contains("压缩"));
    let (kind, receipt) = parse_maintenance_command("/clear").unwrap();
    assert_eq!(kind, SessionMaintenance::Clear);
    assert!(receipt.contains("清空"));
}

#[test]
fn parse_maintenance_extra_args_ignored() {
    // 多余参数忽略：仍按命令名识别。
    let (kind, _) = parse_maintenance_command("/compact  请尽量激进一些").unwrap();
    assert_eq!(kind, SessionMaintenance::Compact);
    let (kind, _) = parse_maintenance_command("  /clear  ").unwrap();
    assert_eq!(kind, SessionMaintenance::Clear);
}

#[test]
fn parse_non_commands_return_none() {
    assert!(parse_maintenance_command("/unknown").is_none());
    assert!(parse_maintenance_command("hello").is_none());
    assert!(parse_maintenance_command("").is_none());
    // 大小写敏感：/Compact 不是维护命令。
    assert!(parse_maintenance_command("/Compact").is_none());
}

#[test]
fn builtin_slash_commands_include_maintenance() {
    // 防回归：rewrite_custom_command 按 BUILTIN 跳过内置名——维护命令若被
    // 自定义命令表遮蔽，gate 拦截就永远轮不到（rewrite 先行改写 content）。
    assert!(AgentLoop::BUILTIN_SLASH_COMMANDS.contains(&"compact"));
    assert!(AgentLoop::BUILTIN_SLASH_COMMANDS.contains(&"clear"));
}

// ---------- compact_session ----------

#[tokio::test]
async fn compact_empty_session_errors() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    al.set_session_store(store.clone());
    let key = unique_key("empty");
    let err = al.compact_session(&key).await.unwrap_err();
    assert!(err.contains("会话为空"), "unexpected err: {err}");
}

#[tokio::test]
async fn compact_success_persists_summary_and_receipt() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![resp("E6 SUMMARY TEXT")])),
        test_config(),
    );
    al.set_session_store(store.clone());
    let key = unique_key("ok");
    populate(&store, &key, 5);

    let receipt = al.compact_session(&key).await.unwrap();
    // covers = 全量 - 保留尾（SMALL_K_FORCE）；纯文本无 tool 对，边界即 raw。
    let covers = 5 - SMALL_K_FORCE;
    assert_eq!(
        receipt,
        format!("✓ 已压缩：摘要覆盖前 {covers} 条，保留近 {SMALL_K_FORCE} 条")
    );
    // 持久化契约：摘要 + covers 落 store（成功路径）。
    assert_eq!(store.get_summary(&key), "E6 SUMMARY TEXT");
    assert_eq!(store.get_summary_covers_up_to(&key), Some(covers));
}

#[tokio::test]
async fn compact_llm_failure_keeps_history() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(FailingLlmProvider), test_config());
    al.set_session_store(store.clone());
    let key = unique_key("fail");
    populate(&store, &key, 5);

    let err = al.compact_session(&key).await.unwrap_err();
    assert!(err.contains("摘要生成失败"), "unexpected err: {err}");
    // 静默失忆契约：covers 不推进、store 无摘要、历史原样。
    assert!(store.get_summary(&key).is_empty());
    assert_eq!(store.get_summary_covers_up_to(&key), None);
    assert_eq!(store.get_history(&key).len(), 5);
}

// ---------- clear_session ----------

#[tokio::test]
async fn clear_session_clears_store_and_chat_log() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    al.set_session_store(store.clone());
    let key = unique_key("clear");
    populate(&store, &key, 2);
    crate::chat_log::append_chat_log(&key, "user", "e6 before clear");

    let receipt = al.clear_session(&key).await.unwrap();
    assert_eq!(receipt, "✓ 已清空会话历史");
    assert!(store.get_history(&key).is_empty());
    // chat_log 截断（jsonl-first）：可读内容归零。
    let (entries, total, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 0, "chat_log not truncated: {entries:?}");

    // key 保留：清空后再 compact 走「会话为空」臂（会话继续可用）。
    let err = al.compact_session(&key).await.unwrap_err();
    assert!(err.contains("会话为空"));
    crate::chat_log::delete_chat_log(&key);
}

// ---------- gate 集成 ----------

#[tokio::test]
async fn gate_maintenance_busy_falls_back() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![resp("SHOULD NOT BE USED")])),
        test_config(),
    );
    al.set_session_store(store.clone());
    let key = unique_key("busy");
    populate(&store, &key, 5);

    // 预占会话：模拟同 key 消息正在处理。
    assert!(al.try_acquire_session(&key));
    let msg = inbound_msg("/compact", &key);
    let (agent_id, response, err) = al.process_inbound_message(&msg).await;
    assert!(err.is_none());
    assert!(agent_id.is_empty());
    assert!(
        response.contains("已忽略"),
        "unexpected response: {response}"
    );
    // busy 路径不消费 LLM、不动 store。
    assert!(store.get_summary(&key).is_empty());
    assert_eq!(store.get_history(&key).len(), 5);
    al.release_session(&key);
}

#[tokio::test]
async fn gate_maintenance_end_to_end_serial() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(
        Box::new(MockLlmProvider::new(vec![resp("E6 GATE SUMMARY")])),
        test_config(),
    );
    al.set_session_store(store.clone());
    let key = unique_key("e2e");
    populate(&store, &key, 5);

    // 串行路径：gate 短路 Maintenance → ⏳ 回执 + 压缩 + ✓ 回执均内联发布，
    // 返回空 triple（调用方 finish_message 早退，不重复发布）。
    let msg = inbound_msg("/compact", &key);
    let (agent_id, response, err) = al.process_inbound_message(&msg).await;
    assert!(err.is_none());
    assert!(agent_id.is_empty());
    assert!(
        response.is_empty(),
        "expected empty response, got: {response}"
    );
    // 会话锁已由 tail 释放：同 key 立即重新可获取（获取后释放复原）。
    assert!(
        al.try_acquire_session(&key),
        "session lock not released by tail"
    );
    al.release_session(&key);
    // 副作用：摘要 + covers 持久化（gate → handle_maintenance → compact_session）。
    assert_eq!(store.get_summary(&key), "E6 GATE SUMMARY");
    assert_eq!(
        store.get_summary_covers_up_to(&key),
        Some(5 - SMALL_K_FORCE)
    );
}

#[tokio::test]
async fn gate_clear_end_to_end_serial() {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(MockLlmProvider::new(vec![])), test_config());
    al.set_session_store(store.clone());
    let key = unique_key("gateclear");
    populate(&store, &key, 2);
    crate::chat_log::append_chat_log(&key, "user", "e6 gate clear");

    let msg = inbound_msg("/clear", &key);
    let (_, response, err) = al.process_inbound_message(&msg).await;
    assert!(err.is_none());
    assert!(response.is_empty());
    assert!(store.get_history(&key).is_empty());
    let (_, total, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 0);
    crate::chat_log::delete_chat_log(&key);
}
