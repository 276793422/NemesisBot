// llm_bridge ProviderAdapter 降级注记链路测试（fallback 用户面标注）：
// FallbackProvider extra 盖章（`_served_by_fallback`，章的真值已由
// nemesis-providers fallback_provider/tests.rs 钉住）→ adapter chat /
// 非流式回落两路取件入槽 → trait 取件口取件即清。内层 mock 只复刻
// 「带章响应」这一最小契约，不重实现链轮转。

use super::*;
use nemesis_agent::r#loop::LlmProvider;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use std::sync::Arc;

/// 内层 mock：按 `note` 配置在响应 extra 盖降级章（Some=盖章）。
/// note 运行期可换——覆盖语义（干净调用清槽）需要同一 adapter 换内层
/// 章值才能验证。
struct SwappableStampProvider {
    note: parking_lot::Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl nemesis_providers::router::LLMProvider for SwappableStampProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        let mut resp = LLMResponse {
            content: "ok".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        };
        if let Some(note) = self.note.lock().clone() {
            resp.extra
                .insert("_served_by_fallback".to_string(), serde_json::json!(note));
        }
        Ok(resp)
    }

    fn default_model(&self) -> &str {
        "stamp-model"
    }

    fn name(&self) -> &str {
        "stamp-inner"
    }
}

fn stamp_adapter(note: Option<String>) -> (ProviderAdapter, Arc<SwappableStampProvider>) {
    let inner = Arc::new(SwappableStampProvider {
        note: parking_lot::Mutex::new(note),
    });
    (
        ProviderAdapter::new(inner.clone(), "stamp-model".to_string()),
        inner,
    )
}

/// chat 路：盖章响应 → 槽持有注记；取件即清；内层改干净（None）后
/// 再调用 → 槽被覆盖为 None（last-call 覆盖，不残留陈旧章）。
#[tokio::test]
async fn adapter_chat_carries_and_clears_degradation_note() {
    let (adapter, inner) = stamp_adapter(Some("主模型x → stamp-inner/备选y".to_string()));
    assert!(
        adapter.take_response_degradation().is_none(),
        "未调用前槽空"
    );

    let resp = adapter
        .chat("", Vec::new(), None, Vec::new())
        .await
        .unwrap();
    assert_eq!(resp.content, "ok");
    assert_eq!(
        adapter.take_response_degradation().as_deref(),
        Some("主模型x → stamp-inner/备选y")
    );
    assert!(adapter.take_response_degradation().is_none(), "取件即清");

    // 干净响应 → last-call 覆盖清槽。
    *inner.note.lock() = None;
    adapter
        .chat("", Vec::new(), None, Vec::new())
        .await
        .unwrap();
    assert!(adapter.take_response_degradation().is_none());
}

/// 流式 lane 非流式回落路（chat_streamed → stream_preferred=false →
/// fallback_non_stream → inner.chat）：同样取件入槽。
#[tokio::test]
async fn adapter_streamed_fallback_stashes_note() {
    let (adapter, _inner) = stamp_adapter(Some("主 → 备".to_string()));
    let resp = adapter
        .chat_streamed("stamp-model", Vec::new(), None, Vec::new(), &|_| {})
        .await
        .unwrap();
    assert_eq!(resp.content, "ok");
    assert_eq!(
        adapter.take_response_degradation().as_deref(),
        Some("主 → 备")
    );
}

// ==========================================================================
// LLM-ERR-B（2026-11-17 治本批）：chat_streamed 自带聚合循环的确定性
// 应用层错误透传回归锁。本聚合循环不走 provider 内部 collect_stream_response
// （上方注释明示），RateLimit/Auth/Billing 在非空 partial 下同样不得包
// StreamInterrupted（分类拍扁 → 零退避 429 风暴 + 缝合怪终答）；Unknown
// 维持既有包装契约（对照臂钉住语义分界）。
// ==========================================================================

/// 流内错误 mock：stream_preferred=true，chat_stream 产出 1 个 delta chunk
/// 后发预设 Err；`chat()` 返回哨兵内容——若被误走到（应走真流式聚合），
/// 断言当即暴露。
struct StreamErrProvider {
    err: FailoverError,
}

#[async_trait::async_trait]
impl nemesis_providers::router::LLMProvider for StreamErrProvider {
    async fn chat(
        &self,
        _messages: &[nemesis_providers::types::Message],
        _tools: &[nemesis_providers::types::ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Ok(LLMResponse {
            content: "SHOULD_NOT_FALLBACK".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    fn chat_stream(
        &self,
        _messages: &[nemesis_providers::types::Message],
        _tools: &[nemesis_providers::types::ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> tokio::sync::mpsc::Receiver<
        Result<nemesis_providers::http_provider::StreamChunk, FailoverError>,
    > {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let _ = tx.try_send(Ok(nemesis_providers::http_provider::StreamChunk {
            delta: "半截".to_string(),
            ..Default::default()
        }));
        let _ = tx.try_send(Err(self.err.clone()));
        rx
    }

    fn stream_preferred(&self, _model: &str) -> bool {
        true
    }

    fn default_model(&self) -> &str {
        "serr-model"
    }

    fn name(&self) -> &str {
        "serr-inner"
    }
}

fn stream_err_adapter(err: FailoverError) -> ProviderAdapter {
    ProviderAdapter::new(
        Arc::new(StreamErrProvider { err }),
        "serr-model".to_string(),
    )
}

#[tokio::test]
async fn streamed_rate_limit_passes_through_without_partial_stitch() {
    let adapter = stream_err_adapter(FailoverError::RateLimit {
        provider: "serr-inner".into(),
        model: "serr-model".into(),
        retry_after: None,
    });
    let err = adapter
        .chat_streamed("serr-model", Vec::new(), None, Vec::new(), &|_| {})
        .await
        .unwrap_err();
    assert!(
        err.starts_with("rate limited by provider"),
        "RateLimit 必须透传（Display 前缀进限流环），实际: {err}"
    );
    assert!(
        !err.contains("半截"),
        "被拒请求的半成品不得缝进错误值（拼接续写根源）: {err}"
    );
    assert!(
        !err.contains("stream interrupted from provider"),
        "不得包 StreamInterrupted（前缀短路恒判 Timeout）: {err}"
    );
}

#[tokio::test]
async fn streamed_unknown_still_wraps_stream_interrupted() {
    // 对照臂：Unknown（传输类）维持既有包装契约——partial 随错误携带。
    let adapter = stream_err_adapter(FailoverError::Unknown {
        provider: "serr-inner".into(),
        message: "boom".into(),
    });
    let err = adapter
        .chat_streamed("serr-model", Vec::new(), None, Vec::new(), &|_| {})
        .await
        .unwrap_err();
    assert!(
        err.starts_with("stream interrupted from provider"),
        "非确定性错误维持 StreamInterrupted 携带契约: {err}"
    );
    assert!(
        err.contains("半截"),
        "partial 随错误保留（既有语义）: {err}"
    );
}
