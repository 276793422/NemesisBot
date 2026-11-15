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
