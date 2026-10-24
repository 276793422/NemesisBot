//! T2a 错误分类器分派测试（扩展计划 D4-2a）：recovery.rs 错误入口消费
//! `nemesis_utils::llm_error_class::classify_llm_error` 后的分派：
//!
//! - Auth/Billing → 零重试零压缩环（单次调用终局，文案带可行动提示 +
//!   原始错误保真入 history/observer/capture）；
//! - "invalid api key" 不再误入 context 压缩环（旧词表 "invalid" 关键字
//!   误吞——回归锁，必须单次调用终局）；
//! - 词表盲区配额文案（Gemini "resource has been exhausted"）→ 限流环
//!   恢复（分类器覆盖盲区的增量收益）；
//! - 词表盲区 deadline 文案 → transient 环恢复；
//! - 传输层盖章（"connect error"）+ TLS 词（"invalid peer certificate"）→
//!   transient 环而非 context 压缩环（盖章错误无 HTTP 响应体，永远不是
//!   context 错误——2026-10-13 双保险的最后一条臂）；
//! - 含 "invalid" 的真实 context 溢出照旧进压缩环（闸的对照面）。
//!
//! 间隔 sleep 经 tokio `start_paused` 自动推进（零真实等待），同
//! rate_limit_retry_tests 形态。

use super::*;

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

/// 恒失败 provider（不可重试类断言「只被调一次」——压缩环耗尽同样只产出
/// 一个 Error 事件，事件数证明不了单次调用，必须看计数器）。
struct AlwaysFailing {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    err_text: String,
}

/// 计数 provider：前 `fail_times` 次返回 `err_text`，之后成功。
struct ClassifiedThenSuccess {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fail_times: usize,
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

#[async_trait]
impl LlmProvider for ClassifiedThenSuccess {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < self.fail_times {
            Err(self.err_text.clone())
        } else {
            Ok(LlmResponse {
                finish_reason: None,
                escalated_to: None,
                content: "Recovered!".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        }
    }
}

fn single_error_text(events: &[AgentEvent]) -> &String {
    let errs: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(errs.len(), 1, "必须恰好一次 Error 终局: {events:?}");
    errs.first().expect("asserted non-empty above")
}

// -- ① Auth：单次调用终局 ---------------------------------------------------

#[tokio::test(start_paused = true)]
async fn auth_error_terminates_without_retry() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = AlwaysFailing {
        calls: calls.clone(),
        err_text: "status 401: authentication failed for model".to_string(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let err_text = single_error_text(&events);
    assert!(
        err_text.contains("认证失败"),
        "用户文案必须带可行动提示: {err_text}"
    );
    assert!(
        err_text.contains("status 401"),
        "原始错误必须保留在用户文案（诊断保真）: {err_text}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Done(_))),
        "不可重试类不得有成功 Done"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "401 必须单次调用终局（零重试零压缩环）"
    );
}

// -- ② Billing：单次调用终局 -------------------------------------------------

#[tokio::test(start_paused = true)]
async fn billing_error_terminates_without_retry() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = AlwaysFailing {
        calls: calls.clone(),
        err_text: "status 402: payment required, insufficient credits".to_string(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let err_text = single_error_text(&events);
    assert!(
        err_text.contains("计费失败"),
        "用户文案必须带可行动提示: {err_text}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "402 必须单次调用终局"
    );
}

// -- ③ "invalid api key" 回归：不得误入 context 压缩环 ------------------------

#[tokio::test(start_paused = true)]
async fn invalid_api_key_skips_context_compression_ring() {
    // 旧词表病灶：'invalid api key' 含 'invalid' → is_context_error 命中
    // → force_compression × 2 白烧。分类器 Auth 优先拦截后必须单次调用
    // （旧行为计数为 3：首呼 + 2 次压缩重试）。
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = AlwaysFailing {
        calls: calls.clone(),
        err_text: "invalid api key provided for this model".to_string(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let err_text = single_error_text(&events);
    assert!(
        err_text.contains("认证失败"),
        "'invalid api key' 必须被分类器识别为认证失败: {err_text}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "不得进入 context 压缩环（旧行为 = 3 次调用）"
    );
}

// -- ④ 配额文案（词表盲区）→ 限流环恢复 --------------------------------------

#[tokio::test(start_paused = true)]
async fn quota_wording_enters_rate_limit_ring_and_recovers() {
    // "resource has been exhausted" 是 error_classifier 的限流模式，但不在
    // loop 侧 RATE_LIMIT_ERROR_KEYWORDS 词表——分类器接入前该文案一次终局。
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = ClassifiedThenSuccess {
        calls: calls.clone(),
        fail_times: 1,
        err_text: "Resource has been exhausted (e.g. check quota).".to_string(),
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m == "Recovered!")),
        "配额文案必须进限流环并恢复: {events:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "首败 + 限流环重试成功 = 恰好 2 次调用"
    );
    // 限流环显式进度可见（裁决③）——证明走的是限流环而非词表兜底终局。
    let mut saw_progress = false;
    while let Ok(msg) = rx.try_recv() {
        if msg.content.contains("重试") {
            saw_progress = true;
        }
    }
    assert!(saw_progress, "限流环进度通知必须发出");
}

// -- ⑤ 传输层盖章 + TLS 词 → transient 环，不进 context 压缩环 ---------------

#[tokio::test(start_paused = true)]
async fn connect_error_with_invalid_word_enters_transient_ring_not_compression() {
    // 2026-10-13 诚实化盖章的遗留缝隙：TLS 失败根因链含 rustls 文案
    // "invalid peer certificate: Expired"——'invalid' 命中 is_context_error
    // 词表，会把传输层错误劫进 context 压缩环（2 次立即重呼 + summary
    // cache 强制推进），正确归宿是 transient 环（3 次重试预算）。用
    // fail_times=3 区分：旧路由（压缩环只有 2 发子弹）= 3 次调用后 Error
    // 终局；修复后（transient 环 3 发）= 第 4 次调用成功恢复。
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = ClassifiedThenSuccess {
        calls: calls.clone(),
        fail_times: 3,
        err_text: "unknown error from provider anthropic: connect error (model m): \
                   error sending request for url (https://api.example.com/v1): \
                   error trying to connect: invalid peer certificate: Expired"
            .to_string(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m == "Recovered!")),
        "传输层盖章错误必须进 transient 环并恢复: {events:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        4,
        "首败 + transient 环 3 次重试 = 恰好 4 次调用（压缩环路由 = 3 次即 Error 终局）"
    );
}

// -- ⑥ 真实 context 溢出不受传输层闸影响（闸的对照面） ------------------------

#[tokio::test(start_paused = true)]
async fn genuine_context_error_still_enters_compression_ring() {
    // 传输层闸只拦 "connect error" 盖章——真实 context 溢出（应用层响应体
    // 文案，无盖章）必须照旧进压缩环恢复（l3_guess_too_big… 已锁机制，
    // 这里锁的是闸没有误伤含 "invalid" 的 context 文案这一对照面）。
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = ClassifiedThenSuccess {
        calls: calls.clone(),
        fail_times: 1,
        err_text: "invalid request: this model's maximum context length is \
                   32768 tokens, however you requested 40000"
            .to_string(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m == "Recovered!")),
        "含 'invalid' 的真实 context 溢出必须照旧进压缩环恢复: {events:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "首败 + 压缩环摘要生成 + 压缩环重呼成功 = 恰好 3 次调用（force_compression \
         推进摘要缓存时经 resolve_summary_provider(false) 走主 provider 发一次真实 \
         摘要 LLM 调用，被同一计数器计入；限流/transient 环不压缩故无此臂）"
    );
}

// -- ⑦ deadline 文案（词表盲区）→ transient 环恢复 ----------------------------

#[tokio::test(start_paused = true)]
async fn deadline_wording_enters_transient_ring_and_recovers() {
    // "deadline exceeded" 在 error_classifier 的 timeout 模式，不在 loop 侧
    // transient 词表（且刻意避开 "context deadline exceeded"——那含 "context"
    // 会先进压缩环，属既有语义不动）。分类器接入前该文案一次终局。
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider = ClassifiedThenSuccess {
        calls: calls.clone(),
        fail_times: 1,
        err_text: "rpc error: upstream deadline exceeded".to_string(),
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(provider),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m == "Recovered!")),
        "deadline 文案必须进 transient 环并恢复: {events:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "首败 + transient 环重试成功 = 恰好 2 次调用"
    );
}
