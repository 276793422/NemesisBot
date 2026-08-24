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

/// Single-source-of-truth row predicate (2026-08-25 fork-fix round): the
/// fork's chat_log projection and the turns endpoint's `end_preview` MUST
/// use the same rule, or the dialog preview would disagree with what the
/// fork actually ends on. Pin it here.
#[test]
fn test_is_projected_chat_row() {
    use super::is_projected_chat_row;
    assert!(is_projected_chat_row("user", "anything"));
    assert!(is_projected_chat_row("user", "")); // user rows always project
    assert!(is_projected_chat_row("assistant", "reply"));
    assert!(!is_projected_chat_row("assistant", "")); // tool_calls intermediate
    assert!(!is_projected_chat_row("assistant", "   \n ")); // whitespace-only
    assert!(!is_projected_chat_row("tool", "result"));
    assert!(!is_projected_chat_row("system", "prompt"));
}

// 2026-08-25: delete_chat_log must also remove the title meta sidecar
// (orphan-file fix); clear_chat_log must KEEP it (title survives a clear).
#[test]
fn test_delete_chat_log_removes_meta_but_clear_keeps_it() {
    let key = format!(
        "test:meta:lifecycle:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    crate::chat_log::append_chat_log(&key, "user", "row");
    crate::chat_log::write_session_meta(&key, "title to keep");

    // clear: jsonl emptied, meta survives
    crate::chat_log::clear_chat_log(&key);
    let (rows, _, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(rows.len(), 0);
    assert_eq!(
        crate::chat_log::read_session_meta(&key).as_deref(),
        Some("title to keep")
    );

    // delete: everything gone including meta
    crate::chat_log::delete_chat_log(&key);
    assert!(crate::chat_log::read_session_meta(&key).is_none());
    let (rows2, _, _, _) = crate::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(rows2.len(), 0);
}
