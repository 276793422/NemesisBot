//! P32（能力扩展 Wave3）cache-warmer 域测试。
//!
//! - TTL 解析：只认模型条目显式 `cache_ttl_secs`（5min=300 / 1h=3600 两档
//!   都由此表达），未知 / 非法 / 缺失 → None（不 warm，诚实降级）；
//! - 90% 触发判定（`warm_due` 纯函数边界）；
//! - 经济闸（`warm_cost_allowed`：估算超限 / 价目表未命中 → 保守跳过）；
//! - 快照存取：上限淘汰 + `max_tokens=1` 字节级重放形态 + 忙会话跳过 +
//!   模型切换清理；
//! - 开关零副作用：默认关 → 配置解析恒 false、capture 关、扫描臂不发请求；
//!   停机态 warmer 主循环立即退出（无定时器残留）。
//!
//! 独立测试文件（生产文件只保留声明行，仓库 2026-08-25 纪律）；mock
//! provider 与 config 构造自建（`loop/tests.rs` 的同名辅助模块私有，跨
//! 文件不可复用，与 ws2_compact_tests 同先例）。

use super::*;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// 本地辅助：config 构造 + 记录型 mock provider
// ---------------------------------------------------------------------------

/// 与 `loop/tests.rs` 的 `test_config()` 同形态（那份模块私有，此处自建）。
fn test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec!["calculator".to_string()],
        models: std::collections::HashMap::new(),
    }
}

fn msg(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

/// 单条 LLM 调用记录（断言重放形态用）。
#[derive(Clone)]
struct RecordedCall {
    model: String,
    messages: Vec<LlmMessage>,
    max_tokens: Option<u32>,
    tool_count: usize,
}

/// 记录型 provider——warm 重放断言「发了什么」。记录句柄经 Arc 外置
/// （provider 被 Box 进 AgentLoop，测试侧留同一份句柄读取）。
struct RecordingProvider {
    calls: std::sync::Arc<std::sync::Mutex<Vec<RecordedCall>>>,
}

impl RecordingProvider {
    fn new(calls: std::sync::Arc<std::sync::Mutex<Vec<RecordedCall>>>) -> Self {
        Self { calls }
    }
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<crate::types::ChatOptions>,
        tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.calls.lock().unwrap().push(RecordedCall {
            model: model.to_string(),
            messages,
            max_tokens: options.as_ref().and_then(|o| o.max_tokens),
            tool_count: tools.len(),
        });
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 构造 loop + 外置调用记录句柄。
fn recording_pair() -> (
    Arc<AgentLoop>,
    std::sync::Arc<std::sync::Mutex<Vec<RecordedCall>>>,
) {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let loop_ = Arc::new(AgentLoop::new(
        Box::new(RecordingProvider::new(calls.clone())),
        test_config(),
    ));
    (loop_, calls)
}

fn call_count(calls: &std::sync::Mutex<Vec<RecordedCall>>) -> usize {
    calls.lock().unwrap().len()
}

/// 临时 config.json（带 model_list 条目 `cache_ttl_secs` + agents.cache_warmer）。
fn write_temp_config(enabled: bool, ttl: Option<u64>) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nb-cache-warmer-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let ttl_line = match ttl {
        Some(t) => t.to_string(),
        None => "null".to_string(),
    };
    let body = format!(
        r#"{{
  "agents": {{ "cache_warmer": {{ "enabled": {} }} }},
  "model_list": [
    {{ "model_name": "test-model", "model": "vendor/test-model", "cache_ttl_secs": {} }}
  ]
}}"#,
        enabled, ttl_line
    );
    let path = dir.join("config.json");
    std::fs::write(&path, body).unwrap();
    path
}

/// 带自定义价目条目的 PricingStore（test-model：$1/M in、$2/M out）。
fn pricing_store_with_test_model() -> Arc<nemesis_data::PricingStore> {
    let dir = std::env::temp_dir().join(format!(
        "nb-cache-warmer-pricing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = nemesis_data::PricingStore::open(&dir).unwrap();
    store
        .upsert_custom(nemesis_data::ModelPricing {
            model_id: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            input_cost_per_million: 1.0,
            output_cost_per_million: 2.0,
            cache_read_cost_per_million: 0.1,
            cache_creation_cost_per_million: 1.25,
            max_input_tokens: None,
            max_output_tokens: None,
            aliases: vec![],
        })
        .unwrap();
    Arc::new(store)
}

// ---------------------------------------------------------------------------
// 配置解析（默认关 = 零副作用的第一道闸）
// ---------------------------------------------------------------------------

/// 默认关：段缺失 / 无 config → enabled=false + $0.05（D-4 opt-in）。
#[test]
fn warmer_config_defaults_to_disabled() {
    let absent = parse_cache_warmer_config(None);
    assert!(!absent.enabled);
    assert!((absent.cost_limit_usd - 0.05).abs() < 1e-9);

    let empty = parse_cache_warmer_config(Some(&serde_json::json!({})));
    assert!(!empty.enabled);
    assert!((empty.cost_limit_usd - 0.05).abs() < 1e-9);
}

/// 宽松解析：cache_warmer 段形态非法（enabled 非布尔等）→ 整体回默认（关）。
#[test]
fn warmer_config_lenient_on_malformed() {
    let bad = parse_cache_warmer_config(Some(&serde_json::json!({
        "cache_warmer": { "enabled": "yes", "cost_limit_usd": "free" }
    })));
    assert!(!bad.enabled);
    assert!((bad.cost_limit_usd - 0.05).abs() < 1e-9);
}

/// 显式开启 + 自定义上限可解析。
#[test]
fn warmer_config_parses_enabled_and_limit() {
    let cfg = parse_cache_warmer_config(Some(&serde_json::json!({
        "cache_warmer": { "enabled": true, "cost_limit_usd": 0.5 }
    })));
    assert!(cfg.enabled);
    assert!((cfg.cost_limit_usd - 0.5).abs() < 1e-9);
}

/// standalone（无 config_path）→ capture 关 + spawn 闸关（零副作用的
/// 可观测面：不建任务、不写快照）。
#[tokio::test]
async fn warmer_disabled_by_default_has_zero_side_effects() {
    let (loop_, calls) = recording_pair();
    assert!(!loop_.cache_warmer_capture_enabled());
    assert!(!loop_.current_cache_warmer_config().enabled);
    // spawn 闸：关 = 不 spawn（这里只验证不 panic 且快照仍空；任务不存在的
    // 结构性保证在 spawn_cache_warmer_if_enabled 的早退分支）。
    loop_.spawn_cache_warmer_if_enabled();
    assert!(loop_.warm_candidates.lock().is_empty());
    assert_eq!(call_count(&calls), 0);
}

// ---------------------------------------------------------------------------
// TTL 解析（未知不 warm 的诚实降级）
// ---------------------------------------------------------------------------

/// 只认显式 `cache_ttl_secs`；anthropic 两档（300/3600）都可表达；
/// 缺失 / 0 / 负数 / 字符串 → None。
#[test]
fn ttl_resolution_explicit_only() {
    let entry = |ttl: serde_json::Value| {
        serde_json::json!({
            "model_list": [
                { "model_name": "test-model", "model": "vendor/test-model", "cache_ttl_secs": ttl }
            ]
        })
    };
    assert_eq!(
        resolve_cache_ttl_secs(Some(&entry(serde_json::json!(300))), "test-model"),
        Some(300)
    );
    assert_eq!(
        resolve_cache_ttl_secs(Some(&entry(serde_json::json!(3600))), "test-model"),
        Some(3600)
    );
    // 0 / 负数 / 字符串数字 → None（非法即未知）。
    assert_eq!(
        resolve_cache_ttl_secs(Some(&entry(serde_json::json!(0))), "test-model"),
        None
    );
    assert_eq!(
        resolve_cache_ttl_secs(Some(&entry(serde_json::json!(-300))), "test-model"),
        None
    );
    assert_eq!(
        resolve_cache_ttl_secs(Some(&entry(serde_json::json!("300"))), "test-model"),
        None
    );
    // 键缺失 → None。
    let no_key = serde_json::json!({
        "model_list": [{ "model_name": "test-model", "model": "vendor/test-model" }]
    });
    assert_eq!(resolve_cache_ttl_secs(Some(&no_key), "test-model"), None);
    // 无 model_list / 未知别名 / 无 config → None。
    assert_eq!(
        resolve_cache_ttl_secs(Some(&serde_json::json!({})), "test-model"),
        None
    );
    assert_eq!(
        resolve_cache_ttl_secs(Some(&entry(serde_json::json!(300))), "other-model"),
        None
    );
    assert_eq!(resolve_cache_ttl_secs(None, "test-model"), None);
}

/// 条目匹配双路：`model_name` 或 `model`（full 名）任一命中。
#[test]
fn ttl_resolution_matches_by_name_or_full_model() {
    let cfg = serde_json::json!({
        "model_list": [
            { "model_name": "alias-a", "model": "vendor/full-b", "cache_ttl_secs": 300 }
        ]
    });
    assert_eq!(resolve_cache_ttl_secs(Some(&cfg), "alias-a"), Some(300));
    assert_eq!(
        resolve_cache_ttl_secs(Some(&cfg), "vendor/full-b"),
        Some(300)
    );
    assert_eq!(resolve_cache_ttl_secs(Some(&cfg), "full-b"), None);
}

// ---------------------------------------------------------------------------
// 90% 触发判定 + 经济闸（纯函数边界）
// ---------------------------------------------------------------------------

/// idle ≥ TTL×90% 触发；边界值（恰好 270s / TTL=300）判到期；TTL=0 恒否。
#[test]
fn warm_due_fires_at_ninety_percent() {
    let anchor = std::time::Instant::now();
    // 300s TTL → 阈值 270s。
    assert!(!warm_due(
        anchor,
        300,
        anchor + std::time::Duration::from_secs(269)
    ));
    assert!(warm_due(
        anchor,
        300,
        anchor + std::time::Duration::from_secs(270)
    ));
    assert!(warm_due(
        anchor,
        300,
        anchor + std::time::Duration::from_secs(3600)
    ));
    // 3600s TTL → 阈值 3240s。
    assert!(!warm_due(
        anchor,
        3600,
        anchor + std::time::Duration::from_secs(3239)
    ));
    assert!(warm_due(
        anchor,
        3600,
        anchor + std::time::Duration::from_secs(3240)
    ));
    // TTL=0（未知形态的防御）恒不触发。
    assert!(!warm_due(
        anchor,
        0,
        anchor + std::time::Duration::from_secs(86_400)
    ));
}

/// 经济闸：估算超限 → 拒；价目表未命中（None）→ 保守拒；免费模型 0 上限
/// 也放行。
#[test]
fn cost_gate_conservative() {
    assert!(warm_cost_allowed(Some(0.04), 0.05));
    assert!(!warm_cost_allowed(Some(0.06), 0.05));
    assert!(!warm_cost_allowed(None, 0.05));
    assert!(warm_cost_allowed(Some(0.0), 0.0));
    assert!(!warm_cost_allowed(Some(0.001), 0.0));
}

// ---------------------------------------------------------------------------
// 快照存取 + 重放形态
// ---------------------------------------------------------------------------

/// 快照上限：第 9 份进 map 时淘汰 anchor 最旧的会话。
#[test]
fn candidate_store_evicts_oldest_beyond_cap() {
    let (loop_, _calls) = recording_pair();
    let mut keys = Vec::new();
    for i in 0..(MAX_WARM_CANDIDATES + 1) {
        let key = format!("sess-{}", i);
        keys.push(key.clone());
        loop_.store_warm_candidate(
            &key,
            vec![msg("user", &format!("m{}", i))],
            vec![],
            "test-model".to_string(),
        );
        // anchor 单调：显式隔开时间戳。
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let map = loop_.warm_candidates.lock();
    assert_eq!(map.len(), MAX_WARM_CANDIDATES);
    // 最老的 sess-0 被淘汰；最新的 sess-N 在。
    assert!(!map.contains_key(&keys[0]));
    assert!(map.contains_key(keys.last().unwrap()));
}

/// 价目表缺失 → 经济闸保守跳过：provider 一次都不被打到。
#[tokio::test]
async fn warm_replay_skips_without_pricing() {
    let (loop_, calls) = recording_pair();
    loop_.store_warm_candidate(
        "s1",
        vec![msg("user", "hello")],
        vec![],
        "test-model".to_string(),
    );
    loop_.warm_session_once("s1", "test-model", 300, 0.05).await;
    assert_eq!(call_count(&calls), 0);
}

/// 重放形态：字节级快照原样（消息内容 + 工具数 + 模型名），
/// max_tokens 钉 1；成功后锚点推进（同 TTL 窗口内不重复触发）。
#[tokio::test]
async fn warm_replay_uses_snapshot_with_max_tokens_one() {
    let (loop_, calls) = recording_pair();
    loop_.set_pricing_store(pricing_store_with_test_model());
    let tools = vec![crate::types::ToolDefinition {
        tool_type: "function".to_string(),
        function: crate::types::ToolFunctionDef {
            name: "read_file".to_string(),
            description: "read a file".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
    }];
    loop_.store_warm_candidate(
        "s1",
        vec![msg("user", "hello"), msg("assistant", "hi")],
        tools,
        "test-model".to_string(),
    );
    loop_.warm_session_once("s1", "test-model", 300, 0.05).await;

    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    let call = &recorded[0];
    assert_eq!(call.model, "test-model");
    assert_eq!(call.max_tokens, Some(1));
    assert_eq!(call.tool_count, 1);
    assert_eq!(call.messages.len(), 2);
    assert_eq!(call.messages[0].content, "hello");
    assert_eq!(call.messages[1].content, "hi");
}

/// 估算超上限（cost_limit_usd=0 且模型非免费）→ 跳过不发请求。
#[tokio::test]
async fn warm_replay_skips_over_cost_limit() {
    let (loop_, calls) = recording_pair();
    loop_.set_pricing_store(pricing_store_with_test_model());
    loop_.store_warm_candidate(
        "s1",
        vec![msg(
            "user",
            "a fairly long prompt body that still costs more than zero",
        )],
        vec![],
        "test-model".to_string(),
    );
    loop_.warm_session_once("s1", "test-model", 300, 0.0).await;
    assert_eq!(call_count(&calls), 0);
}

/// 模型切换 / 模型不匹配：候选对活动模型不可见（不重放），扫描后旧模型
/// 快照被清理。
#[tokio::test]
async fn warm_scan_drops_candidates_of_switched_model() {
    let (loop_, calls) = recording_pair();
    let cfg_path = write_temp_config(true, Some(300));
    loop_.set_config_path(cfg_path);
    // active_model = config.model = "test-model"；候选是旧模型 → 清理。
    loop_.store_warm_candidate(
        "s1",
        vec![msg("user", "stale")],
        vec![],
        "old-model".to_string(),
    );
    // 把锚点拨老（直接构造已到期形态：重新存一份后改 anchor）。
    {
        let mut map = loop_.warm_candidates.lock();
        map.get_mut("s1").unwrap().anchor =
            std::time::Instant::now() - std::time::Duration::from_secs(400);
    }
    loop_.warm_scan_once().await;
    assert!(loop_.warm_candidates.lock().is_empty());
    assert_eq!(call_count(&calls), 0);
}

/// 扫描主臂：到期 + 空闲 → warm；未到期 → 不动；忙会话 → 跳过且候选保留。
#[tokio::test]
async fn warm_scan_fires_only_for_due_idle_sessions() {
    let (loop_, calls) = recording_pair();
    loop_.set_pricing_store(pricing_store_with_test_model());
    loop_.set_config_path(write_temp_config(true, Some(300)));

    // due + idle：应重放。
    loop_.store_warm_candidate(
        "due-idle",
        vec![msg("user", "warm me")],
        vec![],
        "test-model".to_string(),
    );
    // fresh（未到期）：不动。
    loop_.store_warm_candidate(
        "fresh",
        vec![msg("user", "not yet")],
        vec![],
        "test-model".to_string(),
    );
    // due + busy：跳过，候选保留。
    loop_.store_warm_candidate(
        "due-busy",
        vec![msg("user", "busy")],
        vec![],
        "test-model".to_string(),
    );
    {
        let mut map = loop_.warm_candidates.lock();
        map.get_mut("due-idle").unwrap().anchor =
            std::time::Instant::now() - std::time::Duration::from_secs(400);
        map.get_mut("due-busy").unwrap().anchor =
            std::time::Instant::now() - std::time::Duration::from_secs(400);
    }
    assert!(loop_.try_acquire_session("due-busy"));

    loop_.warm_scan_once().await;

    assert_eq!(call_count(&calls), 1, "只有 due-idle 被重放");
    {
        let map = loop_.warm_candidates.lock();
        assert!(map.contains_key("due-busy"), "忙会话候选保留");
        assert!(map.contains_key("fresh"));
        // due-idle 重放成功 → 锚点推进：推到「刚刚」，再扫一轮不再触发。
    }
    loop_.warm_scan_once().await;
    assert_eq!(call_count(&calls), 1, "重放后锚点推进，同窗口不重复 warm");
}

/// 模型未声明 TTL → 扫描诚实跳过（不发请求、不清理候选）。
#[tokio::test]
async fn warm_scan_skips_when_ttl_unknown() {
    let (loop_, calls) = recording_pair();
    loop_.set_config_path(write_temp_config(true, None));
    loop_.set_pricing_store(pricing_store_with_test_model());
    loop_.store_warm_candidate(
        "s1",
        vec![msg("user", "x")],
        vec![],
        "test-model".to_string(),
    );
    {
        let mut map = loop_.warm_candidates.lock();
        map.get_mut("s1").unwrap().anchor =
            std::time::Instant::now() - std::time::Duration::from_secs(400);
    }
    loop_.warm_scan_once().await;
    assert_eq!(call_count(&calls), 0);
    assert!(
        loop_.warm_candidates.lock().contains_key("s1"),
        "TTL 未知只跳过，不误删候选"
    );
}

/// 开启开关的 config → capture 开（快照写入路径的开关面）。
#[test]
fn capture_gate_follows_config() {
    let (loop_, _) = recording_pair();
    loop_.set_config_path(write_temp_config(true, Some(300)));
    assert!(loop_.cache_warmer_capture_enabled());

    let (off, _) = recording_pair();
    off.set_config_path(write_temp_config(false, Some(300)));
    assert!(!off.cache_warmer_capture_enabled());
}

/// 停机态（running=false）主循环首个 tick 即退出——不挂任务、无残留定时器。
#[tokio::test]
async fn warmer_loop_exits_immediately_when_not_running() {
    let (loop_, _) = recording_pair();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        loop_.clone().run_cache_warmer_loop(),
    )
    .await;
    assert!(result.is_ok(), "停机态下 warmer 主循环应立即退出");
}

/// 热关：运行中（running=true）+ 配置 enabled=false → 首 tick 退出。
#[tokio::test]
async fn warmer_loop_exits_when_config_disabled() {
    let (loop_, _) = recording_pair();
    loop_
        .running
        .store(true, std::sync::atomic::Ordering::Release);
    // 无 config_path → current 配置 = 默认关。
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        loop_.clone().run_cache_warmer_loop(),
    )
    .await;
    assert!(result.is_ok(), "配置关闭时 warmer 主循环应热退出");
}
