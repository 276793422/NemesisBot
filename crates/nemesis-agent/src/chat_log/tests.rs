use super::*;

#[test]
fn test_safe_key_conversion() {
    let path = log_path("agent:main:main");
    assert!(path.to_string_lossy().contains("agent_main_main"));
    assert!(path.to_string_lossy().ends_with(".jsonl"));
}

#[test]
fn test_read_nonexistent() {
    let (msgs, total, has_more, oldest) = read_chat_log("test:nonexistent:session", 10, None);
    assert!(msgs.is_empty());
    assert_eq!(total, 0);
    assert!(!has_more);
    assert_eq!(oldest, 0);
}

#[test]
fn test_append_with_model_round_trip() {
    let key = "test:model-badge:roundtrip";
    delete_chat_log(key); // clean slate

    // user row: no model. assistant row: with model badge.
    append_chat_log_with_model(key, "user", "hi", None);
    append_chat_log_with_model(
        key,
        "assistant",
        "hello back",
        Some("deepseek/deepseek-v4-flash"),
    );

    let (msgs, total, _, _) = read_chat_log(key, 10, None);
    assert_eq!(total, 2);
    assert_eq!(msgs.len(), 2);
    // user row has no model field.
    assert_eq!(msgs[0]["role"].as_str(), Some("user"));
    assert!(msgs[0].get("model").is_none());
    // assistant row carries the model badge.
    assert_eq!(msgs[1]["role"].as_str(), Some("assistant"));
    assert_eq!(
        msgs[1]["model"].as_str(),
        Some("deepseek/deepseek-v4-flash")
    );

    // Legacy append_chat_log (model=None) writes no model field → backward compat.
    append_chat_log(key, "assistant", "legacy-no-model");
    let (msgs2, _, _, _) = read_chat_log(key, 10, None);
    let last = msgs2.last().unwrap();
    assert_eq!(last["content"].as_str(), Some("legacy-no-model"));
    assert!(last.get("model").is_none());

    delete_chat_log(key); // cleanup
}
