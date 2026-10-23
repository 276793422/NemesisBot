//! 停机感知回归锁（2026-10-24 tokio 停机 panic 根修）：`retry_transient`
//! 的退避 sleep 必须被 `stop()` 唤醒、循环顶必须看停机令牌——否则 gateway
//! 停机后重试环继续睡 1s/2s/4s 并在停机窗口里新注册 timer，撞 tokio
//! "A Tokio 1.x context was found, but it is being shutdown" panic（实录：
//! gateway boot 测试停机后 30ms 仍见 "LLM transient error, retrying"）。
//!
//! 为什么 abort 覆盖不到：`process_direct` 直调路径（看板 moderator /
//! guardian judge / headless run / ACP）的 LLM 调用跑在调用方任务里，
//! 不在 `turn_task_handles`，`stop()` 的 abort 扫不到——停机令牌是它们
//! 唯一的停机出口。
//!
//! 真实时钟（非 start_paused）：要验的是「stop 打断真实退避等待」，
//! paused 时间的自动推进与 select 交互相互掩盖。断言窗 <5s：未修复形态
//! = 1+2+4s 退避 + 三次立即失败 ≈ 7s，CI 慢机余量内可靠区分。

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

/// 恒失败 provider（transient 措辞；计数器断言停机后零新调用）。
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

fn stop_test_loop(
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::sync::Arc<AgentLoop> {
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(AlwaysTransient { calls }),
        test_config(),
        tx,
        ConcurrentMode::Reject,
        8,
        0,
    ))
}

/// stop() 必须打断进行中的退避 sleep：重试环立即退出（<5s），错误带停机
/// 标注，不再继续盲试（调用次数停在首轮失败，≤2 次）。
#[tokio::test]
async fn stop_cancels_transient_retry_backoff_sleep() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = stop_test_loop(calls.clone());
    let instance = AgentInstance::new(test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();

    let started = std::time::Instant::now();
    let task = {
        let agent_loop = agent_loop.clone();
        tokio::spawn(async move {
            agent_loop
                .retry_transient(
                    &instance,
                    &provider,
                    "test-model",
                    &opts,
                    std::time::Duration::from_secs(2), // ≥1s → 退避形态
                    "upstream deadline exceeded".to_string(),
                )
                .await
        })
    };
    // 落进第一段 1s 退避 sleep 后停机。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    agent_loop.stop();

    let (maybe_resp, last_err) = task
        .await
        .expect("retry task must not panic (停机窗口 timer 注册回归锁)");

    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "stop 后重试环必须立即退出（未修复形态 ≈7s）: {elapsed:?}"
    );
    assert!(maybe_resp.is_none(), "停机退出不得伪造成功响应");
    assert!(
        last_err.contains("stopped"),
        "终局错误必须带停机标注: {last_err}"
    );
    assert!(
        calls.load(std::sync::atomic::Ordering::SeqCst) <= 1,
        "停机后不得再发新 LLM 调用"
    );
}

/// 已停机的 loop 直接跳过重试环：循环顶闸，provider 零调用、立即返回。
#[tokio::test]
async fn stopped_loop_skips_transient_retry_entirely() {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let agent_loop = stop_test_loop(calls.clone());
    agent_loop.stop();

    let instance = AgentInstance::new(test_config());
    let provider = agent_loop.provider.read().clone();
    let opts = crate::types::ChatOptions::default();

    let started = std::time::Instant::now();
    let (maybe_resp, last_err) = agent_loop
        .retry_transient(
            &instance,
            &provider,
            "test-model",
            &opts,
            std::time::Duration::from_millis(500), // <1s 也一样：顶闸不看退避资格
            "upstream deadline exceeded".to_string(),
        )
        .await;

    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(maybe_resp.is_none());
    assert!(
        last_err.contains("stopped"),
        "终局错误必须带停机标注: {last_err}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "已停机 loop 的重试环必须零调用"
    );
}
