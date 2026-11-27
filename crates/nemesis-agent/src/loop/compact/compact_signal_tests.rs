//! + （2026-10-18）compact 域测试。
//!
//! -混合触发信号——provider 实报（完整输入 = prompt +
//!   cache_creation + cache_read，`usage_full_input` 统一定义）参与触发
//!   比较（取大者早触发，安全侧）；无 usage 的 lane 回落纯估算（不多触发）；
//! -无进展守卫——摘要采用前用同轴估算比较压缩前后（基线 = 旧摘要 +
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
        system_prompt_split: None,
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
    // 条目带 context_window=100——窗口已知才不禁自动压缩
    // （本文件测的是信号/预算机制，不是窗口解析；与 instance
    // set_context_window(100) 的阈值语义一致）。
    std::fs::write(
        &path,
        serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": keep}},
            "model_list": [{"model_name": "test-model", "context_window": 100}]
        })
        .to_string(),
    )
    .unwrap();
    path
}

/// 压缩点一等公民化：keep + per-model `compact_context_tokens`
/// 二合一配置（active_model = config.model = "test-model"，条目按别名命中）。
/// 条目同时带 context_window=100（窗口已知；compact>0 时停用闸本就不
/// 触发，带上是为钳制臂与旧语义逐位一致）。
fn compact_point_config(tag: &str, keep: usize, compact: i64) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nemesis_test_compact_point_{tag}_{}.json",
        std::process::id()
    ));
    std::fs::write(
        &path,
        serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": keep}},
            "model_list": [{"model_name": "test-model", "context_window": 100, "compact_context_tokens": compact}]
        })
        .to_string(),
    )
    .unwrap();
    path
}

/// 标准低估算历史：system(4 tok) + 7 × user/assistant(各 "x".repeat(40) =
/// 10 tok) → tail 估算 74 < threshold 75（context_window=100）——纯估算不触发。
/// （2026-11-09 实报锚定 B1：估算公式 chars×2/5 → len/4，载荷长度按新公式
/// 换算保持原 token 值，下游触发/边界算术逐位不变。）
fn low_estimate_history() -> AgentInstance {
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // threshold = 75 tokens
    instance.set_history(vec![
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
    ]);
    instance
}

// ---------------------------------------------------------------------------
// 混合触发信号
// ---------------------------------------------------------------------------

/// provider 实报 ≥ threshold 而字符估算 < threshold → 实报把触发拉起来
/// （取大者早触发，安全侧——估算偏小的风险形态）。
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

    // 实报 90（上一轮请求整请求输入，锚点 at_len=8=当前历史长度=锚后零
    // 增量）≥ 75 → 触发。
    agent_loop.note_compaction_provider_signal("s", 90, 8);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
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
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert!(
        instance.get_summary_cache().is_none(),
        "估算 74 < 75 不触发"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider 不被调");
    let _ = std::fs::remove_file(&cfg_path);
}

/// note 记账：写入 per-session 状态（同口径的值由调用方聚合，此处钉
/// 存取往返与 None 起点；B2 起 tokens 与 at_len 成对写入）。
#[test]
fn note_signal_roundtrip() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    let before = {
        let states = agent_loop.compact_state.lock();
        states
            .get("sess")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(before, None, "起点 None = 纯估算");

    agent_loop.note_compaction_provider_signal("sess", 1234, 7);
    let after = {
        let states = agent_loop.compact_state.lock();
        states
            .get("sess")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(after, Some((Some(1234), Some(7))));
}

/// 零实报不写锚（2026-11-09 V3 真模型实测 glm@cc-switch 中继：流式 lane
/// input 解析为 0 时若照写，锚点=0 → used 塌缩成纯增量估算——54 vs 真实
/// 2.3 万）。note(0) 保持 None，与「无 usage lane 不调」同语义。
#[test]
fn zero_provider_signal_does_not_write_anchor() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    agent_loop.note_compaction_provider_signal("s", 0, 8);
    let st = {
        let states = agent_loop.compact_state.lock();
        states
            .get("s")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(st, None, "零实报不算实报：不写锚，保持 None 纯估算回落");
}

/// 修复⑤（E2E 抓到）：auto 压缩成功采用后，provider 实报信号
/// 必须作废（置 None）。实报描述的是**压缩前**整请求（含已被折叠进摘要的
/// 前缀），对新状态失真——session_context_status 的
/// `used = max(估算, 实报)` 与混合触发都会拿 stale 值把 used 顶回压缩前
/// 水平（实测 covers=6/est=181 而徽标 pct=100）。None = 纯估算回落（
/// note_compaction_provider_signal 既有语义），下一条消息的真实请求实报
/// 重新注入。
#[tokio::test]
async fn auto_adoption_clears_stale_provider_signal() {
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
    let cfg_path = keep_tokens_config("signalclear", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history(); // 估算 74 < 75

    agent_loop.note_compaction_provider_signal("s", 90, 8); // 实报 90 ≥ 75 → 触发
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    let cache = instance
        .get_summary_cache()
        .expect("实报拉起触发，摘要应被采用");
    assert_eq!(cache.covers_up_to, 7);
    let stale = {
        let states = agent_loop.compact_state.lock();
        states
            .get("s")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(
        stale,
        Some((None, None)),
        "压缩成功采用后 provider 锚点必须成对作废（stale 实报会把 used 顶回压缩前水平）"
    );
    let _ = std::fs::remove_file(&cfg_path);
}

/// 修复⑤ force 臂：手动 /compact（force_compression）成功采用后同样作废
/// stale 实报信号（force 与 auto 的成功点必须同语义）。
#[tokio::test]
async fn force_adoption_clears_stale_provider_signal() {
    let (provider, _calls) = QueueProvider::boxed(vec![Ok("FORCE SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );

    let instance = AgentInstance::new(test_config());
    for i in 0..10 {
        instance.add_user_message(&format!("msg_{i}")); // system + 10 = 11
    }
    agent_loop.note_compaction_provider_signal("s", 99999, 11); // system + 10 条 = 11

    agent_loop.force_compression(&instance, "s").await;

    let cache = instance.get_summary_cache().expect("force 摘要应被采用");
    assert_eq!(cache.text, "FORCE SUMMARY");
    let stale = {
        let states = agent_loop.compact_state.lock();
        states
            .get("s")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(
        stale,
        Some((None, None)),
        "force 压缩成功采用后 provider 锚点必须成对作废"
    );
}

// ---------------------------------------------------------------------------
// B2 实报锚定：锚点 + 增量信号（compact_signal_components 单一真相源）
// ---------------------------------------------------------------------------

/// 锚点 + 增量精确断言：note(tokens=90, at_len=8) 锚点新鲜 → signal = 实报
/// 原值；追加 2 轮（各 10 tok）→ signal = 90 + 20 = 110——旧尾部已被实报
/// 按 provider 真值包含（不重复计数），切片恰好只含锚后新增轮次；max 单向
/// 偏早口径废除后，误差每轮被新实报清零（双向收敛）。
#[test]
fn anchored_signal_is_anchor_plus_increment() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    let instance = low_estimate_history(); // 8 turns，tail est 74
    agent_loop.note_compaction_provider_signal("s", 90, 8);

    {
        let states = agent_loop.compact_state.lock();
        let (signal, anchor, delta) =
            compact_signal_components(states.get("s"), &instance.get_history(), 74);
        assert_eq!(
            (signal, anchor, delta),
            (90, Some(90), 0),
            "锚点新鲜（at_len==len）= 实报原值，零增量"
        );
    }

    let mut hist = instance.get_history();
    hist.push(turn("user", &"x".repeat(40))); // +10 tok
    hist.push(turn("assistant", &"x".repeat(40))); // +10 tok
    instance.set_history(hist);

    let states = agent_loop.compact_state.lock();
    let (signal, anchor, delta) =
        compact_signal_components(states.get("s"), &instance.get_history(), 94);
    assert_eq!(
        (signal, anchor, delta),
        (110, Some(90), 20),
        "signal = 实报 90 + 锚后增量 20（旧尾部不重复计数）"
    );
}

/// 切片钳位：at_len > history.len()（trim 裁剪/外部缩短形态）不 panic，
/// 切片钳到 len → delta 0，signal = 实报原值（保守不低估）。
#[test]
fn anchored_signal_clamps_stale_at_len() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    let instance = low_estimate_history(); // 8 turns < at_len=100
    agent_loop.note_compaction_provider_signal("s", 500, 100);

    let states = agent_loop.compact_state.lock();
    let (signal, anchor, delta) =
        compact_signal_components(states.get("s"), &instance.get_history(), 74);
    assert_eq!(
        (signal, anchor, delta),
        (500, Some(500), 0),
        "at_len > len 钳位到 len：零增量、实报原值、不 panic"
    );
}

/// legacy 混合态 (Some(tokens), None)（热升级在途/老测试直写字段形态）：
/// 保守 max(tail, tokens) 不倒退——双向都要验。
#[test]
fn legacy_mixed_state_keeps_conservative_max() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    let instance = low_estimate_history(); // 8 turns，tail est 74
    {
        let mut states = agent_loop.compact_state.lock();
        let st = states.entry("s".to_string()).or_default();
        st.last_provider_prompt_tokens = Some(50);
        st.last_provider_prompt_at_len = None;
    }

    let states = agent_loop.compact_state.lock();
    // tail 74 > tokens 50 → signal = 74（tail 主导）。
    let (signal, anchor, delta) =
        compact_signal_components(states.get("s"), &instance.get_history(), 74);
    assert_eq!((signal, anchor, delta), (74, Some(50), 0));
    // tail 30 < tokens 50 → signal = 50（实报主导，保守不低估）。
    let (signal, anchor, delta) =
        compact_signal_components(states.get("s"), &instance.get_history(), 30);
    assert_eq!((signal, anchor, delta), (50, Some(50), 0));
}

/// 无锚回落：无 compact_state 条目的新键（fork 出的新会话键天然无锚，
/// 不继承父会话实报）→ 纯 tail 估算；无 usage lane（tokens=None）同路径。
#[test]
fn absent_anchor_falls_back_to_pure_estimate() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    let instance = low_estimate_history(); // 8 turns，tail est 74
    let states = agent_loop.compact_state.lock();

    // fork 新键：条目不存在。
    let (signal, anchor, delta) =
        compact_signal_components(states.get("fork-new"), &instance.get_history(), 74);
    assert_eq!((signal, anchor, delta), (74, None, 0));

    // tokens=None（无 usage lane）同回落。
    let mut states = states;
    let st = states.entry("no-usage".to_string()).or_default();
    st.last_provider_prompt_tokens = None;
    st.last_provider_prompt_at_len = Some(8);
    let (signal, anchor, delta) =
        compact_signal_components(states.get("no-usage"), &instance.get_history(), 74);
    assert_eq!((signal, anchor, delta), (74, None, 0));
}

/// 作废单点：invalidate_provider_anchor 双字段同置 None——auto/force 采纳
/// 臂已有专属测试（上方），此测试钉 helper 本体（clear_session /
/// rewind_from_cut 两个维护时点共用同一点）。
#[test]
fn invalidate_provider_anchor_clears_both_fields() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());
    agent_loop.note_compaction_provider_signal("s", 90, 8);
    let before = {
        let states = agent_loop.compact_state.lock();
        states
            .get("s")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(before, Some((Some(90), Some(8))));

    agent_loop.invalidate_provider_anchor("s");
    let after = {
        let states = agent_loop.compact_state.lock();
        states
            .get("s")
            .map(|s| (s.last_provider_prompt_tokens, s.last_provider_prompt_at_len))
    };
    assert_eq!(after, Some((None, None)), "作废必须成对清空两字段");

    // 作废后信号回落纯估算（下轮实报重注入前的诚实态）。
    let instance = low_estimate_history();
    let states = agent_loop.compact_state.lock();
    let (signal, anchor, _) =
        compact_signal_components(states.get("s"), &instance.get_history(), 74);
    assert_eq!((signal, anchor), (74, None));
}

/// 观测面（session_context_status）：锚点分量 + 三态上报 + B4 构成分解。
/// standalone 无 store → 实例按 `AgentInstance::new` 重建（种一条 system
/// turn，len=1，26 字符 = 7 tok），used 仍走锚点实报（纯估算只会给 7——
/// 锚点把 provider 真值带进观测面）；at_len 与重建后历史长度不等 = stale；
/// at_len==1 == 重建后长度 = fresh；无锚键 = absent。
#[test]
fn context_status_exposes_anchor_components_and_state() {
    let agent_loop = AgentLoop::new(QueueProvider::boxed(vec![]).0, test_config());

    // stale：有锚，at_len=500 != 重建后 len=1（钳位 → 零增量，实报原值）。
    agent_loop.note_compaction_provider_signal("s", 109_209, 500);
    let v = agent_loop.session_context_status("s");
    assert_eq!(v["used_tokens"], 109_209, "used = 锚点实报（非空历史估算）");
    assert_eq!(v["anchor_tokens"], 109_209);
    assert_eq!(v["delta_tokens"], 0);
    assert_eq!(v["anchor_state"], "stale");
    assert_eq!(v["provider_reported_tokens"], 109_209, "兼容字段保值");

    // （B4.5 构成分解 7 段化，设计定稿 §7.2：七段合计 == 已用）锚点在 =
    // 比例投影：est 分量（system 7 / tools 2 / mcp 2〔两者都零注册 → 各自
    // 空组序列化 "[]" 同基线〕/ skill 0〔loader 未注入〕/ memory 0
    // 〔workspace 未设，画像段空〕/ summary 0 / history 0——重建实例整条
    // 历史就是种子 system turn）只定配比，绝对值按 used 实报切分；history
    // 段兜尾舍入残差（本场景 est 历史为 0，段值 = 整除零头 1 ≠ 0，合计恒等）。
    let bd = &v["breakdown"];
    let est_sum = 7 + 2 + 2; // est_system + est_tools + est_mcp（skill/memory/summary/history 均 0）
    let expect_sys = 109_209 * 7 / est_sum;
    let expect_tls = 109_209 * 2 / est_sum;
    assert_eq!(bd["system"], expect_sys);
    assert_eq!(
        bd["tools"], expect_tls,
        "零注册工具 → builtin 组序列化 \"[]\" = 基线 1 + 结构字符补收 1 = 2 tok（按比例投影）"
    );
    assert_eq!(
        bd["mcp"], expect_tls,
        "mcp 组同为零注册 → 同 \"[]\" 基线 2 tok → 投影相等（配比相同的诚实产物）"
    );
    assert_eq!(bd["skill"], 0);
    assert_eq!(bd["memory"], 0);
    assert_eq!(bd["summary"], 0);
    assert_eq!(
        bd["history"].as_u64().unwrap(),
        109_209 - expect_sys - 2 * expect_tls,
        "history 段兜尾 = used − system − tools − mcp（合计恒 == used）"
    );
    let sum = [
        "system", "tools", "mcp", "skill", "memory", "summary", "history",
    ]
    .iter()
    .map(|k| bd[k].as_u64().unwrap())
    .sum::<u64>();
    assert_eq!(
        sum,
        v["used_tokens"].as_u64().unwrap(),
        "构成合计与 used 严格一致（锚点态）"
    );
    assert_eq!(
        bd["free"].as_u64().unwrap(),
        v["window"].as_u64().unwrap() - v["used_tokens"].as_u64().unwrap(),
        "free = window − used"
    );

    // fresh：at_len=1 == 重建后长度（system turn 在场）→ 零增量、实报原值。
    agent_loop.note_compaction_provider_signal("fresh", 42, 1);
    let v = agent_loop.session_context_status("fresh");
    assert_eq!(v["used_tokens"], 42);
    assert_eq!(v["anchor_tokens"], 42);
    assert_eq!(v["delta_tokens"], 0);
    assert_eq!(v["anchor_state"], "fresh");

    // absent：无锚键 → 纯估算（重建实例 = system turn 7 tok）。
    let v = agent_loop.session_context_status("other");
    assert_eq!(v["anchor_state"], "absent");
    assert_eq!(v["used_tokens"], 7);
    assert_eq!(v["anchor_tokens"], serde_json::Value::Null);
    // absent 构成：history 段 = 估算 − system 段 = 0（整条历史就是种子
    // system turn）。
    let bd = &v["breakdown"];
    assert_eq!(bd["system"], 7);
    assert_eq!(bd["history"], 0, "absent：used 全部落在 system 段");
    // absent 展示口径（2026-11-09）：pct/free 按「全段合计」（含工具 defs）
    // 推——标签与构成条一致、条加总恒等窗口；used_tokens 保持信号口径
    // （纯历史估算，上面已断言 == 7）。
    let abs_sum = 7 + 2 + 2; // est_system + est_tools + est_mcp（其余段 0）
    assert_eq!(
        bd["free"].as_u64().unwrap(),
        v["window"].as_u64().unwrap() - abs_sum,
        "absent：free = window − 全段合计（与构成条一致）"
    );
    assert_eq!(
        v["pct"].as_u64().unwrap(),
        (abs_sum * 100).div_ceil(v["window"].as_u64().unwrap()),
        "absent：pct = ceil(全段合计/window)——有占用不显示 0%"
    );
}

// ---------------------------------------------------------------------------
// 无进展守卫
// ---------------------------------------------------------------------------

/// 估算单独超阈值（无实报）→ 触发；摘要与尾巴同尺寸 → 守卫弃用 + no_progress
/// 计数；连续 2 次弃用 → stuck 暂停（provider 不再被调）。
#[tokio::test]
async fn b2_no_progress_discard_then_pause() {
    // 触发形态：context_window=100（threshold 75），system(4) + 8 × 10 tok
    // = 84 ≥ 75；keep=10 → covers 8，新尾巴 1 条。
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]);
    // 无进展摘要：300 字符（估算 75 tok）+ 尾巴 10 = projected 85 ≥ 基线
    // 84 → saved 0 < 4 → 弃用。
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
        .maybe_update_summary(&instance, "s", "web", "c", true)
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
        .maybe_update_summary(&instance, "s", "web", "c", true)
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
        .maybe_update_summary(&instance, "s", "web", "c", true)
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
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
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
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert!(instance.get_summary_cache().is_none(), "第 1 轮弃用");

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    let cache = instance
        .get_summary_cache()
        .expect("有效摘要（省幅 83% ≥ 5%）被采用");
    // user-first 钳制：raw=8（预算只装 a8）回退到最近 user（7），尾巴
    // 保留 [u7, a8] 最后一组问答。
    assert_eq!(cache.covers_up_to, 7);
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

/// 回归锁（复查根修）：provider 实报含 system+tools 常量开销时，
/// 基线不得跟着抬高——否则零进展摘要的 saved 被开销污染成「有效」，
/// 守卫被架空（开销 ≥ 5%×signal 时恒过闸）。基线必须用同轴估算
/// （旧摘要 + 尾巴），provider 信号只参与触发判定。
#[tokio::test]
async fn b2_baseline_is_estimate_axis_not_inflated_provider_signal() {
    // 触发形态同 b2_no_progress_discard_then_pause：threshold 75，tail 84；
    // 预注 provider 信号 2000 模拟大 system+tools 开销。
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
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
    // 锚点 at_len=9（system + 8 轮，锚后零增量）——实报 2000 纯作触发用，
    // 守卫只认同轴估算（本测试的断言对象）。
    agent_loop.note_compaction_provider_signal("s", 2000, 9);

    // 第 1 轮：projected = 75(摘要) + 10(尾巴) = 85；基线 est_before =
    // est("") + 84 = 84 → saved 0 < 4 → 必须弃用。旧实现 signal=2000 →
    // saved 1870 ≥ 100 → 误采纳（本测试在旧实现下红）。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
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
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    let cache = instance.get_summary_cache().expect("有效摘要不被误弃");
    // user-first 钳制：raw=8 回退到最近 user（7），尾巴保留 [u7, a8]。
    assert_eq!(cache.covers_up_to, 7);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// 压缩点一等公民化：用户压缩点触发 + 钳制一次性告知
// ---------------------------------------------------------------------------

/// 用户压缩点 < 默认 75% 点：到点即压（不乘比例）——估算 74 在默认
/// threshold 75 之下、用户点 50 之上，只有压缩点形态触发。
#[tokio::test]
async fn user_compact_point_triggers_below_default_ratio() {
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
    let cfg_path = compact_point_config("usertrig", 10, 50);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history(); // 估算 74；默认链 75 不触发

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    let cache = instance
        .get_summary_cache()
        .expect("用户压缩点 50 到点即压");
    assert_eq!(cache.covers_up_to, 7);
    let _ = std::fs::remove_file(&cfg_path);
}

/// 未到用户压缩点：不触发、provider 不被调（到点即压的「未到点」半边）。
#[tokio::test]
async fn user_compact_point_not_reached_no_trigger() {
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
    let cfg_path = compact_point_config("usermiss", 10, 200);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history(); // 估算 74 < 200

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert!(instance.get_summary_cache().is_none(), "74 < 200 未到点");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider 不被调");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 超窗口钳制 + 一次性告知：压缩点 500 > 实例窗口 100 → 钳到 100；
/// 进入状态块即置 clamped_noticed（不等软位/触发位），不重复。
#[tokio::test]
async fn clamp_notice_fires_once_even_below_soft() {
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
    let cfg_path = compact_point_config("clamp", 10, 500);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history(); // 窗口 100 → 钳 100；估算 74 < 100

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert!(
        instance.get_summary_cache().is_none(),
        "74 < 钳后点 100 不触发"
    );
    let noticed = agent_loop
        .compact_state
        .lock()
        .get("s")
        .map(|s| s.clamped_noticed);
    assert_eq!(noticed, Some(true), "首轮即一次性告知");

    // 状态仍在（一次性闸保持），无 panic、无重复触发副作用。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    let still = agent_loop
        .compact_state
        .lock()
        .get("s")
        .map(|s| s.clamped_noticed);
    assert_eq!(still, Some(true));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// 空转前缀闸（小窗口实测根修）：system turn 占据前缀时
// `new_c > c` 是假前进——空转曾按失败记账，两轮烧掉 stuck 闩暂停自动压缩。
// ---------------------------------------------------------------------------

/// 纯函数：前缀只剩 system（边界停在 1）→ false（无事可做）。
#[test]
fn gate_blocks_system_only_prefix() {
    let history = vec![
        turn("system", "sys prompt"),
        turn("user", "hi"),
        turn("assistant", "yo"),
    ];
    assert!(
        !summarize_advance_worthwhile(&history, 0, 1, 100),
        "new_c=1 前缀=[system]，无可摘要内容 → 不值得发起调用"
    );
}

/// 纯函数：新增段含有效 user/assistant 轮 → true（真实前进）。
#[test]
fn gate_passes_when_new_segment_has_valid_turn() {
    let history = vec![
        turn("system", "sys prompt"),
        turn("user", "hi"),
        turn("assistant", "yo"),
    ];
    assert!(summarize_advance_worthwhile(&history, 0, 2, 100));
    assert!(summarize_advance_worthwhile(&history, 0, 3, 100));
    // c≥1（system 已覆盖）时新增段含有效轮同样 true。
    assert!(summarize_advance_worthwhile(&history, 1, 3, 100));
}

/// 纯函数：新增段只有超限轮 → false（summarize 内部也会全跳过，闸门先行
/// 免去失败记账）。window=100 → 单消息上限 50 tok（208 字符，len/4 口径）。
#[test]
fn gate_blocks_oversized_only_segment() {
    let huge = "y".repeat(208); // 估算 52 tok > 50
    let history = vec![
        turn("system", "sys"),
        turn("user", &huge),
        turn("assistant", "yo"),
    ];
    assert!(!summarize_advance_worthwhile(&history, 0, 2, 100));
    // 超限与有效混合：存在任一有效轮即 true。
    let history2 = vec![
        turn("system", "sys"),
        turn("user", &huge),
        turn("user", "ok"),
        turn("assistant", "yo"),
    ];
    assert!(summarize_advance_worthwhile(&history2, 0, 4, 100));
}

/// 回归锁：边界倒退不 panic。`new_c > c` 是内置触发
/// 闸，但策略槽 `Compact`（WASM 组件强制压缩意志）绕过它——rewind 截断 /
/// config 热改 keep_recent_tokens 后 new_c < c 可达，切片曾直接 panic 炸
/// turn。修复 = end <= start 短路返回 false。
#[test]
fn gate_no_panic_on_boundary_regression() {
    let history = vec![
        turn("system", "sys prompt"),
        turn("user", "hi"),
        turn("assistant", "yo"),
    ];
    // rewind 形态：covers_up_to 钳到缩短后的 history.len()=3，策略槽强推
    // 的新边界 < c。旧代码此处 `history[3..2]` panic。
    assert!(!summarize_advance_worthwhile(&history, 3, 2, 100));
    // config 热改形态：keep 预算变大 → 新边界回退到 c 之前。
    assert!(!summarize_advance_worthwhile(&history, 2, 1, 100));
    // 零前进（end == start）：空切片同语义 false，显式短路。
    assert!(!summarize_advance_worthwhile(&history, 1, 1, 100));
}

/// 行为回归（死亡螺旋逆向）：巨大 system + 小尾巴预算 → 首次触发被闸门
/// 拦截（provider 零调用、失败计数零），历史长过尾巴预算后闸门自然放开
/// → 摘要正常发生（covers 推进）。旧代码在就烧掉 stuck 闩。
#[tokio::test]
async fn noop_prefix_gate_blocks_then_opens_as_history_grows() {
    let (provider, calls) = QueueProvider::boxed(vec![Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    // keep=30：尾巴装得下 system 之后的前两轮（各 10 tok），装不下
    // system（100 tok）→ 边界停在 1（前缀只有 system）。
    let cfg_path = keep_tokens_config("noop", 30);
    agent_loop.set_config_path(cfg_path.clone());

    // 巨大 system（估算 100 tok = 400 字符）+ 1 组问答。tail 120 ≥
    // threshold 75 → 触发。
    let huge_system = "S".repeat(400);
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", &huge_system),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "空转前缀：不发起摘要调用（旧代码此处瞬时失败一次）"
    );
    assert!(
        instance.get_summary_cache().is_none(),
        "无事可摘要，covers 不推进"
    );
    let failures = agent_loop
        .compact_state
        .lock()
        .get("s")
        .map(|s| s.consecutive_failures);
    assert_eq!(
        failures,
        Some(0),
        "空转不计失败（旧代码此处 +1，两轮烧掉 stuck 闩）"
    );

    // 历史长过尾巴预算（3 组问答）→ 闸门放开，摘要发生。user-first 钳制
    // 下可折叠区必须含至少一组完整问答：keep=30 边界到 4（保留 [a4,u5,a6]
    // 30 tok），钳制回退到最近 user（3）→ 折叠区 [system, u1, a2] 含问答。
    instance.set_history(vec![
        turn("system", &huge_system),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "边界前进到含问答的有效轮后闸门放开，摘要调用发生"
    );
    let cache = instance.get_summary_cache().expect("有效前进 → 摘要被采用");
    assert_eq!(cache.covers_up_to, 3);
    let failures_after = agent_loop
        .compact_state
        .lock()
        .get("s")
        .map(|s| s.consecutive_failures);
    assert_eq!(failures_after, Some(0), "全程零失败记账");
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// B17（2026-11-08 七轮调研落地批次③）：任务边界压缩——轮末触发点提前
// ---------------------------------------------------------------------------

/// B17 配置助手：keep + `agents.defaults.compact_boundary` 开关二合一
/// （`boundary=None` = 键不写 = 缺省 off；条目带 context_window=100 →
/// trigger=75、soft=50）。
fn boundary_config(tag: &str, keep: usize, boundary: Option<bool>) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nemesis_test_compact_boundary_{tag}_{}.json",
        std::process::id()
    ));
    let mut defaults = serde_json::json!({ "compact_keep_recent_tokens": keep });
    if let Some(b) = boundary {
        defaults["compact_boundary"] = serde_json::json!(b);
    }
    std::fs::write(
        &path,
        serde_json::json!({
            "agents": {"defaults": defaults},
            "model_list": [{"model_name": "test-model", "context_window": 100}]
        })
        .to_string(),
    )
    .unwrap();
    path
}

/// 软位夹具：system(3 tok) + 5 × user/assistant(各 10 tok) → tail 估算
/// 53 ∈ [soft=50, trigger=75)——边界压缩开启必触发、关闭（现状）必不触发的
/// 窗口区间。（token 口径 = B1 新公式 ASCII 快路 ceil(len/4)：
/// "sys prompt"=3、"x"×40=10。）
fn soft_band_history() -> AgentInstance {
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // trigger = 75, soft = 50
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
    ]);
    instance
}

/// 纯函数三臂：off → 触发点与压缩点重合（现状逐位一致）；on → soft 位；
/// soft 的既有定义（trigger ×2/3）不因本批漂移。
#[test]
fn b17_turn_end_effective_trigger_pure_arms() {
    assert_eq!(
        turn_end_effective_trigger(false, 75),
        75,
        "off = 行为与现状一致（触发点=压缩点）"
    );
    assert_eq!(
        turn_end_effective_trigger(true, 75),
        50,
        "on = soft 位（trigger ×2/3）"
    );
    assert_eq!(
        turn_end_effective_trigger(true, 90),
        90 * 2 / 3,
        "soft 定义沿用 trigger ×2/3"
    );
}

/// 开关 on + 信号达 soft 位 → 轮末触发（摘要被采用）。
#[tokio::test]
async fn b17_boundary_on_compacts_at_soft_band() {
    let (provider, calls) = QueueProvider::boxed(vec![Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = boundary_config("on", 10, Some(true));
    agent_loop.set_config_path(cfg_path.clone());
    let instance = soft_band_history(); // 估算 54 ∈ [50, 75)

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "54 ≥ soft 50 → 摘要调用发生（触发点提前生效）"
    );
    let cache = instance
        .get_summary_cache()
        .expect("软位触发，摘要应被采用");
    // keep=10 只装最后一条（10 tok），covers = len-1 = 5。
    assert_eq!(cache.covers_up_to, 5);
    assert_eq!(cache.text, "SUMMARY");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 开关 off（缺省）+ 同一信号 → 不触发、provider 不被调（行为与现状
/// 逐位一致：53 < trigger 75）。
#[tokio::test]
async fn b17_boundary_off_keeps_legacy_threshold() {
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
    let cfg_path = boundary_config("off", 10, None);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = soft_band_history();

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert!(
        instance.get_summary_cache().is_none(),
        "53 < 75 不触发（缺省 off 与现状一致）"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider 不被调");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 开关 on 但信号低于 soft 位 → 不触发（提前到 soft 不等于逢轮必压；
/// 33 < 50）。
#[tokio::test]
async fn b17_below_soft_no_trigger_even_boundary_on() {
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
    let cfg_path = boundary_config("below", 10, Some(true));
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
    ]); // 估算 33 < soft 50

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert!(instance.get_summary_cache().is_none(), "低于 soft 不触发");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider 不被调");
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// 轮顶压力闸（2026-11-27 turn 中途自动压缩）：at_turn_end=false 轴分离
// ---------------------------------------------------------------------------

/// B17 开启 + 轮顶调用点 + 信号落 [soft, trigger) 区间 → 不触发：
/// 中途压缩恒用标准触发位，不参与轮末 soft 提前（B17 用户不会在 50%
/// 窗口被频繁中间压缩）。同一历史 + at_turn_end=true 即触发
/// （b17_boundary_on_compacts_at_soft_band 已钉）。
#[tokio::test]
async fn round_top_ignores_b17_soft_band() {
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
    let cfg_path = boundary_config("roundtop_soft", 10, Some(true));
    agent_loop.set_config_path(cfg_path.clone());
    let instance = soft_band_history(); // 估算 54 ∈ [50, 75) soft 区间

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", false)
        .await;

    assert!(
        instance.get_summary_cache().is_none(),
        "54 < 标准触发位 75：轮顶不跟 B17 soft 位，不触发"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "provider 不被调");
    let _ = std::fs::remove_file(&cfg_path);
}

/// B17 开启 + 轮顶调用点 + 信号达标准触发位 → 照常触发：轴分离只收窄
/// 触发窗口（soft 之下不压），不吞标准触发位之上的正常中途压缩。
#[tokio::test]
async fn round_top_uses_standard_trigger_even_boundary_on() {
    let (provider, calls) = QueueProvider::boxed(vec![Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = boundary_config("roundtop_over", 10, Some(true));
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // trigger = 75
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]); // 估算 3 + 8×10 = 83 ≥ 75

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", false)
        .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "83 ≥ 标准触发位 75 → 中途压缩照常发生"
    );
    let cache = instance.get_summary_cache().expect("标准触发位之上应压缩");
    // keep=10 只装最后一条（10 tok），raw 边界 = len-1 = 8；但中途轴 user
    // 边界回退到不越过最后一条 user（idx 7）——当前交换（user+assistant 回
    // 复）必须留在逐字尾部，否则下一请求 assistant 开头（anthropic 400）。
    assert_eq!(cache.covers_up_to, 7);
    assert_eq!(cache.text, "SUMMARY");
    let _ = std::fs::remove_file(&cfg_path);
}

/// 中途轴 user 边界 · live 400 回归钉（2026-11-27 glm-5.3-flash 实测抓捕）：
/// turn-1 中途历史 [sys, user, assistant(tool_use), tool, tool]——折叠区在
/// user 边界钳制后只剩 system 前缀（无可摘要内容）→ 诚实 no-op（provider
/// 零调用、cache 不动），绝不把 user 折掉制造 assistant 开头请求
/// （anthropic 端点 400 [1214] messages 参数非法）。
#[tokio::test]
async fn mid_turn_user_boundary_skips_when_only_system_foldable() {
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
    let cfg_path = boundary_config("midturn_skip", 10, None);
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // trigger = 75
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(300)), // 当前任务消息（>阈值的大尾巴成员）
        turn("assistant", &"x".repeat(40)),
        turn("tool", &"x".repeat(40)),
        turn("tool", &"x".repeat(40)),
    ]); // 尾部远超 75：触发条件满足，但可折叠区为空

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", false)
        .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "折叠区只剩 system → 无可摘要内容，诚实 no-op 不打摘要器"
    );
    assert!(
        instance.get_summary_cache().is_none(),
        "covers 不推进：不制造 assistant 开头的下一请求"
    );
    let _ = std::fs::remove_file(&cfg_path);
}

/// 中途轴 user 边界 · 正常折叠形态：多轮交换后停在当前 user 消息之前——
/// 老交换折进摘要、当前交换（user+assistant）留在逐字尾部，投影后首条
/// 非 system 消息恒为 user。
#[tokio::test]
async fn mid_turn_user_boundary_folds_old_exchanges_keeps_current() {
    let (provider, calls) = QueueProvider::boxed(vec![Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = boundary_config("midturn_fold", 10, None);
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // trigger = 75
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(60)),      // u1
        turn("assistant", &"x".repeat(60)), // a1
        turn("user", &"x".repeat(60)),      // u2
        turn("assistant", &"x".repeat(60)), // a2
        turn("user", &"x".repeat(60)),      // u3（当前 turn 的任务消息）
        turn("assistant", &"x".repeat(60)), // a3（进行中的回复）
    ]); // 每条 60 字 ≈ 15 tok：估算 3 + 6×15 = 93 ≥ 75 → 触发

    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", false)
        .await;

    assert_eq!(calls.load(Ordering::SeqCst), 1, "超阈值 → 中途压缩发生");
    let cache = instance.get_summary_cache().expect("应折叠老交换");
    assert_eq!(
        cache.covers_up_to, 5,
        "折叠停在最后一条 user（idx 5）之前：u3/a3 留在尾部"
    );
    let projected = crate::r#loop::compact::project_history_for_request(
        &instance.get_history(),
        Some((&cache.text, cache.covers_up_to)),
    );
    assert_eq!(
        projected
            .iter()
            .find(|t| t.role != "system")
            .map(|t| t.role.as_str()),
        Some("user"),
        "投影后首条非 system 消息必须是 user（assistant 开头 = anthropic 400 形态）"
    );
    let _ = std::fs::remove_file(&cfg_path);
}

/// force 应急链 user-first 钳制：上下文错误环的折叠同样无条件回退到
/// ≤ new_c 的最近 user 轮（轴无关，2026-11-27 live 400 根修同款规则）。
/// raw_c=7（tail 8 > SMALL_K_FORCE，history[7]=tool 回退到 assistant 6）
/// → 最近 user ≤ 6 = idx 5 → covers=5，重试请求以 user 开头。
#[tokio::test]
async fn force_clamps_to_user_boundary() {
    let (provider, _calls) = QueueProvider::boxed(vec![Ok("FSUM".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![
        turn("system", "sys prompt"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("tool", &"x".repeat(40)),
        turn("tool", &"x".repeat(40)),
    ]); // len=9，tail 8 > SMALL_K_FORCE(2) → raw_c = 7；history[7]=tool → tool_safe 回退到 6

    let advanced = agent_loop
        .force_compression_opts(&instance, false, "s")
        .await;
    assert!(advanced, "有可推进内容 → 已发起摘要");
    let cache = instance.get_summary_cache().expect("force 应推进 covers");
    assert_eq!(
        cache.covers_up_to, 5,
        "折叠回退到最后一条 user（idx 5），u5 之后留在尾部——重试请求 user 开头"
    );
}

/// force 空尾豁免：当前尾部 ≤ SMALL_K_FORCE(2)（单交换极小历史）→
/// raw = history.len() ≥ len → 不钳——尾部为空 → 请求 = system+摘要
/// （+后续新 user），wire 合法。应急链 tail→0 最后一搏依赖此豁免：
/// 单交换历史若被钳（covers 卡在 last user 前的 system）则折叠区空、
/// 永无进展，上下文错误环死锁。
#[tokio::test]
async fn force_full_fold_exempt_from_user_clamp() {
    let (provider, _calls) = QueueProvider::boxed(vec![Ok("FSUM0".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    // len=2：current_tail_len=2 ≤ SMALL_K_FORCE → raw = 2 = len → 豁免臂。
    let instance = AgentInstance::new(test_config());
    instance.set_history(vec![turn("system", "sys prompt"), turn("user", "Hello")]);

    let advanced = agent_loop
        .force_compression_opts(&instance, false, "s")
        .await;
    assert!(
        advanced,
        "全折叠覆盖 [system, user]，含可摘要内容 → 发起摘要"
    );
    assert_eq!(
        instance
            .get_summary_cache()
            .expect("空尾豁免 → 全折叠被采用")
            .covers_up_to,
        2,
        "raw=history.len() → 空尾豁免 → covers 推到全部历史（尾部为空 = wire 合法）"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261119-175 敌意实报不 panic
// ---------------------------------------------------------------------------

/// 纯函数面：usize::MAX 级入参裸乘 90 曾 debug panic（BUG-20261119-175）；
/// 饱和后常规 90% 平台语义不变。
#[test]
fn summarize_was_ineffective_hostile_no_panic() {
    assert!(summarize_was_ineffective(usize::MAX, usize::MAX));
    assert!(!summarize_was_ineffective(usize::MAX, 0));
    assert!(summarize_was_ineffective(100, 90));
    assert!(!summarize_was_ineffective(100, 89));
}

/// 敌意端点 u64 大量级实报（`usage_full_input` 钳负饱和后的形状）——
/// soft 提示 signal*100、stuck 判定 last*90、观测面 used*100/比例投影/
/// 展示乘法全部走通不 panic。第二轮前重新锚定（首轮压缩采纳已作废锚点、
/// 信号回落纯估算），才能踩到 last_summary_tokens 已记账巨值的 stuck 比较。
#[tokio::test]
async fn hostile_provider_signal_no_panic() {
    let (provider, _calls) =
        QueueProvider::boxed(vec![Ok("SUMMARY".to_string()), Ok("SUMMARY".to_string())]);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        provider,
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    let cfg_path = keep_tokens_config("hostile175", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history();

    agent_loop.note_compaction_provider_signal("s", u64::MAX / 2, 8);
    // 锚点在 = 观测面 used 巨值——pct_trigger / 比例投影 / 展示乘法全走。
    let _status = agent_loop.session_context_status("s");
    // 第一轮：soft 提示行（signal*100）+ 触发压缩 + last_summary_tokens 记账巨值。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert!(instance.get_summary_cache().is_some(), "巨值实报应触发压缩");
    // 第二轮：summarize_was_ineffective(last*90) 位点（首轮已记账巨值）。
    agent_loop.note_compaction_provider_signal("s", u64::MAX / 2, 8);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// 2026-11-23 三 BUG 批 1：误记账根修 + force 清闩 + 死人开关
// ---------------------------------------------------------------------------

/// ① 误记账根修回归锁（16:05 事故形态）：连续三轮**全部成功采用**的压缩
/// 不得置 stuck 闩。旧代码：每轮 tick 在压缩运行前按上一轮基线记账——
/// 成功臂不清 failures/last_summary_tokens → 旧基线被带到下一轮 tick →
/// 「摘要成功」被误记账成 ineffective → 第三轮 tick failures=2 置闩 →
/// provider 只被调 2 次。修复后：成功臂清零失败轴 → 下一轮 ineffective?(0,·)
/// 恒 false → 三轮全跑（calls==3）。
#[tokio::test]
async fn success_adoption_clears_failure_axis_no_latch_on_successive_successes() {
    let (provider, calls) = QueueProvider::boxed(vec![
        Ok("SUMMARY".to_string()),
        Ok("SUMMARY".to_string()),
        Ok("SUMMARY".to_string()),
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
    let cfg_path = keep_tokens_config("misaccount", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let instance = low_estimate_history(); // 8 turns；window 100 → threshold 75

    // 轮 1：note(90, 8) → 触发 → tick（last=0 → ineffective false）→
    // last=90 → 摘要成功采用 covers=7 → 成功臂清 failures/last。
    agent_loop.note_compaction_provider_signal("s", 90, 8);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(instance.get_summary_cache().unwrap().covers_up_to, 7);

    // 轮 2：历史增长 2 轮（边界可前进）→ 重新锚定 → 触发 → tick 不再把
    // 上轮成功误记账（修复点）→ 摘要再成功。user-first 钳制：raw=9（assistant）
    // 回退到最近 user（8），尾巴保留 [u8, a9]。
    let mut hist = instance.get_history();
    hist.push(turn("user", &"x".repeat(40)));
    hist.push(turn("assistant", &"x".repeat(40)));
    instance.set_history(hist);
    agent_loop.note_compaction_provider_signal("s", 90, 10);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(instance.get_summary_cache().unwrap().covers_up_to, 8);

    // 轮 3：同构再一轮——旧代码此处 tick failures=2 置闩、provider 不被调。
    let mut hist = instance.get_history();
    hist.push(turn("user", &"x".repeat(40)));
    hist.push(turn("assistant", &"x".repeat(40)));
    instance.set_history(hist);
    agent_loop.note_compaction_provider_signal("s", 90, 12);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "连续三轮成功压缩全部真实执行（旧代码第三轮被误记账闩死，calls==2）"
    );
    let (failures, stuck, last) = {
        let states = agent_loop.compact_state.lock();
        let st = states.get("s").unwrap();
        (st.consecutive_failures, st.stuck, st.last_summary_tokens)
    };
    assert_eq!(failures, 0);
    assert!(!stuck, "全成功序列不得置闩");
    assert_eq!(last, 0, "成功臂清零 last_summary_tokens（轴无关干净起点）");
    let _ = std::fs::remove_file(&cfg_path);
}

/// ③ force 清闩：两轮无进展置闩后，手动 /compact（force_compression）成功
/// → stuck/failures/no_progress/last_summary_tokens 全清 → 自动压缩恢复
/// （下一条 auto 触发真实跑，不再被闩拦截）。两条暂停注记承诺的
/// 「可手动 /compact 强制压缩」由此闭环。
#[tokio::test]
async fn force_success_clears_latch_and_auto_resumes() {
    let (provider, calls) = QueueProvider::boxed(vec![
        Ok("x".repeat(300)),       // 无进展摘要（弃用）
        Ok("x".repeat(300)),       // 无进展摘要（弃用）→ no_progress=2 → stuck
        Ok("SUMMARY".to_string()), // force 摘要（采用）
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
    let cfg_path = keep_tokens_config("forceclear", 10);
    agent_loop.set_config_path(cfg_path.clone());
    // 9 turns（system + 8），tail 估算 80 ≥ 75 触发；无实报纯估算。
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]);

    // 置闩（同 b2_no_progress_discard_then_pause 形态）。
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert!(
        agent_loop.compact_state.lock().get("s").unwrap().stuck,
        "前置：两轮无进展已置闩"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // force 成功 → 清闩四件套。
    agent_loop.force_compression(&instance, "s").await;
    assert_eq!(instance.get_summary_cache().unwrap().text, "SUMMARY");
    let (stuck, failures, np, last) = {
        let states = agent_loop.compact_state.lock();
        let st = states.get("s").unwrap();
        (
            st.stuck,
            st.consecutive_failures,
            st.no_progress,
            st.last_summary_tokens,
        )
    };
    assert!(!stuck, "force 成功清 stuck 闩");
    assert_eq!(
        (failures, np, last),
        (0, 0, 0),
        "force 成功清失败轴两件 + no_progress"
    );

    // 自动压缩恢复：追加一组问答（user-first 钳制下 covers 已钉在 7——
    // 旧历史无可推进，需新交换才能前进边界）→ 重新锚定触发 → tick 正常
    // 记账（不被闩拦）→ 摘要真实发起（队列耗尽返回 Err 也算「发起了」
    // ——断言对象是闸放行，不是摘要结果）。
    {
        let mut hist = instance.get_history();
        hist.push(turn("user", &"x".repeat(40)));
        hist.push(turn("assistant", &"x".repeat(40)));
        instance.set_history(hist);
    }
    agent_loop.note_compaction_provider_signal("s", 90, 11);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        5,
        "闩清后 auto 路径恢复发起摘要调用（2 弃用 + 1 force + 2 auto——队列耗尽 Err 走 with_one_retry 同窗重试一次）"
    );
    let _ = std::fs::remove_file(&cfg_path);
}

/// ④ 死人开关放行：stuck 闩置位 + 信号 ≥ 窗口 95% → 放行一次救援摘要
/// （provider 被调、last_rescue 记账、摘要被采用）；随后同水位立刻再触发
/// → 冷却期内（<600s）不放行（provider 不被调）——闩的省 token 语义在
/// 冷却窗口内完整保留。
#[tokio::test]
async fn deadman_rescue_runs_at_high_water_then_cooldown_blocks() {
    let (provider, calls) = QueueProvider::boxed(vec![
        Ok("x".repeat(300)),       // 弃用
        Ok("x".repeat(300)),       // 弃用 → stuck
        Ok("SUMMARY".to_string()), // 救援摘要（采用）
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
    let cfg_path = keep_tokens_config("deadman", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert!(agent_loop.compact_state.lock().get("s").unwrap().stuck);

    // 高危水位（96 ≥ 95%×100）→ 救援放行：provider 被调、last_rescue 记账。
    agent_loop.note_compaction_provider_signal("s", 96, 9);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 3, "救援摘要真实发起");
    assert_eq!(instance.get_summary_cache().unwrap().text, "SUMMARY");
    assert!(
        agent_loop
            .compact_state
            .lock()
            .get("s")
            .unwrap()
            .last_rescue
            .is_some(),
        "救援记账落盘"
    );

    // 冷却期内同水位再触发 → 闩照旧暂停（provider 不再被调）。
    agent_loop.note_compaction_provider_signal("s", 96, 9);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "冷却期内（<600s）救援不放行——闩的省 token 语义保留"
    );
    assert!(
        agent_loop.compact_state.lock().get("s").unwrap().stuck,
        "救援成功但信号未回落到阈值下（仍锚定 96），闩保持待呼吸臂清"
    );
    let _ = std::fs::remove_file(&cfg_path);
}

/// ④ 死人开关双闸：低水位（<95% 窗口）不放行不记账；直接模拟冷却到期
/// （last_rescue 拨回 601s 前）→ 同水位放行再试（救援失败=队列耗尽也无妨，
/// 断言对象是闸放行本身——闩不死会话）。
#[tokio::test]
async fn deadman_rescue_low_water_gated_and_cooldown_expiry_retries() {
    let (provider, calls) = QueueProvider::boxed(vec![
        Ok("x".repeat(300)), // 弃用
        Ok("x".repeat(300)), // 弃用 → stuck
                             // 后续救援尝试走队列耗尽（Err）——救援失败形态。
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
    let cfg_path = keep_tokens_config("deadman2", 10);
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    instance.set_history(vec![
        turn("system", "system prompt!"),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
        turn("user", &"x".repeat(40)),
        turn("assistant", &"x".repeat(40)),
    ]);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert!(agent_loop.compact_state.lock().get("s").unwrap().stuck);

    // 低水位（90 < 95%×100）→ 不放行、不记账。
    agent_loop.note_compaction_provider_signal("s", 90, 9);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "低水位闩照旧暂停");
    assert!(
        agent_loop
            .compact_state
            .lock()
            .get("s")
            .unwrap()
            .last_rescue
            .is_none(),
        "低水位不记救援账"
    );

    // 模拟冷却到期 → 同低水位照旧不放行（双闸独立）。
    {
        let mut states = agent_loop.compact_state.lock();
        states.get_mut("s").unwrap().last_rescue =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(601));
    }
    agent_loop.note_compaction_provider_signal("s", 90, 9);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2, "冷却到期但低水位仍不放行");

    // 高水位 + 冷却到期 → 放行（队列耗尽=救援失败，Err 带 with_one_retry
    // 重试一次，闩保持，真实会话不死）。
    agent_loop.note_compaction_provider_signal("s", 96, 9);
    agent_loop
        .maybe_update_summary(&instance, "s", "web", "c", true)
        .await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "高水位+冷却到期 → 救援真实发起（1 尝试 + 1 Err 重试）"
    );
    let _ = std::fs::remove_file(&cfg_path);
}
