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
        tool_fence_profession: None,
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

/// P8（2026-10-10 集群无人值守硬化 A2）：`session_tokens_used` 观测字段。
/// 预算关闭（无 config_path）也能查——账目直读累计器；未记账会话 = 0。
#[test]
fn session_tokens_used_reports_budget_ledger() {
    let (al, _store) = m5_loop();
    let key = "agent:main:session:tokens";
    al.record_session_tokens(key, 1234);
    al.record_session_tokens(key, 11);
    let st = al.session_context_status(key);
    assert_eq!(st["session_tokens_used"], 1245, "累计器增量叠加");
    // 未记账会话（无底账）= 0，不是缺失/错误。
    let st2 = al.session_context_status("agent:main:session:no-tokens");
    assert_eq!(st2["session_tokens_used"], 0);
}

/// R2 2026-10-22 回归锁：`record_round_usage` 的预算 delta = 完整输入
/// （prompt + cache_creation + cache_read，`usage_full_input` 单一真相源）
/// + output——Anthropic 形态（creation>0）不再漏计。走真实 record_round_usage
/// 接线（observer 落账 + 预算累计），不手捏 delta。
#[tokio::test]
async fn record_round_usage_budget_delta_includes_cache_creation() {
    let dir = tempfile::tempdir().unwrap();
    let (mut al, _store) = m5_loop();
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({"agents": {"defaults": {"session_token_budget": 100_000}}}).to_string(),
    )
    .unwrap();
    al.set_config_path(dir.path().join("config.json"));
    let ds = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.path().join("budget_delta.db")).unwrap(),
    );
    al.set_data_store(ds);

    let key = "agent:main:session:budget-delta";
    let context = RequestContext::new("web", "chat-budget", "user-budget", key);
    let mut response = LlmResponse {
        finish_reason: None,
        escalated_to: None,
        content: "ok".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: Some(crate::loop_executor::ObserverUsageInfo {
            prompt_tokens: 100,
            completion_tokens: 30,
            total_tokens: 155,
            cached_tokens: None,
            cache_creation_tokens: Some(10),
            cache_read_tokens: Some(15),
        }),
        raw_request_body: None,
        raw_response_body: None,
    };

    al.record_round_usage(
        &context,
        "tr-budget",
        &mut response,
        std::time::Duration::from_millis(5),
        1,
    )
    .await;

    // 完整输入三列 100+10+15=125，+output 30 = 155（漏计形态给 145）。
    assert_eq!(al.session_tokens_used(key), 155);
    // 落账行同口径可回查（seed-on-miss 从底账种入不得互相打架）。
    let agg = al
        .data_store
        .as_deref()
        .unwrap()
        .aggregate_session_usage(key)
        .unwrap();
    assert_eq!(agg.input_tokens, 125);
    assert_eq!(agg.output_tokens, 30);
}

/// 2026-10-20 压缩点一等公民化：`trigger`/`trigger_source`/`pct_trigger`
/// 观测字段。无用户键 → trigger = 窗口×75%（instance 兜底语义）；设了
/// `compact_context_tokens` → 到点即压 + source=user。
#[test]
fn context_status_reports_trigger_fields() {
    let (al, store) = m5_loop();
    let key = "agent:main:session:trig";
    store.get_or_create(key);
    store.add_message(key, "user", "hello trigger fields");

    // 无 config_path / 无价目表 → 三级链未命中 → 实例兜底 128k。
    let st = al.session_context_status(key);
    assert_eq!(st["trigger"], 96_000, "128k × 75% 既有行为");
    assert_eq!(st["trigger_source"], "instance");
    let used = st["used_tokens"].as_u64().unwrap() as usize;
    assert_eq!(
        st["pct_trigger"],
        (used * 100 / 96_000).min(100),
        "压缩点百分比与 pct 同公式"
    );
}

/// 设了 `compact_context_tokens`：trigger 照用户值、source=user；
/// pct_trigger 相对压缩点（而非窗口）。
#[test]
fn context_status_user_compact_point_overrides() {
    let dir = std::env::temp_dir().join(format!("nb-m5-cc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg_path = dir.join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({
            "model_list": [{"model_name": "test-model", "compact_context_tokens": 1000}]
        })
        .to_string(),
    )
    .unwrap();

    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(NoopProvider), local_test_config());
    al.set_session_store(store.clone());
    al.set_config_path(cfg_path.clone());
    let key = "agent:main:session:ucc";
    store.get_or_create(key);
    store.add_message(key, "user", "hello user compact point");

    let st = al.session_context_status(key);
    assert_eq!(st["trigger"], 1000, "到点即压，不乘比例");
    assert_eq!(st["trigger_source"], "user");
    assert_eq!(st["window"], 128_000, "真实窗口语义不变（三级链兜底）");
    let used = st["used_tokens"].as_u64().unwrap() as usize;
    assert_eq!(st["pct_trigger"], (used * 100 / 1000).min(100));
    let _ = std::fs::remove_dir_all(&dir);
}
