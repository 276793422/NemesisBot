//! wire 语义闸（2026-10-14 T37/T42 根修）：`ProviderAdapter::chat_streamed`
//! 必须尊重 provider 的 `stream_preferred` 裁决——偏好非流式时静默回落
//! `chat()`（usage / raw 请求响应体全量保留），不得越权走真流式。此前该
//! lane 无条件 `chat_stream`，流式路径 raw 体恒 None 且 usage 依赖上游
//! chunk 自带 → 集群 usage 回传断粮 + raw 日志响应件缺失（观测 lane 改写
//! wire 语义的教训，详见提交 docs 报告）。
//!
//! 附带钉死三条真实 lane 的 `stream_preferred` 判据（与各自 `chat()` 内部
//! 切换同源）：http-compat / anthropic auto=非流式，codex auto=流式。

use crate::llm_bridge::ProviderAdapter;
use nemesis_agent::r#loop::LlmMessage;
use nemesis_agent::r#loop::LlmProvider as AgentLlmProvider;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::http_provider::{HttpProvider, HttpProviderConfig, StreamChunk};
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{
    ChatOptions as ProviderChatOptions, LLMResponse, Message as ProviderMessage,
    ToolDefinition as ProviderToolDef, UsageInfo,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 双 lane mock：chat 恒成功（带 usage + raw 体），chat_stream 按配置给
/// 真流/「不支持」探针错误。`stream_called` 记录流 lane 是否被触达——
/// 闸生效的直接证据。
struct GateMock {
    prefer_stream: bool,
    stream_supported: bool,
    stream_called: AtomicBool,
}

impl GateMock {
    fn new(prefer_stream: bool, stream_supported: bool) -> Self {
        Self {
            prefer_stream,
            stream_supported,
            stream_called: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl LLMProvider for GateMock {
    async fn chat(
        &self,
        _messages: &[ProviderMessage],
        _tools: &[ProviderToolDef],
        _model: &str,
        _options: &ProviderChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Ok(LLMResponse {
            content: "非流式正文".to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: Some(UsageInfo {
                prompt_tokens: 11,
                completion_tokens: 7,
                total_tokens: 18,
                ..Default::default()
            }),
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: Some(serde_json::json!({"kind": "req"})),
            raw_response_body: Some("RAW-RESPONSE-BODY".to_string()),
        })
    }

    fn chat_stream(
        &self,
        _messages: &[ProviderMessage],
        _tools: &[ProviderToolDef],
        _model: &str,
        _options: &ProviderChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        self.stream_called.store(true, Ordering::SeqCst);
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        if !self.stream_supported {
            let _ = tx.try_send(Err(FailoverError::Unknown {
                provider: "gate-mock".to_string(),
                message: "streaming (chat_stream) not implemented for this provider".to_string(),
            }));
            return rx;
        }
        let _ = tx.try_send(Ok(StreamChunk {
            delta: "流式".to_string(),
            tool_calls: vec![],
            finish_reason: None,
            usage: None,
            reasoning_content: None,
        }));
        let _ = tx.try_send(Ok(StreamChunk {
            delta: String::new(),
            tool_calls: vec![],
            finish_reason: Some("stop".to_string()),
            usage: Some(UsageInfo {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                ..Default::default()
            }),
            reasoning_content: None,
        }));
        rx
    }

    fn stream_preferred(&self, _model: &str) -> bool {
        self.prefer_stream
    }

    fn default_model(&self) -> &str {
        "mock/model"
    }

    fn name(&self) -> &str {
        "gate-mock"
    }
}

fn one_user_msg() -> Vec<LlmMessage> {
    vec![LlmMessage {
        role: "user".to_string(),
        content: "hi".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }]
}

/// 收集 chat_streamed 的增量回调（合成全量 delta 的观察面）。
fn delta_collector() -> Arc<std::sync::Mutex<Vec<String>>> {
    Arc::new(std::sync::Mutex::new(Vec::new()))
}

#[tokio::test]
async fn gate_non_preferred_falls_back_to_chat_preserving_usage_and_raw() {
    // 偏好非流式 + 流 lane 可用（若被调到即失败）：闸必须短路在 chat_stream
    // 之前，chat() 的 usage / raw 请求响应体原样透传（T37/T42 的生命线）。
    let mock = Arc::new(GateMock::new(false, true));
    let adapter = ProviderAdapter::new(mock.clone(), "mock/model".to_string());
    let deltas = delta_collector();
    let d = deltas.clone();
    let resp = adapter
        .chat_streamed(
            "mock/model",
            one_user_msg(),
            None,
            vec![],
            &move |s: String| d.lock().unwrap().push(s),
        )
        .await
        .expect("non-stream fallback succeeds");

    assert!(
        !mock.stream_called.load(Ordering::SeqCst),
        "闸生效：流 lane 不得被触达"
    );
    assert_eq!(resp.content, "非流式正文");
    assert!(
        resp.usage.is_some(),
        "usage 必须经非流式路径保留（集群记账断粮根因）"
    );
    assert_eq!(
        resp.raw_response_body.as_deref(),
        Some("RAW-RESPONSE-BODY"),
        "raw 响应体必须保留（raw 日志响应件缺失根因）"
    );
    assert!(resp.raw_request_body.is_some());
    // 前端增量面无差别：回落合成一条全量增量。
    let got = deltas.lock().unwrap().join("");
    assert_eq!(got, "非流式正文", "回落须合成全量 delta（L10 消费点契约）");
}

#[tokio::test]
async fn gate_preferred_streams_via_stream_lane() {
    // 偏好流式（stream=yes / codex auto）：走真流 lane——内容/usage 出自
    // chunk，raw 体恒 None（流式路径诚实边界，文档化于此）。
    let mock = Arc::new(GateMock::new(true, true));
    let adapter = ProviderAdapter::new(mock.clone(), "mock/model".to_string());
    let deltas = delta_collector();
    let d = deltas.clone();
    let resp = adapter
        .chat_streamed(
            "mock/model",
            one_user_msg(),
            None,
            vec![],
            &move |s: String| d.lock().unwrap().push(s),
        )
        .await
        .expect("stream lane succeeds");

    assert!(
        mock.stream_called.load(Ordering::SeqCst),
        "偏好流式必须走流 lane"
    );
    assert_eq!(resp.content, "流式");
    assert_eq!(
        resp.usage.as_ref().map(|u| u.total_tokens),
        Some(5),
        "流式 usage 来自终态 chunk（上游自带才 Some）"
    );
    assert_eq!(resp.raw_response_body, None, "流式路径无完整响应体快照");
    assert_eq!(deltas.lock().unwrap().join(""), "流式");
}

#[tokio::test]
async fn gate_preferred_but_unsupported_lane_falls_back() {
    // 偏好流式但 provider 无流 lane（CLI 型）：首块探针「not implemented」
    // → 静默回落非流式（既有回落语义，闸不得破坏）。
    let mock = Arc::new(GateMock::new(true, false));
    let adapter = ProviderAdapter::new(mock.clone(), "mock/model".to_string());
    let deltas = delta_collector();
    let d = deltas.clone();
    let resp = adapter
        .chat_streamed(
            "mock/model",
            one_user_msg(),
            None,
            vec![],
            &move |s: String| d.lock().unwrap().push(s),
        )
        .await
        .expect("probe fallback succeeds");

    assert!(mock.stream_called.load(Ordering::SeqCst), "探针触达流 lane");
    assert_eq!(resp.content, "非流式正文");
    assert!(resp.usage.is_some());
    assert_eq!(resp.raw_response_body.as_deref(), Some("RAW-RESPONSE-BODY"));
    assert_eq!(deltas.lock().unwrap().join(""), "非流式正文");
}

// ---------------------------------------------------------------------------
// 真实 lane 的 stream_preferred 判据（与各自 chat() 内部切换同源钉死）
// ---------------------------------------------------------------------------

#[test]
fn stream_preferred_lane_semantics() {
    let http_cfg = |stream: &str| HttpProviderConfig {
        name: "http-compat".to_string(),
        base_url: "http://127.0.0.1:1/v1".to_string(),
        api_key: "k".to_string(),
        default_model: "m".to_string(),
        timeout_secs: 1,
        headers: HashMap::new(),
        proxy: None,
        preserve_prefix: false,
        stream_mode: stream.to_string(),
    };
    // openai 兼容 lane：auto（空）/no = 非流式，yes = 流式。
    assert!(!HttpProvider::new(http_cfg("")).stream_preferred("m"));
    assert!(!HttpProvider::new(http_cfg("no")).stream_preferred("m"));
    assert!(HttpProvider::new(http_cfg("yes")).stream_preferred("m"));

    // anthropic lane：同 http-compat（auto = 维持非流式现状）。
    let anth_cfg = |stream: &str| nemesis_providers::anthropic::AnthropicConfig {
        api_key: "k".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        default_model: "m".to_string(),
        timeout_secs: 1,
        proxy: None,
        stream_mode: stream.to_string(),
    };
    assert!(
        !nemesis_providers::anthropic::AnthropicProvider::new(anth_cfg("")).stream_preferred("m")
    );
    assert!(
        !nemesis_providers::anthropic::AnthropicProvider::new(anth_cfg("no")).stream_preferred("m")
    );
    assert!(
        nemesis_providers::anthropic::AnthropicProvider::new(anth_cfg("yes")).stream_preferred("m")
    );

    // codex lane：auto = 流式（墙钟根因修复形态），显式 no 才非流式。
    let codex_cfg = |stream: &str| nemesis_providers::codex::CodexConfig {
        api_key: "k".to_string(),
        account_id: String::new(),
        default_model: "m".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        timeout_secs: 1,
        proxy: None,
        stream_mode: stream.to_string(),
        ..Default::default()
    };
    assert!(nemesis_providers::codex::CodexProvider::new(codex_cfg("")).stream_preferred("m"));
    assert!(nemesis_providers::codex::CodexProvider::new(codex_cfg("yes")).stream_preferred("m"));
    assert!(!nemesis_providers::codex::CodexProvider::new(codex_cfg("no")).stream_preferred("m"));
}

#[test]
fn stream_preferred_default_is_false_for_laneless_providers() {
    // trait 缺省（CLI 型等无流 lane provider 不覆写即非流式）——保守缺省，
    // 新增 lane 想流式必须显式表态。
    struct Vanilla;
    #[async_trait::async_trait]
    impl LLMProvider for Vanilla {
        async fn chat(
            &self,
            _: &[ProviderMessage],
            _: &[ProviderToolDef],
            _: &str,
            _: &ProviderChatOptions,
        ) -> Result<LLMResponse, FailoverError> {
            Err(FailoverError::Unknown {
                provider: "vanilla".to_string(),
                message: "n/a".to_string(),
            })
        }
        fn default_model(&self) -> &str {
            "m"
        }
        fn name(&self) -> &str {
            "vanilla"
        }
    }
    assert!(!Vanilla.stream_preferred("m"));
}
