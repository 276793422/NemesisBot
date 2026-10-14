//! P8 会话 token 预算闸测试（集群无人值守硬化 A2；独立测试文件——
//! check-inline-tests 纪律，lib.rs `#[path]` 声明）。
//!
//! 覆盖：账本 seed 防双计（record 首记吃底账 / current 首查吃底账 /
//! 增量叠加）/ 多会话隔离 / soft 一次性闩 / 容量上限清账 / 文案 marker
//! 契约 / loop 侧顶检器官（hard ≥100% Error 终局 + soft ≥75% 一次提示 +
//! budget=0 直通 + 无内存账目底账种入）。

use crate::test_support::capture_logs;
use crate::token_budget::{
    SESSION_TOKEN_BUDGET_MARKER, SessionTokenBudgetLedger, entry_reject_notice, hard_stop_notice,
};

use async_trait::async_trait;

// ---------- 本地 mock（与 loop/s9_tests.rs 同构，模块私有不可共享） ----------

struct MockLlmProvider;

#[async_trait]
impl crate::r#loop::LlmProvider for MockLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Ok(crate::r#loop::LlmResponse {
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

fn test_agent_loop() -> crate::r#loop::AgentLoop {
    crate::r#loop::AgentLoop::new(Box::new(MockLlmProvider), test_agent_config())
}

fn test_agent_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("test".to_string()),
        max_turns: 5,
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    }
}

fn test_instance() -> crate::AgentInstance {
    crate::AgentInstance::new(test_agent_config())
}

fn test_context(session_key: &str) -> crate::RequestContext {
    crate::RequestContext {
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        user: "u1".to_string(),
        session_key: session_key.to_string(),
        correlation_id: None,
        async_callback: None,
        tool_path_base: None,
        approval_ref: None,
    }
}

/// 带用量的 RequestLog 行（seed 数据构造；input/cache_read/output 语义与
/// observer.rs 落账一致）。
fn usage_row(
    session_key: &str,
    input: i64,
    cache_read: i64,
    output: i64,
) -> nemesis_data::RequestLog {
    nemesis_data::RequestLog {
        id: 0,
        trace_id: format!("t-{session_key}-{input}-{output}"),
        model: "test-model".to_string(),
        provider_type: String::new(),
        input_tokens: input,
        output_tokens: output,
        cache_creation_tokens: 0,
        cache_read_tokens: cache_read,
        total_cost_usd: 0.0,
        latency_ms: 1,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 0,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: session_key.to_string(),
    }
}

// ---------- 账本（纯类型） ----------

#[test]
fn record_without_ds_sums_deltas() {
    let _logs = capture_logs();
    let ledger = SessionTokenBudgetLedger::default();
    ledger.record(None, "s1", 100);
    ledger.record(None, "s1", 50);
    assert_eq!(ledger.current(None, "s1"), 150);
}

#[test]
fn seed_on_first_current_reads_db_aggregate() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let ds = nemesis_data::DataStore::open(&dir.path().join("tb-seed.db")).unwrap();
    // 口径 = input（含 cache_read）+ output：行1 (100+20)+30、行2 10+5。
    ds.insert_request_log(&usage_row("sess-a", 100, 20, 30))
        .unwrap();
    ds.insert_request_log(&usage_row("sess-a", 10, 0, 5))
        .unwrap();

    let ledger = SessionTokenBudgetLedger::default();
    assert_eq!(ledger.current(Some(&ds), "sess-a"), 165);
    // 第二次读走内存账目，值稳定。
    assert_eq!(ledger.current(Some(&ds), "sess-a"), 165);
    // 无行会话 = 全零聚合（不是错误）。
    assert_eq!(ledger.current(Some(&ds), "sess-empty"), 0);
}

#[test]
fn record_seed_includes_current_round_no_double_count() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let ds = nemesis_data::DataStore::open(&dir.path().join("tb-nodup.db")).unwrap();
    // 模拟 observer 时序：本轮 insert_request_log 已落账（100+10+40=150），
    // 随后 record(delta=150)——账目缺失路径种入底账，delta 不再叠加。
    ds.insert_request_log(&usage_row("sess-b", 100, 10, 40))
        .unwrap();

    let ledger = SessionTokenBudgetLedger::default();
    ledger.record(Some(&ds), "sess-b", 150);
    assert_eq!(ledger.current(Some(&ds), "sess-b"), 150);

    // 下一轮：底账再落 30 → record(30) 走「账目存在」增量臂。
    ds.insert_request_log(&usage_row("sess-b", 20, 0, 10))
        .unwrap();
    ledger.record(Some(&ds), "sess-b", 30);
    assert_eq!(ledger.current(Some(&ds), "sess-b"), 180);
}

#[test]
fn sessions_are_isolated() {
    let _logs = capture_logs();
    let ledger = SessionTokenBudgetLedger::default();
    ledger.record(None, "s-a", 10);
    ledger.record(None, "s-b", 999);
    assert_eq!(ledger.current(None, "s-a"), 10);
    assert_eq!(ledger.current(None, "s-b"), 999);
}

#[test]
fn soft_latch_fires_once_per_session() {
    let _logs = capture_logs();
    let ledger = SessionTokenBudgetLedger::default();
    assert!(ledger.soft_latch("s1")); // 首次越过 = true
    assert!(!ledger.soft_latch("s1")); // 二次 = 闩住
    assert!(ledger.soft_latch("s2")); // 会话隔离
}

#[test]
fn capacity_overflow_clears_ledger() {
    let _logs = capture_logs();
    let ledger = SessionTokenBudgetLedger::default();
    ledger.record(None, "k0", 7);
    for i in 0..2000 {
        ledger.record(None, &format!("flood-{i}"), 1);
    }
    // 超限全清（512 上限）→ 老账目没了；再读 = 重种入（无 ds → 0）。
    assert_eq!(ledger.current(None, "k0"), 0);
}

#[test]
fn notice_texts_start_with_marker() {
    let hard = hard_stop_notice(1500, 1000);
    let reject = entry_reject_notice(1500, 1000);
    assert!(hard.starts_with(SESSION_TOKEN_BUDGET_MARKER));
    assert!(reject.starts_with(SESSION_TOKEN_BUDGET_MARKER));
    // 两类文案都带占比（人工可读）；入口文案点名 config 键。
    assert!(hard.contains("1500/1000"));
    assert!(reject.contains("1500/1000"));
    assert!(reject.contains("session_token_budget"));
}

// ---------- loop 侧顶检器官（check_session_budget） ----------

/// config_path 注入 + budget 写入的测试装配。
fn agent_loop_with_budget(dir: &std::path::Path, budget: u64) -> crate::r#loop::AgentLoop {
    let agent_loop = test_agent_loop();
    let cfg_path = dir.join("config.json");
    std::fs::write(
        &cfg_path,
        format!(r#"{{"agents":{{"defaults":{{"session_token_budget":{budget}}}}}}}"#),
    )
    .unwrap();
    agent_loop.set_config_path(cfg_path);
    agent_loop
}

#[tokio::test]
async fn organ_budget_zero_is_pass_through() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let agent_loop = agent_loop_with_budget(dir.path(), 0);
    let instance = test_instance();
    let ctx = test_context("sess-organ-0");
    // 预置账目：即便已「超限」，budget=0（关）恒直通。
    agent_loop.record_session_tokens(&ctx.session_key, 999_999);
    assert!(
        agent_loop
            .check_session_budget(&instance, &ctx)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn organ_hard_stop_at_100_percent() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let agent_loop = agent_loop_with_budget(dir.path(), 1000);
    let instance = test_instance();
    let ctx = test_context("sess-organ-hard");
    agent_loop.record_session_tokens(&ctx.session_key, 1000);
    let ev = agent_loop
        .check_session_budget(&instance, &ctx)
        .await
        .expect("hard stop must yield event");
    // hard 终局 = Error 事件，文案带 marker（worker classify 契约）。
    let crate::types::AgentEvent::Error(text) = ev else {
        panic!("expected AgentEvent::Error");
    };
    assert!(text.starts_with(SESSION_TOKEN_BUDGET_MARKER), "got: {text}");
    // 停机提示入历史（下一上下文可见停机原因）。
    assert!(
        instance
            .get_history()
            .iter()
            .any(|t| t.content.contains(SESSION_TOKEN_BUDGET_MARKER))
    );
}

#[tokio::test]
async fn organ_soft_notice_fires_once_below_100() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let agent_loop = agent_loop_with_budget(dir.path(), 1000);
    let instance = test_instance();
    let ctx = test_context("sess-organ-soft");
    agent_loop.record_session_tokens(&ctx.session_key, 760);
    // 760 ≥ 75% 且 < 100% → None（继续）+ 一次性闩。
    assert!(
        agent_loop
            .check_session_budget(&instance, &ctx)
            .await
            .is_none()
    );
    // 第二轮同用量：闩住，仍 None（软档不终局、不重复触发）。
    assert!(
        agent_loop
            .check_session_budget(&instance, &ctx)
            .await
            .is_none()
    );
    // soft 线之下（<75%）不触发。
    let ctx2 = test_context("sess-organ-under");
    agent_loop.record_session_tokens(&ctx2.session_key, 100);
    assert!(
        agent_loop
            .check_session_budget(&instance, &ctx2)
            .await
            .is_none()
    );
}

#[tokio::test]
async fn organ_seeds_from_db_when_no_entry() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let mut agent_loop = agent_loop_with_budget(dir.path(), 500);
    let ds = nemesis_data::DataStore::open(&dir.path().join("tb-organ.db")).unwrap();
    ds.insert_request_log(&usage_row("sess-organ-seed", 400, 50, 60))
        .unwrap();
    agent_loop.set_data_store(std::sync::Arc::new(ds));

    let instance = test_instance();
    let ctx = test_context("sess-organ-seed");
    // 无内存账目 → 从底账种入 510 ≥ 500 → hard 终局（防 loop 重启丢账）。
    let ev = agent_loop
        .check_session_budget(&instance, &ctx)
        .await
        .expect("seeded usage over budget must hard stop");
    let crate::types::AgentEvent::Error(text) = ev else {
        panic!("expected AgentEvent::Error");
    };
    assert!(text.starts_with(SESSION_TOKEN_BUDGET_MARKER));
}
