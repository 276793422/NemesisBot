//! DataStore usage 层覆盖率补充测试（compute_cost_usd 包装 / 工具健康
//! 零调用分支 / get_request_log / pricing 访问器）。

use std::path::PathBuf;

use crate::models::ModelToolHealth;
use crate::usage_store::DataStore;

fn temp_db_path(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nemesis_data_cov_{}_{}_{}.db",
        tag,
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let _ = std::fs::remove_file(&path);
    path
}

fn open_store(tag: &str) -> (DataStore, PathBuf) {
    let path = temp_db_path(tag);
    let store = DataStore::open(&path).expect("open temp DataStore");
    (store, path)
}

/// compute_cost_usd 包装层：已知模型走分层查表算出正成本。
#[test]
fn compute_cost_usd_known_model_matches_layered_table() {
    let (store, path) = open_store("cost_known");
    let table = crate::pricing::PricingTable::embedded();
    let entry = table
        .entries()
        .iter()
        .find(|e| e.input_cost_per_million > 0.0 && e.output_cost_per_million > 0.0)
        .expect("embedded table has a priced entry");

    let cost = store.compute_cost_usd(&entry.model_id, 1_000_000, 1_000_000, 0, 0);
    assert!(cost > 0.0, "expected positive cost, got {cost}");
    let expected = entry.input_cost_per_million + entry.output_cost_per_million;
    assert!(
        (cost - expected).abs() < 1e-6,
        "cost {cost} != layered expected {expected}"
    );
    let _ = std::fs::remove_file(path);
}

/// compute_cost_usd 包装层：未知模型诚实退 0.0（成本是观测数据，不猜）。
#[test]
fn compute_cost_usd_unknown_model_returns_zero() {
    let (store, path) = open_store("cost_unknown");
    let cost = store.compute_cost_usd("no/such-model-xyz", 1000, 1000, 0, 0);
    assert_eq!(cost, 0.0);
    let _ = std::fs::remove_file(path);
}

/// pricing() 访问器返回同层价目表（可读元数据 + 可查表）。
#[test]
fn pricing_accessor_exposes_layered_store() {
    let (store, path) = open_store("pricing_accessor");
    let pricing = store.pricing();
    assert!(pricing.list_custom().is_empty());
    assert!(pricing.lookup("no/such-model-xyz").is_none());
    let _ = std::fs::remove_file(path);
}

/// query_model_tool_health：tool_calls=0 的行（只补 validation_failures，
/// 经裸 SQL 直插模拟）failure_rate 走 0.0 分支（除零保护）。
#[test]
fn model_tool_health_zero_calls_branch_yields_zero_rate() {
    let (store, path) = open_store("health_zero_calls");
    // 直接造一行 tool_calls=0 的记录（API 语义每次调用 tool_calls+1，
    // 只有 schema 层手工数据能出现 0 调用）。
    {
        let db_path = path.to_string_lossy().to_string();
        let raw = rusqlite::Connection::open(&db_path).expect("raw conn");
        raw.execute(
            "INSERT INTO tool_validation_stats (day, model, tool_calls, validation_failures)
             VALUES ('2026-09-01', 'probe-model', 0, 3)",
            [],
        )
        .expect("insert zero-call row");
    }

    let rows = store.query_model_tool_health(60).expect("query health");
    let row = rows
        .iter()
        .find(|r: &&ModelToolHealth| r.model == "probe-model")
        .expect("zero-call model row present");
    assert_eq!(row.tool_calls, 0);
    assert_eq!(row.validation_failures, 3);
    assert_eq!(row.failure_rate, 0.0);
    let _ = std::fs::remove_file(path);
}

/// get_request_log：命中返回行，未命中返回 None（不走 Err 通道）。
#[test]
fn get_request_log_hit_and_miss() {
    let (store, path) = open_store("get_log");

    let log = crate::RequestLog {
        id: 0,
        trace_id: "tr-cov".to_string(),
        model: "m".to_string(),
        provider_type: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: 0.0,
        latency_ms: 7,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 1_700_000_000_000,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: "agent:main:s1".to_string(),
    };
    store.insert_request_log(&log).expect("insert");

    let hit = store.get_request_log(1).expect("query by id");
    assert!(hit.is_some());
    assert_eq!(hit.unwrap().trace_id, "tr-cov");

    let miss = store.get_request_log(999_999).expect("query by id");
    assert!(miss.is_none());
    let _ = std::fs::remove_file(path);
}

// ===========================================================================
// Wave4 覆盖批次：查询/聚合/保留清理层（query_summary / query_trends /
// query_logs / aggregate_session_usage(_by_task) / record_tool_validation /
// retention_sweep / compute_cost_breakdown）。时间戳一律用秒（与
// unixepoch / retention cutoff 同单位）。
// ===========================================================================

use crate::models::LogFilter;

fn w4_log(model: &str, created_at: i64, session_key: &str, status: i32) -> crate::RequestLog {
    crate::RequestLog {
        id: 0,
        trace_id: format!("tr-{model}-{created_at}"),
        model: model.to_string(),
        provider_type: "openai".to_string(),
        input_tokens: 100,
        output_tokens: 40,
        cache_creation_tokens: 10,
        cache_read_tokens: 30,
        total_cost_usd: 0.5,
        latency_ms: 33,
        status_code: status,
        error_message: None,
        is_streaming: true,
        created_at,
        pricing_model: String::new(),
        input_cost_usd: 0.0,
        output_cost_usd: 0.0,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: Some(12),
        session_key: session_key.to_string(),
    }
}

/// query_summary：聚合计数/Token/成本，且 cache_hit_rate 在有缓存 Token 时
/// = read/(creation+read)、在零缓存时回 0.0（除零保护分支）。
#[test]
fn query_summary_aggregates_and_computes_cache_hit_rate() {
    let (store, path) = open_store("w4_summary");
    let now = chrono::Utc::now().timestamp();
    store
        .insert_request_log(&w4_log("m/a", now - 10, "s1", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/b", now - 5, "s2", 500))
        .unwrap();

    let sum = store
        .query_summary(now - 60, now + 60)
        .expect("query_summary");
    assert_eq!(sum.total_requests, 2);
    assert_eq!(sum.success_count, 1, "只有 200 计成功");
    assert_eq!(sum.total_input_tokens, 200);
    assert_eq!(sum.total_output_tokens, 80);
    assert_eq!(sum.total_cache_creation_tokens, 20);
    assert_eq!(sum.total_cache_read_tokens, 60);
    assert!((sum.total_cost_usd - 1.0).abs() < 1e-9);
    assert!(
        (sum.cache_hit_rate - 0.75).abs() < 1e-9,
        "60/(20+60)={}",
        sum.cache_hit_rate
    );

    // 零缓存行 → cacheable=0 → 0.0 分支。
    store
        .insert_request_log(&crate::RequestLog {
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            ..w4_log("m/c", now - 1, "s3", 200)
        })
        .unwrap();
    let sum2 = store.query_summary(now - 2, now).expect("narrow window");
    assert_eq!(sum2.total_requests, 1);
    assert_eq!(sum2.cache_hit_rate, 0.0, "零缓存 token 命中率为 0.0");
    let _ = std::fs::remove_file(path);
}

/// query_trends：hour 与 day 两个分组臂各出点，字段投影齐全。
#[test]
fn query_trends_groups_by_hour_and_day() {
    let (store, path) = open_store("w4_trends");
    // 固定在某个 UTC 整点内的两个时刻（同 hour/同 day 各聚成一桶）。
    let base = 1_770_000_000_i64; // 秒
    store
        .insert_request_log(&w4_log("m/a", base, "s1", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/a", base + 60, "s1", 200))
        .unwrap();

    let hourly = store
        .query_trends(base - 3600, base + 3600, "hour")
        .expect("hour trends");
    assert_eq!(hourly.len(), 1, "同小时聚合为一点：{hourly:?}");
    assert!(
        hourly[0].label.ends_with(":00:00"),
        "小时标签形态：{}",
        hourly[0].label
    );
    assert_eq!(hourly[0].request_count, 2);
    assert_eq!(hourly[0].input_tokens, 200);

    let daily = store
        .query_trends(base - 86400, base + 86400, "day")
        .expect("day trends");
    assert_eq!(daily.len(), 1, "同天聚合为一点：{daily:?}");
    assert_eq!(
        daily[0].label.len(),
        10,
        "日期标签 YYYY-MM-DD：{}",
        daily[0].label
    );
    assert!((daily[0].total_cost_usd - 1.0).abs() < 1e-9);

    // 未知 group_by 走 day 分支（_ 臂）。
    let other = store
        .query_trends(base - 86400, base + 86400, "week")
        .expect("fallback trends");
    assert_eq!(other.len(), 1);
    let _ = std::fs::remove_file(path);
}

/// query_logs：正交过滤（model LIKE / status 精确 / session_key LIKE）、
/// 分页序、行投影回读（row_to_log 全列）与 total 计数。
#[test]
fn query_logs_filters_paginates_and_roundtrips_rows() {
    let (store, path) = open_store("w4_logs");
    let base = 1_770_000_000_i64;
    store
        .insert_request_log(&w4_log("gpt/x", base, "agent:main:s1", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("glm/y", base + 1, "agent:main:s2", 429))
        .unwrap();
    store
        .insert_request_log(&w4_log("gpt/z", base + 2, "rpc:node-b/task-9", 200))
        .unwrap();

    // 无过滤：全量，按 created_at DESC。
    let (logs, total) = store
        .query_logs(base - 10, base + 100, 1, 10, &LogFilter::default())
        .expect("all logs");
    assert_eq!(total, 3);
    assert_eq!(logs[0].trace_id, "tr-gpt/z-1770000002", "DESC 序");

    // model LIKE。
    let f = LogFilter {
        model: Some("gpt".into()),
        ..Default::default()
    };
    let (logs, total) = store
        .query_logs(base - 10, base + 100, 1, 10, &f)
        .expect("model filter");
    assert_eq!(total, 2);
    assert!(logs.iter().all(|l| l.model.starts_with("gpt")));

    // status 精确。
    let f = LogFilter {
        status: Some(429),
        ..Default::default()
    };
    let (logs, total) = store
        .query_logs(base - 10, base + 100, 1, 10, &f)
        .expect("status filter");
    assert_eq!(total, 1);
    assert_eq!(logs[0].status_code, 429);

    // session_key LIKE 子串。
    let f = LogFilter {
        session_key: Some("task-9".into()),
        ..Default::default()
    };
    let (logs, total) = store
        .query_logs(base - 10, base + 100, 1, 10, &f)
        .expect("session filter");
    assert_eq!(total, 1);
    assert_eq!(logs[0].session_key, "rpc:node-b/task-9");

    // 组合过滤 + 分页：page=2, size=1 → 第二新的一条；空 model 串不加条件。
    let f = LogFilter {
        model: Some(String::new()),
        status: None,
        session_key: Some("agent:main".into()),
    };
    let (page2, total) = store
        .query_logs(base - 10, base + 100, 2, 1, &f)
        .expect("combined + page2");
    assert_eq!(total, 2);
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].session_key, "agent:main:s1");

    // row_to_log 全列投影回读（含 bool/Option 字段）。
    let (rows, _) = store
        .query_logs(base - 10, base + 100, 1, 1, &LogFilter::default())
        .expect("row");
    let row = &rows[0];
    assert!(row.is_streaming, "写 true 读 true");
    assert_eq!(row.first_token_ms, Some(12));
    assert_eq!(row.latency_ms, 33);
    assert!((row.total_cost_usd - 0.5).abs() < 1e-9);
    let _ = std::fs::remove_file(path);
}

/// aggregate_session_usage 精确键匹配 + 无行全零；by_task 后缀匹配两种
/// 身份形态（worker id 与 peer 名不同源也能聚合到）。
#[test]
fn session_aggregates_exact_and_task_suffix() {
    let (store, path) = open_store("w4_agg");
    store
        .insert_request_log(&w4_log("m/a", 1, "agent:main:s1", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/a", 2, "agent:main:s1", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/b", 3, "cluster_rpc:Node-B/t-uuid", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/b", 4, "cluster_rpc:worker-777/t-uuid", 200))
        .unwrap();

    let agg = store
        .aggregate_session_usage("agent:main:s1")
        .expect("session agg");
    assert_eq!(agg.requests, 2);
    assert_eq!(agg.input_tokens, 100 * 2 + 30 * 2, "input + cache_read");
    assert_eq!(agg.output_tokens, 80);
    assert!((agg.total_cost_usd - 1.0).abs() < 1e-9);

    // 无行 → 全零（不是错误）。
    let zero = store
        .aggregate_session_usage("agent:main:none")
        .expect("empty agg");
    assert_eq!(zero.requests, 0);
    assert_eq!(zero.total_cost_usd, 0.0);

    // 按任务后缀：两种身份形态都收进来。
    let task = store
        .aggregate_session_usage_by_task("t-uuid")
        .expect("task agg");
    assert_eq!(task.requests, 2);
    let _ = std::fs::remove_file(path);
}

/// record_tool_validation：同日同模型 upsert 累加，成败分别计数。
#[test]
fn tool_validation_upsert_accumulates() {
    let (store, path) = open_store("w4_validation");
    store.record_tool_validation("m/a", false).unwrap();
    store.record_tool_validation("m/a", true).unwrap();
    store.record_tool_validation("m/a", true).unwrap();
    store.record_tool_validation("m/b", false).unwrap();

    let rows = store.query_model_tool_health(7).expect("health");
    let a = rows.iter().find(|r| r.model == "m/a").expect("m/a row");
    assert_eq!(a.tool_calls, 3);
    assert_eq!(a.validation_failures, 2);
    assert!((a.failure_rate - 2.0 / 3.0).abs() < 1e-9);
    let b = rows.iter().find(|r| r.model == "m/b").expect("m/b row");
    assert_eq!(b.tool_calls, 1);
    assert_eq!(b.validation_failures, 0);
    let _ = std::fs::remove_file(path);
}

/// retention_sweep：按天 rollup+删除旧明细、max_rows 裁最旧、None/None
/// 幂等返回 0；rollup 落 daily_rollups 可被裸 SQL 验证。
#[test]
fn retention_sweep_rollups_trims_and_noops() {
    let (store, path) = open_store("w4_sweep");
    let now = chrono::Local::now().timestamp();
    let old = now - 3 * 86400;
    store
        .insert_request_log(&w4_log("m/a", old, "s1", 200))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/a", old + 5, "s1", 500))
        .unwrap();
    store
        .insert_request_log(&w4_log("m/b", now, "s2", 200))
        .unwrap();

    // noop：不传任何阈值 → 0 行删除。
    assert_eq!(store.retention_sweep(None, None).expect("noop"), 0);
    // retention_days=0 的 Some 也视为跳过（filter 语义）。
    assert_eq!(store.retention_sweep(Some(0), Some(0)).expect("zero"), 0);

    // 按天：旧 2 行进 daily_rollups 后删除，新 1 行不动。
    let deleted = store.retention_sweep(Some(1), None).expect("by day");
    assert_eq!(deleted, 2);
    let (left, total) = store
        .query_logs(0, now + 86400, 1, 10, &LogFilter::default())
        .expect("after sweep");
    assert_eq!(total, 1);
    assert_eq!(left[0].session_key, "s2");

    // rollup 表内容可验证（裸 SQL 读 daily_rollups）。
    {
        let raw = rusqlite::Connection::open(path.to_string_lossy().as_ref()).unwrap();
        let (count, requests): (i64, i64) = raw
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(request_count),0) FROM daily_rollups",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(count >= 1, "daily_rollups 应有 rollup 行");
        assert_eq!(requests, 2, "rollup 聚合了旧 2 行");
    }

    // max_rows：再插 5 行新明细（现存共 1+5=6 行），cap=3 → 最旧 3 行被裁。
    for i in 0..5 {
        store
            .insert_request_log(&w4_log("m/c", now + 10 + i, "s3", 200))
            .unwrap();
    }
    let trimmed = store.retention_sweep(None, Some(3)).expect("trim");
    assert_eq!(trimmed, 3, "6→3 裁掉最旧 3 行");
    let (_, total) = store
        .query_logs(0, now + 86400, 1, 10, &LogFilter::default())
        .expect("after trim");
    assert_eq!(total, 3);
    let _ = std::fs::remove_file(path);
}

/// compute_cost_breakdown 包装：已知模型 Some（分项齐全）、未知模型 None。
#[test]
fn cost_breakdown_wrapper_some_and_none() {
    let (store, path) = open_store("w4_breakdown");
    let table = crate::pricing::PricingTable::embedded();
    let entry = table
        .entries()
        .iter()
        .find(|e| e.input_cost_per_million > 0.0 && e.output_cost_per_million > 0.0)
        .expect("embedded table has a priced entry");

    let bd = store
        .compute_cost_breakdown(&entry.model_id, 1_000_000, 1_000_000, 0, 0)
        .expect("known model has breakdown");
    assert!(
        (bd.input_cost_usd - entry.input_cost_per_million).abs() < 1e-6,
        "{bd:?}"
    );
    assert!(
        (bd.output_cost_usd - entry.output_cost_per_million).abs() < 1e-6,
        "{bd:?}"
    );

    assert!(
        store
            .compute_cost_breakdown("no/such-model-xyz", 1, 1, 0, 0)
            .is_none()
    );
    let _ = std::fs::remove_file(path);
}
