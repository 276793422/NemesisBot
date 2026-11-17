//! resource_providers（nemesis-web 侧）单测——usage / session_allow 两类目
//! 的三态与过滤口径（2026-11-17 会话资源盘点 B1）。
//!
//! usage 聚合依赖真 SQLite 账本（dev-dependencies 已把 nemesis-data
//! usage-store feature 带回测试构建）；session_allow 走
//! [`SessionAllowAllState`] 直构（零全局态，无串行锁需求）。

#[cfg(feature = "security")]
use crate::resource_providers::SessionAllowProvider;
use crate::resource_providers::UsageProvider;
use nemesis_types::resources::{CategoryStatus, GlobalResourceProvider, ResourceCtx};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_key(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("agent:main:session:rpw-{tag}-{nanos}")
}

fn ctx_for<'a>(key: &'a str) -> ResourceCtx<'a> {
    ResourceCtx {
        session_key: key,
        bare_sid: "x",
        workspace_root: Path::new("."),
    }
}

fn seed_log(trace: &str, session: &str) -> nemesis_data::RequestLog {
    nemesis_data::RequestLog {
        id: 0,
        trace_id: trace.to_string(),
        model: "test-model".to_string(),
        provider_type: "openai".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 1,
        total_cost_usd: 0.01,
        latency_ms: 100,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 1_000,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: session.to_string(),
    }
}

#[test]
fn usage_without_store_is_unavailable() {
    let key = unique_key("no-store");
    let view = UsageProvider::new(None).collect(&ctx_for(&key));
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));
    assert!(view.status.unavailable_reason().is_some());
}

#[test]
fn usage_empty_without_requests() {
    let dir = std::env::temp_dir().join(format!("nb-rpw-usage-empty-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let store = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.join("data.db")).expect("open store"),
    );
    let key = unique_key("empty");
    let view = UsageProvider::new(Some(std::sync::Arc::clone(&store))).collect(&ctx_for(&key));
    assert_eq!(view.status, CategoryStatus::Empty);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn usage_aggregates_requests_and_filters_by_session() {
    let dir = std::env::temp_dir().join(format!("nb-rpw-usage-agg-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let store = std::sync::Arc::new(
        nemesis_data::DataStore::open(&dir.join("data.db")).expect("open store"),
    );
    let mine = unique_key("agg");
    let other = unique_key("agg-other");
    // 本会话两行：input 口径 = 100 + 10 + 1 = 111/行。
    store
        .insert_request_log(&seed_log("t1", &mine))
        .expect("insert t1");
    store
        .insert_request_log(&seed_log("t2", &mine))
        .expect("insert t2");
    // 他会话一行（不计入本会话切面）。
    store
        .insert_request_log(&seed_log("t3", &other))
        .expect("insert t3");

    let view = UsageProvider::new(Some(std::sync::Arc::clone(&store))).collect(&ctx_for(&mine));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 2);
    assert_eq!(view.summary["requests"], serde_json::json!(2));
    assert_eq!(view.summary["input_tokens"], serde_json::json!(222));
    assert_eq!(view.summary["output_tokens"], serde_json::json!(100));
    let cost = view.summary["total_cost_usd"].as_f64().expect("cost");
    assert!((cost - 0.02).abs() < 1e-9);

    // 无请求会话 → Empty。
    let missing = unique_key("agg-missing");
    let view = UsageProvider::new(Some(store)).collect(&ctx_for(&missing));
    assert_eq!(view.status, CategoryStatus::Empty);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "security")]
#[test]
fn session_allow_three_states() {
    use std::sync::Arc;
    let key = unique_key("allow");

    // 未装配 → Unavailable。
    let view = SessionAllowProvider::new(None).collect(&ctx_for(&key));
    assert!(matches!(view.status, CategoryStatus::Unavailable { .. }));

    let state = Arc::new(nemesis_security::session_allow_all::SessionAllowAllState::new());

    // 无条目（从未开启）→ Empty。
    let view = SessionAllowProvider::new(Some(Arc::clone(&state))).collect(&ctx_for(&key));
    assert_eq!(view.status, CategoryStatus::Empty);

    // 开启 → ok（since 在场，自动批准计数起步 0）。
    state.set(&key, true);
    let view = SessionAllowProvider::new(Some(Arc::clone(&state))).collect(&ctx_for(&key));
    assert_eq!(view.status, CategoryStatus::Ok);
    assert_eq!(view.total, 1);
    assert_eq!(view.summary["enabled"], serde_json::json!(true));
    assert!(!view.summary["since"].as_str().expect("since").is_empty());
    assert_eq!(view.summary["auto_approved"], serde_json::json!(0));

    // 放行计数可见。
    state.bump(&key);
    let view = SessionAllowProvider::new(Some(Arc::clone(&state))).collect(&ctx_for(&key));
    assert_eq!(view.summary["auto_approved"], serde_json::json!(1));

    // 关闭（条目保留但 enabled=false）→ Empty——盘点不展示关过的旧条目。
    state.set(&key, false);
    let view = SessionAllowProvider::new(Some(state)).collect(&ctx_for(&key));
    assert_eq!(view.status, CategoryStatus::Empty);
}
