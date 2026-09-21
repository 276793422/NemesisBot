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
async fn overloaded_error_enters_rate_limit_loop_and_recovers() {
    // 2026-09-20 BUG：`provider codex is overloaded`（FailoverError::Overloaded
    // 展平形态，状态码已丢）此前两环都不命中 → 一次终局。词表加 overloaded
    // 后必须按限流对待（对齐 error_classifier 既有约定）并显式进度。
    let provider = RateLimitedThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: 2,
        err_text: "provider codex is overloaded".to_string(),
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
        "过载重试后必须恢复（不得一次终局）: {events:?}"
    );
    let mut progresses = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if msg.content.contains("重试") {
            progresses.push(msg.content);
        }
    }
    assert_eq!(progresses.len(), 2, "过载进度通知: {progresses:?}");
    assert!(progresses[0].contains("第 1/10 次"));
}

#[tokio::test(start_paused = true)]
async fn overloaded_exhausted_yields_structured_terminal_error() {
    // 过载耗尽同样走结构化终局（上游限流口径），不裸抛。
    let provider = RateLimitedThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: usize::MAX,
        err_text: "provider codex is overloaded".to_string(),
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
        "过载终局必须结构化标注: {errs:?}"
    );
    assert!(
        errs.iter()
            .any(|m| m.contains("provider codex is overloaded")),
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

// -- BUG 2026-09-21 ②A：单次调用超时 ---------------------------------------

/// 永不返回的 provider（429 后连接挂起、不回响应体的形态；也是纯首捕
/// 挂死的形态）。
struct Hanging;

#[async_trait]
impl LlmProvider for Hanging {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        std::future::pending::<Result<LlmResponse, String>>().await
    }
}

/// 先 429 一次、之后挂死的 provider（用户实测案例 1:1：上游先回 429，
/// 重试环内的调用建立连接后不回响应体）。
struct RateLimitedThenHang(std::sync::atomic::AtomicUsize);

#[async_trait]
impl LlmProvider for RateLimitedThenHang {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Err("rate limited by provider codex/gpt-test".to_string());
        }
        std::future::pending::<Result<LlmResponse, String>>().await
    }
}

#[tokio::test(start_paused = true)]
async fn rate_limited_then_hanging_call_times_out_and_loop_stays_bounded() {
    // ②A（用户案例形态）：首捕 429 → 重试环内调用挂死 → 按超时失败计，
    // 走完既有环后结构化终局——不再无限等待（实测挂死 27 分钟）。预算关
    // （0）以隔离验证超时本身。
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {
            "rate_limit_retries": 2,
            "provider_call_timeout_secs": 60,
            "rate_limit_budget_secs": 0,
        }}})
        .to_string(),
    )
    .unwrap();

    let agent_loop = AgentLoop::new(
        Box::new(RateLimitedThenHang(std::sync::atomic::AtomicUsize::new(0))),
        test_config(),
    );
    agent_loop.set_config_path(cfg_path);
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
        errs.iter()
            .any(|m| m.contains("已重试 2 次") && m.contains("上游调用超时")),
        "挂死调用必须按超时失败计并结构化终局: {errs:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn hanging_first_call_times_out_into_transient_loop_and_terminates() {
    // ②A（首捕挂死形态）：首捕调用统一包超时（chat_call_bounded 收口的
    // 五个调用点之一）——超时文案含 "timed out" 落入 transient 环有限
    // 重试（每次重试同样包超时），3 次耗尽后终局，不再无限挂。
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {
            "provider_call_timeout_secs": 60,
        }}})
        .to_string(),
    )
    .unwrap();

    let agent_loop = AgentLoop::new(Box::new(Hanging), test_config());
    agent_loop.set_config_path(cfg_path);
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
        errs.iter().any(|m| m.contains("上游调用超时")),
        "首捕挂死必须按超时失败计并终局: {errs:?}"
    );
}

// -- BUG 2026-09-21 ②B：重试环总预算 ---------------------------------------

#[tokio::test(start_paused = true)]
async fn retry_budget_exhausted_yields_budget_terminal_error() {
    // ②B：预算（等待+调用累计）超限直接终局——10 次上限只限次数不限总
    // 时长（实测 46 分钟）的补丁。预算 30s + 单次调用超时 60s：首捕 429
    // 后首轮调用挂满 60s，第二轮循环顶预算检查命中 → 终局「已重试 1 次」。
    let tmp = tempfile::tempdir().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(
        &cfg_path,
        serde_json::json!({"agents": {"defaults": {
            "rate_limit_retries": 10,
            "provider_call_timeout_secs": 60,
            "rate_limit_budget_secs": 30,
        }}})
        .to_string(),
    )
    .unwrap();

    let agent_loop = AgentLoop::new(
        Box::new(RateLimitedThenHang(std::sync::atomic::AtomicUsize::new(0))),
        test_config(),
    );
    agent_loop.set_config_path(cfg_path);
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
        errs.iter()
            .any(|m| m.contains("总时长超过 30 秒预算") && m.contains("已重试 1 次")),
        "预算耗尽必须结构化终局: {errs:?}"
    );
}

// -- BUG 2026-09-21 ①：retry_status 快照 ------------------------------------

#[test]
fn retry_status_snapshot_roundtrip_and_expiry() {
    let agent_loop = AgentLoop::new(Box::new(Hanging), test_config());
    let key = "agent:main:session:s1";
    assert!(agent_loop.retry_status(key).is_none(), "未写入 = None");

    agent_loop.set_rate_limit_status(
        key,
        RateLimitStatus {
            retry: 3,
            max_retries: 10,
            wait_secs: 40,
            model: "gpt-test".to_string(),
            updated_at: std::time::Instant::now(),
        },
    );
    let st = agent_loop.retry_status(key).expect("新鲜快照必须可读");
    assert_eq!(st.retry, 3);
    assert_eq!(st.max_retries, 10);
    assert_eq!(st.wait_secs, 40);
    assert_eq!(st.model, "gpt-test");

    // 过期快照（E-STOP abort 残留形态）读侧判失效并回收。
    agent_loop.set_rate_limit_status(
        key,
        RateLimitStatus {
            retry: 4,
            max_retries: 10,
            wait_secs: 60,
            model: "gpt-test".to_string(),
            updated_at: std::time::Instant::now()
                - std::time::Duration::from_secs(RETRY_STATUS_STALE_SECS + 1),
        },
    );
    assert!(
        agent_loop.retry_status(key).is_none(),
        "超 RETRY_STATUS_STALE_SECS 的快照必须视为失效"
    );
    assert!(
        agent_loop.rate_limit_status.lock().is_empty(),
        "失效快照读侧顺手回收"
    );

    agent_loop.set_rate_limit_status(
        key,
        RateLimitStatus {
            retry: 1,
            max_retries: 10,
            wait_secs: 5,
            model: "m".to_string(),
            updated_at: std::time::Instant::now(),
        },
    );
    agent_loop.clear_rate_limit_status(key);
    assert!(agent_loop.retry_status(key).is_none(), "clear 后 = None");
}
