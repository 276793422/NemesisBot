//! H1/H2（2026-09-05）：`chat.todo_get` 支撑函数测试——读路径与
//! TodoWriteTool 写路径的同构性锁定。

use super::chat::read_session_todos;
use std::time::{SystemTime, UNIX_EPOCH};

/// 每测试唯一的临时 workspace。
fn unique_workspace(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nb_todo_get_{tag}_{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().to_string()
}

/// 构造与 TodoWriteTool 完全一致的文件名。
fn todo_path(workspace: &str, session_id: &str) -> std::path::PathBuf {
    let s = nemesis_agent::session::SessionStore::sanitize_session_id(session_id);
    let safe_key = format!("agent:main:session:{s}").replace(':', "_");
    std::path::Path::new(workspace)
        .join("sessions")
        .join(format!("todo_{safe_key}.json"))
}

#[test]
fn missing_file_returns_empty() {
    let ws = unique_workspace("missing");
    let todos = read_session_todos(&ws, "abc123");
    assert!(todos.is_empty(), "no todo file = empty list, not an error");
}

#[test]
fn written_file_round_trips() {
    let ws = unique_workspace("roundtrip");
    let path = todo_path(&ws, "sess-1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"[{"content":"task a","status":"pending"},{"content":"task b","status":"completed"}]"#,
    )
    .unwrap();

    let todos = read_session_todos(&ws, "sess-1");
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].content, "task a");
    assert_eq!(todos[1].status, nemesis_types::agent::TodoStatus::Completed);
}

#[test]
fn corrupt_file_returns_empty() {
    let ws = unique_workspace("corrupt");
    let path = todo_path(&ws, "sess-2");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "not json at all {{{").unwrap();

    let todos = read_session_todos(&ws, "sess-2");
    assert!(
        todos.is_empty(),
        "corrupt file degrades to empty, not error"
    );
}
