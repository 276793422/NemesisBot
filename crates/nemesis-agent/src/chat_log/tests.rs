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
