//! JSONL-based persistence for workflow executions.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{TimeDelta, Utc};

use crate::types::Execution;

/// Error type for persistence operations.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("execution not found: {0}")]
    NotFound(String),
}

/// JSONL-backed persistence store for workflow executions.
///
/// Each execution is appended as a single JSON line to the configured file.
/// Loading reads all lines and keeps the most recent entry for each execution ID.
pub struct WorkflowPersistence {
    file_path: PathBuf,
}

impl WorkflowPersistence {
    /// Create a new persistence store backed by the given file path.
    pub fn new<P: AsRef<Path>>(file_path: P) -> Self {
        Self {
            file_path: file_path.as_ref().to_path_buf(),
        }
    }

    /// Save (append) an execution record to the JSONL file.
    pub fn save_execution(&self, execution: &Execution) -> Result<(), PersistenceError> {
        // Ensure parent directory exists.
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_path)?;

        let mut line = serde_json::to_string(execution)?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Load the most recent execution with the given ID.
    pub fn load_execution(&self, id: &str) -> Result<Execution, PersistenceError> {
        let executions = self.list_executions()?;
        executions
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| PersistenceError::NotFound(id.to_string()))
    }

    /// List all executions, keeping only the most recent record for each ID.
    pub fn list_executions(&self) -> Result<Vec<Execution>, PersistenceError> {
        if !self.file_path.exists() {
            return Ok(vec![]);
        }

        let file = std::fs::File::open(&self.file_path)?;
        let reader = std::io::BufReader::new(file);

        // Later entries overwrite earlier ones for the same ID.
        let mut map: HashMap<String, Execution> = HashMap::new();

        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(execution) = serde_json::from_str::<Execution>(trimmed) {
                map.insert(execution.id.clone(), execution);
            }
        }

        Ok(map.into_values().collect())
    }

    /// Delete a specific execution by workflow name and execution ID.
    ///
    /// Reads all records, removes the one matching the given ID, and rewrites
    /// the file without it. Returns `Ok(true)` if the execution was found and
    /// deleted, `Ok(false)` if it was not found.
    pub fn delete_execution(
        &self,
        _workflow_name: &str,
        id: &str,
    ) -> Result<bool, PersistenceError> {
        if !self.file_path.exists() {
            return Ok(false);
        }

        let all = self.list_executions()?;
        let before_len = all.len();
        let remaining: Vec<&Execution> = all.iter().filter(|e| e.id != id).collect();

        if remaining.len() == before_len {
            // Not found
            return Ok(false);
        }

        // Rewrite the file with only the remaining executions
        self.rewrite_file(&remaining)?;
        Ok(true)
    }

    /// Remove executions older than the specified number of days.
    ///
    /// Compares each execution's `started_at` timestamp against the current
    /// time. Executions started more than `max_age_days` ago are removed.
    /// Returns the number of executions that were cleaned up.
    pub fn cleanup_old_executions(
        &self,
        max_age_days: u64,
    ) -> Result<usize, PersistenceError> {
        if !self.file_path.exists() {
            return Ok(0);
        }

        let all = self.list_executions()?;
        let cutoff = Utc::now() - TimeDelta::days(max_age_days as i64);

        let remaining: Vec<&Execution> = all
            .iter()
            .filter(|e| e.started_at > cutoff)
            .collect();

        let removed = all.len() - remaining.len();
        if removed > 0 {
            self.rewrite_file(&remaining)?;
        }

        Ok(removed)
    }

    /// Rewrite the JSONL file with the given executions.
    fn rewrite_file(&self, executions: &[&Execution]) -> Result<(), PersistenceError> {
        // Write to a temp file first, then rename for atomicity
        let tmp_path = self.file_path.with_extension("jsonl.tmp");

        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)?;

            for execution in executions {
                let mut line = serde_json::to_string(execution)?;
                line.push('\n');
                file.write_all(line.as_bytes())?;
            }
        }

        // Atomic rename
        std::fs::rename(&tmp_path, &self.file_path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
