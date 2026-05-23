use super::*;

#[test]
fn test_new_request() {
    let msg = Message::new_request("test.method", serde_json::json!({"key": "value"}));
    assert!(msg.is_request());
    assert!(!msg.is_notification());
    assert!(!msg.is_response());
    assert!(msg.id.is_some());
    assert_eq!(msg.method.as_deref(), Some("test.method"));
}

#[test]
fn test_new_notification() {
    let msg = Message::new_notification("test.notify", serde_json::Value::Null);
    assert!(!msg.is_request());
    assert!(msg.is_notification());
    assert!(msg.id.is_none());
}

#[test]
fn test_new_response() {
    let msg = Message::new_response("id-1", serde_json::json!({"status": "ok"}));
    assert!(msg.is_response());
    assert!(msg.is_success_response());
    assert!(!msg.is_error_response());
}

#[test]
fn test_new_error_response() {
    let msg = Message::new_error_response("id-1", ERR_METHOD_NOT_FOUND, "not found", None);
    assert!(msg.is_response());
    assert!(!msg.is_success_response());
    assert!(msg.is_error_response());
    let err = msg.error.unwrap();
    assert_eq!(err.code, ERR_METHOD_NOT_FOUND);
}

#[test]
fn test_message_serialization() {
    let msg = Message::new_request("ping", serde_json::Value::Null);
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"method\":\"ping\""));
}

#[test]
fn test_message_roundtrip() {
    let msg = Message::new_request("method", serde_json::json!({"a": 1}));
    let json = serde_json::to_string(&msg).unwrap();
    let back: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(back.method, msg.method);
    assert_eq!(back.id, msg.id);
}

#[test]
fn test_decode_params() {
    #[derive(serde::Deserialize, Debug)]
    struct Params {
        key: String,
        value: i32,
    }
    let msg = Message::new_request("test", serde_json::json!({"key": "hello", "value": 42}));
    let params: Params = msg.decode_params().unwrap();
    assert_eq!(params.key, "hello");
    assert_eq!(params.value, 42);
}

#[test]
fn test_decode_params_missing() {
    let msg = Message::new_response("id-1", serde_json::json!("ok"));
    let result: Result<serde_json::Value, _> = msg.decode_params();
    assert!(result.is_err());
}

#[test]
fn test_decode_result() {
    #[derive(serde::Deserialize, Debug)]
    struct Result {
        status: String,
    }
    let msg = Message::new_response("id-1", serde_json::json!({"status": "ok"}));
    let res: Result = msg.decode_result().unwrap();
    assert_eq!(res.status, "ok");
}

#[test]
fn test_decode_result_missing() {
    let msg = Message::new_request("test", serde_json::Value::Null);
    let result: Result<serde_json::Value, _> = msg.decode_result();
    assert!(result.is_err());
}

#[test]
fn test_decode_error_data() {
    #[derive(serde::Deserialize, Debug)]
    struct ErrorDetail {
        detail: String,
    }
    let msg = Message::new_error_response(
        "id-1",
        ERR_METHOD_NOT_FOUND,
        "not found",
        Some(serde_json::json!({"detail": "extra info"})),
    );
    let data: ErrorDetail = msg.decode_error_data().unwrap();
    assert_eq!(data.detail, "extra info");
}

#[test]
fn test_decode_error_data_missing() {
    // Error with no data field
    let msg = Message::new_error_response("id-1", ERR_INTERNAL, "err", None);
    let result: Result<serde_json::Value, _> = msg.decode_error_data();
    assert!(result.is_err());
}

#[test]
fn test_decode_error_data_no_error() {
    let msg = Message::new_response("id-1", serde_json::json!("ok"));
    let result: Result<serde_json::Value, _> = msg.decode_error_data();
    assert!(result.is_err());
}

#[test]
fn test_new_request_with_id() {
    let msg = Message::new_request_with_id("my-id", "test.method", serde_json::json!({"a": 1}));
    assert!(msg.is_request());
    assert_eq!(msg.id.as_deref(), Some("my-id"));
    assert_eq!(msg.method.as_deref(), Some("test.method"));
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_version_constant() {
    assert_eq!(VERSION, "2.0");
}

#[test]
fn test_error_codes() {
    assert_eq!(ERR_PARSE_ERROR, -32700);
    assert_eq!(ERR_INVALID_REQUEST, -32600);
    assert_eq!(ERR_METHOD_NOT_FOUND, -32601);
    assert_eq!(ERR_INVALID_PARAMS, -32602);
    assert_eq!(ERR_INTERNAL, -32603);
    assert_eq!(ERR_TIMEOUT, -32001);
    assert_eq!(ERR_NOT_READY, -32002);
    assert_eq!(ERR_WINDOW_NOT_FOUND, -32003);
}

#[test]
fn test_error_payload_display() {
    let err = ErrorPayload {
        code: -32601,
        message: "method not found".to_string(),
        data: None,
    };
    let display = format!("{}", err);
    assert!(display.contains("-32601"));
    assert!(display.contains("method not found"));
}

#[test]
fn test_error_payload_with_data() {
    let err = ErrorPayload {
        code: -32001,
        message: "timeout".to_string(),
        data: Some(serde_json::json!({"elapsed_ms": 5000})),
    };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: ErrorPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.code, -32001);
    assert!(parsed.data.is_some());
}

#[test]
fn test_message_serialization_skips_none_fields() {
    let msg = Message::new_notification("test", serde_json::Value::Null);
    let json = serde_json::to_string(&msg).unwrap();
    // id should be absent for notification
    assert!(!json.contains("\"id\""));
    // params should be absent when Null
    assert!(!json.contains("\"params\""));
}

#[test]
fn test_message_new_request_unique_ids() {
    let msg1 = Message::new_request("test", serde_json::Value::Null);
    let msg2 = Message::new_request("test", serde_json::Value::Null);
    assert_ne!(msg1.id, msg2.id);
}

#[test]
fn test_message_with_null_params() {
    let msg = Message::new_request("test", serde_json::Value::Null);
    assert!(msg.params.is_none());
}

#[test]
fn test_message_with_params() {
    let msg = Message::new_request("test", serde_json::json!({"key": "val"}));
    assert!(msg.params.is_some());
}

#[test]
fn test_message_notification_with_params() {
    let msg = Message::new_notification("test", serde_json::json!({"data": 123}));
    assert!(msg.is_notification());
    assert!(msg.params.is_some());
    assert!(msg.id.is_none());
}

#[test]
fn test_message_deserialization_from_raw_json() {
    let json = r#"{"jsonrpc":"2.0","id":"abc","method":"ping","params":{"key":"val"}}"#;
    let msg: Message = serde_json::from_str(json).unwrap();
    assert!(msg.is_request());
    assert_eq!(msg.id.as_deref(), Some("abc"));
    assert_eq!(msg.method.as_deref(), Some("ping"));
}

#[test]
fn test_message_error_response_with_data() {
    let msg = Message::new_error_response(
        "id-1",
        ERR_TIMEOUT,
        "request timed out",
        Some(serde_json::json!({"timeout_ms": 30000})),
    );
    assert!(msg.is_error_response());
    let err = msg.error.unwrap();
    assert_eq!(err.code, ERR_TIMEOUT);
    assert!(err.data.is_some());
}

#[test]
fn test_decode_params_type_mismatch() {
    #[derive(serde::Deserialize, Debug)]
    struct Params { value: i32 }
    let msg = Message::new_request("test", serde_json::json!({"value": "not_a_number"}));
    let result: Result<Params, _> = msg.decode_params();
    assert!(result.is_err());
}

#[test]
fn test_decode_result_type_mismatch() {
    #[derive(serde::Deserialize, Debug)]
    struct MyResult { count: i32 }
    let msg = Message::new_response("id-1", serde_json::json!({"count": "string"}));
    let result: Result<MyResult, _> = msg.decode_result();
    assert!(result.is_err());
}

#[test]
fn test_full_request_response_cycle() {
    let request = Message::new_request("add", serde_json::json!({"a": 1, "b": 2}));
    let id = request.id.clone().unwrap();

    // Simulate server processing
    let response = Message::new_response(&id, serde_json::json!({"result": 3}));
    assert!(response.is_success_response());
    assert_eq!(response.id, request.id);

    let result_val: serde_json::Value = response.decode_result().unwrap();
    assert_eq!(result_val["result"], 3);
}

// ============================================================
// Additional tests for ~92% coverage
// ============================================================

#[test]
fn test_message_debug_format() {
    let msg = Message::new_request("test", serde_json::Value::Null);
    let debug = format!("{:?}", msg);
    assert!(debug.contains("2.0"));
    assert!(debug.contains("test"));
}

#[test]
fn test_error_payload_debug() {
    let err = ErrorPayload {
        code: -32601,
        message: "method not found".to_string(),
        data: None,
    };
    let debug = format!("{:?}", err);
    assert!(debug.contains("-32601"));
}

#[test]
fn test_error_payload_clone() {
    let err = ErrorPayload {
        code: -32601,
        message: "method not found".to_string(),
        data: Some(serde_json::json!({"extra": true})),
    };
    let cloned = err.clone();
    assert_eq!(cloned.code, err.code);
    assert_eq!(cloned.message, err.message);
}

#[test]
fn test_error_payload_serialization_roundtrip() {
    let err = ErrorPayload {
        code: -32001,
        message: "timeout".to_string(),
        data: Some(serde_json::json!({"elapsed": 5000})),
    };
    let json = serde_json::to_string(&err).unwrap();
    let parsed: ErrorPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.code, -32001);
    assert_eq!(parsed.message, "timeout");
    assert!(parsed.data.is_some());
}

#[test]
fn test_error_payload_no_data_serialization() {
    let err = ErrorPayload {
        code: -32603,
        message: "internal error".to_string(),
        data: None,
    };
    let json = serde_json::to_string(&err).unwrap();
    assert!(!json.contains("\"data\""));
    let parsed: ErrorPayload = serde_json::from_str(&json).unwrap();
    assert!(parsed.data.is_none());
}

#[test]
fn test_message_clone() {
    let msg = Message::new_request("test", serde_json::json!({"key": "val"}));
    let cloned = msg.clone();
    assert_eq!(cloned.id, msg.id);
    assert_eq!(cloned.method, msg.method);
}

#[test]
fn test_new_request_with_id_serialization() {
    let msg = Message::new_request_with_id("fixed-id", "method", serde_json::json!({"a": 1}));
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id.as_deref(), Some("fixed-id"));
    assert_eq!(parsed.method.as_deref(), Some("method"));
}

#[test]
fn test_message_is_checks_combinations() {
    // Response with error
    let msg = Message::new_error_response("id-1", ERR_INTERNAL, "err", None);
    assert!(msg.is_response());
    assert!(!msg.is_success_response());
    assert!(msg.is_error_response());
    assert!(!msg.is_request());
    assert!(!msg.is_notification());

    // Response without error (success)
    let msg = Message::new_response("id-2", serde_json::json!(true));
    assert!(msg.is_response());
    assert!(msg.is_success_response());
    assert!(!msg.is_error_response());
}

#[test]
fn test_deserialize_minimal_message() {
    let json = r#"{"jsonrpc":"2.0"}"#;
    let msg: Message = serde_json::from_str(json).unwrap();
    assert!(msg.id.is_none());
    assert!(msg.method.is_none());
    assert!(msg.params.is_none());
    assert!(msg.result.is_none());
    assert!(msg.error.is_none());
    assert!(!msg.is_request());
    assert!(!msg.is_notification());
    assert!(!msg.is_response());
}

#[test]
fn test_decode_params_wrong_type() {
    #[derive(serde::Deserialize, Debug)]
    struct StrictParams { count: i32 }
    let msg = Message::new_request("test", serde_json::json!({"count": "not_int"}));
    let result: Result<StrictParams, _> = msg.decode_params();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("params decode error"));
}

#[test]
fn test_decode_result_wrong_type() {
    #[derive(serde::Deserialize, Debug)]
    struct StrictResult { value: i32 }
    let msg = Message::new_response("id-1", serde_json::json!({"value": "string"}));
    let result: Result<StrictResult, _> = msg.decode_result();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("result decode error"));
}

#[test]
fn test_decode_error_data_wrong_type() {
    #[derive(serde::Deserialize, Debug)]
    struct StrictError { code: i32 }
    let msg = Message::new_error_response(
        "id-1", ERR_INTERNAL, "err",
        Some(serde_json::json!({"code": "string"})),
    );
    let result: Result<StrictError, _> = msg.decode_error_data();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("error.data decode error"));
}

#[test]
fn test_error_response_without_data() {
    let msg = Message::new_error_response("id-1", ERR_INVALID_PARAMS, "bad params", None);
    let err = msg.error.unwrap();
    assert_eq!(err.code, ERR_INVALID_PARAMS);
    assert!(err.data.is_none());
}

#[test]
fn test_notification_with_null_params_serialization() {
    let msg = Message::new_notification("event", serde_json::Value::Null);
    let json = serde_json::to_string(&msg).unwrap();
    assert!(!json.contains("\"params\""));
    assert!(!json.contains("\"id\""));
    assert!(!json.contains("\"result\""));
    assert!(!json.contains("\"error\""));
}

#[test]
fn test_request_with_params_serialization() {
    let msg = Message::new_request("method", serde_json::json!({"x": 1, "y": 2}));
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"params\""));
    assert!(json.contains("\"method\""));
    assert!(json.contains("\"id\""));
}

#[test]
fn test_response_result_null() {
    let msg = Message::new_response("id-1", serde_json::Value::Null);
    // Null result should still be serialized (it's Some(Value::Null))
    assert!(msg.result.is_some());
    assert!(msg.is_success_response());
}
