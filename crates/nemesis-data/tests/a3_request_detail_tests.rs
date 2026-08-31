//! A3 请求明细增强测试（2026-08-31）。
//!
//! 覆盖：v1→v2 迁移（旧库 ALTER + 旧行保留）、分项成本 breakdown、
//! LogFilter 三维过滤、get_request_log 详情往返。

use nemesis_data::{CostBreakdown, DataStore, LogFilter, RequestLog};
use rusqlite::Connection;
use std::fs;

fn temp_db_path(tag: &str) -> std::path::PathBuf {
    // 唯一性 = 进程 id + 进程内单调计数器（时间戳命名在满载并行下有计时
    // 精度撞名窗口，见 unit_tests.rs 同名 helper 注释）。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nemesis_data_a3_{tag}_{}_{}.db",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    let _ = fs::remove_file(&path);
    path
}

fn full_log(trace: &str, model: &str, session: &str, status: i32, ts: i64) -> RequestLog {
    RequestLog {
        id: 0,
        trace_id: trace.to_string(),
        model: model.to_string(),
        provider_type: "openai".to_string(),
        input_tokens: 1000,
        output_tokens: 500,
        cache_creation_tokens: 100,
        cache_read_tokens: 200,
        total_cost_usd: 0.0021,
        latency_ms: 1500,
        status_code: status,
        error_message: None,
        is_streaming: false,
        created_at: ts,
        pricing_model: "deepseek-chat".to_string(),
        input_cost_usd: 0.001,
        output_cost_usd: 0.001,
        cache_creation_cost_usd: 0.0001,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: session.to_string(),
    }
}

/// v1 库（旧 schema + 旧行）打开 → 自动 ALTER 到 v2：新列存在、
/// user_version=2、旧行保留且新字段为默认值。
#[test]
fn migration_v1_to_v2_preserves_rows_and_adds_columns() {
    let db_path = temp_db_path("migrate");

    // 手工建 v1 库（复刻 SCHEMA_V1 的 request_logs + user_version=1）。
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE request_logs (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                trace_id        TEXT    NOT NULL,
                model           TEXT    NOT NULL,
                provider_type   TEXT    NOT NULL DEFAULT '',
                input_tokens    INTEGER NOT NULL DEFAULT 0,
                output_tokens   INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
                total_cost_usd  REAL    NOT NULL DEFAULT 0.0,
                latency_ms      INTEGER NOT NULL DEFAULT 0,
                status_code     INTEGER NOT NULL DEFAULT 200,
                error_message   TEXT,
                is_streaming     INTEGER NOT NULL DEFAULT 0,
                created_at      INTEGER NOT NULL
            );
            INSERT INTO request_logs (trace_id, model, created_at)
                VALUES ('legacy-row', 'gpt-4', 1700000000);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }

    let store = DataStore::open(&db_path).expect("v1 → v2 migration must succeed");

    // 版本到 2。
    let version: i32 = Connection::open(&db_path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 2);

    // 旧行保留 + 新字段默认值（迁移只加列，不回填历史）。
    let (logs, total) = store
        .query_logs(0, 2000000000, 1, 10, &LogFilter::default())
        .unwrap();
    assert_eq!(total, 1);
    assert_eq!(logs[0].trace_id, "legacy-row");
    assert_eq!(logs[0].pricing_model, "");
    assert_eq!(logs[0].session_key, "");
    assert_eq!(logs[0].first_token_ms, None);

    // 迁移后可正常写入新字段行。
    store.insert_request_log(&full_log("new-row", "deepseek-chat", "s1", 200, 1700000100)).unwrap();
    let got = store.get_request_log(2).unwrap().expect("new row");
    assert_eq!(got.pricing_model, "deepseek-chat");

    let _ = fs::remove_file(&db_path);
}

/// 分项成本：DataStore 分层入口命中已知模型 → pricing_model + 分项 + 总和一致；
/// 未命中 → None。
#[test]
fn cost_breakdown_layers_and_unknown() {
    let db_path = temp_db_path("breakdown");
    let store = DataStore::open(&db_path).unwrap();

    // deepseek-chat 在内置 36 模型表内（provider/前缀裸名匹配也覆盖）。
    let bd = store
        .compute_cost_breakdown("deepseek/deepseek-chat", 1_000_000, 0, 0, 0)
        .expect("known model must hit");
    assert_eq!(bd.pricing_model, "deepseek-chat");
    assert!(bd.input_cost_usd > 0.0);
    assert_eq!(bd.output_cost_usd, 0.0);
    assert!(
        (bd.total_cost_usd - (bd.input_cost_usd + bd.output_cost_usd
            + bd.cache_creation_cost_usd + bd.cache_read_cost_usd)).abs() < 1e-12,
        "total must equal sum of components"
    );

    // 未命中 → None（调用方记空名 + 全 0）。
    assert_eq!(
        store.compute_cost_breakdown("no-such-model-xyz", 1, 2, 0, 0),
        None::<CostBreakdown>
    );

    let _ = fs::remove_file(&db_path);
}

/// LogFilter 三维过滤：model 子串 / status 精确 / session 子串，且可组合。
#[test]
fn log_filter_dimensions() {
    let db_path = temp_db_path("filter");
    let store = DataStore::open(&db_path).unwrap();
    store.insert_request_log(&full_log("t1", "gpt-4", "direct-a", 200, 1000)).unwrap();
    store.insert_request_log(&full_log("t2", "claude-3", "direct-b", 500, 2000)).unwrap();
    store.insert_request_log(&full_log("t3", "gpt-4o", "rpc:node-a/x", 200, 3000)).unwrap();

    // model 子串命中 gpt-4 与 gpt-4o。
    let (logs, total) = store
        .query_logs(0, 99999, 1, 100, &LogFilter { model: Some("gpt-4".into()), ..Default::default() })
        .unwrap();
    assert_eq!(total, 2);
    assert!(logs.iter().all(|l| l.model.contains("gpt-4")));

    // status 精确。
    let (_, total) = store
        .query_logs(0, 99999, 1, 100, &LogFilter { status: Some(500), ..Default::default() })
        .unwrap();
    assert_eq!(total, 1);

    // session 子串（跨 direct/rpc 前缀按片段）。
    let (_, total) = store
        .query_logs(0, 99999, 1, 100, &LogFilter { session_key: Some("node-a".into()), ..Default::default() })
        .unwrap();
    assert_eq!(total, 1);

    // 组合：model + status。
    let (_, total) = store
        .query_logs(
            0,
            99999,
            1,
            100,
            &LogFilter { model: Some("gpt".into()), status: Some(200), ..Default::default() },
        )
        .unwrap();
    assert_eq!(total, 2);

    // 空串 = 不过滤。
    let (_, total) = store
        .query_logs(0, 99999, 1, 100, &LogFilter::default())
        .unwrap();
    assert_eq!(total, 3);

    let _ = fs::remove_file(&db_path);
}

/// 详情往返：get_request_log 全字段（含 v2 新列）与写入一致；不存在 → None。
#[test]
fn get_request_log_roundtrip() {
    let db_path = temp_db_path("detail");
    let store = DataStore::open(&db_path).unwrap();
    let mut log = full_log("detail-1", "glm-4.7", "direct-c", 200, 5000);
    log.first_token_ms = Some(320);
    log.error_message = Some("oops".to_string());
    store.insert_request_log(&log).unwrap();

    let got = store.get_request_log(1).unwrap().expect("row exists");
    assert_eq!(got.trace_id, "detail-1");
    assert_eq!(got.model, "glm-4.7");
    assert_eq!(got.pricing_model, "deepseek-chat"); // helper 固定的计价名
    assert_eq!(got.session_key, "direct-c");
    assert_eq!(got.first_token_ms, Some(320));
    assert_eq!(got.error_message.as_deref(), Some("oops"));
    assert!((got.input_cost_usd - 0.001).abs() < 1e-9);

    assert!(store.get_request_log(999).unwrap().is_none());

    let _ = fs::remove_file(&db_path);
}

/// 保留策略：retention_days 按天 rollup+删除；None = 跳过按天步；
/// max_rows 超限裁最旧。
#[test]
fn retention_sweep_days_and_max_rows() {
    let db_path = temp_db_path("retention");
    let store = DataStore::open(&db_path).unwrap();
    let now = chrono::Local::now().timestamp();

    // 40 天前 1 行（按天步应删并 rollup）。
    store.insert_request_log(&full_log("old", "gpt-4", "s", 200, now - 40 * 86400)).unwrap();
    // 今天 3 行。
    for i in 0..3 {
        store.insert_request_log(&full_log(&format!("new-{i}"), "gpt-4", "s", 200, now - i)).unwrap();
    }

    // 按天 30 天：40 天前那行删掉 + rollup（rollup 查询直接开裸连接验证，
    // 不为测试加生产方法）。
    let deleted = store.retention_sweep(Some(30), None).unwrap();
    assert_eq!(deleted, 1);
    let (_, total) = store.query_logs(0, now + 86400, 1, 100, &LogFilter::default()).unwrap();
    assert_eq!(total, 3);
    let rollup_models: Vec<String> = Connection::open(&db_path)
        .unwrap()
        .prepare("SELECT DISTINCT model FROM daily_rollups")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert!(rollup_models.contains(&"gpt-4".to_string()), "rollup must include the old row's model");

    // 条数裁到 2：今天 3 行中最旧的「new-2」被裁。
    let deleted = store.retention_sweep(None, Some(2)).unwrap();
    assert_eq!(deleted, 1);
    let (remaining, total) = store
        .query_logs(0, now + 86400, 1, 100, &LogFilter::default())
        .unwrap();
    assert_eq!(total, 2);
    let traces: Vec<&str> = remaining.iter().map(|l| l.trace_id.as_str()).collect();
    assert!(traces.contains(&"new-0") && traces.contains(&"new-1") && !traces.contains(&"new-2"));

    let _ = fs::remove_file(&db_path);
}
