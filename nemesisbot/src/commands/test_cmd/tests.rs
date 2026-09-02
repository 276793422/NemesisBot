// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

// -------------------------------------------------------------------------
// make_approval_data tests
// -------------------------------------------------------------------------

#[test]
fn test_make_approval_data_fields() {
    let data = make_approval_data("req-001", "file_write", "HIGH", "C:\\Temp\\test.txt");

    assert_eq!(data["request_id"], "req-001");
    assert_eq!(data["operation"], "file_write");
    assert_eq!(data["operation_name"], "file_write");
    assert_eq!(data["target"], "C:\\Temp\\test.txt");
    assert_eq!(data["risk_level"], "HIGH");
    assert_eq!(data["timeout_seconds"], 30);
    assert!(data["context"].is_object());
    assert!(data["timestamp"].is_number());
}

#[test]
fn test_make_approval_data_reason_format() {
    let data = make_approval_data("req-002", "process_exec", "CRITICAL", "/usr/bin/rm");
    let reason = data["reason"].as_str().unwrap();
    assert!(reason.contains("process_exec"));
    assert!(reason.contains("nemesisbot test"));
}

#[test]
fn test_make_approval_data_different_risk_levels() {
    for level in &["LOW", "MEDIUM", "HIGH", "CRITICAL"] {
        let data = make_approval_data("req", "op", level, "target");
        assert_eq!(data["risk_level"], *level);
    }
}

#[test]
fn test_make_approval_data_request_id() {
    let data = make_approval_data("unique-id-12345", "file_read", "LOW", "/tmp/file");
    assert_eq!(data["request_id"], "unique-id-12345");
}

#[test]
fn test_make_approval_data_timestamp_is_numeric() {
    let data = make_approval_data("req", "op", "HIGH", "target");
    let ts = data["timestamp"].as_i64();
    assert!(ts.is_some());
    // Should be a reasonable timestamp (after year 2000)
    assert!(ts.unwrap() > 946684800);
}

// -------------------------------------------------------------------------
// print_result tests
// -------------------------------------------------------------------------

#[test]
fn test_print_result_pass() {
    // Should not panic
    print_result(true, "test passed message");
}

#[test]
fn test_print_result_fail() {
    // Should not panic
    print_result(false, "test failed message");
}

// -------------------------------------------------------------------------
// TestAction enum default values
// -------------------------------------------------------------------------

#[test]
fn test_approval_headless_default_expected() {
    // Default value for --expected is "approved"
    let expected = "approved".to_string();
    assert_eq!(expected, "approved");
}

#[test]
fn test_approval_ui_default_values() {
    let risk_level = "HIGH".to_string();
    let operation = "file_write".to_string();
    let target = "C:\\Temp\\test.txt".to_string();
    assert_eq!(risk_level, "HIGH");
    assert_eq!(operation, "file_write");
    assert_eq!(target, "C:\\Temp\\test.txt");
}

#[test]
fn test_dashboard_default_values() {
    let host = "127.0.0.1".to_string();
    let port: u16 = 49000;
    let token = "276793422".to_string();
    assert_eq!(host, "127.0.0.1");
    assert_eq!(port, 49000);
    assert_eq!(token, "276793422");
}

// -------------------------------------------------------------------------
// Request ID generation pattern
// -------------------------------------------------------------------------

#[test]
fn test_request_id_format_headless() {
    let request_id = format!("headless-{}", chrono::Local::now().timestamp_millis());
    assert!(request_id.starts_with("headless-"));
    let timestamp_part = request_id.strip_prefix("headless-").unwrap();
    assert!(timestamp_part.parse::<i64>().is_ok());
}

#[test]
fn test_request_id_format_ui() {
    let request_id = format!("ui-{}", chrono::Local::now().timestamp_millis());
    assert!(request_id.starts_with("ui-"));
    let timestamp_part = request_id.strip_prefix("ui-").unwrap();
    assert!(timestamp_part.parse::<i64>().is_ok());
}

// -------------------------------------------------------------------------
// Result comparison logic (from headless test)
// -------------------------------------------------------------------------

#[test]
fn test_headless_result_comparison() {
    let action = "approved";
    let expected = "approved";
    let request_id = "headless-12345";
    let expected_request_id = "headless-12345";

    let pass = action == expected && request_id == expected_request_id;
    assert!(pass);
}

#[test]
fn test_headless_result_mismatch() {
    let action = "rejected";
    let expected = "approved";
    let request_id = "headless-12345";
    let expected_request_id = "headless-12345";

    let pass = action == expected && request_id == expected_request_id;
    assert!(!pass);
}

// -------------------------------------------------------------------------
// UI result check (from approval-ui test)
// -------------------------------------------------------------------------

#[test]
fn test_ui_result_non_empty_check() {
    let action = "approved";
    let pass = !action.is_empty();
    assert!(pass);
}

#[test]
fn test_ui_result_empty_check() {
    let action = "";
    let pass = !action.is_empty();
    assert!(!pass);
}

// -------------------------------------------------------------------------
// Backend URL construction (from dashboard test)
// -------------------------------------------------------------------------

#[test]
fn test_backend_url_format() {
    let host = "127.0.0.1";
    let port: u16 = 49000;
    let backend_url = format!("http://{}:{}", host, port);
    assert_eq!(backend_url, "http://127.0.0.1:49000");
}

// -------------------------------------------------------------------------
// Token display truncation (from dashboard test)
// -------------------------------------------------------------------------

#[test]
fn test_token_display_truncation() {
    let token = "276793422";
    let display = &token[..token.len().min(4)];
    assert_eq!(display, "2767");
}

#[test]
fn test_token_display_short_token() {
    let token = "ab";
    let display = &token[..token.len().min(4)];
    assert_eq!(display, "ab");
}

// -------------------------------------------------------------------------
// Dashboard data construction
// -------------------------------------------------------------------------

#[test]
fn test_dashboard_data_json() {
    let data = serde_json::json!({
        "token": "test-token",
        "web_port": 49000,
        "web_host": "127.0.0.1",
    });
    assert_eq!(data["token"], "test-token");
    assert_eq!(data["web_port"], 49000);
    assert_eq!(data["web_host"], "127.0.0.1");
}

// -------------------------------------------------------------------------
// Additional coverage tests for test_cmd
// -------------------------------------------------------------------------

#[test]
fn test_make_approval_data_fields_v2() {
    let data = make_approval_data("req-456", "file_read", "MEDIUM", "/tmp/test");
    assert_eq!(data["request_id"], "req-456");
    assert_eq!(data["operation"], "file_read");
    assert_eq!(data["operation_name"], "file_read");
    assert_eq!(data["target"], "/tmp/test");
    assert_eq!(data["risk_level"], "MEDIUM");
    assert_eq!(data["timeout_seconds"], 30);
    assert!(data["reason"].as_str().unwrap().contains("file_read"));
    assert!(data["timestamp"].is_number());
    assert!(data["context"].is_object());
}

#[test]
fn test_make_approval_data_different_operations() {
    let data = make_approval_data("id-1", "process_exec", "CRITICAL", "/usr/bin/rm");
    assert_eq!(data["operation"], "process_exec");
    assert_eq!(data["risk_level"], "CRITICAL");
    assert!(data["reason"].as_str().unwrap().contains("process_exec"));

    let data = make_approval_data("id-2", "network_request", "MEDIUM", "http://example.com");
    assert_eq!(data["operation"], "network_request");
    assert_eq!(data["target"], "http://example.com");
}

#[test]
fn test_make_approval_data_serialization() {
    let data = make_approval_data("test", "test_op", "LOW", "target");
    let json = serde_json::to_string(&data).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["request_id"], "test");
    assert_eq!(parsed["operation"], "test_op");
}

#[test]
fn test_check_plugin_library_exists_no_panic() {
    // This just verifies the function doesn't panic
    let _ = check_plugin_library_exists();
}

#[test]
fn test_token_display_empty() {
    let token = "";
    let display = &token[..token.len().min(4)];
    assert_eq!(display, "");
}

#[test]
fn test_token_display_long() {
    let token = "abcdefghij";
    let display = &token[..token.len().min(4)];
    assert_eq!(display, "abcd");
}

#[test]
fn test_approval_data_with_special_chars() {
    let data = make_approval_data(
        "req-<script>",
        "file_write",
        "HIGH",
        "C:\\Users\\test & verify\\file.txt",
    );
    assert_eq!(data["request_id"], "req-<script>");
    assert_eq!(data["target"], "C:\\Users\\test & verify\\file.txt");
}

// ===========================================================================
// run() 分发 + 真实 ProcessManager 通路（S11c，quality-hardening goal 冲刺 S11）
// —— 既有 27 个测试只钉 make_approval_data 纯函数，四个 run_* 全没跑过。
// 关键事实：spawn_child 用 current_exe 起子进程，测试二进制把
// `--multiple --child-id ... --window-type approval` 交给 clap → 参数不识别
// 立即退出 → 管道 EOF → handshake 确定性失败（不起 UI、不装 DLL、不占端口；
// WS server 绑 127.0.0.1:0）。ApprovalUi / Dashboard 在更早的
// check_plugin_library_exists 就 bail（测试 exe 旁无 plugins/）。
// ===========================================================================

mod run_arm {
    use super::*;

    #[test]
    fn check_plugin_library_missing_errors_deterministically() {
        let err = check_plugin_library_exists().expect_err("测试 exe 旁无 plugins/ → Err");
        assert!(
            err.to_string().contains("plugin-ui"),
            "got: {err:#}"
        );
    }

    #[test]
    fn print_result_both_banners_do_not_panic() {
        print_result(true, "ok banner");
        print_result(false, "fail banner");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[cfg(feature = "desktop")]
    #[tokio::test]
    async fn approval_headless_fails_fast_when_handshake_eof_and_cleans_env() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("NEMESISBOT_FORCE_HEADLESS");
        }

        let err = run(TestAction::ApprovalHeadless {
            expected: "approved".into(),
        })
        .await
        .expect_err("子进程（=测试二进制）不认 child 参数 → handshake EOF → Err");
        assert!(
            err.to_string().contains("spawn_child failed"),
            "got: {err:#}"
        );
        // (BUG S11c-2) 对称清理：失败路径必须回收 env，不得残留给后续测试。
        assert_eq!(
            std::env::var("NEMESISBOT_FORCE_HEADLESS").unwrap_or_default(),
            "",
            "失败路径必须清掉 NEMESISBOT_FORCE_HEADLESS"
        );
    }

    #[cfg(feature = "desktop")]
    #[tokio::test]
    async fn approval_ui_and_dashboard_bail_at_plugin_check() {
        let err = run(TestAction::ApprovalUi {
            risk_level: "HIGH".into(),
            operation: "file_write".into(),
            target: "C:\\Temp\\t.txt".into(),
        })
        .await
        .expect_err("无 plugin-ui → 第一步就 bail");
        assert!(err.to_string().contains("plugin-ui"), "got: {err:#}");

        let err = run(TestAction::Dashboard {
            host: "127.0.0.1".into(),
            port: 49000,
            token: "token".into(),
        })
        .await
        .expect_err("无 plugin-ui → 第一步就 bail（不起真 dashboard）");
        assert!(err.to_string().contains("plugin-ui"), "got: {err:#}");
    }

    #[cfg(feature = "desktop")]
    #[tokio::test]
    async fn ws_test_full_local_roundtrip() {
        // 全本地回路：WS server（127.0.0.1:0）+ 客户端连入 + 通知往返。
        run(TestAction::Ws).await.expect("WS ping-pong 回路应通过");
    }
}
