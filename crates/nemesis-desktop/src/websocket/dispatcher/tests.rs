use super::*;


#[test]
fn test_register_and_dispatch_request() {
    let dispatcher = Dispatcher::new();
    dispatcher.register("ping", |msg| {
        let id = msg.id.as_deref().unwrap_or("");
        Ok(Message::new_response(id, serde_json::json!({"status": "pong"})))
    });

    let msg = Message::new_request("ping", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap();
    assert!(result.is_some());
    let resp = result.unwrap();
    assert!(resp.is_success_response());
}

#[test]
fn test_dispatch_notification() {
    let dispatcher = Dispatcher::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();

    dispatcher.register_notification("event", move |_msg| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    let msg = Message::new_notification("event", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap();
    assert!(result.is_none());
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn test_dispatch_unknown_method() {
    let dispatcher = Dispatcher::new();
    let msg = Message::new_request("unknown", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap();
    assert!(result.is_some());
    let resp = result.unwrap();
    assert!(resp.is_error_response());
}

#[test]
fn test_fallback_handler() {
    let dispatcher = Dispatcher::new();
    dispatcher.set_fallback(|msg| {
        let id = msg.id.as_deref().unwrap_or("");
        Ok(Message::new_response(id, serde_json::json!({"fallback": true})))
    });

    let msg = Message::new_request("anything", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap();
    let resp = result.unwrap();
    assert_eq!(resp.result.as_ref().unwrap()["fallback"], serde_json::json!(true));
}

#[test]
fn test_dispatch_response_returns_error() {
    let dispatcher = Dispatcher::new();
    let msg = Message::new_response("id-1", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg);
    assert!(result.is_err());
}

// ============================================================
// Additional tests for ~92% coverage
// ============================================================

#[test]
fn test_dispatcher_default() {
    let dispatcher = Dispatcher::default();
    let msg = Message::new_request("test", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap();
    assert!(result.is_some());
    assert!(result.unwrap().is_error_response());
}

#[test]
fn test_register_overwrites_handler() {
    let dispatcher = Dispatcher::new();
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));

    // Register first handler
    let c1 = counter.clone();
    dispatcher.register("method", move |msg| {
        c1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("first")))
    });

    // Overwrite with second handler
    let c2 = counter.clone();
    dispatcher.register("method", move |msg| {
        c2.fetch_add(10, std::sync::atomic::Ordering::SeqCst);
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("second")))
    });

    let msg = Message::new_request("method", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap().unwrap();
    assert_eq!(result.result.as_ref().unwrap(), &serde_json::json!("second"));
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 10);
}

#[test]
fn test_handler_returns_error() {
    let dispatcher = Dispatcher::new();
    dispatcher.register("fail", |_msg| {
        Err("handler error".to_string())
    });

    let msg = Message::new_request("fail", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("handler error"));
}

#[test]
fn test_notification_handler_not_registered() {
    let dispatcher = Dispatcher::new();
    let msg = Message::new_notification("unknown_event", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap();
    assert!(result.is_none()); // notifications return None
}

#[test]
fn test_dispatch_notification_with_params() {
    let dispatcher = Dispatcher::new();
    let received = std::sync::Arc::new(std::sync::Mutex::new(None));
    let received_clone = received.clone();

    dispatcher.register_notification("update", move |msg| {
        let mut guard = received_clone.lock().unwrap();
        *guard = msg.params.clone();
    });

    let msg = Message::new_notification("update", serde_json::json!({"key": "value"}));
    dispatcher.dispatch(&msg).unwrap();

    let guard = received.lock().unwrap();
    assert_eq!(guard.as_ref().unwrap()["key"], serde_json::json!("value"));
}

#[test]
fn test_dispatch_request_to_fallback() {
    let dispatcher = Dispatcher::new();
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called_clone = called.clone();

    dispatcher.set_fallback(move |msg| {
        called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("handled")))
    });

    let msg = Message::new_request("unregistered_method", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap().unwrap();
    assert!(called.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(result.result.as_ref().unwrap(), &serde_json::json!("handled"));
}

#[test]
fn test_dispatch_request_registered_takes_priority_over_fallback() {
    let dispatcher = Dispatcher::new();
    let which = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let w1 = which.clone();
    let w2 = which.clone();

    dispatcher.register("method", move |msg| {
        w1.store(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("handler")))
    });

    dispatcher.set_fallback(move |msg| {
        w2.store(2, std::sync::atomic::Ordering::SeqCst);
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("fallback")))
    });

    let msg = Message::new_request("method", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap().unwrap();
    assert_eq!(result.result.as_ref().unwrap(), &serde_json::json!("handler"));
    assert_eq!(which.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn test_dispatch_multiple_different_methods() {
    let dispatcher = Dispatcher::new();

    dispatcher.register("add", |msg| {
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("added")))
    });
    dispatcher.register("remove", |msg| {
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), serde_json::json!("removed")))
    });

    let msg1 = Message::new_request("add", serde_json::Value::Null);
    let resp1 = dispatcher.dispatch(&msg1).unwrap().unwrap();
    assert_eq!(resp1.result.as_ref().unwrap(), &serde_json::json!("added"));

    let msg2 = Message::new_request("remove", serde_json::Value::Null);
    let resp2 = dispatcher.dispatch(&msg2).unwrap().unwrap();
    assert_eq!(resp2.result.as_ref().unwrap(), &serde_json::json!("removed"));
}

#[test]
fn test_dispatch_request_accesses_params() {
    let dispatcher = Dispatcher::new();
    dispatcher.register("echo", |msg| {
        let params = msg.params.clone().unwrap_or_default();
        Ok(Message::new_response(msg.id.as_deref().unwrap_or(""), params))
    });

    let msg = Message::new_request("echo", serde_json::json!({"hello": "world"}));
    let resp = dispatcher.dispatch(&msg).unwrap().unwrap();
    assert_eq!(resp.result.as_ref().unwrap()["hello"], serde_json::json!("world"));
}

#[test]
fn test_dispatch_method_not_found_error_code() {
    let dispatcher = Dispatcher::new();
    let msg = Message::new_request_with_id("req-1", "nonexistent", serde_json::Value::Null);
    let result = dispatcher.dispatch(&msg).unwrap().unwrap();
    assert!(result.is_error_response());
    let err = result.error.unwrap();
    assert_eq!(err.code, crate::websocket::protocol::ERR_METHOD_NOT_FOUND);
    assert!(err.message.contains("nonexistent"));
}
