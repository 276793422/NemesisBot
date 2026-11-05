//! 集群续行循环的 max_tokens 截断判定 + 续写。
//!
//! 覆盖（执行清单 步骤 3）：截断判定纯函数三态（数值命中 / length
//! 回落 / escalated_to 防误判）、续写追加形态、空内容 2 连停、预算耗尽
//! 诚实注记、截断带 tool_calls 丢弃。行为面用脚本化 provider 驱动完整
//! handle_cluster_continuation（快照 → 续行 LLM 循环 → 出站）。
//!
//! 自造 helper（兄弟测试模块不能互相 import）。

use super::*;
use crate::r#loop::LlmResponse;
use crate::test_support::capture_logs;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

const CAP: u32 = 4096;

fn make_message(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

/// 脚本化 provider：按序弹出预置响应；逐调用记录收到的 messages（供
/// 续写追加形态断言）。脚本耗尽返回 Err（测试断言会暴露提前烧脚本）。
struct ScriptedProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    calls: std::sync::Mutex<Vec<Vec<LlmMessage>>>,
}

impl ScriptedProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn call_messages(&self, i: usize) -> Vec<LlmMessage> {
        self.calls.lock().unwrap()[i].clone()
    }
}

#[async_trait]
impl crate::r#loop::LlmProvider for ScriptedProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.lock().unwrap().push(messages.clone());
        let mut q = self.responses.lock().unwrap();
        if q.is_empty() {
            return Err("script exhausted (unexpected extra LLM call)".to_string());
        }
        Ok(q.remove(0))
    }
}

fn usage(completion_tokens: i64) -> Option<crate::loop_executor::ObserverUsageInfo> {
    Some(crate::loop_executor::ObserverUsageInfo {
        prompt_tokens: 100,
        completion_tokens,
        total_tokens: 100 + completion_tokens,
        cached_tokens: None,
        cache_creation_tokens: None,
        cache_read_tokens: None,
    })
}

/// 截断响应（有 usage，数值恰好命中 cap——数值路径）。
fn truncated_content_resp(content: &str, cap: u32) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: false,
        reasoning_content: None,
        finish_reason: Some("length".to_string()),
        escalated_to: None,
        usage: usage(cap as i64),
        raw_request_body: None,
        raw_response_body: None,
    }
}

fn truncated_empty_resp(cap: u32) -> LlmResponse {
    truncated_content_resp("", cap)
}

/// 截断响应（无 usage——finish_reason="length" 回落路径）。
fn truncated_no_usage_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: false,
        reasoning_content: None,
        finish_reason: Some("length".to_string()),
        escalated_to: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }
}

/// 未截断终答。
fn full_resp(content: &str) -> LlmResponse {
    LlmResponse {
        content: content.to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        finish_reason: Some("stop".to_string()),
        escalated_to: None,
        usage: usage(10),
        raw_request_body: None,
        raw_response_body: None,
    }
}

// ---------------------------------------------------------------------------
// is_length_truncated：截断判定纯函数三态
// ---------------------------------------------------------------------------

#[test]
fn truncation_detection_numeric_usage_path() {
    // 数值命中：completion_tokens >= cap → 截断。
    let mut resp = full_resp("x");
    resp.usage = usage(CAP as i64);
    assert!(is_length_truncated(&resp, CAP));

    // 未达 cap → 未截断。
    let mut resp = full_resp("x");
    resp.usage = usage((CAP - 1) as i64);
    assert!(!is_length_truncated(&resp, CAP));
}

#[test]
fn truncation_detection_finish_reason_fallback() {
    // 无 usage：finish_reason="length" 回落命中。
    assert!(is_length_truncated(&truncated_no_usage_resp("half"), CAP));

    // 无 usage：finish_reason="stop" → 未截断（CLI lane 恒 stop——无法
    // 检测就不假装检测）。
    let mut resp = truncated_no_usage_resp("done");
    resp.finish_reason = Some("stop".to_string());
    assert!(!is_length_truncated(&resp, CAP));

    // finish_reason 缺席 → 未截断。
    let mut resp = truncated_no_usage_resp("half");
    resp.finish_reason = None;
    assert!(!is_length_truncated(&resp, CAP));
}

#[test]
fn truncation_detection_escalated_to_precedence() {
    // escalated_to 优先（同因防误判）：config cap 4096，provider 已
    // 翻倍到 16384——usage 10000 ≥ 4096 但 < 16384，是完整响应不是截断。
    let mut resp = full_resp("complete answer");
    resp.usage = usage(10_000);
    resp.escalated_to = Some(16_384);
    assert!(!is_length_truncated(&resp, CAP));

    // 翻倍后仍命中事实 cap → 截断。
    let mut resp = full_resp("half");
    resp.usage = usage(16_384);
    resp.escalated_to = Some(16_384);
    assert!(is_length_truncated(&resp, CAP));
}

// ---------------------------------------------------------------------------
// 续写循环行为面：脚本化 provider 驱动完整 handle_cluster_continuation
// ---------------------------------------------------------------------------

/// 装配：快照（空 session_key = 跳过会话持久化路径，行为面只看循环与出站）
/// + 出站通道，跑完整个续行处理。
async fn run_continuation(
    provider: &ScriptedProvider,
    tools: &HashMap<String, Arc<dyn crate::r#loop::Tool>>,
) -> tokio::sync::mpsc::Receiver<nemesis_types::channel::OutboundMessage> {
    let manager = ContinuationManager::new();
    let task = format!("l28trunc_{}", std::process::id());
    manager
        .save_continuation(
            &task,
            vec![make_message("user", "do work")],
            "tc_0",
            "web",
            "chat-l28",
            "",
            "",
        )
        .await;
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(64);
    handle_cluster_continuation(
        &manager,
        &task,
        "task result",
        false,
        None,
        provider,
        "test-model",
        CAP,
        262144, // + max_tokens_ceiling（测试 = 默认封顶）
        tools,
        &outbound_tx,
        None,
        None,
        true, // vision_supported
        None, // source_node
    )
    .await;
    outbound_rx
}

/// 续写追加形态：截断半截 → assistant 入对话 + 续写提示 user 追加 →
/// 完整终答出站。
#[tokio::test]
async fn truncated_response_appends_continue_prompt_and_resumes() {
    let _logs = capture_logs();
    let provider = ScriptedProvider::new(vec![
        truncated_content_resp("half answer one", CAP),
        full_resp("final complete answer"),
    ]);
    let mut outbound_rx = run_continuation(
        &provider,
        &HashMap::<String, Arc<dyn crate::r#loop::Tool>>::new(),
    )
    .await;

    assert_eq!(provider.call_count(), 2, "截断后续写恰好再调一轮");

    // 第二次调用的 messages 比第一次多两条：assistant(半截) + user(续写提示)。
    let first = provider.call_messages(0);
    let second = provider.call_messages(1);
    assert_eq!(second.len(), first.len() + 2);
    let assistant = &second[second.len() - 2];
    assert_eq!(assistant.role, "assistant");
    assert_eq!(assistant.content, "half answer one");
    assert!(assistant.tool_calls.is_none());
    let user = &second[second.len() - 1];
    assert_eq!(user.role, "user");
    assert_eq!(user.content, LENGTH_CONTINUE_PROMPT);

    // 终局出站 = 完整回复（半截不冒充终局）。
    let out = outbound_rx.try_recv().expect("final response sent");
    assert_eq!(out.content, "final complete answer");
}

/// 空内容 2 连守卫：连续两轮空内容撞 cap → 提前停轮 + 诚实注记
/// （不再烧剩余预算的 prefill）。
#[tokio::test]
async fn empty_truncated_content_stops_after_two_rounds() {
    let _logs = capture_logs();
    let provider = ScriptedProvider::new(vec![
        truncated_empty_resp(CAP),
        truncated_empty_resp(CAP),
        truncated_empty_resp(CAP), // 不该被消费
    ]);
    // ceiling 钉在 cap（4096）= 阶梯被 ceiling 闸死不升档，测试专注空内容
    // 守卫本体（语义基线；阶梯行为有专项测试）。
    let mut outbound_rx =
        run_continuation_with_ceiling(&provider, &HashMap::new(), CAP as u64).await;

    assert_eq!(
        provider.call_count(),
        2,
        "空内容 2 连必须提前停轮，第三轮不发生"
    );
    let out = outbound_rx.try_recv().expect("honest notice sent");
    assert!(out.content.contains("空输出"), "{}", out.content);
    assert!(out.content.contains("max_tokens"), "{}", out.content);
    assert!(out.content.contains("4096"), "{}", out.content);
}

/// 预算耗尽：每轮都截断 → 烧满 1..=20 同池预算 → 半截正文 + 诚实注记
/// 终局（不再无声把半截当完整回复）。
#[tokio::test]
async fn budget_exhaustion_emits_truncation_notice() {
    let _logs = capture_logs();
    let responses: Vec<LlmResponse> = (1..=20)
        .map(|i| truncated_content_resp(&format!("seg{i}"), CAP))
        .collect();
    let provider = ScriptedProvider::new(responses);
    // ceiling 钉在 cap = 不升档（20 轮恒 4096 截断语义基线；升档行为有
    // 专项测试 continuation_escalates_cap_on_repeated_truncation_two_step_cap）。
    let mut outbound_rx =
        run_continuation_with_ceiling(&provider, &HashMap::new(), CAP as u64).await;

    assert_eq!(provider.call_count(), 20, "续写轮消耗同一 1..=20 预算");
    let out = outbound_rx.try_recv().expect("notice sent");
    assert!(out.content.starts_with("seg20"), "{}", out.content);
    assert!(
        out.content
            .contains("[输出反复超过 max_tokens 上限（4096）"),
        "{}",
        out.content
    );
    assert!(out.content.contains("续行预算已耗尽"), "{}", out.content);
}

/// 截断响应带 tool_calls 一律丢弃（执行半截 args 是写坏文件事故）：
/// 已注册的探针工具绝不被执行，续写提示照常追加，终答出站。
#[tokio::test]
async fn truncated_tool_calls_are_dropped_not_executed() {
    let _logs = capture_logs();

    struct RecordingTool(AtomicBool);
    #[async_trait]
    impl crate::r#loop::Tool for RecordingTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            self.0.store(true, Ordering::SeqCst);
            Ok("executed".to_string())
        }
    }
    let tool = Arc::new(RecordingTool(AtomicBool::new(false)));
    let mut tools: HashMap<String, Arc<dyn crate::r#loop::Tool>> = HashMap::new();
    tools.insert(
        "probe_tool".to_string(),
        tool.clone() as Arc<dyn crate::r#loop::Tool>,
    );

    let mut truncated_with_tools = truncated_content_resp("half then tool call", CAP);
    truncated_with_tools.tool_calls = vec![ToolCallInfo {
        id: "tc_trunc".to_string(),
        name: "probe_tool".to_string(),
        arguments: "{}".to_string(),
    }];
    let provider = ScriptedProvider::new(vec![truncated_with_tools, full_resp("done after drop")]);
    let mut outbound_rx = run_continuation(&provider, &tools).await;

    assert!(
        !tool.0.load(Ordering::SeqCst),
        "截断响应的 tool_calls 必须丢弃，绝不执行半截 args"
    );
    assert_eq!(provider.call_count(), 2);
    // 入对话的 assistant 消息 tool_calls 已剥。
    let second = provider.call_messages(1);
    let assistant = &second[second.len() - 2];
    assert_eq!(assistant.content, "half then tool call");
    assert!(assistant.tool_calls.is_none());

    let out = outbound_rx.try_recv().expect("final response sent");
    assert_eq!(out.content, "done after drop");
}

// ---------------------------------------------------------------------------
// +：续写再撞顶翻倍阶梯（档封顶）
// ---------------------------------------------------------------------------

/// run_continuation 的自定义 ceiling 版（阶梯封顶测试需要不同 ceiling 值）。
async fn run_continuation_with_ceiling(
    provider: &ScriptedProvider,
    tools: &HashMap<String, Arc<dyn crate::r#loop::Tool>>,
    ceiling: u64,
) -> tokio::sync::mpsc::Receiver<nemesis_types::channel::OutboundMessage> {
    let manager = ContinuationManager::new();
    let task = format!("l28plus_{}", std::process::id());
    manager
        .save_continuation(
            &task,
            vec![make_message("user", "do work")],
            "tc_0",
            "web",
            "chat-l28p",
            "",
            "",
        )
        .await;
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(64);
    handle_cluster_continuation(
        &manager,
        &task,
        "task result",
        false,
        None,
        provider,
        "test-model",
        CAP,
        ceiling,
        tools,
        &outbound_tx,
        None,
        None,
        true,
        None,
    )
    .await;
    outbound_rx
}

/// 截断响应（usage 恰好命中指定 cap——升档后判定锚随档位走）。
fn truncated_at(content: &str, cap: u64) -> LlmResponse {
    truncated_content_resp(content, cap as u32)
}

/// 阶梯行为（档封顶 + 公式对齐主循环 `(c×2).max(32K 地板)`）：
/// 4096 撞顶 → 首升档直接跳 32K（地板抬升）→ 32K 再撞顶 → 次升档 65K
/// → 65K 档完整回复终局。断言：三轮调用 + 末轮 messages 首轮 + 两对
/// 续写追加 + 出站为完整回复。
#[tokio::test]
async fn continuation_escalates_cap_on_repeated_truncation_two_step_cap() {
    let _logs = capture_logs();
    let provider = ScriptedProvider::new(vec![
        truncated_at("seg at 4096", 4096),
        truncated_at(
            "seg at 32768",
            crate::r#loop::length_escalation::ESCALATION_FLOOR,
        ),
        full_resp("final after escalation"),
    ]);
    let mut outbound_rx = run_continuation_with_ceiling(&provider, &HashMap::new(), 262_144).await;

    assert_eq!(provider.call_count(), 3, "两次截断 + 一次终答");
    let out = outbound_rx.try_recv().expect("final response sent");
    assert_eq!(out.content, "final after escalation");
    // （65K 档）messages = 首轮 + 两对续写追加。
    let first = provider.call_messages(0);
    let third = provider.call_messages(2);
    assert_eq!(third.len(), first.len() + 4);
}

/// ceiling 闸：cap 4096、ceiling 20000——首升档 (4096*2).max(32K)=32768 超
/// ceiling → 不升档，维持 4096 续写到底（iteration 预算兜底）。
#[tokio::test]
async fn continuation_escalation_respects_ceiling() {
    let _logs = capture_logs();
    let provider = ScriptedProvider::new(vec![
        truncated_at("seg1", 4096),
        truncated_at("seg2", 4096),
        full_resp("done within old cap"),
    ]);
    let mut outbound_rx = run_continuation_with_ceiling(&provider, &HashMap::new(), 20_000).await;

    // ceiling 20000 < 首升档 32768 → 全程不升档，行为与 基线一致：
    // 三轮调用（截断→截断→终答）。
    assert_eq!(provider.call_count(), 3);
    let out = outbound_rx.try_recv().expect("final response sent");
    assert_eq!(out.content, "done within old cap");
}
