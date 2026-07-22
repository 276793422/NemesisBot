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
        fn complete_task(
            &self,
            _task_id: &str,
            _response: &str,
            _success: bool,
            _error: Option<&str>,
        ) {
        }
    }

    let handler = CallbackHandler::with_completer("node-b".into(), Box::new(MockCompleter));
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

// -- Additional tests: handle_failure without error message, completer interactions --

#[test]
fn test_handle_failure_without_error_message() {
    use std::sync::{Arc, Mutex};

    struct MockCompleter {
        calls: Arc<Mutex<Vec<(String, String, bool, Option<String>)>>>,
    }
    impl TaskCompleter for MockCompleter {
        fn complete_task(&self, task_id: &str, response: &str, success: bool, error: Option<&str>) {
            self.calls.lock().unwrap().push((
                task_id.to_string(),
                response.to_string(),
                success,
                error.map(|s| s.to_string()),
            ));
        }
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = CallbackHandler::with_completer(
        "node-c".into(),
        Box::new(MockCompleter {
            calls: calls.clone(),
        }),
    );

    // Failure with error: None triggers the unwrap_or("unknown error") path
    let payload = CallbackPayload {
        task_id: "task-noerr".into(),
        response: String::new(),
        success: false,
        error: None,
    };

    let result = handler.handle(&payload);
    assert!(result.accepted);
    // The CallbackResult.error is cloned from payload.error which is None
    assert!(result.error.is_none());

    // Verify completer was called with "unknown error"
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "task-noerr");
    assert_eq!(calls[0].1, "");
    assert!(!calls[0].2);
    assert_eq!(calls[0].3, Some("unknown error".to_string()));
}

#[test]
fn test_handle_success_with_completer() {
    use std::sync::{Arc, Mutex};

    struct MockCompleter {
        calls: Arc<Mutex<Vec<(String, String, bool, Option<String>)>>>,
    }
    impl TaskCompleter for MockCompleter {
        fn complete_task(&self, task_id: &str, response: &str, success: bool, error: Option<&str>) {
            self.calls.lock().unwrap().push((
                task_id.to_string(),
                response.to_string(),
                success,
                error.map(|s| s.to_string()),
            ));
        }
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = CallbackHandler::with_completer(
        "node-d".into(),
        Box::new(MockCompleter {
            calls: calls.clone(),
        }),
    );

    let payload = CallbackPayload {
        task_id: "task-ok".into(),
        response: "The answer is 42".into(),
        success: true,
        error: None,
    };

    let result = handler.handle(&payload);
    assert!(result.accepted);
    assert!(result.error.is_none());

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "task-ok");
    assert_eq!(calls[0].1, "The answer is 42");
    assert!(calls[0].2);
    assert!(calls[0].3.is_none());
}

#[test]
fn test_handle_failure_with_completer() {
    use std::sync::{Arc, Mutex};

    struct MockCompleter {
        calls: Arc<Mutex<Vec<(String, String, bool, Option<String>)>>>,
    }
    impl TaskCompleter for MockCompleter {
        fn complete_task(&self, task_id: &str, response: &str, success: bool, error: Option<&str>) {
            self.calls.lock().unwrap().push((
                task_id.to_string(),
                response.to_string(),
                success,
                error.map(|s| s.to_string()),
            ));
        }
    }

    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler = CallbackHandler::with_completer(
        "node-e".into(),
        Box::new(MockCompleter {
            calls: calls.clone(),
        }),
    );

    let payload = CallbackPayload {
        task_id: "task-fail".into(),
        response: String::new(),
        success: false,
        error: Some("OOM".into()),
    };

    let result = handler.handle(&payload);
    assert!(result.accepted);
    assert_eq!(result.error.as_deref(), Some("OOM"));

    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "task-fail");
    assert_eq!(calls[0].1, "");
    assert!(!calls[0].2);
    assert_eq!(calls[0].3, Some("OOM".to_string()));
}
