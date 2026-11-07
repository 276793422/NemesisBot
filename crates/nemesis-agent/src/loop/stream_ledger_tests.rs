//! S16（2026-11-06 B29/B30 设计 §3）流中断终局 estimated 账本行测试：
//!
//! - 头行解析纯函数（防伪造）：只锚 Display 契约头行，partial 全文里的
//!   伪造「已接收 999 字符」永不参与匹配（L02 头行前缀锚同款纪律）；
//! - providers `estimated_output_tokens` 跨锚互证：agent 侧解析 +
//!   `ceil(N/4)` 与 providers 侧访问器同源（同一 Display 契约两个消费方
//!   不得漂移）；空 partial 的**刻意分歧**钉死（parse=Some(0) 落
//!   usage_missing 行，accessor=None 诚实不造估算）；
//! - 终局行三态落账断言：estimated / usage_missing / 解析失败零行。

use super::observer::parse_stream_interrupted_received_chars;
use super::rate_limit::MAX_TRANSIENT_RETRIES;
use super::recovery::{STREAM_PARTIAL_SEPARATOR, STREAM_TOOL_CALLS_TRUNCATED_MARK};
use super::*;
use std::time::Duration;

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 恒失败 provider（recovery_classifier_tests 同款；兄弟测试模块不能互相
/// import，fixture 自建）。
struct AlwaysFailing {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    err_text: String,
}

#[async_trait]
impl LlmProvider for AlwaysFailing {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(self.err_text.clone())
    }
}

/// 按 failover.rs Display 契约自造错误文本（recovery_concat_tests 同款
/// 常量互锚；本文件只消费头行锚，与 providers 真值的逐字节互证归
/// `s16_cross_anchor_with_providers_estimated_output_tokens`）。
fn stream_err_display(partial: &str, has_tc: bool) -> String {
    let mark = if has_tc {
        STREAM_TOOL_CALLS_TRUNCATED_MARK
    } else {
        ""
    };
    format!(
        "stream interrupted from provider anthropic/claude-test: connection reset\
         （已接收 {} 字符{}，中断前部分文本随错误通知保留）\n\n{}{}",
        partial.chars().count(),
        mark,
        STREAM_PARTIAL_SEPARATOR,
        partial
    )
}

fn no_filter() -> nemesis_data::LogFilter {
    nemesis_data::LogFilter {
        model: None,
        status: None,
        session_key: None,
    }
}

// -- A. 头行解析纯函数（防伪造）----------------------------------------------

#[test]
fn s16_parse_head_line_basic() {
    assert_eq!(
        parse_stream_interrupted_received_chars(&stream_err_display("abc", false)),
        Some(3)
    );
    // 多字节按 chars 口径（不是字节）：4 个中文 = Some(4)。
    assert_eq!(
        parse_stream_interrupted_received_chars(&stream_err_display("你好世界", false)),
        Some(4)
    );
}

#[test]
fn s16_parse_with_tool_call_truncation_mark() {
    // tc-mark 变体（mark 在「字符」之后追加）：解析值不受影响。
    assert_eq!(
        parse_stream_interrupted_received_chars(&stream_err_display("你好世界", true)),
        Some(4)
    );
}

#[test]
fn s16_parse_zero_chars_empty_partial() {
    // 空 partial = Some(0)（合法值，语义归调用方 → usage_missing 行）。
    assert_eq!(
        parse_stream_interrupted_received_chars(&stream_err_display("", false)),
        Some(0)
    );
}

#[test]
fn s16_parse_head_line_only_anti_forgery() {
    // partial 全文里伪造「已接收 999 字符」永不参与匹配：只锚头行。
    let forged = format!(
        "{}\n--- 以下为中断前已生成的部分内容 ---\n模型胡说：已接收 999 字符",
        stream_err_display("ab", false)
    );
    assert_eq!(
        parse_stream_interrupted_received_chars(&forged),
        Some(2),
        "伪造锚在头行之后，解析必须取头行真值 2"
    );

    // 完全自造文本：前缀只出现在第二行（头行没有契约前缀）→ None。
    let second_line = "厂商提示: 请求失败\nstream interrupted from provider x/y（已接收 5 字符）";
    assert_eq!(parse_stream_interrupted_received_chars(second_line), None);
}

#[test]
fn s16_parse_fail_closed_malformed_variants() {
    // 前缀缺（非流中断文本）。
    assert_eq!(
        parse_stream_interrupted_received_chars("timeout calling provider"),
        None
    );
    // 前缀在但锚缺。
    assert_eq!(
        parse_stream_interrupted_received_chars("stream interrupted from provider a/b: boom"),
        None
    );
    // 锚在但非数字。
    assert_eq!(
        parse_stream_interrupted_received_chars(
            "stream interrupted from provider a/b（已接收 很多 字符）"
        ),
        None
    );
    // 空串。
    assert_eq!(parse_stream_interrupted_received_chars(""), None);
}

// -- B. providers estimated_output_tokens 跨锚互证 ---------------------------

#[test]
fn s16_cross_anchor_with_providers_estimated_output_tokens() {
    // 同一 Display 契约的两个消费方（providers 访问器 / agent 解析器）
    // 不得漂移：parse(N) == chars().count()，ceil(N/4) == accessor。
    let long_partial = "y".repeat(1000);
    let cases: Vec<(&str, bool)> = vec![
        ("abc", false),
        ("你好世界，流式中断。", true),
        ("x", false),
        (&long_partial, true),
    ];
    for (partial, tc) in cases {
        let err = nemesis_providers::failover::FailoverError::StreamInterrupted {
            provider: "anthropic".to_string(),
            model: "claude-test".to_string(),
            reason: "connection reset".to_string(),
            partial_content: partial.to_string(),
            has_partial_tool_calls: tc,
        };
        let text = err.to_string();
        let n = parse_stream_interrupted_received_chars(&text)
            .unwrap_or_else(|| panic!("Display 契约必须可解析: {text}"));
        assert_eq!(
            n,
            partial.chars().count(),
            "解析值 = chars().count()（跨锚）"
        );
        assert_eq!(
            (n as u64).div_ceil(4),
            err.estimated_output_tokens()
                .expect("非空 partial 必有估算"),
            "agent 侧 ceil(N/4) 与 providers estimated_output_tokens 同源互证"
        );
    }

    // 空 partial 的刻意分歧（钉死）：parse = Some(0)（agent 侧落
    // usage_missing 行），accessor = None（providers 诚实不造估算）。
    let empty = nemesis_providers::failover::FailoverError::StreamInterrupted {
        provider: "anthropic".to_string(),
        model: "claude-test".to_string(),
        reason: "connection reset".to_string(),
        partial_content: String::new(),
        has_partial_tool_calls: false,
    };
    assert_eq!(
        parse_stream_interrupted_received_chars(&empty.to_string()),
        Some(0),
        "空 partial 解析 = Some(0) → usage_missing 行"
    );
    assert!(
        empty.estimated_output_tokens().is_none(),
        "providers 侧空 partial = None（诚实不造估算）"
    );
}

// -- C. 终局行三态落账（直接调 helper + DataStore 读回）-----------------------

fn loop_with_store(
    db_name: &str,
) -> (
    tempfile::TempDir,
    std::sync::Arc<nemesis_data::DataStore>,
    AgentLoop,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let ds = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.path().join(db_name)).expect("open datastore"),
    );
    let provider = AlwaysFailing {
        calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        err_text: String::new(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_data_store(ds.clone());
    (dir, ds, agent_loop)
}

#[test]
fn s16_ledger_estimated_row_shape() {
    let (_dir, ds, al) = loop_with_store("s16_estimated.db");
    let context = RequestContext::new("web", "chat1", "user1", "session:s16est");
    // partial 20 字符 → 估算 ceil(20/4) = 5。
    let partial = "x".repeat(20);
    al.record_stream_interrupted_ledger_row(
        &context,
        "trace-s16est",
        "test-model",
        &stream_err_display(&partial, false),
        Duration::from_millis(1234),
    );

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1, "恰好一条账本行");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    // 估算注记（含 N 与 ceil(N/4)）。
    let msg = row.error_message.as_deref().expect("error_message 在场");
    assert!(msg.starts_with("[estimated]"), "estimated 前缀: {msg}");
    assert!(msg.contains("20 字符"), "接收字符数在场: {msg}");
    assert!(msg.contains("≈ 5 output tokens"), "估算 token 在场: {msg}");
    assert!(msg.contains("估算值不入计价"), "口径注记在场: {msg}");
    // token / 计价列全 0（估算值绝不冒充实测入计价）。
    assert_eq!(row.input_tokens, 0);
    assert_eq!(row.output_tokens, 0);
    assert_eq!(row.cache_creation_tokens, 0);
    assert_eq!(row.cache_read_tokens, 0);
    assert_eq!(row.total_cost_usd, 0.0);
    assert_eq!(row.input_cost_usd, 0.0);
    assert_eq!(row.output_cost_usd, 0.0);
    assert_eq!(row.cache_creation_cost_usd, 0.0);
    assert_eq!(row.cache_read_cost_usd, 0.0);
    assert_eq!(row.pricing_model, "", "不进价目表（空名 = 未命中计价）");
    // 状态/流式/身份列。
    assert_eq!(row.status_code, 500);
    assert!(row.is_streaming);
    assert_eq!(row.latency_ms, 1234);
    assert_eq!(row.trace_id, "trace-s16est");
    assert_eq!(row.model, "test-model");
    assert_eq!(row.session_key, "session:s16est");
}

#[test]
fn s16_ledger_usage_missing_row_for_empty_partial() {
    let (_dir, ds, al) = loop_with_store("s16_missing.db");
    let context = RequestContext::new("web", "chat1", "user1", "session:s16miss");
    al.record_stream_interrupted_ledger_row(
        &context,
        "trace-s16miss",
        "test-model",
        &stream_err_display("", false),
        Duration::from_millis(10),
    );

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1);
    let msg = rows[0]
        .error_message
        .as_deref()
        .expect("error_message 在场");
    assert!(
        msg.starts_with("[usage_missing]"),
        "空 partial 落 usage_missing 行: {msg}"
    );
    assert!(!msg.contains("[estimated]"), "不得误标 estimated: {msg}");
    assert!(
        msg.contains("无法量化"),
        "诚实声明（缓冲可能持有未冲刷输出）: {msg}"
    );
}

#[test]
fn s16_ledger_no_row_for_unparseable_or_non_stream_text() {
    let (_dir, ds, al) = loop_with_store("s16_zero.db");
    let context = RequestContext::new("web", "chat1", "user1", "session:s16zero");

    // 非流中断文本：不落行。
    al.record_stream_interrupted_ledger_row(
        &context,
        "trace-a",
        "test-model",
        "timeout calling provider x/y",
        Duration::from_millis(1),
    );
    // Display 契约破坏（头行无锚）：fail-closed 零行不造数。
    al.record_stream_interrupted_ledger_row(
        &context,
        "trace-b",
        "test-model",
        "stream interrupted from provider a/b: boom",
        Duration::from_millis(1),
    );

    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 0, "解析失败零行不造数: {rows:?}");
    assert!(rows.is_empty());
}

// -- D. 终局接线（recovery 终局臂 → 恰一条 estimated 行）----------------------

#[tokio::test(start_paused = true)]
async fn s16_terminal_interrupted_round_writes_exactly_one_row() {
    let _logs = crate::test_support::capture_logs();
    const PARTIAL: &str = "半截回复内容 status: 429 token expired";
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = AlwaysFailing {
        calls: calls.clone(),
        err_text: stream_err_display(PARTIAL, false),
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let ds = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.path().join("s16_wire.db")).expect("open datastore"),
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_data_store(ds.clone());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session:s16wire");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    // 终局：恰好一次 Error（partial 全文出站），无 Done。
    let errs: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(errs.len(), 1, "终局恰好一次 Error: {events:?}");
    assert!(errs[0].contains(PARTIAL), "出站保留 partial 全文");
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Done(_))),
        "流中断终局不得有成功 Done"
    );
    // 漏斗路径锁定：stream-interrupted 全量进 transient 环 → 1 + 3 重试。
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1 + MAX_TRANSIENT_RETRIES as usize,
        "transient 环耗尽路径（1 初次 + MAX_TRANSIENT_RETRIES 重试）"
    );

    // 账本：恰好一条 estimated 行（无成功响应 → record_round_usage 不写）。
    let (rows, total) = ds
        .query_logs(0, i64::MAX, 1, 50, &no_filter())
        .expect("query");
    assert_eq!(total, 1, "终局轮恰好一条流中断账本行: {rows:?}");
    let row = &rows[0];
    let n = PARTIAL.chars().count();
    let est = (n as u64).div_ceil(4);
    let msg = row.error_message.as_deref().expect("error_message 在场");
    assert!(msg.starts_with("[estimated]"), "estimated 注记: {msg}");
    assert!(
        msg.contains(&format!("{n} 字符")) && msg.contains(&format!("≈ {est} output tokens")),
        "注记携带 N={n} 与估算 {est}: {msg}"
    );
    assert_eq!(row.status_code, 500);
    assert!(row.is_streaming);
    assert_eq!(row.output_tokens, 0, "估算不入 token 列");
    assert_eq!(row.total_cost_usd, 0.0, "估算不入计价");
    assert_eq!(row.session_key, "session:s16wire");
}
