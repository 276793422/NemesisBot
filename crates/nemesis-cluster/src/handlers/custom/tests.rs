use super::*;

#[test]
fn test_register_and_execute() {
    let handler = CustomHandler::new();
    handler.register(
        "custom_echo",
        Arc::new(|_action, payload| Ok(payload)),
    );

    let result = handler.execute("custom_echo", serde_json::json!({"msg": "hello"}));
    assert_eq!(result.unwrap()["msg"], "hello");
}

#[test]
fn test_execute_unregistered() {
    let handler = CustomHandler::new();
    let result = handler.execute("nonexistent", serde_json::json!({}));
    assert!(result.is_err());
}

#[test]
fn test_unregister() {
    let handler = CustomHandler::new();
    handler.register("temp", Arc::new(|_, p| Ok(p)));
    assert!(handler.has_handler("temp"));
    assert!(handler.unregister("temp"));
    assert!(!handler.has_handler("temp"));
}

#[test]
fn test_list_actions() {
    let handler = CustomHandler::new();
    handler.register("a1", Arc::new(|_, p| Ok(p)));
    handler.register("a2", Arc::new(|_, p| Ok(p)));

    let actions = handler.list_actions();
    assert_eq!(actions.len(), 2);
    assert!(actions.contains(&"a1".to_string()));
}

// -- Additional tests --

#[test]
fn test_default_trait_impl() {
    let handler = CustomHandler::default();
    assert!(handler.list_actions().is_empty());
}

#[test]
fn test_re_register_replaces() {
    let handler = CustomHandler::new();
    handler.register("action", Arc::new(|_, _| Ok(serde_json::json!({"v": 1}))));
    let result = handler.execute("action", serde_json::json!({})).unwrap();
    assert_eq!(result["v"], 1);

    // Re-register replaces
    handler.register("action", Arc::new(|_, _| Ok(serde_json::json!({"v": 2}))));
    let result = handler.execute("action", serde_json::json!({})).unwrap();
    assert_eq!(result["v"], 2);
}

#[test]
fn test_execute_after_unregister() {
    let handler = CustomHandler::new();
    handler.register("temp", Arc::new(|_, p| Ok(p)));
    handler.unregister("temp");

    let result = handler.execute("temp", serde_json::json!({}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No handler registered"));
}

#[test]
fn test_list_actions_empty() {
    let handler = CustomHandler::new();
    assert!(handler.list_actions().is_empty());
}

#[test]
fn test_unregister_nonexistent() {
    let handler = CustomHandler::new();
    assert!(!handler.unregister("nonexistent"));
}

#[test]
fn test_has_handler_false() {
    let handler = CustomHandler::new();
    assert!(!handler.has_handler("no_such_action"));
}
