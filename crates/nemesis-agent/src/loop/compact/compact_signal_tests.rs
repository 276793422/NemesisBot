//! 自查② + B2（2026-10-18 B 级清零）compact 域测试。
//!
//! - 自查②：混合触发信号——provider 实报（prompt + cache_read）参与触发
//!   比较（取大者早触发，安全侧）；无 usage 的 lane 回落纯估算（不多触发）；
//! - B2：无进展守卫——摘要采用前用同轴估算比较压缩前后（基线 = 旧摘要 +
//!   尾巴，不含 provider 实报的 system+tools 开销——2026-10-19 根修），省幅
//!   不足 5% 弃用并计入 no_progress；连续 2 次暂停自动压缩（stuck 闩，
//!   provider 不再被调）；成功采用清零计数。
//!
//! 独立测试文件（生产文件只保留声明行，仓库纪律）；provider/turn 辅助自建
//! （与 ws2_compact_tests 同理——那份的辅助是模块私有）。

use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// 本地辅助
// ---------------------------------------------------------------------------

fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

fn turn(role: &str, content: &str) -> crate::types::ConversationTurn {
    crate::types::ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

/// 计数 + 队列回复的 mock provider：队列按序消费；耗尽 → Err fallback。
/// 计数器是 `Arc` 共享的（agent_loop 把 provider 装箱成 trait object 后
/// 具体类型被抹掉——断言「守卫暂停后 provider 不再被调」走共享计数器）。
struct QueueProvider {
    replies: std::sync::Mutex<std::collections::VecDeque<Result<String, String>>>,
    calls: Arc<AtomicUsize>,
}

impl QueueProvider {
    fn boxed(replies: Vec<Result<String, String>>) -> (Box<dyn LlmProvider>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Box::new(Self {
                replies: std::sync::Mutex::new(replies.into()),
                calls: Arc::clone(&calls),
            }),
            calls,
        )
    }
}

#[async_trait]
impl LlmProvider for QueueProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("queue exhausted".to_string()))?;
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: reply,
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 落盘 keep_recent_tokens 配置（尾巴按 token 预算定界），返回路径（调用方删）。
fn keep_tokens_config(tag: &str, keep: usize) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nemesis_test_compact_signal_{tag}_{}.json",
        std::process::id()
    ));
    std::fs::write(
        &path,
        serde_json::json!({"agents": {"defaults": {"compact_keep_recent_tokens": keep}}})
            .to_string(),
    )
    .unwrap();
    path
}

/// 标准低估算历史：system(4 tok) + 7 × user/assistant(各 "x".repeat(25) = 10
/// tok) → tail 估算 74 < threshold 75（context_window=100）——纯估算不触发。
fn low_estimate_history() -> AgentInstance {
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
    ]);
    instance
}

// ---------------------------------------------------------------------------
// 自查②：混合触发信号
// ---------------------------------------------------------------------------

/// provider 实报 ≥ threshold 而字符估算 < threshold → 实报把触发拉起来
/// （取大者早触发，安全侧——估算偏小的 nanobot 同款风险形态）。
#[tokio::test]
async fn provider_signal_pulls_trigger_above_estimate() {
    let (provider, _calls) = QueueProvider::boxed(vec![Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = keep_tokens_config("pull", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history(); // 估算 74 < 75

    // 实报 90（上一轮请求整请求输入）≥ 75 → 触发。
    agent_loop.note_compaction_provider_signal("s", 90);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;

    let cache = instance
        .get_summary_cache()
        .expect("实报拉起触发，摘要应被采用");
    // keep=10 只装最后一条（10 tok），covers = len-1 = 7。
    assert_eq!(cache.covers_up_to, 7);
    assert_eq!(cache.text, "SUMMARY");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 无 usage lane（信号 None）回落纯估算：估算 < threshold 不触发、provider
/// 不被调（诚实回落——不因缺 usage 而凭空多压缩）。
#[tokio::test]
async fn no_provider_signal_falls_back_to_estimate() {
    let (provider, calls) = QueueProvider::boxed(vec![]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = keep_tokens_config("fallback", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history();

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;

    assert!(
        instance.get_summary_cache().is_none(),
        "估算 74 < 75 不触发"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider 不被调");
    let _ = std::fs::remove_file(&cfg_path);
}

/// note 记账：写入 per-session 状态（P8 同口径的值由调用方聚合，此处钉
/// 存取往返与 None 起点）。
#[test]
fn note_signal_roundtrip() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    let before = agent_loop
        .compact_state
        .lock()
        .get("sess")
        .and_then(|s| s.last_provider_prompt_tokens);
    assert_eq!(before, None, "起点 None = 纯估算");

    agent_loop.note_compaction_provider_signal("sess", 1234);
    let after = agent_loop
        .compact_state
        .lock()
        .get("sess")
        .and_then(|s| s.last_provider_prompt_tokens);
    assert_eq!(after, Some(1234));
}

// ---------------------------------------------------------------------------
// B2：无进展守卫
// ---------------------------------------------------------------------------

/// 估算单独超阈值（无实报）→ 触发；摘要与尾巴同尺寸 → 守卫弃用 + no_progress
/// 计数；连续 2 次弃用 → stuck 暂停（第三轮 provider 不再被调）。
#[tokio::test]
async fn b2_no_progress_discard_then_pause() {
    // 触发形态：context_window=100（threshold 75），system(4) + 8 × 10 tok
    // = 84 ≥ 75；keep=10 → covers 8，新尾巴 1 条。
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
    ]);
    // 无进展摘要：120 tok（估算）≈ 1.4 × 压缩前信号 84 → saved 0 < 4。
    let (provider, calls) = QueueProvider::boxed(vec![Ok("x".repeat(300)), Ok("x".repeat(300))]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = keep_tokens_config("b2pause", 10);
    agent_loop.set_config_path(cfg_path.clone());

    // 第 1 轮：触发 → 摘要无进展 → 弃用（covers 不进、cache None）。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert!(instance.get_summary_cache().is_none(), "无进展摘要被弃用");
    let (np, stuck) = {
        let states = agent_loop.compact_state.lock();
        let st = states.get("s").unwrap();
        (st.no_progress, st.stuck)
    };
    assert_eq!(np, 1, "第 1 次无进展计数");
    assert!(!stuck);

    // 第 2 轮：同 tail 再触发（last_summary_tokens 已记账 → ineffective 轴
    // 同步 +1）→ 再弃用 → no_progress=2 → stuck 置位。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert!(instance.get_summary_cache().is_none());
    let (np, stuck) = {
        let states = agent_loop.compact_state.lock();
        let st = states.get("s").unwrap();
        (st.no_progress, st.stuck)
    };
    assert_eq!(np, 2, "连续第 2 次无进展");
    assert!(stuck, "达 COMPACT_NO_PROGRESS_LIMIT → 暂停自动压缩");

    // 第 3 轮：stuck 闩 → 提前 return，provider 不再被调。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "暂停后不再烧摘要 LLM 调用");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 正常压缩不受守卫影响：弃用 1 次后换小摘要 → 采用 + no_progress 清零；
/// 守卫阈值 5% 不会被真实有效压缩误伤（signal 84 → projected 14，saved 83%）。
#[tokio::test]
async fn b2_effective_summary_adopted_and_counter_resets() {
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
    ]);
    // 第 1 个回复无进展（弃用），第 2 个有效小摘要（采用）。
    let (provider, _calls) =
        QueueProvider::boxed(vec![Ok("x".repeat(300)), Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = keep_tokens_config("b2adopt", 10);
    agent_loop.set_config_path(cfg_path.clone());

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert!(instance.get_summary_cache().is_none(), "第 1 轮弃用");

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    let cache = instance
        .get_summary_cache()
        .expect("有效摘要（省幅 83% ≥ 5%）被采用");
    assert_eq!(cache.covers_up_to, 8);
    assert_eq!(cache.text, "SUMMARY");
    let np = agent_loop
        .compact_state
        .lock()
        .get("s")
        .unwrap()
        .no_progress;
    assert_eq!(np, 0, "成功采用清零 no_progress");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 回归锁（2026-10-19 复查根修）：provider 实报含 system+tools 常量开销时，
/// B2 基线不得跟着抬高——否则零进展摘要的 saved 被开销污染成「有效」，
/// 守卫被架空（开销 ≥ 5%×signal 时恒过闸）。基线必须用同轴估算
/// （旧摘要 + 尾巴），provider 信号只参与触发判定。
#[tokio::test]
async fn b2_baseline_is_estimate_axis_not_inflated_provider_signal() {
    // 触发形态同 b2_no_progress_discard_then_pause：threshold 75，tail 84；
    // 预注 provider 信号 2000 模拟大 system+tools 开销。
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
        turn("user", &"x".repeat(25)),
        turn("assistant", &"x".repeat(25)),
    ]);
    let (provider, calls) = QueueProvider::boxed(vec![
        Ok("x".repeat(300)),       // 无进展摘要：est 120
        Ok("SUMMARY".to_string()), // 有效摘要：est 2
    ]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = keep_tokens_config("b2axis", 10);
    agent_loop.set_config_path(cfg_path.clone());
    agent_loop.note_compaction_provider_signal("s", 2000);

    // 第 1 轮：projected = 120(摘要) + 10(尾巴) = 130；基线 est_before =
    // est("") + 84 = 84 → saved 0 < 5 → 必须弃用。旧实现 signal=2000 →
    // saved 1870 ≥ 100 → 误采纳（本测试在旧实现下红）。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    assert!(
        instance.get_summary_cache().is_none(),
        "开销抬高的 provider 信号不得让零进展摘要过闸"
    );
    let np = agent_loop
        .compact_state
        .lock()
        .get("s")
        .unwrap()
        .no_progress;
    assert_eq!(np, 1, "无进展计数照常 +1");

    // 第 2 轮：projected = 2 + 10 = 12 → saved 72 ≥ 4 → 采用（反向：provider
    // 信号在场也不得误弃有效摘要）。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c")
        .await;
    let cache = instance.get_summary_cache().expect("有效摘要不被误弃");
    assert_eq!(cache.covers_up_to, 8);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let _ = std::fs::remove_file(&cfg_path);
}
