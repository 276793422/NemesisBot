use super::*;
use std::io::Cursor;

#[test]
fn test_has_child_mode_flag() {
    // Test runner doesn't pass --multiple, so should be false
    assert!(!has_child_mode_flag());
}

#[test]
fn test_child_handshake_success() {
    // Simulate parent sending handshake, child reading it
    let parent_msg = r#"{"type":"handshake","version":"1.0","data":{"protocol":"anon-pipe-v1","version":"1.0"}}"#;
    let mut input = Cursor::new(parent_msg.to_string());
    let mut output = Vec::new();

    let result = child_handshake(&mut input, &mut output).unwrap();
    assert!(result.success);

    // Verify ACK was written
    let output_str = String::from_utf8(output).unwrap();
    let ack: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
    assert_eq!(ack.msg_type, "ack");
}

#[test]
fn test_child_handshake_wrong_type() {
    let parent_msg = r#"{"type":"ws_key","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(parent_msg.to_string());
    let mut output = Vec::new();

    let result = child_handshake(&mut input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected handshake"));
}

#[test]
fn test_parent_handshake_success() {
    // Parent writes handshake, then reads ACK
    let ack_response = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
    let mut input = Cursor::new(ack_response.to_string());
    let mut output = Vec::new();

    let result = parent_handshake(&mut output, &mut input).unwrap();
    assert!(result.success);

    // Verify handshake was written
    let output_str = String::from_utf8(output).unwrap();
    let hs: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
    assert_eq!(hs.msg_type, "handshake");
}

#[test]
fn test_receive_ws_key() {
    let ws_msg =
        r#"{"type":"ws_key","version":"1.0","data":{"key":"abc123","port":8080,"path":"/ws"}}"#;
    let mut input = Cursor::new(ws_msg.to_string());
    let mut output = Vec::new();

    let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
    assert_eq!(key, "abc123");
    assert_eq!(port, 8080);
    assert_eq!(path, "/ws");

    // Verify ACK was written
    let output_str = String::from_utf8(output).unwrap();
    let ack: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
    assert!(ack.is_ack());
}

#[test]
fn test_send_ws_key() {
    let ack_response = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
    let mut input = Cursor::new(ack_response.to_string());
    let mut output = Vec::new();

    send_ws_key(&mut output, &mut input, "test-key", 9090, "/api").unwrap();

    let output_str = String::from_utf8(output).unwrap();
    let msg: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
    assert!(msg.is_ws_key());
    assert_eq!(msg.data["key"], serde_json::json!("test-key"));
    assert_eq!(msg.data["port"], serde_json::json!(9090));
}

#[test]
fn test_receive_window_data() {
    let wd_msg = r#"{"type":"window_data","version":"1.0","data":{"data":{"request_id":"r1","operation":"file_write","operation_name":"Write File","target":"test.txt","risk_level":"HIGH","reason":"test","timeout_seconds":30,"context":{},"timestamp":1234567890}}}"#;
    let mut input = Cursor::new(wd_msg.to_string());
    let mut output = Vec::new();

    let data = receive_window_data(&mut input, &mut output).unwrap();
    assert_eq!(data["request_id"], "r1");
    assert_eq!(data["risk_level"], "HIGH");

    // Verify ACK
    let output_str = String::from_utf8(output).unwrap();
    let ack: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
    assert!(ack.is_ack());
}

#[test]
fn test_send_window_data() {
    let ack_response = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
    let mut input = Cursor::new(ack_response.to_string());
    let mut output = Vec::new();

    let data = serde_json::json!({"title": "Test Window"});
    send_window_data(&mut output, &mut input, &data).unwrap();

    let output_str = String::from_utf8(output).unwrap();
    let msg: PipeMessage = serde_json::from_str(output_str.trim()).unwrap();
    assert!(msg.is_window_data());
}

#[test]
fn test_full_handshake_flow() {
    // Simulate full parent-child handshake flow:
    // Parent writes handshake → Child reads handshake → Child writes ACK → Parent reads ACK
    let mut parent_to_child = Vec::new();
    let mut child_to_parent = Vec::new();

    // Parent sends handshake
    {
        let mut writer = PipeWriter::new(&mut parent_to_child);
        writer.write_message(&PipeMessage::handshake()).unwrap();
    }

    // Child receives handshake and sends ACK
    {
        let mut reader = PipeReader::new(Cursor::new(
            String::from_utf8(parent_to_child.clone()).unwrap(),
        ));
        let mut writer = PipeWriter::new(&mut child_to_parent);
        let msg = reader.read_message().unwrap();
        assert!(msg.is_handshake());
        writer.write_message(&PipeMessage::ack()).unwrap();
    }

    // Parent reads ACK
    {
        let mut reader = PipeReader::new(Cursor::new(
            String::from_utf8(child_to_parent.clone()).unwrap(),
        ));
        let ack = reader.read_message().unwrap();
        assert!(ack.is_ack());
    }
}

#[test]
fn test_full_ws_key_exchange() {
    let mut parent_to_child = Vec::new();
    let mut child_to_parent = Vec::new();

    // Parent sends ws_key
    {
        let mut writer = PipeWriter::new(&mut parent_to_child);
        writer
            .write_message(&PipeMessage::ws_key("my-key", 8080, "/ws"))
            .unwrap();
    }

    // Child receives ws_key and sends ACK
    {
        let mut reader = PipeReader::new(Cursor::new(
            String::from_utf8(parent_to_child.clone()).unwrap(),
        ));
        let mut writer = PipeWriter::new(&mut child_to_parent);
        let msg = reader.read_message().unwrap();
        assert!(msg.is_ws_key());
        assert_eq!(msg.data["key"], serde_json::json!("my-key"));
        writer.write_message(&PipeMessage::ack()).unwrap();
    }

    // Parent reads ACK
    {
        let mut reader = PipeReader::new(Cursor::new(
            String::from_utf8(child_to_parent.clone()).unwrap(),
        ));
        let ack = reader.read_message().unwrap();
        assert!(ack.is_ack());
    }
}

#[test]
fn test_approval_window_data_serde() {
    let data = ApprovalWindowData {
        request_id: "r1".to_string(),
        operation: "file_write".to_string(),
        operation_name: "Write File".to_string(),
        target: "test.txt".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test reason".to_string(),
        timeout_seconds: 30,
        context: HashMap::new(),
        timestamp: 1234567890,
    };
    let json = serde_json::to_string(&data).unwrap();
    let parsed: ApprovalWindowData = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.request_id, "r1");
    assert_eq!(parsed.risk_level, "HIGH");
}

#[test]
fn test_dashboard_window_data_serde() {
    let data = DashboardWindowData {
        token: "tok123".to_string(),
        web_port: 8080,
        web_host: "0.0.0.0".to_string(),
    };
    let json = serde_json::to_string(&data).unwrap();
    let parsed: DashboardWindowData = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.token, "tok123");
    assert_eq!(parsed.web_port, 8080);
}

#[test]
fn test_pipe_message_roundtrip() {
    let msg = PipeMessage::handshake();
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PipeMessage = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_handshake());
    assert_eq!(parsed.version, "1.0");
}

#[test]
fn test_pipe_reader_empty_input() {
    let input = Cursor::new(String::new());
    let mut reader = PipeReader::new(input);
    let result = reader.read_message();
    assert!(result.is_err());
}

#[test]
fn test_pipe_reader_empty_line() {
    let input = Cursor::new("\n\n".to_string());
    let mut reader = PipeReader::new(input);
    let result = reader.read_message();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty message"));
}

#[test]
fn test_pipe_reader_invalid_json() {
    let input = Cursor::new("not json\n".to_string());
    let mut reader = PipeReader::new(input);
    let result = reader.read_message();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("pipe parse"));
}

#[test]
fn test_pipe_writer_writes_json() {
    let mut output = Vec::new();
    let mut writer = PipeWriter::new(&mut output);
    writer.write_message(&PipeMessage::ack()).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("ack"));
    assert!(output_str.ends_with('\n'));
}

#[test]
fn test_pipe_writer_multiple_messages() {
    let mut output = Vec::new();
    let mut writer = PipeWriter::new(&mut output);
    writer.write_message(&PipeMessage::handshake()).unwrap();
    writer.write_message(&PipeMessage::ack()).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    let lines: Vec<&str> = output_str.lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn test_get_child_id_not_set() {
    // Test runner doesn't pass --child-id, so should be None
    assert!(get_child_id().is_none());
}

#[test]
fn test_get_window_type_not_set() {
    // Test runner doesn't pass --window-type, so should be None
    assert!(get_window_type().is_none());
}

#[test]
fn test_child_handshake_eof() {
    let mut input = Cursor::new(String::new());
    let mut output = Vec::new();
    let result = child_handshake(&mut input, &mut output);
    assert!(result.is_err());
}

#[test]
fn test_parent_handshake_wrong_response() {
    let wrong_response = r#"{"type":"handshake","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(wrong_response.to_string());
    let mut output = Vec::new();
    let result = parent_handshake(&mut output, &mut input);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected ack"));
}

#[test]
fn test_receive_ws_key_wrong_type() {
    let wrong_msg = r#"{"type":"handshake","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(wrong_msg.to_string());
    let mut output = Vec::new();
    let result = receive_ws_key(&mut input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected ws_key"));
}

#[test]
fn test_receive_ws_key_defaults() {
    let ws_msg = r#"{"type":"ws_key","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(ws_msg.to_string());
    let mut output = Vec::new();
    let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
    assert_eq!(key, "");
    assert_eq!(port, 0);
    assert_eq!(path, "");
}

#[test]
fn test_receive_window_data_wrong_type() {
    let wrong_msg = r#"{"type":"handshake","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(wrong_msg.to_string());
    let mut output = Vec::new();
    let result = receive_window_data(&mut input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected window_data"));
}

#[test]
fn test_receive_window_data_missing_data_field() {
    let msg = r#"{"type":"window_data","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(msg.to_string());
    let mut output = Vec::new();
    let result = receive_window_data(&mut input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("missing data field"));
}

#[test]
fn test_approval_window_data_with_context() {
    let mut context = HashMap::new();
    context.insert("user".to_string(), "alice".to_string());
    context.insert("channel".to_string(), "web".to_string());
    let data = ApprovalWindowData {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        operation_name: "Write".to_string(),
        target: "/tmp/test.txt".to_string(),
        risk_level: "MEDIUM".to_string(),
        reason: "user request".to_string(),
        timeout_seconds: 60,
        context,
        timestamp: 1700000000,
    };
    let json = serde_json::to_string(&data).unwrap();
    let parsed: ApprovalWindowData = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.context.get("user").unwrap(), "alice");
    assert_eq!(parsed.context.get("channel").unwrap(), "web");
}

#[test]
fn test_run_window_unknown_type() {
    let data = serde_json::json!({});
    let result = run_window(
        "child-1",
        "unknown_type",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown window type"));
}

#[test]
fn test_run_window_approval() {
    let data = serde_json::json!({
        "request_id": "r1",
        "operation": "file_write",
        "operation_name": "Write",
        "target": "test.txt",
        "risk_level": "HIGH",
        "reason": "test",
        "timeout_seconds": 30,
        "context": {},
        "timestamp": 1234567890
    });
    let result = run_window(
        "child-1",
        "approval",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    // Without plugin-ui.dll, expect "not found" error
    // With plugin-ui.dll, expect Ok(()) or a runtime error from the DLL
    match result {
        Ok(()) => {}
        Err(e) => assert!(
            e.contains("plugin") || e.contains("not found") || e.contains("DLL"),
            "unexpected error: {}",
            e
        ),
    }
}

#[test]
fn test_run_window_headless() {
    let data = serde_json::json!({
        "request_id": "r2",
        "operation": "file_read",
        "operation_name": "Read",
        "target": "test.txt",
        "risk_level": "LOW",
        "reason": "auto",
        "timeout_seconds": 10,
        "context": {},
        "timestamp": 1234567890
    });
    let result = run_window(
        "child-2",
        "headless",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    assert!(result.is_ok());
}

#[test]
fn test_run_window_dashboard() {
    let data = serde_json::json!({
        "token": "tok123",
        "web_port": 8080,
        "web_host": "0.0.0.0"
    });
    let result = run_window(
        "child-3",
        "dashboard",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    // Without plugin-ui.dll, expect "not found" error
    // With plugin-ui.dll, expect Ok(()) or a runtime error from the DLL
    match result {
        Ok(()) => {}
        Err(e) => assert!(
            e.contains("plugin") || e.contains("not found") || e.contains("DLL"),
            "unexpected error: {}",
            e
        ),
    }
}

#[test]
fn test_run_window_approval_invalid_data() {
    let data = serde_json::json!({"invalid": "data"});
    let result = run_window(
        "child-1",
        "approval",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid approval window data"));
}

#[test]
fn test_run_window_headless_invalid_data() {
    let data = serde_json::json!({"invalid": "data"});
    let result = run_window(
        "child-1",
        "headless",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid headless window data"));
}

#[test]
fn test_run_window_dashboard_invalid_data() {
    let data = serde_json::json!({"invalid": "data"});
    let result = run_window(
        "child-1",
        "dashboard",
        &data,
        "key".to_string(),
        8080,
        "/ws".to_string(),
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .contains("invalid dashboard window data")
    );
}

#[test]
fn test_build_plugin_config_dashboard() {
    let data = serde_json::json!({
        "token": "mytoken",
        "web_port": 49000,
        "web_host": "127.0.0.1"
    });
    let config = build_plugin_config("dashboard", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["window_type"], "dashboard");
    assert_eq!(parsed["title"], "NemesisBot Dashboard");
    assert_eq!(parsed["url"], "http://127.0.0.1:49000");
    assert!(parsed["init_script"].as_str().unwrap().contains("mytoken"));
    assert!(
        parsed["init_script"]
            .as_str()
            .unwrap()
            .contains("127.0.0.1:49000")
    );
    assert_eq!(parsed["width"], 1280.0);
    assert_eq!(parsed["height"], 800.0);
    // Old fields should NOT be present
    assert!(parsed.get("backend_url").is_none());
    assert!(parsed.get("auth_token").is_none());
}

#[test]
fn test_build_plugin_config_approval() {
    let data = serde_json::json!({
        "request_id": "req-1",
        "operation": "file_write",
        "operation_name": "Write File",
        "target": "/tmp/test.txt",
        "risk_level": "HIGH",
        "reason": "user requested",
        "timeout_seconds": 60,
        "context": {},
        "timestamp": 1234567890
    });
    let config = build_plugin_config("approval", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["window_type"], "approval");
    assert_eq!(parsed["title"], "Security Approval - NemesisBot");
    assert_eq!(parsed["width"], 750.0);
    assert_eq!(parsed["height"], 700.0);
    // HTML content should be generated
    let html = parsed["html"].as_str().unwrap();
    assert!(html.contains("req-1"));
    assert!(html.contains("Write File"));
    assert!(html.contains("/tmp/test.txt"));
    assert!(html.contains("HIGH"));
    assert!(html.contains("__approval_result"));
    // Old field should NOT be present
    assert!(parsed.get("approval_data").is_none());
}

#[test]
fn test_load_and_run_plugin_window_dll_not_found() {
    let data = serde_json::json!({
        "token": "test",
        "web_port": 8080,
        "web_host": "127.0.0.1"
    });
    let result = load_and_run_plugin_window("dashboard", &data, "key", 8080, "/ws");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("plugin") || err.contains("not found") || err.contains("DLL"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_send_ws_key_wrong_ack() {
    let wrong_ack = r#"{"type":"handshake","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(wrong_ack.to_string());
    let mut output = Vec::new();
    let result = send_ws_key(&mut output, &mut input, "key", 8080, "/ws");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected ack"));
}

#[test]
fn test_send_window_data_wrong_ack() {
    let wrong_ack = r#"{"type":"handshake","version":"1.0","data":{}}"#;
    let mut input = Cursor::new(wrong_ack.to_string());
    let mut output = Vec::new();
    let data = serde_json::json!({"test": true});
    let result = send_window_data(&mut output, &mut input, &data);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("expected ack"));
}

#[test]
fn test_receive_ws_key_partial_data() {
    let ws_msg = r#"{"type":"ws_key","version":"1.0","data":{"key":"only-key"}}"#;
    let mut input = Cursor::new(ws_msg.to_string());
    let mut output = Vec::new();
    let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
    assert_eq!(key, "only-key");
    assert_eq!(port, 0); // missing port defaults to 0
    assert_eq!(path, ""); // missing path defaults to empty
}

#[test]
fn test_parent_handshake_eof() {
    let mut input = Cursor::new(String::new());
    let mut output = Vec::new();
    let result = parent_handshake(&mut output, &mut input);
    assert!(result.is_err());
}

#[test]
fn test_pipe_reader_multiple_lines() {
    let input = Cursor::new(
        r#"{"type":"handshake","version":"1.0","data":{}}
{"type":"ack","version":"1.0","data":{}}
"#
        .to_string(),
    );
    let mut reader = PipeReader::new(input);
    let msg1 = reader.read_message().unwrap();
    assert!(msg1.is_handshake());
    let msg2 = reader.read_message().unwrap();
    assert!(msg2.is_ack());
}

#[test]
fn test_approval_window_data_default_fields() {
    let json = r#"{"request_id":"r1","operation":"file_write","operation_name":"","target":"test.txt","risk_level":"HIGH","reason":"","timeout_seconds":0,"timestamp":0}"#;
    let data: ApprovalWindowData = serde_json::from_str(json).unwrap();
    assert_eq!(data.request_id, "r1");
    assert_eq!(data.operation_name, "");
    assert_eq!(data.reason, "");
    assert_eq!(data.timeout_seconds, 0);
    assert!(data.context.is_empty());
    assert_eq!(data.timestamp, 0);
}

#[test]
fn test_dashboard_window_data_from_json() {
    let json = r#"{"token":"abc","web_port":9090,"web_host":"localhost"}"#;
    let data: DashboardWindowData = serde_json::from_str(json).unwrap();
    assert_eq!(data.token, "abc");
    assert_eq!(data.web_port, 9090);
    assert_eq!(data.web_host, "localhost");
}

#[test]
fn test_child_handshake_eof_reads_empty() {
    // Empty stdin → read_line returns 0 → error
    let mut input = Cursor::new(String::new());
    let mut output = Vec::new();
    let result = child_handshake(&mut input, &mut output);
    assert!(result.is_err());
}

#[test]
fn test_bring_to_front_fn_ptr_null() {
    // Without a DLL loaded, calling should be a no-op (ptr is null)
    BRING_TO_FRONT_FN_PTR.call();
    // Should not panic
}

#[test]
fn test_connect_ws_with_handler_no_key() {
    // Empty key should return None
    let result = connect_ws_with_handler("", 0, "", false);
    assert!(result.is_none());
}

#[test]
fn test_connect_ws_with_handler_zero_port() {
    let result = connect_ws_with_handler("some-key", 0, "/ws", false);
    assert!(result.is_none());
}

// --- Approval HTML rendering tests ---

#[test]
fn test_risk_color() {
    assert_eq!(risk_color("CRITICAL"), "#dc3545");
    assert_eq!(risk_color("HIGH"), "#fd7e14");
    assert_eq!(risk_color("MEDIUM"), "#ffc107");
    assert_eq!(risk_color("LOW"), "#28a745");
    assert_eq!(risk_color("unknown"), "#6c757d");
    assert_eq!(risk_color("high"), "#fd7e14"); // case insensitive
}

#[test]
fn test_html_escape() {
    assert_eq!(
        html_escape("<script>alert('xss')</script>"),
        "&lt;script&gt;alert(&#39;xss&#39;)&lt;/script&gt;"
    );
    assert_eq!(
        html_escape("a&b<c>d\"e'f"),
        "a&amp;b&lt;c&gt;d&quot;e&#39;f"
    );
    assert_eq!(html_escape("normal text"), "normal text");
}

#[test]
fn test_render_approval_html_basic() {
    let data = ApprovalWindowData {
        request_id: "req-1".to_string(),
        operation: "file_write".to_string(),
        operation_name: "Write File".to_string(),
        target: "/tmp/test.txt".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "User requested write".to_string(),
        timeout_seconds: 10,
        context: HashMap::new(),
        timestamp: 1234567890,
    };
    let html = render_approval_html(&data);
    assert!(html.contains("req-1"));
    assert!(html.contains("Write File"));
    assert!(html.contains("/tmp/test.txt"));
    assert!(html.contains("HIGH"));
    assert!(html.contains("User requested write"));
    assert!(html.contains("#fd7e14")); // HIGH risk color
    assert!(html.contains("respond('approved')"));
    assert!(html.contains("respond('rejected')"));
    assert!(html.contains("__approval_result"));
    assert!(html.contains("TIMEOUT = 30")); // min 30 seconds
}

#[test]
fn test_render_approval_html_critical_risk() {
    let data = ApprovalWindowData {
        request_id: "req-crit".to_string(),
        operation: "process_exec".to_string(),
        operation_name: "Execute".to_string(),
        target: "cmd.exe".to_string(),
        risk_level: "CRITICAL".to_string(),
        reason: "Dangerous".to_string(),
        timeout_seconds: 30,
        context: HashMap::new(),
        timestamp: 1234567890,
    };
    let html = render_approval_html(&data);
    assert!(html.contains("#dc3545")); // CRITICAL risk color (red)
}

#[test]
fn test_render_approval_html_xss_protection() {
    let data = ApprovalWindowData {
        request_id: "req-xss".to_string(),
        operation: "file_write".to_string(),
        operation_name: "<script>alert(1)</script>".to_string(),
        target: "<img onerror=alert(1) src=x>".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "\"injection\" attempt".to_string(),
        timeout_seconds: 30,
        context: HashMap::new(),
        timestamp: 1234567890,
    };
    let html = render_approval_html(&data);
    // Should NOT contain raw HTML tags from input
    assert!(!html.contains("<script>alert(1)</script>"));
    assert!(!html.contains("<img onerror"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("&lt;img"));
}

// --- render_approval_html: remaining risk-level branches ---

fn approval_with_risk(level: &str) -> ApprovalWindowData {
    ApprovalWindowData {
        request_id: "req".to_string(),
        operation: "file_write".to_string(),
        operation_name: "Op".to_string(),
        target: "target".to_string(),
        risk_level: level.to_string(),
        reason: "r".to_string(),
        timeout_seconds: 30,
        context: HashMap::new(),
        timestamp: 1,
    }
}

#[test]
fn test_render_approval_html_medium_risk() {
    let html = render_approval_html(&approval_with_risk("MEDIUM"));
    // MEDIUM risk color (yellow) should be used in the badge
    assert!(html.contains("#ffc107"));
}

#[test]
fn test_render_approval_html_low_risk() {
    let html = render_approval_html(&approval_with_risk("LOW"));
    // LOW risk color (green) should be used in the badge
    assert!(html.contains("#28a745"));
}

#[test]
fn test_render_approval_html_unknown_risk_falls_back() {
    // Unknown level falls back to the default grey color
    let html = render_approval_html(&approval_with_risk("nonexistent_level"));
    assert!(html.contains("#6c757d"));
}

#[test]
fn test_render_approval_html_timeout_clamped_to_min() {
    // timeout_seconds below 30 must be clamped up to 30
    let mut data = approval_with_risk("HIGH");
    data.timeout_seconds = 5;
    let html = render_approval_html(&data);
    assert!(html.contains("TIMEOUT = 30"));
}

#[test]
fn test_render_approval_html_timeout_above_min_preserved() {
    // timeout_seconds above 30 should be preserved as-is
    let mut data = approval_with_risk("HIGH");
    data.timeout_seconds = 120;
    let html = render_approval_html(&data);
    assert!(html.contains("TIMEOUT = 120"));
}

#[test]
fn test_render_approval_html_escapes_reason() {
    let mut data = approval_with_risk("LOW");
    data.reason = "broken <b>tag</b> & stuff".to_string();
    let html = render_approval_html(&data);
    assert!(!html.contains("<b>tag</b>"));
    assert!(html.contains("&lt;b&gt;tag&lt;/b&gt;"));
    assert!(html.contains("&amp; stuff"));
}

#[test]
fn test_render_approval_html_includes_doctype() {
    let html = render_approval_html(&approval_with_risk("LOW"));
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<html lang=\"en\">"));
}

// --- build_plugin_config: error-fallback and edge branches ---

#[test]
fn test_build_plugin_config_unknown_window_type() {
    let config = build_plugin_config("mystery", &serde_json::json!({"a": 1}));
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    // Unknown type only emits the window_type field
    assert_eq!(parsed["window_type"], "mystery");
    assert!(parsed.get("title").is_none());
    assert!(parsed.get("html").is_none());
}

#[test]
fn test_build_plugin_config_dashboard_invalid_data_fallback() {
    // Invalid dashboard data (missing required fields) triggers the
    // serde error fallback, which emits only window_type + title.
    let config = build_plugin_config("dashboard", &serde_json::json!({"unrelated": true}));
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["window_type"], "dashboard");
    assert_eq!(parsed["title"], "NemesisBot Dashboard");
    // Fallback path does NOT emit url / init_script
    assert!(parsed.get("url").is_none());
    assert!(parsed.get("init_script").is_none());
}

#[test]
fn test_build_plugin_config_approval_invalid_data_fallback() {
    // Invalid approval data triggers the serde error fallback.
    let config = build_plugin_config("approval", &serde_json::json!({"unrelated": true}));
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["window_type"], "approval");
    assert_eq!(parsed["title"], "Security Approval - NemesisBot");
    assert!(parsed.get("html").is_none());
    assert!(parsed.get("timeout_seconds").is_none());
}

#[test]
fn test_build_plugin_config_dashboard_token_sanitization() {
    // Backslashes and double quotes in the token must be escaped so the
    // generated init_script stays valid JS.
    let data = serde_json::json!({
        "token": r#"a"b\c"#,
        "web_port": 49000,
        "web_host": "127.0.0.1",
    });
    let config = build_plugin_config("dashboard", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    let init = parsed["init_script"].as_str().unwrap();
    // The raw unescaped token substring must NOT appear
    assert!(!init.contains(r#"__DASHBOARD_TOKEN__="a"b\c""#));
    // Escaped forms should be present
    assert!(init.contains("\\\""));
    assert!(init.contains("\\\\"));
}

#[test]
fn test_build_plugin_config_approval_timeout_clamped() {
    // Approval config clamps timeout_seconds to the 30s minimum.
    let data = serde_json::json!({
        "request_id": "r1",
        "operation": "file_write",
        "operation_name": "Write",
        "target": "t",
        "risk_level": "LOW",
        "reason": "r",
        "timeout_seconds": 1,
        "context": {},
        "timestamp": 1,
    });
    let config = build_plugin_config("approval", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["timeout_seconds"], 30);
}

#[test]
fn test_build_plugin_config_approval_timeout_preserved() {
    // Approval config keeps a timeout above the 30s minimum.
    let data = serde_json::json!({
        "request_id": "r1",
        "operation": "file_write",
        "operation_name": "Write",
        "target": "t",
        "risk_level": "LOW",
        "reason": "r",
        "timeout_seconds": 99,
        "context": {},
        "timestamp": 1,
    });
    let config = build_plugin_config("approval", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["timeout_seconds"], 99);
}

#[test]
fn test_build_plugin_config_dashboard_url_format() {
    // Verify the assembled URL host:port string.
    let data = serde_json::json!({
        "token": "tok",
        "web_port": 12345,
        "web_host": "example.com",
    });
    let config = build_plugin_config("dashboard", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["url"], "http://example.com:12345");
    assert!(
        parsed["init_script"]
            .as_str()
            .unwrap()
            .contains("example.com:12345")
    );
}

#[test]
fn test_build_plugin_config_dashboard_dimensions() {
    let data = serde_json::json!({
        "token": "tok",
        "web_port": 1,
        "web_host": "h",
    });
    let config = build_plugin_config("dashboard", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["width"], 1280.0);
    assert_eq!(parsed["height"], 800.0);
}

#[test]
fn test_build_plugin_config_approval_dimensions_and_html() {
    let data = serde_json::json!({
        "request_id": "r1",
        "operation": "file_write",
        "operation_name": "Write",
        "target": "t",
        "risk_level": "HIGH",
        "reason": "r",
        "timeout_seconds": 60,
        "context": {},
        "timestamp": 1,
    });
    let config = build_plugin_config("approval", &data);
    let parsed: serde_json::Value = serde_json::from_str(&config).unwrap();
    assert_eq!(parsed["width"], 750.0);
    assert_eq!(parsed["height"], 700.0);
    assert!(parsed["html"].is_string());
    assert!(
        parsed["html"]
            .as_str()
            .unwrap()
            .contains("Security Approval")
    );
}

// --- PipeReader / PipeWriter additional edge cases ---

#[test]
fn test_pipe_reader_skips_leading_whitespace() {
    // Leading/trailing whitespace around the JSON line should be trimmed.
    let input =
        Cursor::new("   {\"type\":\"ack\",\"version\":\"1.0\",\"data\":{}}   \n".to_string());
    let mut reader = PipeReader::new(input);
    let msg = reader.read_message().unwrap();
    assert!(msg.is_ack());
}

#[test]
fn test_pipe_writer_serializes_full_message_fields() {
    let mut output = Vec::new();
    let mut writer = PipeWriter::new(&mut output);
    let msg = PipeMessage::ws_key("k", 7000, "/path");
    writer.write_message(&msg).unwrap();
    let s = String::from_utf8(output).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(s.trim()).unwrap();
    assert_eq!(parsed["type"], "ws_key");
    assert_eq!(parsed["version"], "1.0");
    assert_eq!(parsed["data"]["key"], "k");
    assert_eq!(parsed["data"]["port"], 7000);
    assert_eq!(parsed["data"]["path"], "/path");
}

#[test]
fn test_receive_ws_key_extra_fields_ignored() {
    // Unknown data fields should be silently ignored.
    let ws_msg = r#"{"type":"ws_key","version":"1.0","data":{"key":"abc","port":1,"path":"/ws","extra":"ignored"}}"#;
    let mut input = Cursor::new(ws_msg.to_string());
    let mut output = Vec::new();
    let (key, port, path) = receive_ws_key(&mut input, &mut output).unwrap();
    assert_eq!(key, "abc");
    assert_eq!(port, 1);
    assert_eq!(path, "/ws");
}

#[test]
fn test_send_window_data_then_receive_roundtrip() {
    // End-to-end: parent writes window_data, child reads it back.
    let mut pipe = Vec::new();
    let payload = serde_json::json!({"hello": "world", "n": 42});

    // Parent side: write then read an ACK
    let mut ack_input = Cursor::new(r#"{"type":"ack","version":"1.0","data":{}}"#.to_string());
    send_window_data(&mut pipe, &mut ack_input, &payload).unwrap();

    // Child side: read the window_data, then send back an ACK
    let mut child_input = Cursor::new(String::from_utf8(pipe.clone()).unwrap());
    let mut child_output = Vec::new();
    let data = receive_window_data(&mut child_input, &mut child_output).unwrap();
    assert_eq!(data["hello"], "world");
    assert_eq!(data["n"], 42);

    // Child's ACK is written out
    let ack: PipeMessage =
        serde_json::from_str(String::from_utf8(child_output).unwrap().trim()).unwrap();
    assert!(ack.is_ack());
}
