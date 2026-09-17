//! 429 限流重试环测试（2026-09-17 BUG 文档裁决④⑧）。
//!
//! 覆盖：阶梯 + Retry-After 取 max（纯函数）；重试成功恢复；耗尽后结构化
//! 终局文案；`agents.defaults.rate_limit_retries` fresh-read 覆盖生效；
//! 显式进度通知经 outbound_tx 可见。间隔 sleep 经 tokio `start_paused`
//! 自动推进（零真实等待）。

use super::*;

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

/// 限流 N 次后成功的 provider（记录调用次数）。
struct RateLimitedThenSuccess {
    calls: std::sync::atomic::AtomicUsize,
    fail_times: usize,
    err_text: String,
}

#[async_trait]
impl LlmProvider for RateLimitedThenSuccess {
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

// -- 纯函数：阶梯 + Retry-After 取 max ------------------------------------

#[test]
fn ladder_follows_10_step_design() {
    // 裁决④：5s→10s→20s→40s→60s→90s→120s→150s→180s→210s。
    let want = [5u64, 10, 20, 40, 60, 90, 120, 150, 180, 210];
    for (i, w) in want.iter().enumerate() {
        assert_eq!(rate_limit_ladder_secs(i as u32 + 1), *w);
    }
    assert_eq!(rate_limit_ladder_secs(11), 210, "超出阶梯取末档");
}

#[test]
fn retry_after_and_ladder_take_max() {
    // 裁决⑧：遵从 Retry-After，但不低于本地阶梯。
    assert_eq!(
        rate_limit_wait_secs("rate limited by provider p/m (retry_after=120s)", 1),
        120,
        "上游 120s > 阶梯 5s → 等上游"
    );
    assert_eq!(
        rate_limit_wait_secs("rate limited by provider p/m (retry_after=2s)", 3),
        20,
        "上游 2s < 阶梯 20s → 取阶梯"
    );
    assert_eq!(
        rate_limit_wait_secs("rate limited by provider p/m", 2),
        10,
        "无 Retry-After → 纯阶梯"
    );
    assert_eq!(extract_retry_after_secs("no suffix"), None);
}

// -- 环行为 ----------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn rate_limit_retry_succeeds_and_notifies_progress() {
    let provider = RateLimitedThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: 2,
        err_text: "rate limited by provider codex/gpt-test".to_string(),
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

    // 成功恢复：Done 正常。
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Done(m) if m == "Recovered!")),
        "限流重试后必须恢复: {events:?}"
    );

    // 裁决③「显式进度」：两次失败 → 恰好两条进度通知经 outbound_tx。
    let mut progresses = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if msg.content.contains("重试") {
            progresses.push(msg.content);
        }
    }
    assert_eq!(progresses.len(), 2, "进度通知: {progresses:?}");
    assert!(progresses[0].contains("第 1/10 次"));
    assert!(progresses[0].contains("等待 5 秒"));
    assert!(progresses[1].contains("第 2/10 次"));
}

#[tokio::test(start_paused = true)]
async fn rate_limit_exhausted_yields_structured_terminal_error() {
    // 默认 10 次（fresh-read 无 config_path = 默认口径）：1 首捕 + 10 重试
    // = 11 次调用；终局文案结构化标注（裁决④），不再裸抛。
    let provider = RateLimitedThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: usize::MAX,
        err_text: "rate limited by provider codex/gpt-test".to_string(),
    };
    let agent_loop = AgentLoop::new(Box::new(provider), test_config());
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;

    let errs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert!(
        errs.iter().any(|m| m.contains("上游限流，已重试 10 次")),
        "终局错误必须结构化标注: {errs:?}"
    );
    assert!(
        errs.iter().any(|m| m.contains("rate limited by provider")),
        "终局错误保留原始原因: {errs:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn rate_limit_retries_config_override_takes_effect() {
    // `agents.defaults.rate_limit_retries` fresh-read（F8 模式）：设 2 →
    // 1 首捕 + 2 重试 = 3 次调用即终局。
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {"rate_limit_retries": 2}}}).to_string(),
    )
    .unwrap();

    struct Counting {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    #[async_trait]
    impl LlmProvider for Counting {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<crate::types::ChatOptions>,
            _tools: Vec<crate::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err("rate limited by provider p/m".to_string())
        }
    }
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let agent_loop = AgentLoop::new(
        Box::new(Counting {
            calls: calls.clone(),
        }),
        test_config(),
    );
    agent_loop.set_config_path(cfg_path);
    let instance = AgentInstance::new(test_config());
    let context = RequestContext::new("web", "chat1", "user1", "session1");

    let events = agent_loop.run(&instance, "Hello", &context).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::Error(m) if m.contains("已重试 2 次"))),
        "配置覆盖 2 次必须生效: {events:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "1 首捕 + 2 重试 = 恰好 3 次调用"
    );
}
