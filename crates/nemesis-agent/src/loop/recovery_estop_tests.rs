//! BUG-20261107-026 回归锁：恢复环 cancel/estop 中断面。
//!
//! 2026-07-13 estop Phase 2 只包了首呼与 hook/凭据卡重呼，三恢复环
//! （transient/限流/context）重试调用裸 `.await`——当时的兜底假设是
//! 「checkpoint A 下一轮迭代兜住，最多多跑一次重试调用」。此后限流环
//! 演进出 900s 预算 + Retry-After 梯子、transient 环加了退避梯子，急停
//! 后环内可继续发多次新 LLM 调用（烧 token）+ 白等梯子，「冻结全部
//! agent 活动」对错误路径失效。修复 = 三检查点接入（环顶闸 / sleep 臂 /
//! 调用 select 臂），与 stop_token 同位同型。
//!
//! 真实时钟（非 start_paused）：要验的是「取消/急停打断真实等待」，
//! 同 recovery_stop_tests 纪律。断言窗刻意收紧到 sleep 档长以内（transient
//! 退避首档 1s → <900ms；限流等待档 max(retry_after, 梯子) 5s → <2s），
//! 以区分「sleep/调用臂立即打断」与「环顶闸下一轮迭代兜底」两种形态
//! ——后者无新调用但响应慢一拍，同样算中断面退化，必须红。

use super::*;

fn estop_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn wire_estop(agent_loop: &AgentLoop) -> std::sync::Arc<crate::estop::EstopState> {
    let estop = std::sync::Arc::new(crate::estop::EstopState::new());
    agent_loop.set_estop(estop.clone());
    estop
}

/// 恒失败 provider（transient 措辞；计数器断言急停后零新调用）。
struct AlwaysTransient {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl LlmProvider for AlwaysTransient {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("upstream deadline exceeded".to_string())
    }
}

fn transient_test_loop(
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::sync::Arc<AgentLoop> {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(AlwaysTransient { calls }),
        estop_test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ))
}

/// 恒失败 provider（限流措辞 + retry_after 钉死等待）。
struct AlwaysRateLimited {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl LlmProvider for AlwaysRateLimited {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("status 429 rate limited (retry_after=3)".to_string())
    }
}

fn rate_limit_test_loop(
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::sync::Arc<AgentLoop> {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(AlwaysRateLimited { calls }),
        estop_test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ))
}

/// 首败 transient 措辞，重试调用挂死（entered 翻标志）——验调用 select
/// 臂的在途打断（「中途打断 LLM」对恢复环同样成立）。
struct FailThenHang {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    entered: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl LlmProvider for FailThenHang {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n == 0 {
            return Err("upstream deadline exceeded".to_string());
        }
        self.entered
            .store(true, std::sync::atomic::Ordering::SeqCst);
        std::future::pending::<()>().await;
        unreachable!()
    }
}

fn assert_estop_done(result: Result<(Option<LlmResponse>, String), AgentEvent>) {
    match result {
        Err(AgentEvent::Done(m)) => {
            assert_eq!(
                m,
                recovery::ESTOP_DONE_MSG,
                "急停终局文案必须与首呼臂逐字节一致"
            );
        }
        other => panic!("期望 Err(Done(estop))，得到 {other:?}"),
    }
}

/// 急停必须打断进行中的退避 sleep（~0.1s 退出，而非等满 1s 退避后被
/// 环顶闸兜住）：断言窗 <900ms 刻意收紧以区分「sleep 臂打断」与「闸在
/// 下一轮迭代兜底（≥1.1s）」两种形态——后者无新调用但急停响应慢一拍，
/// 同样算中断面退化。Err(Done(estop))，不再发新调用。
#[tokio::test]
async fn estop_cancels_transient_retry_backoff_sleep() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = transient_test_loop(calls.clone());
    let estop = wire_estop(&agent_loop);
    let instance = AgentInstance::new(estop_test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new("web", "et-chat", "et-sender", "agent:main:session:etback");

    let cancel = tokio_util::sync::CancellationToken::new();
    let started = std::time::Instant::now();
    let task = {
        let agent_loop = agent_loop.clone();
        tokio::spawn(async move {
            agent_loop
                .retry_transient(
                    &instance,
                    &context,
                    &provider,
                    "test-model",
                    &opts,
                    &cancel,
                    None,
                    std::time::Duration::from_secs(2), // ≥1s → 退避形态
                    "upstream deadline exceeded".to_string(),
                )
                .await
        })
    };
    // 落进 1s 退避 sleep 后触发急停。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    estop.trigger();

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("急停后重试环必须立即退出（未修复形态 ≈7s）")
        .expect("task 不应 panic");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(900),
        "急停必须打断退避 sleep 本体（闸兜底形态 ≈1.1s）: {:?}",
        started.elapsed()
    );
    assert_estop_done(result);
    assert!(
        calls.load(std::sync::atomic::Ordering::SeqCst) <= 1,
        "急停后不得再发新 LLM 调用"
    );
}

/// 已急停的 loop 直接跳过 transient 重试环：环顶闸，provider 零调用。
#[tokio::test]
async fn engaged_loop_skips_transient_retry_entirely() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = transient_test_loop(calls.clone());
    let estop = wire_estop(&agent_loop);
    estop.trigger();

    let instance = AgentInstance::new(estop_test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new("web", "et-chat", "et-sender", "agent:main:session:etskip");
    let cancel = tokio_util::sync::CancellationToken::new();

    let started = std::time::Instant::now();
    let result = agent_loop
        .retry_transient(
            &instance,
            &context,
            &provider,
            "test-model",
            &opts,
            &cancel,
            None,
            std::time::Duration::from_millis(500),
            "upstream deadline exceeded".to_string(),
        )
        .await;

    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_estop_done(result);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "已急停 loop 的重试环必须零调用"
    );
}

/// 急停必须打断进行中的限流等待 sleep（~0.1s 退出）：Err(Done(estop))，
/// 实时快照已清，不再发新调用。断言窗 <2s：摘 sleep 臂的回退态 = 白等
/// 整个等待档（retry_after=3 与梯子首档 5 取 max = 5s）后才被环顶闸兜住
/// （≈5.1s）→ 必红。
#[tokio::test]
async fn estop_cancels_rate_limit_retry_wait_sleep() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = rate_limit_test_loop(calls.clone());
    let estop = wire_estop(&agent_loop);
    let instance = AgentInstance::new(estop_test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new("web", "et-chat", "et-sender", "agent:main:session:etrls");

    let cancel = tokio_util::sync::CancellationToken::new();
    let started = std::time::Instant::now();
    let task = {
        let agent_loop = agent_loop.clone();
        tokio::spawn(async move {
            agent_loop
                .retry_rate_limited(
                    &context,
                    &instance,
                    &provider,
                    "test-model",
                    &opts,
                    &cancel,
                    None,
                    "status 429 rate limited (retry_after=3)".to_string(),
                )
                .await
        })
    };
    // 落进限流等待 sleep（等待 = max(retry_after=3, 梯子首档 5) = 5s）后
    // 触发急停。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    estop.trigger();

    let result = tokio::time::timeout(std::time::Duration::from_secs(8), task)
        .await
        .expect("急停后限流环必须立即退出（回退态白等 5s）")
        .expect("task 不应 panic");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "急停必须打断限流等待 sleep 本体（闸兜底形态 ≈5.1s）: {:?}",
        started.elapsed()
    );
    assert_estop_done(result);
    assert!(
        calls.load(std::sync::atomic::Ordering::SeqCst) <= 1,
        "急停后不得再发新 LLM 调用"
    );
    assert!(
        agent_loop
            .retry_status("agent:main:session:etrls")
            .is_none(),
        "急停退出必须清实时快照（环出口统一清快照不变量）"
    );
}

/// 已急停的 loop 直接跳过限流重试环：环顶闸，provider 零调用。
#[tokio::test]
async fn engaged_loop_skips_rate_limit_retry_entirely() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = rate_limit_test_loop(calls.clone());
    let estop = wire_estop(&agent_loop);
    estop.trigger();

    let instance = AgentInstance::new(estop_test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new("web", "et-chat", "et-sender", "agent:main:session:etrls2");
    let cancel = tokio_util::sync::CancellationToken::new();

    let started = std::time::Instant::now();
    let result = agent_loop
        .retry_rate_limited(
            &context,
            &instance,
            &provider,
            "test-model",
            &opts,
            &cancel,
            None,
            "status 429 rate limited (retry_after=3)".to_string(),
        )
        .await;

    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_estop_done(result);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "已急停 loop 的限流重试环必须零调用"
    );
}

/// 用户取消（cancel_token）同样打断退避 sleep——恢复环对取消令牌的响应
/// 与急停同型（首呼臂 cancel 先于 estop 的臂序约定）。
#[tokio::test]
async fn cancel_cancels_transient_retry_backoff_sleep() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = transient_test_loop(calls.clone());
    let _estop = wire_estop(&agent_loop);
    let instance = AgentInstance::new(estop_test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new("web", "et-chat", "et-sender", "agent:main:session:etcxl");

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_in = cancel.clone();
    let started = std::time::Instant::now();
    let task = {
        let agent_loop = agent_loop.clone();
        tokio::spawn(async move {
            agent_loop
                .retry_transient(
                    &instance,
                    &context,
                    &provider,
                    "test-model",
                    &opts,
                    &cancel_in,
                    None,
                    std::time::Duration::from_secs(2),
                    "upstream deadline exceeded".to_string(),
                )
                .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    cancel.cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("取消后重试环必须立即退出")
        .expect("task 不应 panic");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(900),
        "取消必须打断退避 sleep 本体（闸兜底形态 ≈1.1s）: {:?}",
        started.elapsed()
    );
    match result {
        Err(AgentEvent::Done(m)) => assert_eq!(m, recovery::CANCEL_DONE_MSG),
        other => panic!("期望 Err(Done(已取消))，得到 {other:?}"),
    }
    assert!(
        calls.load(std::sync::atomic::Ordering::SeqCst) <= 1,
        "取消后不得再发新 LLM 调用"
    );
}

/// 急停必须打断在途的重试调用（不只是 sleep）：首败 → 重呼挂死 → estop
/// select 臂 drop chat future 立即返回。
#[tokio::test]
async fn estop_interrupts_in_flight_transient_retry_call() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let entered = std::sync::Arc::new(AtomicBool::new(false));
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(FailThenHang {
            calls: calls.clone(),
            entered: entered.clone(),
        }),
        estop_test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    let estop = wire_estop(&agent_loop);
    let instance = AgentInstance::new(estop_test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();
    let context = RequestContext::new("web", "et-chat", "et-sender", "agent:main:session:etinfl");
    let cancel = tokio_util::sync::CancellationToken::new();

    let task = {
        let agent_loop = agent_loop.clone();
        tokio::spawn(async move {
            agent_loop
                .retry_transient(
                    &instance,
                    &context,
                    &provider,
                    "test-model",
                    &opts,
                    &cancel,
                    None,
                    std::time::Duration::from_millis(500), // <1s → 不退避，直达重呼
                    "upstream deadline exceeded".to_string(),
                )
                .await
        })
    };
    // 等重呼挂死（说明调用 select 已在等），再触发急停——保证命中调用臂
    // 而不是环顶闸。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !entered.load(Ordering::SeqCst) {
        if std::time::Instant::now() > deadline {
            panic!("重呼从未被 poll——没走到环内调用");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    estop.trigger();

    let result = tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .expect("急停必须中断挂死的重呼（而非干等到超时）")
        .expect("task 不应 panic");
    assert_estop_done(result);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "首败 1 次 + 挂死重呼 1 次，急停后不再有第三次"
    );
}
