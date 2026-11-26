//! BUG-3（2026-11-25 远程 home 取证）transient 重试环「显式进度」回归。
//!
//! 现场：630s 首呼超时 → transient 环（3 次重试 × 退避 1/2/4s + 每次调用
//! 最长 600s）——用户在整段期间只见哑转圈，前端零反馈（info! 日志管线
//! 已死，无人看见）。修复 = transient 环补齐 rate-limit 环同款三板斧：
//! ① send_retry_progress 进度帧（文案对齐前端 parseRateLimitFrame 正则，
//!    复用徽标+倒计时渲染，前端零改动）；
//! ② set_rate_limit_status 实时快照（WSAPI agent.retry_status）；
//! ③ 全出口 clear_rate_limit_status（环顶三闸 / backoff select 三臂 /
//!    调用 select 两臂 / 环出口统一）。
//!
//! 纪律：立即拒绝形态（失败耗时 <1s，CI mock 链）**不发进度**——亚秒级
//! 重试闪徽标是噪声；只有真网络等待形态（backoff_eligible）才发。

use super::*;

fn test_config() -> AgentConfig {
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

/// 慢失败 transient provider：`fail_times` 次后成功。backoff 资格靠
/// **错误形态词**（os error 10051 网络不可达 → is_unreachable_error_shape
/// 命中）而非失败耗时——start_paused 虚拟时钟只推进 tokio::time，std
/// Instant::elapsed() 走真实时钟，瞬时 mock 失败耗时 <1s 判不了真等待。
struct SlowTransientThenSuccess {
    calls: std::sync::atomic::AtomicUsize,
    fail_times: usize,
    err_text: String,
}

#[async_trait]
impl LlmProvider for SlowTransientThenSuccess {
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

const TRANSIENT_ERR: &str = "connect error (model test-model): error sending request for url \
     (http://10.255.255.1/v1): os error 10051: 网络不可达。";

/// 立即拒绝形态：connect refused 不命中 unreachable 词表 + 瞬时失败耗时
/// <1s → backoff_eligible=false（零退避零进度帧的 CI mock 链形态）。
const IMMEDIATE_ERR: &str = "connect error (model test-model): error sending request for url \
     (http://127.0.0.1:1/v1): connection refused (os error 111)";

fn collect_progress(
    rx: &mut tokio::sync::mpsc::Receiver<nemesis_types::channel::OutboundMessage>,
) -> Vec<nemesis_types::channel::OutboundMessage> {
    let mut out = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        out.push(msg);
    }
    out
}

/// 真等待形态重试：进度帧发出 + 文案对齐前端正则 + note_kind=rate_limit
/// （复用徽标/倒计时渲染管线的关键）+ 快照终局清空（成功恢复路径）。
#[tokio::test(start_paused = true)]
async fn transient_retry_emits_progress_frames_and_clears_snapshot_on_success() {
    let provider = SlowTransientThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: 2,
        err_text: TRANSIENT_ERR.to_string(),
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
        "transient 重试后必须恢复: {events:?}"
    );

    // 两次失败 → 恰好两条进度帧；文案含退避梯子秒数（1s/2s），且能被
    // 前端 parseRateLimitFrame 的正则（「第 N/M 次重试，等待 S 秒」）命中。
    let progresses: Vec<_> = collect_progress(&mut rx)
        .into_iter()
        .filter(|m| m.content.contains("重试"))
        .collect();
    assert_eq!(
        progresses.len(),
        2,
        "进度帧: {:?}",
        progresses.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    assert!(
        progresses[0].content.contains("第 1/3 次"),
        "{}",
        progresses[0].content
    );
    assert!(
        progresses[0].content.contains("等待 1 秒"),
        "{}",
        progresses[0].content
    );
    assert!(
        progresses[0].content.contains("上游异常"),
        "transient 标签: {}",
        progresses[0].content
    );
    assert!(
        progresses[1].content.contains("第 2/3 次"),
        "{}",
        progresses[1].content
    );
    assert!(
        progresses[1].content.contains("等待 2 秒"),
        "{}",
        progresses[1].content
    );
    // 复用前端徽标管线：note_kind 必须是 rate_limit（前端 isRateLimitNote
    // 谓词入口），receipt:true 不落盘。
    for p in &progresses {
        assert_eq!(
            p.meta.note_kind.as_deref(),
            Some("rate_limit"),
            "进度帧必须带 rate_limit note_kind（前端徽标管线入口）: {:?}",
            p.meta
        );
        assert!(p.meta.receipt, "进度帧必须是 receipt 瞬态帧");
    }

    // ③环出口统一清快照（成功 break 路径也过这里）。
    assert!(
        agent_loop.retry_status(&context.session_key).is_none(),
        "环终局（成功）后快照必须清空"
    );
}

/// 恒失败耗尽形态：3 次重试全部发出进度帧（1s/2s/4s 梯子完整），
/// 终局后快照清空。
#[tokio::test(start_paused = true)]
async fn transient_retry_exhausts_with_full_progress_ladder_and_clears_snapshot() {
    let provider = SlowTransientThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: usize::MAX,
        err_text: TRANSIENT_ERR.to_string(),
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

    let errs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error(m) => Some(m.clone()),
            _ => None,
        })
        .collect();
    assert!(
        errs.iter().any(|m| m.contains("网络不可达")),
        "终局保留原始原因: {errs:?}"
    );

    let progresses: Vec<_> = collect_progress(&mut rx)
        .into_iter()
        .filter(|m| m.content.contains("重试"))
        .collect();
    assert_eq!(
        progresses.len(),
        3,
        "MAX_TRANSIENT_RETRIES=3 次全部可见: {:?}",
        progresses.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    let want = [
        ("第 1/3 次", "等待 1 秒"),
        ("第 2/3 次", "等待 2 秒"),
        ("第 3/3 次", "等待 4 秒"),
    ];
    for (p, (nth, wait)) in progresses.iter().zip(want) {
        assert!(
            p.content.contains(nth) && p.content.contains(wait),
            "退避梯子: {}",
            p.content
        );
    }
    assert!(
        agent_loop.retry_status(&context.session_key).is_none(),
        "环终局（耗尽）后快照必须清空"
    );
}

/// 立即拒绝形态（失败耗时 <1s，CI mock 链）：**不发进度帧**（亚秒级重试
/// 闪徽标是噪声），行为其余不变（照常重试恢复）。
#[tokio::test(start_paused = true)]
async fn immediate_rejection_shape_emits_no_progress_frames() {
    let provider = SlowTransientThenSuccess {
        calls: std::sync::atomic::AtomicUsize::new(0),
        fail_times: 2,
        err_text: IMMEDIATE_ERR.to_string(),
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
        "立即拒绝形态照常重试恢复: {events:?}"
    );
    let progress_count = collect_progress(&mut rx)
        .iter()
        .filter(|m| m.content.contains("重试"))
        .count();
    assert_eq!(
        progress_count, 0,
        "立即拒绝形态（<1s）不得发进度帧（闪徽标是噪声）"
    );
    assert!(
        agent_loop.retry_status(&context.session_key).is_none(),
        "未写快照（立即拒绝形态不进快照写入面）"
    );
}
