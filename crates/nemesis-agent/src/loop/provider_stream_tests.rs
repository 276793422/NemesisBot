//! provider_stream 覆盖（L10，2026-10-13）：sink 合并窗/相位序列/seq 单调、
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
