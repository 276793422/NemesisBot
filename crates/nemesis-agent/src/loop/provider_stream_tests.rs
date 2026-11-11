//! provider_stream 覆盖（2026-10-13）：sink 合并窗/相位序列/seq 单调、
//! 空流零帧、error 截断终态、StreamingLlmProvider 合成增量与真实流两臂。

use async_trait::async_trait;

use super::llm_types::{LlmMessage, LlmProvider, LlmResponse};
use super::provider_stream::{ProviderStreamSink, StreamingLlmProvider};

/// 阈值镜像（provider_stream 私有常量；改动时同步）：
/// FLUSH_CHARS=64（合并窗字符）、FLUSH_MS=150、ERROR_TEXT_MAX=300。
const FLUSH_CHARS_MIRROR: usize = 64;

fn make_sink() -> (
    ProviderStreamSink,
    tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) {
    let (tx, rx) = tokio::sync::broadcast::channel(256);
    let sink = ProviderStreamSink::new(
        tx,
        "web:main:s1".to_string(),
        "web:conn1".to_string(),
        "test-model".to_string(),
    );
    (sink, rx)
}

fn recv_all(
    rx: &mut tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) -> Vec<nemesis_types::agent::AgentEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

fn phases(events: &[nemesis_types::agent::AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { phase, .. } => Some(phase.clone()),
            _ => None,
        })
        .collect()
}

fn texts(events: &[nemesis_types::agent::AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn start_delta_end_sequence_with_coalescing() {
    let (sink, mut rx) = make_sink();

    // 三个小增量（各 10 字符 <64）：首增量立即触发 start+delta（last_flush
    // 为 None → 立即到期），后续两个进缓冲。
    for i in 0..3 {
        sink.on_delta(&format!("chunk-{i:0>6}-!"));
    }
    sink.finish();

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "delta", "end"],
        "start 惰性首发 + 残余缓冲冲刷 + end 收尾"
    );
    // seq 单调递增
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, vec![0, 1, 2, 3]);
    // end 帧 text 空、带 elapsed
    if let nemesis_types::agent::AgentEvent::ProviderStream {
        phase,
        text,
        elapsed_ms,
        ..
    } = &events[3]
    {
        assert_eq!(phase, "end");
        assert!(text.is_empty());
        assert!(elapsed_ms.is_some(), "end 帧必须带耗时");
    } else {
        panic!("第 4 帧应为 end");
    }
}

#[tokio::test]
async fn finish_on_empty_stream_emits_nothing() {
    let (sink, mut rx) = make_sink();
    // 纯 tool-call 轮：零增量零文本 → finish 不发任何帧（面板零噪音）
    sink.finish();
    let events = recv_all(&mut rx);
    assert!(
        events.is_empty(),
        "空流 finish 必须零帧，实际 {:?}",
        phases(&events)
    );
}

#[tokio::test]
async fn error_frame_is_terminal_and_truncated() {
    let (sink, mut rx) = make_sink();
    sink.on_delta(&"x".repeat(FLUSH_CHARS_MIRROR));
    let big = "y".repeat(1000);
    sink.error(&big);

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "error"],
        "error 帧后不补 end（终态语义）"
    );
    if let nemesis_types::agent::AgentEvent::ProviderStream {
        text, elapsed_ms, ..
    } = &events[2]
    {
        assert!(
            text.len() <= 300,
            "error 文本必须截断 ≤300，实际 {}",
            text.len()
        );
        assert!(elapsed_ms.is_some());
    } else {
        panic!("第 3 帧应为 error");
    }
}

#[tokio::test]
async fn session_and_chat_id_stamped_on_every_frame() {
    let (sink, mut rx) = make_sink();
    sink.on_delta(&"z".repeat(FLUSH_CHARS_MIRROR));
    sink.finish();
    for ev in recv_all(&mut rx) {
        match ev {
            nemesis_types::agent::AgentEvent::ProviderStream {
                session_key,
                chat_id,
                model,
                ..
            } => {
                assert_eq!(session_key, "web:main:s1");
                assert_eq!(chat_id, "web:conn1");
                assert_eq!(model, "test-model");
            }
            other => panic!("非 ProviderStream 事件混入: {other:?}"),
        }
    }
}

/// 非流式桩（不覆写 chat_streamed → trait 缺省实现忽略回调）。
struct NonStreamProvider {
    content: &'static str,
}

#[async_trait]
impl LlmProvider for NonStreamProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: self.content.to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 流式桩：覆写 chat_streamed，把两段增量送进回调后返回聚合结果。
struct RealStreamProvider {
    chunk_a: String,
    chunk_b: String,
}

#[async_trait]
impl LlmProvider for RealStreamProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        unreachable!("chat_streamed 覆写后 chat 不应被调用")
    }

    async fn chat_streamed(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<LlmResponse, String> {
        on_delta(self.chunk_a.clone());
        on_delta(self.chunk_b.clone());
        Ok(LlmResponse {
            content: format!("{}{}", self.chunk_a, self.chunk_b),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn empty_args() -> (
    Vec<LlmMessage>,
    Option<crate::types::ChatOptions>,
    Vec<crate::types::ToolDefinition>,
) {
    (Vec::new(), None, Vec::new())
}

#[tokio::test]
async fn wrapper_synthesizes_full_delta_for_non_stream_provider() {
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:s2".to_string(),
        "web:conn2".to_string(),
        "m".to_string(),
    ));
    let wrapper = StreamingLlmProvider::new(
        std::sync::Arc::new(NonStreamProvider { content: "ok" }),
        sink.clone(),
    );
    let (messages, options, tools) = empty_args();
    let resp = wrapper
        .chat("m", messages, options, tools)
        .await
        .expect("chat 应成功");
    assert_eq!(resp.content, "ok");

    // 非流式 provider（忽略回调）→ 包装器合成全量增量：start/delta/end
    let events = recv_all(&mut rx);
    assert_eq!(phases(&events), vec!["start", "delta", "end"]);
    assert_eq!(texts(&events)[1], "ok", "合成增量 = 完整 content");
}

#[tokio::test]
async fn wrapper_passes_through_real_stream_without_synthetic_duplicate() {
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:s3".to_string(),
        "web:conn3".to_string(),
        "m".to_string(),
    ));
    let wrapper = StreamingLlmProvider::new(
        std::sync::Arc::new(RealStreamProvider {
            chunk_a: "a".repeat(FLUSH_CHARS_MIRROR), // ≥64 → 立即成帧
            chunk_b: "b".repeat(10),                 // <64 → finish 冲刷
        }),
        sink.clone(),
    );
    let (messages, options, tools) = empty_args();
    let resp = wrapper
        .chat("m", messages, options, tools)
        .await
        .expect("chat 应成功");
    assert_eq!(resp.content.len(), FLUSH_CHARS_MIRROR + 10);

    // 真实流：start/delta(70a)/delta(10b)/end——无合成重复帧
    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "delta", "end"],
        "已有真实增量时不得合成额外全量帧"
    );
    assert_eq!(texts(&events)[1], "a".repeat(FLUSH_CHARS_MIRROR));
    assert_eq!(texts(&events)[2], "b".repeat(10));
}

struct FailingStreamProvider;

#[async_trait]
impl LlmProvider for FailingStreamProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        unreachable!()
    }

    async fn chat_streamed(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
        _on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<LlmResponse, String> {
        Err("upstream exploded".to_string())
    }
}

/// 重试桩：首次 chat_streamed 送增量后失败（复刻
/// recovery.rs 瞬态重试环拓扑——同一 wrapper/sink 跨首捕+重试两次调用）。
struct RetryThenSuccessProvider {
    failed_once: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl LlmProvider for RetryThenSuccessProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        unreachable!()
    }

    async fn chat_streamed(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<LlmResponse, String> {
        if !self
            .failed_once
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            on_delta("first-attempt-partial".to_string());
            return Err("transient upstream failure".to_string());
        }
        on_delta("second-attempt-full".to_string());
        Ok(LlmResponse {
            content: "second-attempt-full".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 回归锁：recovery.rs 五调用点共享同一 wrapper/sink，
/// 重试 = 新流（recovery.rs 模块注释钦定契约：seq 归零 + start 帧重置
/// 前端缓冲）——第二次 chat() 必须以 start 开新流（前端卡片唯一 reset
/// 信号），且本 attempt 文本纯净（上一 attempt 残尾不渗入）。
#[tokio::test]
async fn retry_on_shared_sink_restarts_stream_with_start_frame() {
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:r1".to_string(),
        "web:conn1".to_string(),
        "m".to_string(),
    ));
    let wrapper = StreamingLlmProvider::new(
        std::sync::Arc::new(RetryThenSuccessProvider {
            failed_once: std::sync::atomic::AtomicBool::new(false),
        }),
        sink.clone(),
    );
    let (messages, options, tools) = empty_args();

    // attempt 1：瞬态失败（wrapper Err 臂发 error 帧）
    let e1 = wrapper
        .chat("m", messages.clone(), options.clone(), tools.clone())
        .await
        .unwrap_err();
    assert_eq!(e1, "transient upstream failure");

    // attempt 2：重试成功——必须以 start 开新流
    let r2 = wrapper.chat("m", messages, options, tools).await.unwrap();
    assert_eq!(r2.content, "second-attempt-full");

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "error", "start", "delta", "end"],
        "重试必须以 start 开新流（共享 sink 不重置时该帧缺席）"
    );
    // attempt-2 delta 纯净：不携带 attempt-1 残尾
    assert_eq!(texts(&events)[4], "second-attempt-full");
}

/// 对称臂：非流式 provider（trait 缺省 chat_streamed）
/// 跨两次 chat()——合成全量增量的判据 deltas_emitted 必须按 attempt 判定
/// （重试成功的回复文本不得被上一 attempt 的 delta 历史短路丢失）。
#[tokio::test]
async fn retry_non_stream_provider_synthesis_not_short_circuited() {
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:r2".to_string(),
        "web:conn2".to_string(),
        "m".to_string(),
    ));
    let wrapper = StreamingLlmProvider::new(
        std::sync::Arc::new(NonStreamProvider { content: "ok" }),
        sink.clone(),
    );
    let (messages, options, tools) = empty_args();

    wrapper
        .chat("m", messages.clone(), options.clone(), tools.clone())
        .await
        .unwrap();
    wrapper.chat("m", messages, options, tools).await.unwrap();

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "end", "start", "delta", "end"],
        "第二次调用仍须合成全量增量（deltas_emitted 按 attempt 判定）"
    );
    assert_eq!(texts(&events)[4], "ok");
}

#[tokio::test]
async fn wrapper_error_path_emits_error_frame() {
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:s4".to_string(),
        "web:conn4".to_string(),
        "m".to_string(),
    ));
    let wrapper =
        StreamingLlmProvider::new(std::sync::Arc::new(FailingStreamProvider), sink.clone());
    let (messages, options, tools) = empty_args();
    let err = wrapper
        .chat("m", messages, options, tools)
        .await
        .unwrap_err();
    assert_eq!(err, "upstream exploded");

    let events = recv_all(&mut rx);
    assert_eq!(phases(&events), vec!["error"]);
    assert_eq!(texts(&events)[0], "upstream exploded");
}

// ===========================================================================
// B3（2026-11-09）思考流相位：think_start（惰性首发）→ think_delta* →
// think_end（finish/error 收口）。与正文流共用同一 seq 计数器（流内全局
// 单调）；纯思考流（零正文）finish 走 think-only 臂——正文 end 帧不发，
// think_end 照常收口；reset_for_new_stream 清思考相位状态。
// ===========================================================================

#[tokio::test]
async fn think_phases_lazy_start_delta_end() {
    let (sink, mut rx) = make_sink();

    // 首思考片段：惰性 think_start + 立即冲刷（r_last_flush None → 到期）。
    sink.on_reasoning_delta("短的");
    // 第二片段进缓冲（<64 字符、150ms 窗内）。
    sink.on_reasoning_delta("第二段思考");
    // 纯思考流（零正文增量）finish：think-only 臂。
    sink.finish();

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["think_start", "think_delta", "think_delta", "think_end"],
        "惰性 think_start + 缓冲冲刷 + think_end 收口；纯思考流无正文 end 帧"
    );
    assert_eq!(texts(&events)[1], "短的");
    assert_eq!(texts(&events)[2], "第二段思考");
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, vec![0, 1, 2, 3], "共享 seq 计数器单调");
}

#[tokio::test]
async fn mixed_content_and_think_share_seq_monotonic() {
    let (sink, mut rx) = make_sink();

    sink.on_delta("hello"); // start + delta
    sink.on_reasoning_delta("思考"); // think_start + think_delta
    // finish：正文 buf 空（首增量已冲）但 any_deltas=true → end 帧；
    // 思考残余已冲 → think_end。
    sink.finish();

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec![
            "start",
            "delta",
            "think_start",
            "think_delta",
            "end",
            "think_end"
        ],
        "正文与思考帧交错时相位正确"
    );
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    for (i, s) in seqs.iter().enumerate() {
        assert_eq!(*s, i as u64, "seq 流内全局单调（正文与思考共享计数器）");
    }
}

#[tokio::test]
async fn error_path_flushes_residual_think_end() {
    let (sink, mut rx) = make_sink();

    sink.on_reasoning_delta("未完的思考");
    sink.error("boom");

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["think_start", "think_delta", "think_end", "error"],
        "error 收口前先补 think_end（前端思考卡不悬挂）"
    );
}

#[tokio::test]
async fn no_reasoning_zero_think_frames() {
    let (sink, mut rx) = make_sink();

    sink.on_delta("正文而已");
    sink.finish();

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "end"],
        "零思考 = 零 think 帧（既有行为不变）"
    );
}

#[tokio::test]
async fn reset_for_new_stream_clears_think_state() {
    let (sink, mut rx) = make_sink();

    // 第一流：思考 → 收口。
    sink.on_reasoning_delta("第一流的思考");
    sink.finish();
    // 重试 = 新流。
    sink.reset_for_new_stream();
    // 第二流：思考相位状态必须已清——think_start 重新惰性首发，seq 归零。
    sink.on_reasoning_delta("第二流的思考");
    sink.finish();

    let events = recv_all(&mut rx);
    let ps = phases(&events);
    assert_eq!(
        ps,
        vec![
            "think_start",
            "think_delta",
            "think_end",
            "think_start",
            "think_delta",
            "think_end"
        ],
        "reset 后 think_start 重新惰性首发"
    );
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(seqs[3], 0, "第二流 seq 从 0 重开");
    assert_eq!(texts(&events)[4], "第二流的思考");
}

// ---------------------------------------------------------------------------
// StreamingLlmProvider 双回调接线：inner 覆写 chat_streamed_with_reasoning
// → 思考片段进 sink（think 帧）；trait 缺省实现（忽略思考回调）→ 即使响应
// 带 reasoning_content 也零 think 帧（合成增量只覆盖正文，不重复思考）。
// ---------------------------------------------------------------------------

struct ReasoningStreamProvider;

#[async_trait]
impl LlmProvider for ReasoningStreamProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        unreachable!("chat_streamed_with_reasoning 覆写后 chat 不应被调用")
    }

    async fn chat_streamed(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
        _on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<LlmResponse, String> {
        unreachable!("wrapper 应走 chat_streamed_with_reasoning")
    }

    async fn chat_streamed_with_reasoning(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
        on_delta: &(dyn Fn(String) + Send + Sync),
        on_reasoning_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<LlmResponse, String> {
        on_reasoning_delta("第一步".to_string());
        on_delta("答".to_string());
        on_reasoning_delta("第二步".to_string());
        Ok(LlmResponse {
            content: "答".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: Some("第一步第二步".to_string()),
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[tokio::test]
async fn wrapper_real_stream_carries_think_frames() {
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:s5".to_string(),
        "web:conn5".to_string(),
        "m".to_string(),
    ));
    let wrapper =
        StreamingLlmProvider::new(std::sync::Arc::new(ReasoningStreamProvider), sink.clone());
    let (messages, options, tools) = empty_args();
    let resp = wrapper.chat("m", messages, options, tools).await.unwrap();
    assert_eq!(resp.reasoning_content.as_deref(), Some("第一步第二步"));

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec![
            "think_start",
            "think_delta",
            "start",
            "delta",
            "end",
            "think_delta",
            "think_end"
        ],
        "双回调接线：思考帧与正文帧同流交错；finish 先发正文 end、后冲刷残余\
         思考（think_delta + think_end 收口）"
    );
    assert_eq!(texts(&events)[1], "第一步");
    assert_eq!(texts(&events)[3], "答");
    assert_eq!(texts(&events)[5], "第二步");
}

struct NonStreamWithReasoningProvider;

#[async_trait]
impl LlmProvider for NonStreamWithReasoningProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "全文".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: Some("思考全文".to_string()),
            usage: None,
            finish_reason: None,
            escalated_to: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[tokio::test]
async fn wrapper_default_reasoning_path_no_think_frames() {
    // trait 缺省实现忽略思考回调：非流式 provider 即使响应带
    // reasoning_content 也零 think 帧（终态回复自带思考全文，不重复发射）。
    let (tx, _) = tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    let mut rx = tx.subscribe();
    let sink = std::sync::Arc::new(ProviderStreamSink::new(
        tx,
        "web:main:s6".to_string(),
        "web:conn6".to_string(),
        "m".to_string(),
    ));
    let wrapper = StreamingLlmProvider::new(
        std::sync::Arc::new(NonStreamWithReasoningProvider),
        sink.clone(),
    );
    let (messages, options, tools) = empty_args();
    wrapper.chat("m", messages, options, tools).await.unwrap();

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec!["start", "delta", "end"],
        "缺省思考路径零 think 帧"
    );
}

// ===========================================================================
// seq 取号回归锁（BUG-20261111：finish/finish_reasoning pending 分支发两帧
// 只推进 1——end 帧与 think_delta 帧同号 / error 帧与 think_end 帧同号，
// 违反 SinkState「流内全局单调」契约）。既有 B3 测试没踩中：缓冲都在
// finish 前已冲空（pending=false 走单帧臂）。触发形态 = 残尾同时在缓冲。
// ===========================================================================

/// 正文残尾 + 思考残尾同时进缓冲 → finish 发 delta/end/think_delta/
/// think_end 四帧，seq 必须严格递增无重复。
#[tokio::test]
async fn finish_pending_text_and_think_frame_seqs_unique() {
    let (sink, mut rx) = make_sink();

    sink.on_delta("hello"); // start@0 + delta@1（首增量立即冲）
    sink.on_delta("残尾"); // 正文残尾进缓冲（<64 字符、窗内不冲）
    sink.on_reasoning_delta("思考中"); // think_start@2 + think_delta@3（首片段立即冲）
    sink.on_reasoning_delta("残思"); // 思考残尾进缓冲
    sink.finish(); // delta@4 + end@5，随后 think_delta@6 + think_end@7

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec![
            "start",
            "delta",
            "think_start",
            "think_delta",
            "delta",
            "end",
            "think_delta",
            "think_end"
        ],
        "正文两帧 + 思考两帧按序收口"
    );
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4, 5, 6, 7],
        "共享计数器流内全局单调：修复前 end@5 与 think_delta@5 同号"
    );
    assert_eq!(texts(&events)[4], "残尾");
    assert_eq!(texts(&events)[6], "残思");
}

/// 思考残尾进缓冲后 error() 收口 → think_delta/think_end/error 三帧
/// seq 严格递增：修复前 error 帧与 think_end 同号。
#[tokio::test]
async fn error_after_residual_think_frame_seq_unique() {
    let (sink, mut rx) = make_sink();

    sink.on_reasoning_delta("思考"); // think_start@0 + think_delta@1（首片段立即冲）
    sink.on_reasoning_delta("残"); // 残尾进缓冲
    sink.error("boom"); // think_delta@2 + think_end@3，error@4

    let events = recv_all(&mut rx);
    assert_eq!(
        phases(&events),
        vec![
            "think_start",
            "think_delta",
            "think_delta",
            "think_end",
            "error"
        ],
        "error 收口前先冲思考残尾（think_delta + think_end）再发 error 终态帧"
    );
    let seqs: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            nemesis_types::agent::AgentEvent::ProviderStream { seq, .. } => Some(*seq),
            _ => None,
        })
        .collect();
    assert_eq!(
        seqs,
        vec![0, 1, 2, 3, 4],
        "修复前 think_end@3 与 error@3 同号"
    );
}
