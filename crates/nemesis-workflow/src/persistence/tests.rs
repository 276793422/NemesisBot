use super::*;
use crate::types::ExecutionState;
use std::collections::HashMap;

#[test]
fn test_save_and_load_execution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let execution = Execution::new("test_wf".to_string(), HashMap::new());
    let id = execution.id.clone();

    persistence.save_execution(&execution).unwrap();

    let loaded = persistence.load_execution(&id).unwrap();
    assert_eq!(loaded.id, id);
    assert_eq!(loaded.workflow_name, "test_wf");
    assert_eq!(loaded.state, ExecutionState::Pending);
}

#[test]
fn test_list_executions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let e1 = Execution::new("wf1".to_string(), HashMap::new());
    let e2 = Execution::new("wf2".to_string(), HashMap::new());

    persistence.save_execution(&e1).unwrap();
    persistence.save_execution(&e2).unwrap();

    let list = persistence.list_executions().unwrap();
    assert_eq!(list.len(), 2);

    let names: Vec<&str> = list.iter().map(|e| e.workflow_name.as_str()).collect();
    assert!(names.contains(&"wf1"));
    assert!(names.contains(&"wf2"));
}

#[test]
fn test_load_nonexistent_returns_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    // No file exists yet.
    let result = persistence.load_execution("does_not_exist");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, PersistenceError::NotFound(_)));
}

#[test]
fn test_delete_execution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let e1 = Execution::new("wf1".to_string(), HashMap::new());
    let e2 = Execution::new("wf2".to_string(), HashMap::new());
    let id1 = e1.id.clone();

    persistence.save_execution(&e1).unwrap();
    persistence.save_execution(&e2).unwrap();

    // Delete e1
    let deleted = persistence.delete_execution("wf1", &id1).unwrap();
    assert!(deleted);

    // e1 should be gone
    let result = persistence.load_execution(&id1);
    assert!(result.is_err());

    // e2 should still be there
    let remaining = persistence.list_executions().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].workflow_name, "wf2");
}

#[test]
fn test_delete_execution_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let e1 = Execution::new("wf1".to_string(), HashMap::new());
    persistence.save_execution(&e1).unwrap();

    let deleted = persistence
        .delete_execution("wf1", "nonexistent_id")
        .unwrap();
    assert!(!deleted);

    // Original should still be there
    let list = persistence.list_executions().unwrap();
    assert_eq!(list.len(), 1);
}

#[test]
fn test_delete_execution_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let deleted = persistence.delete_execution("wf", "any").unwrap();
    assert!(!deleted);
}

#[test]
fn test_cleanup_old_executions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    // Save a current execution
    let current = Execution::new("current_wf".to_string(), HashMap::new());
    persistence.save_execution(&current).unwrap();

    // Cleanup with 0 days should remove everything (started_at is now, which is > cutoff of now - 0 days)
    // Actually with 0 days, cutoff = now, and started_at = now, so started_at > cutoff is false (they're equal).
    // Let's use a very small max_age_days to test that recent executions survive.
    let removed = persistence.cleanup_old_executions(1).unwrap();
    assert_eq!(removed, 0); // Nothing removed, execution is from just now

    // The execution should still be there
    let list = persistence.list_executions().unwrap();
    assert_eq!(list.len(), 1);
}

#[test]
fn test_cleanup_old_executions_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let removed = persistence.cleanup_old_executions(30).unwrap();
    assert_eq!(removed, 0);
}

// ---- New tests ----

#[test]
fn test_persistence_error_display() {
    let e1 = PersistenceError::NotFound("id-123".into());
    assert!(e1.to_string().contains("id-123"));

    let e2 = PersistenceError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file gone",
    ));
    assert!(e2.to_string().contains("file gone"));
}

#[test]
fn test_save_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep/nested/executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let execution = Execution::new("wf".to_string(), HashMap::new());
    persistence.save_execution(&execution).unwrap();
    assert!(path.exists());
}

#[test]
fn test_overwrite_same_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let mut e1 = Execution::new("wf1".to_string(), HashMap::new());
    let id = e1.id.clone();
    persistence.save_execution(&e1).unwrap();

    // Update state and save again
    e1.state = ExecutionState::Completed;
    persistence.save_execution(&e1).unwrap();

    let loaded = persistence.load_execution(&id).unwrap();
    assert_eq!(loaded.state, ExecutionState::Completed);
}

#[test]
fn test_list_executions_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let list = persistence.list_executions().unwrap();
    assert!(list.is_empty());
}

#[test]
fn test_save_multiple_executions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    for i in 0..20 {
        let e = Execution::new(format!("wf-{}", i), HashMap::new());
        persistence.save_execution(&e).unwrap();
    }

    let list = persistence.list_executions().unwrap();
    assert_eq!(list.len(), 20);
}

#[test]
fn test_cleanup_with_old_execution() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let persistence = WorkflowPersistence::new(&path);

    let mut old = Execution::new("old_wf".to_string(), HashMap::new());
    old.started_at = Local::now() - TimeDelta::days(60);
    old.ended_at = Some(Local::now() - TimeDelta::days(60));
    persistence.save_execution(&old).unwrap();

    let removed = persistence.cleanup_old_executions(30).unwrap();
    assert_eq!(removed, 1);

    let list = persistence.list_executions().unwrap();
    assert!(list.is_empty());
}

#[test]
fn test_new_accepts_pathbuf() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.jsonl");
    let _persistence = WorkflowPersistence::new(path);
}

#[test]
fn test_new_accepts_str() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.jsonl");
    let _persistence = WorkflowPersistence::new(path.as_os_str());
}

// ---------------------------------------------------------------------------
// coverage gap closure (create_dir_all failure, invalid JSONL line skip)
// ---------------------------------------------------------------------------

#[test]
fn w4a_save_execution_parent_is_file_returns_err() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"i am a file").unwrap();
    // Parent of the JSONL path is a regular file -> create_dir_all fails
    let persistence = WorkflowPersistence::new(blocker.join("nested").join("e.jsonl"));
    let execution = Execution::new("test_wf".to_string(), HashMap::new());
    assert!(persistence.save_execution(&execution).is_err());
}

#[test]
fn w4a_list_executions_skips_garbage_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("executions.jsonl");
    let execution = Execution::new("test_wf".to_string(), HashMap::new());
    let id = execution.id.clone();
    let good = serde_json::to_string(&execution).unwrap();
    // One garbage line, one blank line, one good line
    let content = format!("not json at all\n\n{}\n", good);
    std::fs::write(&path, content).unwrap();

    let persistence = WorkflowPersistence::new(&path);
    let listed = persistence.list_executions().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    // load_execution still finds the valid record
    assert_eq!(
        persistence.load_execution(&id).unwrap().workflow_name,
        "test_wf"
    );
}

// ---------------------------------------------------------------------------
// 横扫五轮（2026-11-06）：终态执行文件按龄清扫（sweep_stale_execution_files）
// 生产布局一执行一文件；挂起态永不删；空/全垃圾行回落 mtime 闸。
// 带参钟免测试：ZERO=全过期、100 天=全新鲜（夹具 mtime 不可回拨，不引 filetime）。
// ---------------------------------------------------------------------------

/// 追加两行同 ID 快照（Pending → terminal），模拟真实生命周期。
fn write_lifecycle(dir: &Path, name: &str, final_state: ExecutionState) -> std::path::PathBuf {
    use std::io::Write;
    let mut e = Execution::new(name.to_string(), HashMap::new());
    e.state = ExecutionState::Pending;
    let mut line = serde_json::to_string(&e).unwrap();
    line.push('\n');
    e.state = final_state;
    e.ended_at = Some(Local::now() - TimeDelta::days(60));
    line.push_str(&serde_json::to_string(&e).unwrap());
    line.push('\n');
    let path = dir.join(format!("{}_{}.jsonl", name, e.id));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(line.as_bytes()).unwrap();
    path
}

#[test]
fn sweep_executions_zero_age_clears_terminal_keeps_suspended() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("executions");
    std::fs::create_dir_all(&root).unwrap();

    let done = write_lifecycle(&root, "wf-done", ExecutionState::Completed);
    let failed = write_lifecycle(&root, "wf-failed", ExecutionState::Failed);
    let running = write_lifecycle(&root, "wf-running", ExecutionState::Running);
    let waiting = write_lifecycle(&root, "wf-waiting", ExecutionState::Waiting);
    // 空文件：无可恢复数据 → mtime 闸
    let empty = root.join("wf-empty_deadbeef.jsonl");
    std::fs::write(&empty, b"").unwrap();
    // 全垃圾行：同上
    let garbage = root.join("wf-garbage_deadbeef.jsonl");
    std::fs::write(&garbage, b"not json\nalso not json\n").unwrap();
    // 非 .jsonl 后缀与子目录不碰
    let stray = root.join("stray.txt");
    std::fs::write(&stray, b"keep me").unwrap();
    let sub = root.join("subdir");
    std::fs::create_dir_all(sub.join("wf-x_1.jsonl")).unwrap();

    let removed = super::sweep_stale_execution_files(&root, std::time::Duration::ZERO);
    // 终态 2（done/failed）+ 空文件 + 垃圾文件 = 4；挂起态（running/waiting）永不删。
    assert_eq!(removed, 4, "terminal x2 + empty + garbage, suspended kept");
    assert!(!done.exists());
    assert!(!failed.exists());
    assert!(running.exists(), "Running 挂起态永不删");
    assert!(waiting.exists(), "Waiting 挂起态永不删");
    assert!(!empty.exists());
    assert!(!garbage.exists());
    assert!(stray.exists());
    assert!(sub.join("wf-x_1.jsonl").exists());
}

#[test]
fn sweep_executions_large_age_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("executions");
    std::fs::create_dir_all(&root).unwrap();

    let done = write_lifecycle(&root, "wf-done", ExecutionState::Completed);
    let waiting = write_lifecycle(&root, "wf-waiting", ExecutionState::Waiting);

    let removed =
        super::sweep_stale_execution_files(&root, std::time::Duration::from_secs(100 * 24 * 3600));
    assert_eq!(removed, 0);
    assert!(done.exists());
    assert!(waiting.exists());
}

#[test]
fn sweep_executions_missing_dir_is_zero() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("no-such-dir");
    let removed = super::sweep_stale_execution_files(&root, std::time::Duration::ZERO);
    assert_eq!(removed, 0);
}
