//! S8（2026-11-06 B29/B30 设计 §4）aux 杂务账本归因测试：
//!
//! - title / summary / branch_summary 三族落账行断言：usage 在场 = 真实
//!   token + 计价同源（organ-5 主链行形态对齐）；usage 缺失 = estimated
//!   注记 + token/计价列全零（估算绝不冒充实测入计价）；
//! - 失败路径零行（空输出重试耗尽 / provider 恒败：只在成功返回处落账）；
//! - 重试成功恰一行（fail-then-succeed = 2 次调用 1 条账本行）。

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn test_config() -> AgentConfig {
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

/// aux 通道可控 provider（兄弟测试模块不能互相 import，fixture 自建）：
/// - `fail_until`：第 N 次调用前恒 Err（1 = 仅首败重试成功；u64::MAX = 恒败）；
/// - `empty_output`：返回空白输出（触发护栏空输出拒绝 → 单次重试）；
/// - 其余返回 `content` + `usage`。
struct AuxProvider {
    fail_until: u64,
    empty_output: bool,
    content: String,
    usage: Option<crate::loop_executor::ObserverUsageInfo>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for AuxProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) as u64;
        if n < self.fail_until {
            return Err("transient provider boom".to_string());
        }
        if self.empty_output {
            return Ok(LlmResponse {
                content: "   \n".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                finish_reason: Some("stop".to_string()),
                escalated_to: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            });
        }
        Ok(LlmResponse {
            content: self.content.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            finish_reason: Some("stop".to_string()),
            escalated_to: None,
            usage: self.usage.clone(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn usage(prompt: i64, completion: i64) -> crate::loop_executor::ObserverUsageInfo {
    crate::loop_executor::ObserverUsageInfo {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: prompt + completion,
        cached_tokens: None,
        cache_creation_tokens: Some(3),
        cache_read_tokens: Some(4),
    }
}

fn no_filter() -> nemesis_data::LogFilter {
    nemesis_data::LogFilter {
        model: None,
        status: None,
        session_key: None,
    }
}

fn loop_with_store(db_name: &str) -> (tempfile::TempDir, Arc<nemesis_data::DataStore>, AgentLoop) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ds =
        Arc::new(nemesis_data::DataStore::open(&dir.path().join(db_name)).expect("open datastore"));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(AuxProvider {
            fail_until: u64::MAX,
            empty_output: false,
            content: String::new(),
            usage: None,
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_data_store(ds.clone());
    (dir, ds, agent_loop)
}

// -- A. title 族：usage 在场 = 真实 token + 计价同源 --------------------------

#[tokio::test]
async fn s8_title_row_with_usage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ds = Arc::new(
        nemesis_data::DataStore::open(&dir.path().join("s8_title.db")).expect("open datastore"),
    );
    let provider = Arc::new(AuxProvider {
        fail_until: 0,
        empty_output: false,
        content: "登录修复".to_string(),
        usage: Some(usage(100, 7)),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let title = generate_title_from_first_message(
        provider.as_ref(),
        "test-model",
        "修复登录bug 帮我看看",
        Some(&ds),
        Some("session:s8title"),
    )
    .await;
    assert_eq!(title.as_deref(), Some("登录修复"), "返回值不受落账影响");

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1, "恰好一条 aux 账本行: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.provider_type, "title", "provider_type = aux_kind");
    assert!(
        row.trace_id.starts_with("aux-title-"),
        "trace_id 前缀: {}",
        row.trace_id
    );
    assert_eq!(row.model, "test-model");
    assert_eq!(row.input_tokens, 100);
    assert_eq!(row.output_tokens, 7);
    assert_eq!(row.cache_creation_tokens, 3);
    assert_eq!(row.cache_read_tokens, 4);
    assert_eq!(row.status_code, 200);
    assert!(!row.is_streaming);
    assert!(row.first_token_ms.is_none());
    assert_eq!(row.session_key, "session:s8title");
    assert!(row.error_message.is_none(), "usage 在场无 estimated 注记");
    // test-model 不在价目表 → 计价未命中 = 全零 + 空名（与 organ-5 同源口径）。
    assert_eq!(row.pricing_model, "");
    assert_eq!(row.total_cost_usd, 0.0);
    assert_eq!(row.input_cost_usd, 0.0);
    assert_eq!(row.output_cost_usd, 0.0);
}

// -- B. summary 族（chore_completion）：usage 缺失 = estimated 零计价 ---------

#[tokio::test]
async fn s8_chore_summary_estimated_row() {
    let (_dir, ds, al) = loop_with_store("s8_summary.db");
    let content = "连接器监控语义变化摘要：新增重试字段。".to_string();
    al.set_small_model(Some((
        Arc::new(AuxProvider {
            fail_until: 0,
            empty_output: false,
            content: content.clone(),
            usage: None,
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        "small-model".to_string(),
    )));
    let out = al
        .chore_completion("connector-summary", "请总结".to_string(), 256)
        .await
        .expect("正常返回");
    assert_eq!(out, content);

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1, "恰好一条: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.provider_type, "summary");
    assert!(
        row.trace_id.starts_with("aux-summary-"),
        "trace_id: {}",
        row.trace_id
    );
    assert_eq!(row.model, "small-model");
    assert_eq!(row.session_key, "", "chore 通道无 session_key 归因 = 空串");
    // estimated 注记（N 按字符口径，估算 ceil(N/4)）。
    let n = content.chars().count();
    let est = (n as u64).div_ceil(4);
    let msg = row.error_message.as_deref().expect("estimated 注记在场");
    assert!(msg.starts_with("[estimated]"), "前缀: {msg}");
    assert!(msg.contains("summary"), "aux_kind 在场: {msg}");
    assert!(msg.contains(&format!("{n} 字符")), "字符数: {msg}");
    assert!(
        msg.contains(&format!("≈ {est} output tokens")),
        "估算: {msg}"
    );
    assert!(msg.contains("估算值不入计价"), "口径注记: {msg}");
    // token / 计价列全零。
    assert_eq!(row.input_tokens, 0);
    assert_eq!(row.output_tokens, 0);
    assert_eq!(row.cache_creation_tokens, 0);
    assert_eq!(row.cache_read_tokens, 0);
    assert_eq!(row.pricing_model, "");
    assert_eq!(row.total_cost_usd, 0.0);
    assert_eq!(row.status_code, 200);
    assert!(!row.is_streaming);
}

// -- C. branch_summary 族（gateway 形态 prepare → run 归因 data_store）--------

#[tokio::test]
async fn s8_branch_summary_row_via_prepare() {
    let (_dir, ds, al) = loop_with_store("s8_branch.db");
    al.set_small_model(Some((
        Arc::new(AuxProvider {
            fail_until: 0,
            empty_output: false,
            content: "分支摘要内容：遗弃段做了三件事。".to_string(),
            usage: Some(usage(200, 20)),
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        "small-model".to_string(),
    )));
    let dropped: Vec<serde_json::Value> = (0..3)
        .map(|i| {
            serde_json::json!({"role": "user", "content": format!("第{i}轮消息：帮我调试模块{i}")})
        })
        .collect();
    let prepared = prepare_branch_summary(&al, "session:s8branch", &dropped, 3)
        .expect("small_model 已配置且 ≥3 轮 → Some");
    let summary = prepared.run().await.expect("provider 正常返回");
    assert!(summary.contains("分支摘要内容"), "摘要返回: {summary}");

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1, "恰好一条: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.provider_type, "branch_summary");
    assert!(
        row.trace_id.starts_with("aux-branch_summary-"),
        "trace_id: {}",
        row.trace_id
    );
    assert_eq!(row.session_key, "session:s8branch", "会话键如实归因");
    assert_eq!(row.input_tokens, 200);
    assert_eq!(row.output_tokens, 20);
    assert_eq!(row.cache_creation_tokens, 3);
    assert_eq!(row.cache_read_tokens, 4);
    assert!(row.error_message.is_none());
}

// -- D. 失败路径零行 -----------------------------------------------------------

#[tokio::test]
async fn s8_failure_paths_write_no_row() {
    let (_dir, ds, al) = loop_with_store("s8_fail.db");
    // 空输出：护栏拒绝 → 单次重试 → 仍空 = Err，零行。
    let empty_calls = Arc::new(AtomicUsize::new(0));
    al.set_small_model(Some((
        Arc::new(AuxProvider {
            fail_until: 0,
            empty_output: true,
            content: String::new(),
            usage: None,
            calls: empty_calls.clone(),
        }),
        "small-model".to_string(),
    )));
    let r = al
        .chore_completion("connector-summary", "请总结".to_string(), 256)
        .await;
    assert!(r.is_err(), "空输出按失败处理");
    assert_eq!(empty_calls.load(Ordering::SeqCst), 2, "空输出触发一次重试");
    // provider 恒败：Err，零行。
    let err_calls = Arc::new(AtomicUsize::new(0));
    al.set_small_model(Some((
        Arc::new(AuxProvider {
            fail_until: u64::MAX,
            empty_output: false,
            content: "不该被返回".to_string(),
            usage: None,
            calls: err_calls.clone(),
        }),
        "small-model".to_string(),
    )));
    let r = al
        .chore_completion("connector-summary", "请总结".to_string(), 256)
        .await;
    assert!(r.is_err(), "恒败 = Err");
    assert!(err_calls.load(Ordering::SeqCst) >= 2, "恒败走重试环");

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 0, "失败路径零行不造数: {rows:?}");
    assert!(rows.is_empty());
}

// -- E. 重试成功恰一行 ----------------------------------------------------------

#[tokio::test]
async fn s8_retry_success_writes_exactly_one_row() {
    let (_dir, ds, al) = loop_with_store("s8_retry.db");
    let calls = Arc::new(AtomicUsize::new(0));
    al.set_small_model(Some((
        Arc::new(AuxProvider {
            fail_until: 1,
            empty_output: false,
            content: "恢复完成".to_string(),
            usage: Some(usage(50, 9)),
            calls: calls.clone(),
        }),
        "small-model".to_string(),
    )));
    let out = al
        .chore_completion("connector-summary", "请总结".to_string(), 256)
        .await
        .expect("首败重试成功");
    assert_eq!(out, "恢复完成");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "1 初次 + 1 重试");

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1, "重试成功恰一行（无重复落账）: {rows:?}");
    let row = &rows[0];
    assert_eq!(row.provider_type, "summary");
    assert_eq!(row.input_tokens, 50);
    assert_eq!(row.output_tokens, 9);
    assert!(row.error_message.is_none());
}
