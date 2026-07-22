//! JSONL-based persistence for workflow executions.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{Local, TimeDelta};

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
    pub fn cleanup_old_executions(&self, max_age_days: u64) -> Result<usize, PersistenceError> {
        if !self.file_path.exists() {
            return Ok(0);
        }

        let all = self.list_executions()?;
        let cutoff = Local::now() - TimeDelta::days(max_age_days as i64);

        let remaining: Vec<&Execution> = all.iter().filter(|e| e.started_at > cutoff).collect();

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
mod tests;
