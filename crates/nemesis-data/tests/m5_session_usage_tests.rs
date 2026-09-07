//! M5（devtool-upgrade 阶段 3）：`DataStore::aggregate_session_usage` 测试。
//!
//! 会话级用量聚合的三个契约：
//! 1. 求和口径——`input_tokens` 含 `cache_read`（模型实际"读到"的输入，
//!    与 UsageStoreSlot 统计同式），cost 直接 SUM；
//! 2. session_key **精确匹配**隔离（不是 LIKE 子串——两个会话互不污染）；
//! 3. 未知 key → 全零聚合（前端按零值不渲染，而非报错）。

use nemesis_data::{DataStore, RequestLog};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 与 unit_tests.rs 同款唯一路径（pid + 进程内单调计数器，不用时间戳——
/// Windows 计时精度窗口碰撞教训，见 unit_tests.rs 注释）。
fn temp_db_path() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nemesis_data_m5_session_usage_test_{}_{}.db",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = fs::remove_file(&path);
    path
}

fn log(session_key: &str, input: i64, cache_read: i64, output: i64, cost: f64) -> RequestLog {
    RequestLog {
        trace_id: format!("m5-{session_key}"),
        model: "test-model".to_string(),
        provider_type: "test".to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        total_cost_usd: cost,
        status_code: 200,
        session_key: session_key.to_string(),
        ..Default::default()
    }
}

#[test]
fn aggregate_sums_input_including_cache_read() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("open db");
    let key = "agent:main:session:agg-a";

    store
        .insert_request_log(&log(key, 100, 50, 30, 0.01))
        .unwrap();
    store
        .insert_request_log(&log(key, 200, 0, 40, 0.02))
        .unwrap();

    let agg = store.aggregate_session_usage(key).expect("aggregate");
    assert_eq!(agg.requests, 2);
    // input 口径含 cache_read：(100+50) + (200+0) = 350。
    assert_eq!(agg.input_tokens, 350);
    assert_eq!(agg.output_tokens, 70);
    assert!((agg.total_cost_usd - 0.03).abs() < 1e-9);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn exact_key_match_isolates_sessions() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("open db");

    let a = "agent:main:session:iso-a";
    let b = "agent:main:session:iso-b";
    store.insert_request_log(&log(a, 100, 0, 10, 0.01)).unwrap();
    // b 是 a 的前缀超集：若实现误用 LIKE '%a'，b 的行会漏进 a 的聚合。
    store.insert_request_log(&log(b, 900, 0, 90, 0.90)).unwrap();

    let agg_a = store.aggregate_session_usage(a).expect("aggregate a");
    assert_eq!(agg_a.requests, 1, "b 的行不得漏进 a（精确匹配）");
    assert_eq!(agg_a.input_tokens, 100);
    assert_eq!(agg_a.output_tokens, 10);

    let agg_b = store.aggregate_session_usage(b).expect("aggregate b");
    assert_eq!(agg_b.requests, 1);
    assert_eq!(agg_b.input_tokens, 900);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn unknown_key_yields_zero_aggregate() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("open db");

    let agg = store
        .aggregate_session_usage("agent:main:session:nope")
        .expect("aggregate unknown key");
    assert_eq!(agg, nemesis_data::SessionUsageAgg::default());

    let _ = fs::remove_file(&db_path);
}
