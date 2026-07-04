//! Integration tests for nemesis-data crate.
//!
//! Tests cover:
//! - Database initialization and migrations
//! - CRUD operations (insert, query, update, delete)
//! - Data aggregation and summary queries
//! - Trend analysis queries
//! - Pagination and filtering
//! - Data rollup operations
//! - Error handling and edge cases
//! - Concurrent access

use nemesis_data::{DataStore, RequestLog, TrendPoint, UsageSummary};
use std::fs;
use std::path::PathBuf;

/// Create a temporary database path for testing.
fn temp_db_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("nemesis_data_test_{}.db", std::thread::current().name().unwrap_or("default").replace("::", "_").replace("\"", "")));
    // Ensure file doesn't exist
    let _ = fs::remove_file(&path);
    path
}

/// Create a test RequestLog with default values.
fn create_test_log(id: i64, trace_id: &str, model: &str) -> RequestLog {
    RequestLog {
        id,
        trace_id: trace_id.to_string(),
        model: model.to_string(),
        provider_type: "test_provider".to_string(),
        input_tokens: 100 + id,
        output_tokens: 50 + id,
        cache_creation_tokens: 10 + id,
        cache_read_tokens: 5 + id,
        total_cost_usd: 0.001 * id as f64,
        latency_ms: 100 + id * 10,
        status_code: 200,
        error_message: None,
        is_streaming: id % 2 == 0,
        created_at: 1700000000 + id * 3600, // Hourly timestamps
    }
}

#[test]
fn test_database_initialization() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Verify database file was created
    assert!(db_path.exists());

    // Verify we can query the database (schema exists)
    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    assert_eq!(summary.total_requests, 0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_database_creates_parent_directory() {
    let mut db_path = std::env::temp_dir();
    db_path.push("nemesis_test_nested");
    db_path.push("subdir");
    db_path.push("test.db");

    // Ensure parent directories don't exist
    let _ = fs::remove_dir_all(db_path.parent().unwrap());

    let _store = DataStore::open(&db_path).expect("Failed to create database with nested directories");
    assert!(db_path.exists());

    // Cleanup
    let _ = fs::remove_dir_all(db_path.parent().unwrap().parent().unwrap());
}

#[test]
fn test_insert_single_request_log() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    let log = create_test_log(1, "trace-1", "gpt-4");
    store.insert_request_log(&log).expect("Failed to insert log");

    // Verify insertion
    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.success_count, 1);
    assert_eq!(summary.total_input_tokens, 101);
    assert_eq!(summary.total_output_tokens, 51);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_insert_multiple_request_logs() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert multiple logs
    for i in 1..=10 {
        let log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    assert_eq!(summary.total_requests, 10);
    assert_eq!(summary.success_count, 10);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_summary_with_time_range() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs with different timestamps
    for i in 1..=5 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = 1700000000 + i * 3600; // Different hours
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    // Query full range
    let summary = store.query_summary(1700000000, 1700000000 + 6 * 3600).expect("Failed to query summary");
    assert_eq!(summary.total_requests, 5);

    // Query partial range
    let partial_summary = store.query_summary(1700000000, 1700000000 + 3 * 3600).expect("Failed to query summary");
    assert_eq!(partial_summary.total_requests, 2);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_summary_with_error_responses() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert successful request
    let mut success_log = create_test_log(1, "trace-success", "gpt-4");
    success_log.status_code = 200;
    success_log.error_message = None;
    store.insert_request_log(&success_log).expect("Failed to insert success log");

    // Insert failed request
    let mut error_log = create_test_log(2, "trace-error", "gpt-4");
    error_log.status_code = 500;
    error_log.error_message = Some("Rate limit exceeded".to_string());
    store.insert_request_log(&error_log).expect("Failed to insert error log");

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    assert_eq!(summary.total_requests, 2);
    assert_eq!(summary.success_count, 1); // Only 200 status counts as success

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_cache_hit_rate_calculation() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert log with cache hits
    let mut log = create_test_log(1, "trace-1", "gpt-4");
    log.cache_creation_tokens = 100;
    log.cache_read_tokens = 400;
    log.input_tokens = 500;
    store.insert_request_log(&log).expect("Failed to insert log");

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    // Cache hit rate = cache_read_tokens / (cache_creation_tokens + cache_read_tokens)
    // — input tokens are NOT part of the cacheable denominator (see usage_store.rs).
    // = 400 / (100 + 400) = 400 / 500 = 0.8
    assert!((summary.cache_hit_rate - 0.8).abs() < 0.01);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_cache_hit_rate_with_no_cacheable_tokens() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert log with zero cacheable tokens
    let mut log = create_test_log(1, "trace-1", "gpt-4");
    log.cache_creation_tokens = 0;
    log.cache_read_tokens = 0;
    log.input_tokens = 0;
    store.insert_request_log(&log).expect("Failed to insert log");

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    assert_eq!(summary.cache_hit_rate, 0.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_trends_by_hour() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs across multiple hours
    for i in 0..=5 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = 1700000000 + i * 3600; // Different hours
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    let trends = store.query_trends(1700000000, 1700000000 + 6 * 3600, "hour")
        .expect("Failed to query hourly trends");

    assert!(trends.len() >= 6);
    assert!(trends.iter().all(|t| t.request_count > 0));

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_trends_by_day() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs across multiple days
    for i in 0..=3 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = 1700000000 + i * 86400; // Different days
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    let trends = store.query_trends(1700000000, 1700000000 + 4 * 86400, "day")
        .expect("Failed to query daily trends");

    assert!(trends.len() >= 4);
    assert!(trends.iter().all(|t| t.request_count > 0));

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_trends_with_invalid_group_by() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert test data
    let log = create_test_log(1, "trace-1", "gpt-4");
    store.insert_request_log(&log).expect("Failed to insert log");

    // Invalid group_by should default to "day"
    let trends = store.query_trends(0, 2000000000, "invalid")
        .expect("Failed to query trends with invalid group_by");

    assert!(!trends.is_empty());

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_logs_pagination() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert 25 logs
    for i in 1..=25 {
        let log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    // Query first page
    let (page1, total) = store.query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query first page");
    assert_eq!(page1.len(), 10);
    assert_eq!(total, 25);

    // Query second page
    let (page2, _) = store.query_logs(0, 2000000000, 2, 10)
        .expect("Failed to query second page");
    assert_eq!(page2.len(), 10);

    // Query third page (partial)
    let (page3, _) = store.query_logs(0, 2000000000, 3, 10)
        .expect("Failed to query third page");
    assert_eq!(page3.len(), 5);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_logs_with_time_filter() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs with different timestamps
    for i in 1..=10 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = 1700000000 + i * 3600;
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    // Query with time filter
    let (logs, total) = store.query_logs(1700000000 + 3 * 3600, 1700000000 + 8 * 3600, 1, 100)
        .expect("Failed to query logs with time filter");

    assert_eq!(total, 5); // Logs 4-8
    assert_eq!(logs.len(), 5);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_logs_ordering_by_created_at_desc() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs with sequential timestamps
    for i in 1..=5 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = 1700000000 + i * 100;
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    let (logs, _) = store.query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query logs");

    // Should be ordered by created_at DESC (newest first)
    assert!(logs[0].created_at > logs[1].created_at);
    assert!(logs[1].created_at > logs[2].created_at);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_logs_with_invalid_page() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert test data
    let log = create_test_log(1, "trace-1", "gpt-4");
    store.insert_request_log(&log).expect("Failed to insert log");

    // Page 0 should be treated as page 1
    let (logs, total) = store.query_logs(0, 2000000000, 0, 10)
        .expect("Failed to query with page 0");
    assert_eq!(logs.len(), 1);
    assert_eq!(total, 1);

    // Negative page should be treated as page 1
    let (logs2, _) = store.query_logs(0, 2000000000, -1, 10)
        .expect("Failed to query with negative page");
    assert_eq!(logs2.len(), 1);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_logs_empty_result() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Query empty database
    let (logs, total) = store.query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query empty database");

    assert_eq!(logs.len(), 0);
    assert_eq!(total, 0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_rollup_old_logs() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    let now = chrono::Local::now().timestamp();

    // Insert old logs (>30 days ago)
    for i in 1..=5 {
        let mut log = create_test_log(i, &format!("old-trace-{}", i), "gpt-4");
        log.created_at = now - 31 * 86400 + i * 3600;
        store.insert_request_log(&log).expect("Failed to insert old log");
    }

    // Insert recent logs (<30 days ago)
    for i in 1..=3 {
        let mut log = create_test_log(i + 10, &format!("recent-trace-{}", i), "gpt-4");
        log.created_at = now - 10 * 86400 + i * 3600;
        store.insert_request_log(&log).expect("Failed to insert recent log");
    }

    let deleted = store.rollup_old_logs().expect("Failed to rollup old logs");

    // Should delete 5 old logs
    assert_eq!(deleted, 5);

    // Verify recent logs still exist
    let (recent_logs, total) = store.query_logs(now - 20 * 86400, now + 86400, 1, 100)
        .expect("Failed to query recent logs");
    assert_eq!(total, 3);
    assert_eq!(recent_logs.len(), 3);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_rollup_old_logs_when_no_old_logs() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    let now = chrono::Local::now().timestamp();

    // Insert only recent logs
    for i in 1..=3 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = now - 10 * 86400 + i * 3600;
        store.insert_request_log(&log).expect("Failed to insert recent log");
    }

    let deleted = store.rollup_old_logs().expect("Failed to rollup old logs");

    // Should delete 0 logs
    assert_eq!(deleted, 0);

    // Verify all logs still exist
    let (logs, total) = store.query_logs(now - 20 * 86400, now + 86400, 1, 100)
        .expect("Failed to query logs");
    assert_eq!(total, 3);
    assert_eq!(logs.len(), 3);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_sequential_insertion_stress() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert many logs sequentially to test performance and stability
    for i in 0..100 {
        let log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    // Verify all logs were inserted
    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");
    assert_eq!(summary.total_requests, 100);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_insert_log_with_streaming_flag() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert streaming request
    let mut streaming_log = create_test_log(1, "trace-streaming", "gpt-4");
    streaming_log.is_streaming = true;
    store.insert_request_log(&streaming_log).expect("Failed to insert streaming log");

    // Insert non-streaming request
    let mut non_streaming_log = create_test_log(2, "trace-non-streaming", "gpt-4");
    non_streaming_log.is_streaming = false;
    store.insert_request_log(&non_streaming_log).expect("Failed to insert non-streaming log");

    let (logs, _) = store.query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query logs");

    assert_eq!(logs.len(), 2);
    assert!(logs.iter().any(|log| log.is_streaming));
    assert!(logs.iter().any(|log| !log.is_streaming));

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_insert_log_with_error_message() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    let mut error_log = create_test_log(1, "trace-error", "gpt-4");
    error_log.status_code = 429;
    error_log.error_message = Some("Rate limit exceeded".to_string());
    store.insert_request_log(&error_log).expect("Failed to insert error log");

    let (logs, _) = store.query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query logs");

    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].error_message, Some("Rate limit exceeded".to_string()));
    assert_eq!(logs[0].status_code, 429);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_summary_with_no_matching_data() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert test data
    let mut log = create_test_log(1, "trace-1", "gpt-4");
    log.created_at = 1700000000;
    store.insert_request_log(&log).expect("Failed to insert log");

    // Query non-overlapping time range
    let summary = store.query_summary(1800000000, 1900000000)
        .expect("Failed to query summary with no data");

    assert_eq!(summary.total_requests, 0);
    assert_eq!(summary.total_input_tokens, 0);
    assert_eq!(summary.total_cost_usd, 0.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_query_summary_aggregation_accuracy() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert multiple logs with known values
    let log1 = create_test_log(1, "trace-1", "gpt-4");
    let log2 = create_test_log(2, "trace-2", "gpt-4");
    let log3 = create_test_log(3, "trace-3", "gpt-4");

    store.insert_request_log(&log1).expect("Failed to insert log1");
    store.insert_request_log(&log2).expect("Failed to insert log2");
    store.insert_request_log(&log3).expect("Failed to insert log3");

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");

    // Verify aggregations
    assert_eq!(summary.total_requests, 3);
    assert_eq!(summary.total_input_tokens, 101 + 102 + 103);
    assert_eq!(summary.total_output_tokens, 51 + 52 + 53);
    assert_eq!(summary.total_cache_creation_tokens, 11 + 12 + 13);
    assert_eq!(summary.total_cache_read_tokens, 6 + 7 + 8);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_multiple_models_in_trends() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs for different models
    for i in 0..4 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.created_at = 1700000000 + i * 3600;
        store.insert_request_log(&log).expect("Failed to insert gpt-4 log");
    }

    for i in 0..3 {
        let mut log = create_test_log(i + 10, &format!("trace-{}", i + 10), "gpt-3.5");
        log.created_at = 1700000000 + i * 3600;
        store.insert_request_log(&log).expect("Failed to insert gpt-3.5 log");
    }

    let trends = store.query_trends(1700000000, 1700000000 + 5 * 3600, "hour")
        .expect("Failed to query trends");

    // Verify trends aggregate across all models
    assert!(trends.len() >= 4);
    let total_requests: i64 = trends.iter().map(|t| t.request_count).sum();
    assert!(total_requests >= 4); // At least the gpt-4 requests

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_trend_data_format() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert a log at a specific time
    let mut log = create_test_log(1, "trace-1", "gpt-4");
    log.created_at = 1704067200; // 2024-01-01 00:00:00 UTC
    store.insert_request_log(&log).expect("Failed to insert log");

    // Query hourly trends
    let trends = store.query_trends(log.created_at - 3600, log.created_at + 3600, "hour")
        .expect("Failed to query hourly trends");

    assert_eq!(trends.len(), 1);
    let trend = &trends[0];

    // Verify trend point structure
    assert!(!trend.label.is_empty());
    assert!(trend.timestamp > 0);
    assert!(trend.input_tokens > 0);
    assert!(trend.output_tokens > 0);
    assert!(trend.request_count > 0);
    assert!(trend.total_cost_usd > 0.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_avg_latency_calculation() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert requests with different latencies
    let mut log1 = create_test_log(1, "trace-1", "gpt-4");
    log1.latency_ms = 100;
    log1.status_code = 200;

    let mut log2 = create_test_log(2, "trace-2", "gpt-4");
    log2.latency_ms = 200;
    log2.status_code = 200;

    let mut log3 = create_test_log(3, "trace-3", "gpt-4");
    log3.latency_ms = 300;
    log3.status_code = 500; // Failed request

    store.insert_request_log(&log1).expect("Failed to insert log1");
    store.insert_request_log(&log2).expect("Failed to insert log2");
    store.insert_request_log(&log3).expect("Failed to insert log3");

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");

    // Average latency should only consider successful requests (200 status)
    // (100 + 200) / 2 = 150
    assert_eq!(summary.avg_latency_ms, 150.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_large_cost_aggregation() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert logs with varying costs
    for i in 1..=100 {
        let mut log = create_test_log(i, &format!("trace-{}", i), "gpt-4");
        log.total_cost_usd = 0.01 * i as f64; // 0.01 to 1.00
        store.insert_request_log(&log).expect("Failed to insert log");
    }

    let summary = store.query_summary(0, 2000000000).expect("Failed to query summary");

    // Sum of 0.01, 0.02, ..., 1.00 = 0.01 * (1 + 2 + ... + 100) = 0.01 * 5050 = 50.50
    assert!((summary.total_cost_usd - 50.50).abs() < 0.01);

    let _ = fs::remove_file(&db_path);
}
