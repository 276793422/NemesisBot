//! （2026-10-25 加固；2026-11-09 压缩提示徽标化改造）压缩注记域测试 +
//! 压缩点解析窗口来源各臂。
//!
//! - 注记限频（纯函数 + emit 集成）、soft 预警/完成注记文本要素、
//!   jsonl `note_kind="compaction"` 键、hook 标签二值；
//! - 窗口来源各臂：`resolve_compact_trigger` 的 window/source/
//!   window_source/clamped_from 各形态（config/catalog/user/fallback）。
//!   2026-11-05：三级链全 miss 时按 fallback 窗口（128k）照常
//!   自动压缩——旧 「窗口未知整条停用」形态废除，fallback 照跑回归锁
//!   在本文件（fallback_window_still_auto_summarizes）。
//!
//! 独立测试文件（生产文件只保留声明行，仓库纪律）。`test_config`/`turn`
//! 自建——`loop/tests.rs` 同名辅助模块私有，跨文件不可复用（ws2 同款）。
//! chat_log 注记断言走进程单例 home 的唯一键（本目录测试惯例），测毕删除。

use super::*;
use async_trait::async_trait;
use serde_json::Value;

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

fn unique_key(tag: &str) -> String {
    format!(
        "test:w1w2:{tag}:{}:{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// 已知窗口配置（窗口来源 config 臂；阈值 = 100×75% = 75）。
fn known_window_config(tag: &str, keep: i64) -> std::path::PathBuf {
    write_temp_config(
        tag,
        &serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": keep}},
            "model_list": [{"model_name": "test-model", "context_window": 100}]
        }),
    )
}

/// 只配 keep 不配窗口——三级链双 miss 形态（fallback 窗口生效）。
fn keep_only_config(tag: &str, keep: i64) -> std::path::PathBuf {
    write_temp_config(
        tag,
        &serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": keep}}
        }),
    )
}

fn write_temp_config(tag: &str, body: &serde_json::Value) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nemesis_test_w1w2_{tag}_{}.json",
        std::process::id()
    ));
    std::fs::write(&path, body.to_string()).unwrap();
    path
}

/// 捕获型 provider：固定回复（摘要文本）。
struct OnceProvider {
    reply: String,
}

#[async_trait]
impl LlmProvider for OnceProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: self.reply.clone(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn system_rows(key: &str) -> Vec<String> {
    let (rows, _, _, _) = crate::chat_log::read_chat_log(key, 200, None);
    rows.iter()
        .filter(|v| v.get("role").and_then(Value::as_str) == Some("system"))
        .filter_map(|v| {
            v.get("content")
                .and_then(Value::as_str)
                .map(std::string::ToString::to_string)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 纯函数
// ---------------------------------------------------------------------------

#[test]
fn note_allowed_first_and_cross_tag() {
    // 无记账 → 放行。
    assert!(compaction_note_allowed(None, "summarize", 1_000));
    // 不同档位随时可发（soft 与 summarize 是两个事实，互不挤占）。
    assert!(compaction_note_allowed(
        Some(("summarize", 1_000)),
        "soft",
        1_001
    ));
}

#[test]
fn note_allowed_same_tag_rate_limited() {
    // 同档位：窗口内（< 10min）静默，窗口到点放行（saturating 防时钟回拨）。
    assert!(!compaction_note_allowed(
        Some(("summarize", 1_000)),
        "summarize",
        1_000 + COMPACT_NOTE_RATELIMIT_MS - 1
    ));
    assert!(compaction_note_allowed(
        Some(("summarize", 1_000)),
        "summarize",
        1_000 + COMPACT_NOTE_RATELIMIT_MS
    ));
    // 时钟回拨（now < ts）：saturating_sub 归零 → 仍限频（不刷屏方向）。
    assert!(!compaction_note_allowed(
        Some(("summarize", 5_000)),
        "summarize",
        1_000
    ));
}

#[test]
fn note_text_contains_elements() {
    let t = compaction_soft_note_text(45_000, 96_000, "catalog");
    assert!(t.starts_with("[压缩预警] "), "档位名打头: {t}");
    assert!(
        t.contains("45000") && t.contains("96000"),
        "信号/阈值在文: {t}"
    );
    assert!(t.contains("窗口来源 catalog"), "窗口来源在文: {t}");
    assert!(t.ends_with("。"), "注记族句号收尾: {t}");
    let outcome = compaction_outcome_text(6, 4, std::time::Duration::from_secs(95));
    assert!(
        outcome.starts_with("[自动压缩] "),
        "完成注记档位名打头: {outcome}"
    );
    assert!(
        outcome.contains("覆盖 6 条") && outcome.contains("4 条原样保留"),
        "完成注记带覆盖/保留计数: {outcome}"
    );
    // 耗时片段（2026-11-09 用户需求）：完成记录永久可查压缩耗时。
    assert!(outcome.contains("1分35秒"), "分钟级耗时格式化: {outcome}");
    let secs = compaction_outcome_text(1, 1, std::time::Duration::from_secs(7));
    assert!(secs.contains("7 秒"), "秒级耗时格式化: {secs}");
}

#[test]
fn auto_note_hook_kind() {
    // hook 标签二值不变（方言消费方零影响）。
    assert_eq!(CompactReason::Manual.hook_kind(), "manual");
    assert_eq!(CompactReason::AutoSummarize.hook_kind(), "auto");
    assert_eq!(CompactReason::AutoSoft.hook_kind(), "auto");
}

#[test]
fn compact_state_defaults_for_new_note_fields() {
    // 新字段默认态（子模块可见私有字段）：限频无记账。
    let st = CompactState::default();
    assert_eq!(st.last_auto_note, None);
}

// ---------------------------------------------------------------------------
// 压缩点解析：窗口来源各臂
// ---------------------------------------------------------------------------

#[test]
fn trigger_fallback_window_compact_point() {
    // 三级链双 miss → fallback 窗口 + 窗口×75%（fallback 照常
    // 压缩，旧 auto_suppressed 停用形态废除）。agent 运行期形态：实例兜底
    // 标签。
    let trig = resolve_compact_trigger(None, "obscure/local-7b", None, (100, "instance"));
    assert_eq!(trig.window, 100);
    assert_eq!(trig.trigger, 75);
    assert_eq!(trig.source, "instance");
    assert_eq!(trig.window_source, "instance");
    // CLI 展示形态：编译期默认 128k。
    let trig = resolve_compact_trigger(
        None,
        "obscure/local-7b",
        None,
        (FALLBACK_CONTEXT_WINDOW, "fallback-128k"),
    );
    assert_eq!(trig.window, FALLBACK_CONTEXT_WINDOW);
    assert_eq!(trig.trigger, 96_000);
    assert_eq!(trig.source, "fallback-128k");
    assert_eq!(trig.window_source, "fallback-128k");
}

#[test]
fn trigger_user_point_and_config_window() {
    // 用户压缩点 = 显式意志（哪怕窗口仍是兜底值）。
    let cfg: serde_json::Value = serde_json::from_str(
        r#"{"model_list": [{"model_name": "local", "compact_context_tokens": 50}]}"#,
    )
    .unwrap();
    let trig = resolve_compact_trigger(Some(&cfg), "local", None, (100, "instance"));
    assert_eq!(trig.source, "user");
    assert_eq!(trig.trigger, 50);
    assert_eq!(trig.window_source, "instance", "窗口来源独立于触发点来源");
    // config 窗口命中 → 窗口来源 config。
    let cfg: serde_json::Value = serde_json::from_str(
        r#"{"model_list": [{"model_name": "opus", "context_window": 200000}]}"#,
    )
    .unwrap();
    let trig = resolve_compact_trigger(
        Some(&cfg),
        "opus",
        None,
        (FALLBACK_CONTEXT_WINDOW, "fallback-128k"),
    );
    assert_eq!(trig.source, "config");
    assert_eq!(trig.window_source, "config");
    assert_eq!(trig.trigger, 150_000);
}

#[test]
fn trigger_catalog_window() {
    // 价目表 catalog 命中 → 窗口已知（config 臂见
    // trigger_user_point_and_config_window；fallback 臂见
    // trigger_fallback_window_compact_point）。
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(nemesis_data::PricingStore::open(dir.path()).unwrap());
    store
        .replace_downloaded(
            vec![nemesis_data::ModelPricing {
                model_id: "local-7b".to_string(),
                display_name: "test".to_string(),
                input_cost_per_million: 1.0,
                output_cost_per_million: 2.0,
                cache_read_cost_per_million: 0.0,
                cache_creation_cost_per_million: 0.0,
                max_input_tokens: Some(96_000),
                max_output_tokens: None,
                aliases: Vec::new(),
                price_tiers: None,
            }],
            nemesis_data::PricingMeta::default(),
        )
        .unwrap();
    let trig = resolve_compact_trigger(
        None,
        "obscure/local-7b",
        Some(store.as_ref()),
        (FALLBACK_CONTEXT_WINDOW, "fallback-128k"),
    );
    assert_eq!(trig.window_source, "catalog");
    assert_eq!(trig.window, 96_000);
    assert_eq!(trig.trigger, 72_000);
}

// ---------------------------------------------------------------------------
// fallback 照跑回归锁
// ---------------------------------------------------------------------------

/// 三级链全 miss（只配 keep 不配窗口 + 显式小实例窗口）时自动压缩照常跑
/// ——不再有「窗口未知整条停用」形态（旧 废除后的行为锁）。
#[tokio::test]
async fn fallback_window_still_auto_summarizes() {
    let key = unique_key("fallback-compacts");
    let cfg_path = keep_only_config("fallback-compacts", 40);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(OnceProvider {
            reply: "SUMMARY".into(),
        }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // 阈值 75——fallback 窗口派生
    // 单消息 ≤ 摘要上限（window=100 → 50 tok → ≤125 字符）防空转闸截胡；
    // 总量 ≥ 75 保证信号越点。
    for _ in 0..6 {
        instance.add_user_message(&"x".repeat(100));
        instance.add_assistant_message(&"x".repeat(100), Vec::new(), None);
    }

    agent_loop
        .maybe_update_summary(&instance, &key, "web", "c")
        .await;

    let cache = instance
        .get_summary_cache()
        .expect("fallback 窗口下自动压缩照常跑");
    assert_eq!(cache.text, "SUMMARY");

    crate::chat_log::delete_chat_log(&key);
    let _ = std::fs::remove_file(&cfg_path);
}

/// 读取会话全部 system 行（保留完整 jsonl 条目——note_kind 键断言用）。
fn system_entries(key: &str) -> Vec<Value> {
    let (rows, _, _, _) = crate::chat_log::read_chat_log(key, 200, None);
    rows.iter()
        .filter(|v| v.get("role").and_then(Value::as_str) == Some("system"))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// 集成：summarize 触发落注记（soft 预警 + 完成注记，2026-11-09 徽标化）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auto_summarize_emits_soft_and_summarize_notes() {
    let key = unique_key("notes");
    let cfg_path = known_window_config("notes", 40);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(OnceProvider {
            reply: "SUMMARY".into(),
        }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    for _ in 0..4 {
        instance.add_user_message(&"x".repeat(125)); // 50 tok/条
        instance.add_assistant_message(&"x".repeat(125), Vec::new(), None);
    }

    agent_loop
        .maybe_update_summary(&instance, &key, "web", "c")
        .await;

    let cache = instance.get_summary_cache().expect("摘要应已采用");
    assert_eq!(cache.text, "SUMMARY");
    let rows = system_entries(&key);
    let soft = rows.iter().find(|v| {
        v.get("content")
            .and_then(Value::as_str)
            .map(|c| c.starts_with("[压缩预警] "))
            .unwrap_or(false)
    });
    assert!(soft.is_some(), "soft 档注记在文: {rows:?}");
    let outcome = rows.iter().find(|v| {
        v.get("content")
            .and_then(Value::as_str)
            .map(|c| c.starts_with("[自动压缩] "))
            .unwrap_or(false)
    });
    let outcome = outcome.expect("完成注记（结果导向）在文: {rows:?}");
    let content = outcome
        .get("content")
        .and_then(Value::as_str)
        .expect("content 字符串");
    assert!(
        content.contains("已折叠较早历史为摘要"),
        "完成注记结果导向: {content}"
    );
    // 徽标化核心契约（2026-11-09）：历史行带 note_kind="compaction"——前端
    // 据此渲染折叠小徽标（不再蓝底长文）。
    for row in [soft.unwrap(), outcome] {
        assert_eq!(
            row.get("note_kind").and_then(Value::as_str),
            Some("compaction"),
            "注记行必须带 note_kind 键: {row}"
        );
    }

    crate::chat_log::delete_chat_log(&key);
    let _ = std::fs::remove_file(&cfg_path);
}

/// ⑩ stuck 闩停用态聊天注记（可见性修复）回归锁：空摘要
/// （thinking 烧穿形态，provider 恒回空正文）连续 COMPACT_STUCK_LIMIT 次
/// 触发失效轴置位 → 聊天流落一条「[自动压缩已暂停]」system 注记（此前
/// 只落 warn 日志——用户侧「98% 永不压缩」零解释）；false→true 转变沿
/// 一次性，后续调用不重复。
#[tokio::test]
async fn stuck_pause_emits_one_shot_chat_note() {
    let key = unique_key("stuck-note");
    let cfg_path = known_window_config("stuck-note", 40);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(OnceProvider {
            reply: String::new(),
        }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100); // 阈值 75
    // 空转前缀闸：单消息须在摘要上限内（window=100 → 50 tok →
    // ≤125 字符；125+前缀会超限整段被判「无事可做」烧不到计数）。stuck
    // 场景走真失败形态 = 有效尺寸轮 + provider 恒回空正文。
    for i in 0..6 {
        instance.add_user_message(&format!("user {i} {}", "x".repeat(95)));
        instance.add_assistant_message(&format!("reply {i} {}", "x".repeat(95)), Vec::new(), None);
    }

    // 三轮： 记账 last；/ 失效轴连续计数到
    // COMPACT_STUCK_LIMIT(2) → stuck 置位 + 注记。
    for _ in 0..3 {
        agent_loop
            .maybe_update_summary(&instance, &key, "web", "c")
            .await;
    }

    assert!(instance.get_summary_cache().is_none(), "空摘要绝不折叠历史");
    let count_stuck = |rows: &[String]| -> usize {
        rows.iter()
            .filter(|r| r.contains("[自动压缩已暂停]"))
            .count()
    };
    let rows = system_rows(&key);
    assert_eq!(count_stuck(&rows), 1, "恰好一条 stuck 停用注记: {rows:?}");
    let hit = rows
        .iter()
        .find(|r| r.contains("[自动压缩已暂停]"))
        .expect("stuck 注记在文");
    assert!(hit.contains("连续 2 次"), "{}", hit);

    // 闩保持（paused_stuck 提前返回），转变沿一次性不重复注记。
    agent_loop
        .maybe_update_summary(&instance, &key, "web", "c")
        .await;
    let rows = system_rows(&key);
    assert_eq!(count_stuck(&rows), 1, "one-shot：不重复: {rows:?}");

    crate::chat_log::delete_chat_log(&key);
    let _ = std::fs::remove_file(&cfg_path);
}

#[tokio::test]
async fn emit_note_rate_limited_per_reason() {
    let key = unique_key("ratelimit");
    let agent_loop = AgentLoop::new(Box::new(OnceProvider { reply: "S".into() }), test_config());
    agent_loop
        .emit_compaction_note(
            &key,
            "web",
            "c",
            CompactReason::AutoSummarize,
            &compaction_outcome_text(1, 1, std::time::Duration::from_secs(1)),
        )
        .await;
    agent_loop
        .emit_compaction_note(
            &key,
            "web",
            "c",
            CompactReason::AutoSummarize,
            &compaction_outcome_text(2, 2, std::time::Duration::from_secs(2)),
        )
        .await;
    let rows = system_entries(&key);
    assert_eq!(rows.len(), 1, "同档位限频：只落一条: {rows:?}");
    agent_loop
        .emit_compaction_note(
            &key,
            "web",
            "c",
            CompactReason::AutoSoft,
            &compaction_soft_note_text(30, 50, "config"),
        )
        .await;
    let rows = system_entries(&key);
    assert_eq!(rows.len(), 2, "不同档位随时可发: {rows:?}");
    crate::chat_log::delete_chat_log(&key);
}

// ---------------------------------------------------------------------------
// session_context_status 窗口字段
// ---------------------------------------------------------------------------

#[test]
fn session_status_reports_window_fields() {
    // 已知窗口（config 命中）→ 来源 config。
    let cfg_path = known_window_config("status", 40);
    let (outbound_tx, _) = tokio::sync::mpsc::channel(16);
    let agent_loop = AgentLoop::new_bus(
        Box::new(OnceProvider { reply: "S".into() }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    );
    agent_loop.set_config_path(cfg_path.clone());
    let v = agent_loop.session_context_status("test:w1w2:status-cfg");
    assert_eq!(v["window_source"], "config");
    let _ = std::fs::remove_file(&cfg_path);

    // standalone（无 config）→ 实例兜底标签（fallback 窗口照跑形态）。
    let agent_loop = AgentLoop::new(Box::new(OnceProvider { reply: "S".into() }), test_config());
    let v = agent_loop.session_context_status("test:w1w2:status-standalone");
    assert_eq!(v["window_source"], "instance");
    // token 实报化观测分量在场：无 usage lane 回落纯估算。
    assert_eq!(v["provider_reported_tokens"], 0);
    assert!(v["estimated_tokens"].is_u64());
    assert_eq!(v["used_tokens"], v["estimated_tokens"]);
}

// ---------------------------------------------------------------------------
// 压缩进行中登记（2026-11-09 刷新恢复方案 A）
// ---------------------------------------------------------------------------

/// 登记生命周期 + context_status 投影：未登记三字段全否 → register →
/// running=true（label/started_at 在文）→ guard drop → 全否（无僵尸态）。
#[test]
fn compaction_in_flight_registry_and_status_projection() {
    let agent_loop = AgentLoop::new(Box::new(OnceProvider { reply: "S".into() }), test_config());
    let key = "test:w1w2:inflight";
    // 未登记：三字段全否。
    let v = agent_loop.session_context_status(key);
    assert_eq!(v["compaction_running"], false);
    assert!(v["compaction_started_at"].is_null());
    assert!(v["compaction_label"].is_null());
    // 登记：投影 running + label + started_at（RFC3339 墙钟非空）。
    let guard = agent_loop.register_compaction_in_flight(key, COMPACT_IN_PROGRESS_NOTE_MANUAL);
    let v = agent_loop.session_context_status(key);
    assert_eq!(v["compaction_running"], true);
    assert_eq!(
        v["compaction_label"].as_str().unwrap(),
        COMPACT_IN_PROGRESS_NOTE_MANUAL
    );
    let started = v["compaction_started_at"].as_str().unwrap();
    assert!(!started.is_empty(), "started_at 非空: {started}");
    // Drop 摘除：全否——成功/失败/panic 各出口由 Drop 覆盖的语义底座。
    drop(guard);
    let v = agent_loop.session_context_status(key);
    assert_eq!(v["compaction_running"], false);
    assert!(v["compaction_started_at"].is_null());
    assert!(v["compaction_label"].is_null());
}

/// 摘要 LLM 上带 gate 的 provider：放行前挂起——测试在 `maybe_update_summary`
/// spawn 执行期间轮询 context_status，锁「摘要执行中登记在册、结束后摘除」
/// 的接线点（刷新恢复的后端语义核心）。摘除点在摘要执行结束、采用/回执
/// 之前（race-free：✓ 到场时后端已非 running，30s 轮询不复活徽标）。
struct GateProvider {
    gate: tokio::sync::watch::Receiver<bool>,
}

#[async_trait]
impl LlmProvider for GateProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let mut rx = self.gate.clone();
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                break;
            }
        }
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "GATED SUMMARY".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[tokio::test]
async fn auto_summarize_in_flight_visible_during_and_cleared_after() {
    let key = unique_key("inflight-auto");
    let cfg_path = known_window_config("inflight-auto", 40);
    let (gate_tx, gate_rx) = tokio::sync::watch::channel(false);
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    let agent_loop = std::sync::Arc::new(AgentLoop::new_bus(
        Box::new(GateProvider { gate: gate_rx }),
        test_config(),
        outbound_tx,
        ConcurrentMode::Reject,
        8,
        0,
    ));
    agent_loop.set_config_path(cfg_path.clone());
    let mut instance = AgentInstance::new(test_config());
    instance.set_context_window(100);
    for _ in 0..6 {
        instance.add_user_message(&"x".repeat(100));
        instance.add_assistant_message(&"x".repeat(100), Vec::new(), None);
    }

    // spawn：maybe_update_summary 卡在摘要 LLM 的 gate 上。
    let al2 = agent_loop.clone();
    let key2 = key.clone();
    let handle = tokio::spawn(async move {
        al2.maybe_update_summary(&instance, &key2, "web", "c").await;
    });

    // 摘要执行期间：登记在册（running=true）。
    let mut saw_running = false;
    for _ in 0..150 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if agent_loop.session_context_status(&key)["compaction_running"] == true {
            saw_running = true;
            break;
        }
    }
    assert!(saw_running, "摘要执行期间 context_status 投影 running=true");

    // 放行 → 执行结束（登记先摘）→ 采用/回执快段 → 完成：running=false。
    let _ = gate_tx.send(true);
    handle.await.unwrap();
    let v = agent_loop.session_context_status(&key);
    assert_eq!(
        v["compaction_running"], false,
        "执行结束登记摘除（race-free：✓ 到场时已非 running）"
    );

    crate::chat_log::delete_chat_log(&key);
    let _ = std::fs::remove_file(&cfg_path);
}

// ---------------------------------------------------------------------------
// BUG-20261113-170：summarizing 槽 RAII guard——Drop 摘除旗标，panic/
// 取消路径不再滞留（旧行为：函数尾显式 remove 被跳过 → 旗标滞留，
// 该会话自动压缩静默失效到重启）。
// ---------------------------------------------------------------------------

#[test]
fn summarizing_slot_guard_removes_flag_on_drop() {
    let agent_loop = AgentLoop::new(Box::new(OnceProvider { reply: "S".into() }), test_config());
    let key = "main:guard-session";
    agent_loop.summarizing.lock().insert(key.to_string(), true);
    {
        let _slot = SummarizingSlotGuard {
            registry: agent_loop.summarizing.clone(),
            session_key: key.to_string(),
        };
        assert!(
            agent_loop.summarizing.lock().contains_key(key),
            "guard 存活期旗标在场"
        );
    }
    assert!(
        !agent_loop.summarizing.lock().contains_key(key),
        "guard drop 必须摘除旗标（panic 路径兜底语义）"
    );
}
