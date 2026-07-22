//! Unit tests for nemesis-data crate.
//!
//! These tests verify edge cases and internal behavior.

use nemesis_data::{DataStore, RequestLog};
use std::fs;
use std::path::PathBuf;

/// Create a temporary database path for testing.
fn temp_db_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    let uuid = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    path.push(format!("nemesis_data_unit_test_{}.db", uuid));
    // Ensure file doesn't exist
    let _ = fs::remove_file(&path);
    path
}

#[test]
fn test_empty_database_summary() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Query summary on empty database should return zero values
    let summary = store
        .query_summary(0, 2000000000)
        .expect("Failed to query summary from empty database");

    assert_eq!(summary.total_requests, 0);
    assert_eq!(summary.success_count, 0);
    assert_eq!(summary.total_input_tokens, 0);
    assert_eq!(summary.total_output_tokens, 0);
    assert_eq!(summary.total_cache_creation_tokens, 0);
    assert_eq!(summary.total_cache_read_tokens, 0);
    assert_eq!(summary.total_cost_usd, 0.0);
    assert_eq!(summary.avg_latency_ms, 0.0);
    assert_eq!(summary.cache_hit_rate, 0.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_database_reuse() {
    let db_path = temp_db_path();

    // Create and populate database
    {
        let store = DataStore::open(&db_path).expect("Failed to open database");
        let log = RequestLog {
            id: 1,
            trace_id: "test-trace".to_string(),
            model: "gpt-4".to_string(),
            provider_type: "test".to_string(),
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_tokens: 10,
            cache_read_tokens: 5,
            total_cost_usd: 0.01,
            latency_ms: 100,
            status_code: 200,
            error_message: None,
            is_streaming: false,
            created_at: 1700000000,
        };
        store
            .insert_request_log(&log)
            .expect("Failed to insert log");
    }

    // Reopen database and verify data persists
    {
        let store = DataStore::open(&db_path).expect("Failed to reopen database");
        let summary = store
            .query_summary(0, 2000000000)
            .expect("Failed to query summary");
        assert_eq!(summary.total_requests, 1);
    }

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_zero_values_in_log() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert log with zero values
    let log = RequestLog {
        id: 1,
        trace_id: "zero-test".to_string(),
        model: "gpt-4".to_string(),
        provider_type: "test".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: 0.0,
        latency_ms: 0,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 1700000000,
    };
    store
        .insert_request_log(&log)
        .expect("Failed to insert zero-value log");

    let summary = store
        .query_summary(0, 2000000000)
        .expect("Failed to query summary");
    assert_eq!(summary.total_requests, 1);
    assert_eq!(summary.total_input_tokens, 0);
    assert_eq!(summary.total_output_tokens, 0);
    assert_eq!(summary.total_cost_usd, 0.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_negative_timestamps() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert log with negative timestamp (before Unix epoch)
    let mut log = RequestLog {
        id: 1,
        trace_id: "negative-ts".to_string(),
        model: "gpt-4".to_string(),
        provider_type: "test".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        total_cost_usd: 0.01,
        latency_ms: 100,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: -1000000, // Negative timestamp
    };
    store
        .insert_request_log(&log)
        .expect("Failed to insert log with negative timestamp");

    // Query with negative time range
    let summary = store
        .query_summary(-2000000, 0)
        .expect("Failed to query with negative timestamps");
    assert_eq!(summary.total_requests, 1);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_large_request_log() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert log with very large values
    let log = RequestLog {
        id: 1,
        trace_id: "large-values".to_string(),
        model: "gpt-4".to_string(),
        provider_type: "test".to_string(),
        input_tokens: i64::MAX / 2,
        output_tokens: i64::MAX / 2,
        cache_creation_tokens: i64::MAX / 4,
        cache_read_tokens: i64::MAX / 4,
        total_cost_usd: 1000000.0,
        latency_ms: i64::MAX / 1000,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: 1700000000,
    };
    store
        .insert_request_log(&log)
        .expect("Failed to insert large-value log");

    let summary = store
        .query_summary(0, 2000000000)
        .expect("Failed to query summary with large values");
    assert_eq!(summary.total_requests, 1);
    assert!(summary.total_input_tokens > 0);
    assert!(summary.total_cost_usd > 0.0);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_special_characters_in_strings() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert log with special characters
    let log = RequestLog {
        id: 1,
        trace_id: "trace-with-'quotes'-and-\"double-quotes\"".to_string(),
        model: "model-with-emoji-🚀".to_string(),
        provider_type: "provider\nwith\nnewlines".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        total_cost_usd: 0.01,
        latency_ms: 100,
        status_code: 200,
        error_message: Some("Error: \n\t\"special\" chars".to_string()),
        is_streaming: false,
        created_at: 1700000000,
    };
    store
        .insert_request_log(&log)
        .expect("Failed to insert log with special characters");

    let (logs, _) = store
        .query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query logs");
    assert_eq!(logs.len(), 1);
    assert!(logs[0].trace_id.contains("'quotes'"));
    assert!(logs[0].model.contains("emoji"));

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_very_long_strings() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Create very long strings
    let long_trace = "a".repeat(10000);
    let long_error = "e".repeat(5000);

    let log = RequestLog {
        id: 1,
        trace_id: long_trace,
        model: "gpt-4".to_string(),
        provider_type: "test".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        total_cost_usd: 0.01,
        latency_ms: 100,
        status_code: 500,
        error_message: Some(long_error),
        is_streaming: false,
        created_at: 1700000000,
    };
    store
        .insert_request_log(&log)
        .expect("Failed to insert log with long strings");

    let (logs, _) = store
        .query_logs(0, 2000000000, 1, 10)
        .expect("Failed to query logs");
    assert_eq!(logs.len(), 1);
    assert!(logs[0].trace_id.len() > 9000);
    assert!(logs[0].error_message.as_ref().unwrap().len() > 4000);

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_multiple_page_sizes() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    // Insert exactly 50 logs
    for i in 1..=50 {
        let log = RequestLog {
            id: i,
            trace_id: format!("trace-{}", i),
            model: "gpt-4".to_string(),
            provider_type: "test".to_string(),
            input_tokens: i,
            output_tokens: i,
            cache_creation_tokens: i,
            cache_read_tokens: i,
            total_cost_usd: 0.01,
            latency_ms: 100,
            status_code: 200,
            error_message: None,
            is_streaming: false,
            created_at: 1700000000 + i,
        };
        store
            .insert_request_log(&log)
            .expect("Failed to insert log");
    }

    // Test different page sizes
    let (page1, total) = store
        .query_logs(0, 2000000000, 1, 7)
        .expect("Failed to query with page_size=7");
    assert_eq!(page1.len(), 7);
    assert_eq!(total, 50);

    let (page2, _) = store
        .query_logs(0, 2000000000, 1, 13)
        .expect("Failed to query with page_size=13");
    assert_eq!(page2.len(), 13);

    let (page3, _) = store
        .query_logs(0, 2000000000, 1, 100)
        .expect("Failed to query with page_size=100");
    assert_eq!(page3.len(), 50); // All logs

    let _ = fs::remove_file(&db_path);
}

#[test]
fn test_exact_boundary_rollup() {
    let db_path = temp_db_path();
    let store = DataStore::open(&db_path).expect("Failed to open database");

    let now = chrono::Local::now().timestamp();

    // Insert very old log (>30 days ago)
    let mut old_log = RequestLog {
        id: 1,
        trace_id: "very-old".to_string(),
        model: "gpt-4".to_string(),
        provider_type: "test".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        total_cost_usd: 0.01,
        latency_ms: 100,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: now - 31 * 86400, // 31 days ago
    };
    store
        .insert_request_log(&old_log)
        .expect("Failed to insert old log");

    // Insert recent log (should not be rolled up)
    let mut recent_log = RequestLog {
        id: 2,
        trace_id: "recent".to_string(),
        model: "gpt-4".to_string(),
        provider_type: "test".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: 10,
        cache_read_tokens: 5,
        total_cost_usd: 0.01,
        latency_ms: 100,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: now - 10 * 86400, // 10 days ago
    };
    store
        .insert_request_log(&recent_log)
        .expect("Failed to insert recent log");

    let deleted = store.rollup_old_logs().expect("Failed to rollup");
    assert_eq!(deleted, 1); // Only the old log should be deleted

    let (remaining, _) = store
        .query_logs(0, now + 86400, 1, 100)
        .expect("Failed to query remaining logs");
    assert_eq!(remaining.len(), 1); // Only the recent log should remain
    assert_eq!(remaining[0].trace_id, "recent");

    let _ = fs::remove_file(&db_path);
}
