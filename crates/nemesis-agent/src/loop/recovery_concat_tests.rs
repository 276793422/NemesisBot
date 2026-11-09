//! （2026-11-02）：流中断拼接续写——partial 作 assistant 前缀
//! 加续写提示重试（transient 环），成功后前拼出完整文本；开关
//! `agents.stream_concat_retry` 默认开，显式 false 回退「重试从头」。
//!
//! 覆盖：extract 纯函数四态 / 拼接行为（messages 追加 + 内容前拼）/ 开关
//! 关回退 / tc 标记不可续 / 重试再撞中断 partial 合并（续写片段累积 /
//! 复述识别防重复，BUG-20261107-028）/ 常量字面量互锚。
//!
//! 自造 provider（兄弟测试模块不能互相 import）。

use super::recovery::{
    STREAM_PARTIAL_SEPARATOR, STREAM_TOOL_CALLS_TRUNCATED_MARK, extract_stream_partial,
};
use super::*;
use crate::test_support::capture_logs;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// 常量字面量互锚（providers 侧 failover::STREAM_* 的同文副本——任一端改
// 文本，两端测试必红；见 recovery.rs 常量注释分层约束）。
// ---------------------------------------------------------------------------

#[test]
fn stream_partial_constants_match_provider_side_contract() {
    assert_eq!(
        STREAM_PARTIAL_SEPARATOR,
        "\n\n--- 以下为中断前已生成的部分内容 ---\n"
    );
    assert_eq!(
        STREAM_TOOL_CALLS_TRUNCATED_MARK,
        "，工具调用已截断（不可拼接续写）"
    );
}

// ---------------------------------------------------------------------------
// extract_stream_partial 纯函数
// ---------------------------------------------------------------------------

/// 构造与 providers Display 契约同形的流中断错误串（failover 侧测试钉
/// Display 结构，本侧测试钉解析行为）。
fn stream_err(partial: &str, tc: bool) -> String {
    format!(
        "stream interrupted from provider anthropic/glm-5: connection reset（已接收 {} 字符{}，中断前部分文本随错误通知保留）{SEP}{partial}",
        partial.chars().count(),
        if tc {
            STREAM_TOOL_CALLS_TRUNCATED_MARK
        } else {
            ""
        },
        SEP = STREAM_PARTIAL_SEPARATOR,
    )
}

#[test]
fn extract_rejects_non_stream_errors() {
    assert!(extract_stream_partial("timeout calling provider x/y").is_none());
    assert!(extract_stream_partial("").is_none());
}

#[test]
fn extract_extracts_partial_without_tool_calls() {
    let p = extract_stream_partial(&stream_err("半截正文第一行\n第二行", false))
        .expect("可续形态必须提取成功");
    assert_eq!(p.content, "半截正文第一行\n第二行");
    assert!(!p.has_tool_calls);
}

#[test]
fn extract_flags_truncated_tool_calls() {
    let p = extract_stream_partial(&stream_err("some partial", true)).expect("形态完整仍提取");
    assert!(p.has_tool_calls, "头行 tc 标记必须被识别");
}

#[test]
fn extract_rejects_missing_separator_and_blank_partial() {
    // 前缀命中但分隔符缺失（Display 契约破坏）→ 保守 None。
    assert!(
        extract_stream_partial("stream interrupted from provider x/y: reason（已接收 3 字符）")
            .is_none()
    );
    // partial 全空白 → None（零产出无可续）。
    assert!(extract_stream_partial(&stream_err("   ", false)).is_none());
}

// ---------------------------------------------------------------------------
// 拼接行为：ScriptedStreamProvider 驱动 retry_transient
// ---------------------------------------------------------------------------

/// 脚本化 provider：按序弹出预置结果；记录每次收到的 messages 尾部两条。
struct ScriptedStream {
    steps: std::sync::Mutex<Vec<Result<LlmResponse, String>>>,
    /// 每次调用的 messages 尾部两条：(role, content) × 2。
    last_tail: std::sync::Mutex<Vec<Vec<(String, String)>>>,
}

impl ScriptedStream {
    fn new(steps: Vec<Result<LlmResponse, String>>) -> Self {
        Self {
            steps: std::sync::Mutex::new(steps),
            last_tail: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedStream {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut q = self.steps.lock().unwrap();
        let step = if q.is_empty() {
            Err("script exhausted".to_string())
        } else {
            q.remove(0)
        };
        drop(q);
        let tail: Vec<(String, String)> = messages
            .iter()
            .rev()
            .take(2)
            .rev()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();
        self.last_tail.lock().unwrap().push(tail);
        step
    }
}

fn ok_resp(content: &str) -> Result<LlmResponse, String> {
    Ok(LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        finish_reason: Some("stop".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    })
}

async fn run_retry(
    agent_loop: &AgentLoop,
    provider: &std::sync::Arc<dyn LlmProvider>,
    first_err: String,
) -> (Option<LlmResponse>, String) {
    let instance = AgentInstance::new(AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    });
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new(
        "web",
        "l29-chat",
        "l29-sender",
        "agent:main:session:l29concat",
    );
    let cancel = tokio_util::sync::CancellationToken::new();
    // first_call_failed_after < 1s = 立即拒绝形态（不退避，测试零等待）。
    agent_loop
        .retry_transient(
            &instance,
            &context,
            provider,
            "test-model",
            &opts,
            &cancel,
            None,
            std::time::Duration::from_millis(5),
            first_err,
        )
        .await
        .expect("拼接续写走 Ok 路径（无中断）")
}

/// 拼接续写主线：首轮流中断（partial 非空、无 tc）→ 重试请求尾部追加
/// assistant(partial) + user(续写提示) → 成功后 resp.content = partial +
/// 续写段（history/出站只见完整文本）。
#[tokio::test]
async fn concat_retry_appends_partial_prefix_and_splices_result() {
    let _logs = capture_logs();
    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        Err(stream_err("第一段半截", false)),
        ok_resp("第二段续写"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        stream_err("第一段半截", false),
    )
    .await;

    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(
        resp.content, "第一段半截第二段续写",
        "partial 必须前拼到续写段（完整文本一体出站）"
    );
    // 第二次调用的尾部两条 = assistant(partial) + user(续写提示)。
    let tails = scripted.last_tail.lock().unwrap();
    assert_eq!(tails.len(), 2, "一次失败 + 一次成功重试");
    let tail = &tails[1];
    assert_eq!(tail[0].0, "assistant");
    assert_eq!(tail[0].1, "第一段半截");
    assert_eq!(tail[1].0, "user");
    assert_eq!(tail[1].1, STREAM_CONTINUE_PROMPT);
}

/// 开关关（config 显式 agents.stream_concat_retry=false）→ 回退「重试从头」：
/// 重试请求不追加、结果不前拼。
#[tokio::test]
async fn concat_retry_disabled_falls_back_to_retry_from_scratch() {
    let _logs = capture_logs();
    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.json");
    std::fs::write(&cfg_path, r#"{"agents": {"stream_concat_retry": false}}"#).unwrap();

    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        Err(stream_err("第一段半截", false)),
        ok_resp("从头重写的完整回复"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    agent_loop.set_config_path(cfg_path);

    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        stream_err("第一段半截", false),
    )
    .await;
    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(resp.content, "从头重写的完整回复", "开关关不得前拼 partial");
    let tails = scripted.last_tail.lock().unwrap();
    let tail = &tails[1];
    assert_ne!(
        tail[0].0, "assistant",
        "开关关重试请求尾部不得追加 assistant 前缀"
    );
}

/// tc 标记形态（半截 tool_calls）不可续：不拼接（重试从头），结果不前拼。
#[tokio::test]
async fn truncated_tool_calls_partial_not_concatenated() {
    let _logs = capture_logs();
    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        Err(stream_err("带了半截工具调用的 partial", true)),
        ok_resp("完整回复"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        stream_err("带了半截工具调用的 partial", true),
    )
    .await;
    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(resp.content, "完整回复", "tc 形态不得前拼");
    let tails = scripted.last_tail.lock().unwrap();
    let tail = &tails[1];
    assert_ne!(tail[0].0, "assistant", "tc 形态重试请求不得追加前缀");
}

/// 重试再撞流中断 + 模型违约复述前缀（new 以 prev 开头）→ 按重生成取 new
/// 单独值防重复（BUG-20261107-028 合并语义的识别分支），成功后拼 new。
#[tokio::test]
async fn repeated_stream_break_updates_partial() {
    let _logs = capture_logs();
    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        Err(stream_err("第一段", false)),
        Err(stream_err("第一段第二段", false)),
        ok_resp("第三段"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        stream_err("第一段", false),
    )
    .await;
    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(
        resp.content, "第一段第二段第三段",
        "成功轮拼的是最新 partial（第二段的）"
    );
    // 第二次重试请求尾部 assistant 前缀 = 第二个 partial。
    let tails = scripted.last_tail.lock().unwrap();
    let tail = &tails[2];
    assert_eq!(tail[0].1, "第一段第二段");
}

/// 重试再撞流中断（主形态：带前缀重试产出纯续写片段，STREAM_CONTINUE_
/// PROMPT「no recap」契约）→ 新 partial **累积**在旧前缀后（BUG-20261107-028：
/// 此前覆盖语义把「第一段」从最终拼接产物静默丢掉），成功轮拼累积全量。
#[tokio::test]
async fn repeated_stream_break_accumulates_continuation_fragment() {
    let _logs = capture_logs();
    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        // 重试#1 请求带 assistant("第一段") 前缀；模型续写产出「第二段」
        // 后流再断——本次流的 partial 只有续写片段（不含前缀）。
        Err(stream_err("第二段", false)),
        ok_resp("第三段"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
        system_prompt_split: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        stream_err("第一段", false),
    )
    .await;
    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(
        resp.content, "第一段第二段第三段",
        "续写片段必须累积在前缀后（覆盖语义会丢「第一段」）"
    );
    // 第二次重试请求尾部 assistant 前缀 = 累积全量（第一段+第二段）。
    let tails = scripted.last_tail.lock().unwrap();
    let tail = &tails[1];
    assert_eq!(tail[0].1, "第一段第二段");
}

/// 中途撞非流中断（timeout 等）→ partial 重置；随后从头重试再撞流中断 →
/// 新 partial 独立生效（不与已重置的旧前缀合并，BUG-20261107-028 None 臂）。
#[tokio::test]
async fn partial_reset_then_fresh_break_stands_alone() {
    let _logs = capture_logs();
    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        // 重试#1：timeout（extract None）→ partial 重置。
        Err("upstream deadline exceeded".to_string()),
        // 重试#2（从头）：撞流中断，产出全新 partial。
        Err(stream_err("从头重生成", false)),
        ok_resp("后续"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
        system_prompt_split: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        stream_err("第一段", false),
    )
    .await;
    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(
        resp.content, "从头重生成后续",
        "重置后新 partial 独立生效，不得与已重置旧前缀合并"
    );
    // 第三次调用尾部 assistant 前缀 = 新 partial 单独值。
    let tails = scripted.last_tail.lock().unwrap();
    let tail = &tails[2];
    assert_eq!(tail[0].1, "从头重生成");
}

/// 非流中断 transient（timeout 等）进入重试环 → 无拼接行为（对照）。
#[tokio::test]
async fn non_stream_transient_error_no_concat() {
    let _logs = capture_logs();
    let scripted = std::sync::Arc::new(ScriptedStream::new(vec![
        Err("upstream deadline exceeded".to_string()),
        ok_resp("fresh reply"),
    ]));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let config = AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    };
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(ScriptedStream::new(vec![])),
        config,
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let (maybe_resp, _err) = run_retry(
        &agent_loop,
        &(scripted.clone() as _),
        "upstream deadline exceeded".to_string(),
    )
    .await;
    let resp = maybe_resp.expect("重试成功必须有响应");
    assert_eq!(resp.content, "fresh reply");
    let tails = scripted.last_tail.lock().unwrap();
    let tail = &tails[1];
    assert_ne!(tail[0].0, "assistant", "非流中断错误不得触发拼接");
}

// ---------------------------------------------------------------------------
// 开关解析纯函数（config_watch::stream_concat_retry_from_value）
// ---------------------------------------------------------------------------

#[test]
fn stream_concat_switch_defaults_and_parsing() {
    use crate::r#loop::config_watch::stream_concat_retry_from_value as sw;
    // None（standalone）/ 键缺席 / 类型错 → 默认开。
    assert!(sw(None));
    assert!(sw(Some(&serde_json::json!({}))));
    assert!(sw(Some(&serde_json::json!({"agents": {}}))));
    assert!(sw(Some(
        &serde_json::json!({"agents": {"stream_concat_retry": "yes"}})
    )));
    // 显式 true 同默认；显式 false = 关。
    assert!(sw(Some(
        &serde_json::json!({"agents": {"stream_concat_retry": true}})
    )));
    assert!(!sw(Some(
        &serde_json::json!({"agents": {"stream_concat_retry": false}})
    )));
}
