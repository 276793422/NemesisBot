use super::*;

#[test]
fn test_handle_success_callback() {
    let handler = CallbackHandler::new("node-a".into());
    let payload = CallbackPayload {
        task_id: "task-001".into(),
        response: "Hello from remote".into(),
        success: true,
        error: None,
    };

    let result = handler.handle(&payload);
    assert!(result.accepted);
    assert!(result.error.is_none());
}

#[test]
fn test_handle_failure_callback() {
    let handler = CallbackHandler::new("node-a".into());
    let payload = CallbackPayload {
        task_id: "task-002".into(),
        response: String::new(),
        success: false,
        error: Some("timeout".into()),
    };

    let result = handler.handle(&payload);
    assert!(result.accepted);
    assert_eq!(result.error.as_deref(), Some("timeout"));
}

#[test]
fn test_validate_payload() {
    let handler = CallbackHandler::new("node-a".into());

    // Valid success
    let valid = CallbackPayload {
        task_id: "t1".into(),
        response: "ok".into(),
        success: true,
        error: None,
    };
    assert!(handler.validate(&valid).is_ok());

    // Invalid: empty task_id
    let invalid = CallbackPayload {
        task_id: String::new(),
        response: "ok".into(),
        success: true,
        error: None,
    };
    assert!(handler.validate(&invalid).is_err());

    // Invalid: success but empty response
    let invalid2 = CallbackPayload {
        task_id: "t1".into(),
        response: String::new(),
        success: true,
        error: None,
    };
    assert!(handler.validate(&invalid2).is_err());
}

// -- Additional tests --

#[test]
fn test_callback_payload_serialization_roundtrip() {
    let payload = CallbackPayload {
        task_id: "task-123".into(),
        response: "result data".into(),
        success: true,
        error: None,
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: CallbackPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(back.task_id, "task-123");
    assert_eq!(back.response, "result data");
    assert!(back.success);
    assert!(back.error.is_none());
}

#[test]
fn test_callback_payload_serialization_with_error() {
    let payload = CallbackPayload {
        task_id: "task-456".into(),
        response: String::new(),
        success: false,
        error: Some("connection refused".into()),
    };
    let json = serde_json::to_string(&payload).unwrap();
    let back: CallbackPayload = serde_json::from_str(&json).unwrap();
    assert!(!back.success);
    assert_eq!(back.error.unwrap(), "connection refused");
}

#[test]
fn test_validate_failure_with_no_error() {
    let handler = CallbackHandler::new("node-a".into());
    let payload = CallbackPayload {
        task_id: "task-789".into(),
        response: String::new(),
        success: false,
        error: None,
    };
    // Failure with no error message should fail validation
    assert!(handler.validate(&payload).is_err());
}

#[test]
fn test_with_completer_constructor() {
    use crate::handlers::callback::TaskCompleter;

    struct MockCompleter;
    impl TaskCompleter for MockCompleter {
        fn complete_task(&self, _task_id: &str, _response: &str, _success: bool, _error: Option<&str>) {
        }
    }

    let handler = CallbackHandler::with_completer(
        "node-b".into(),
        Box::new(MockCompleter),
    );
    // Just ensure it was constructed without panicking
    let payload = CallbackPayload {
        task_id: "task-1".into(),
        response: "ok".into(),
        success: true,
        error: None,
    };
    let result = handler.handle(&payload);
    assert!(result.accepted);
}
