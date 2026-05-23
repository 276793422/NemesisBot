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

    let deleted = persistence.delete_execution("wf1", "nonexistent_id").unwrap();
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

    let e2 = PersistenceError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "file gone"));
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
    old.started_at = Utc::now() - TimeDelta::days(60);
    old.ended_at = Some(Utc::now() - TimeDelta::days(60));
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
