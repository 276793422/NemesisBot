//! M5（devtool-upgrade 阶段 3）：`AgentLoop::session_context_status` 测试。
//!
//! 契约：与压缩压力测量**同一公式**（摘要覆盖点之后的逐字尾部 ×
//! MODEL-FACING 投影估算 ÷ 三级解析链窗口），即显示的百分比 =
//! 「下一轮真实占用」。注意 covers_up_to 是**含 system turn（index 0）
//! 的全历史索引**（SummaryCache 不变量），system prompt 每请求必发，
//! 计入 used 是正确口径。四个场景：空会话 / 无摘要全量 / 有摘要截尾 /
//! covers 越界钳制。

use super::*;
use crate::{ChatOptions, ToolDefinition};

/// 最小 config（system_prompt 与 instance.rs 注入路径联动，被断言计数）。
fn local_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

/// 空 provider——测量是纯读路径，不发 LLM 请求，占位即可。
struct NoopProvider;
#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("noop".to_string())
    }
}

fn m5_loop() -> (AgentLoop, std::sync::Arc<crate::session::SessionStore>) {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(NoopProvider), local_test_config());
    al.set_session_store(store.clone());
    (al, store)
}

/// 用与重建后完全相同的 turns 序列走同一估算函数得期望值（测的是
/// session_context_status 的接线与口径，不重测估算器本身）。
fn expected_used(turns: &[crate::types::ConversationTurn]) -> usize {
    crate::session::estimate_tokens_for_turns_projected(turns)
}

/// 实例重建后 index 0 恒为 config 的 system prompt（set_history 保留语义）。
fn sys_turn() -> crate::types::ConversationTurn {
    crate::types::ConversationTurn {
        role: "system".to_string(),
        content: "You are a test assistant.".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

fn stored_turns(messages: &[crate::session::StoredMessage]) -> Vec<crate::types::ConversationTurn> {
    messages.iter().map(|m| m.clone().into()).collect()
}

#[test]
fn empty_session_counts_only_system_prompt() {
    let (al, _store) = m5_loop();
    let st = al.session_context_status("agent:main:session:empty");
    assert_eq!(st["history_len"], 1, "仅 system turn（无历史）");
    assert_eq!(st["covers_up_to"], 0);
    assert_eq!(st["summarized"], false);
    // 无 config / 无价目表 → 三级链未命中 → 实例默认（= FALLBACK 128k）。
    assert_eq!(st["window"], 128_000);
    let used = expected_used(&[sys_turn()]);
    assert_eq!(
        st["used_tokens"], used,
        "system prompt 每请求必发，计入占用"
    );
    assert_eq!(st["pct"], (used * 100 / 128_000).min(100));
}

#[test]
fn history_from_store_measured_verbatim() {
    let (al, store) = m5_loop();
    let key = "agent:main:session:verbatim";
    store.get_or_create(key);
    let contents = [
        "Help me write a function that parses INI files.",
        "Sure, here is a small INI parser in Rust. [moderately long answer]",
        "Now add unit tests for the section header edge cases.",
        "Done — three tests added, covering empty lines and comments too.",
    ];
    for (i, c) in contents.iter().enumerate() {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        store.add_message(key, role, c);
    }

    let st = al.session_context_status(key);
    assert_eq!(st["history_len"], 5, "system turn + 4 条播种消息");
    assert_eq!(st["covers_up_to"], 0, "无摘要 → 全量逐字尾部");
    assert_eq!(st["summarized"], false);

    let mut turns = vec![sys_turn()];
    turns.extend(stored_turns(&store.get_or_create(key).messages));
    let used = expected_used(&turns);
    assert_eq!(st["used_tokens"], used);
    assert_eq!(st["pct"], (used * 100 / 128_000).min(100));
}

#[test]
fn summary_cover_point_excludes_summarized_prefix() {
    let (al, store) = m5_loop();
    let key = "agent:main:session:summarized";
    store.get_or_create(key);
    for i in 0..10 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        store.add_message(key, role, &format!("turn {i}: {}", "detail ".repeat(20)));
    }
    // covers=6 是全历史索引（[sys, s0..s9] 的 index 6 = s5）。
    store.set_summary(key, "Earlier: user and agent discussed turns 0-4.");
    store.set_summary_covers_up_to(key, Some(6));

    let st = al.session_context_status(key);
    assert_eq!(st["history_len"], 11);
    assert_eq!(st["covers_up_to"], 6);
    assert_eq!(st["summarized"], true);

    let messages = store.get_or_create(key).messages;
    let tail = expected_used(&stored_turns(&messages[5..]));
    let mut full_turns = vec![sys_turn()];
    full_turns.extend(stored_turns(&messages));
    let full = expected_used(&full_turns);
    assert_eq!(st["used_tokens"], tail, "只计覆盖点之后的逐字尾部");
    assert!(
        tail < full,
        "覆盖点之后的尾部必须小于全量（摘要前缀不计入下一轮占用）"
    );
}

#[test]
fn covers_clamped_into_history_bounds() {
    let (al, store) = m5_loop();
    let key = "agent:main:session:clamp";
    store.get_or_create(key);
    store.add_message(key, "user", "only one message");
    // 病态数据：covers 超出历史长度（store 手改/旧版本残留）。
    store.set_summary(key, "stale summary");
    store.set_summary_covers_up_to(key, Some(99));

    let st = al.session_context_status(key);
    assert_eq!(st["history_len"], 2, "system turn + 1 条播种消息");
    assert_eq!(st["covers_up_to"], 2, "covers 必须钳到 history.len()");
    assert_eq!(st["used_tokens"], 0, "尾部为空 → 占用 0");
    assert_eq!(st["pct"], 0);
}
