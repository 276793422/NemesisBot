//! JSONL-based persistence for workflow executions.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, TimeDelta};

use crate::types::{Execution, ExecutionState};

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

/// 终态执行记录按龄清扫（横扫五轮，2026-11-06）。
///
/// 生产布局是一执行一文件（engine `persist_execution`：
/// `{workflow_name}_{execution_id}.jsonl`，追加即全量快照，最后一行 =
/// 当前状态）。全仓此前没有删除路径——`cleanup_old_executions` /
/// `delete_execution` 是单文件模型的历史遗留，零生产调用（
/// BUG-20261102-298 观察项只裁决了「枚举漏页」面）。长跑实例下文件数
/// 与完成数线性增长。
///
/// 删除条件（同时满足）：文件内最新快照的执行状态 ∈ 终态
/// （Completed / Failed / Cancelled）**且** `ended_at`（缺省回落文件
/// mtime）早于 `max_age`。Pending / Running / Waiting **永不删**——
/// 工作流可在 human_review 闸挂起任意久，挂起期间文件 mtime 不刷新，
/// 纯 mtime 闸会误删合法挂起执行。空文件 / 全垃圾行（无可解析记录）
/// 回落 mtime 闸（没有任何可恢复数据可丢）；`list_executions` 读失败
/// 本轮保守跳过。非 `.jsonl` 后缀条目与子目录不碰。删除走 fsutil
/// 韧性包装（尽力而为，残留交下轮）。返回清除文件数。
pub fn sweep_stale_execution_files(dir: &Path, max_age: std::time::Duration) -> usize {
    use nemesis_utils::fsutil;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") || !path.is_file() {
            continue;
        }
        // 最新快照：list_executions 按 ID 去重保留后者；一文件一执行的
        // 生产形态下即最后一行。防御性多 ID 文件取 ended_at（缺省
        // started_at）最新的那条记录裁决。
        let freshest = match WorkflowPersistence::new(&path).list_executions() {
            Ok(list) if list.is_empty() => None,
            Ok(list) => {
                let pick = |e: &Execution| e.ended_at.unwrap_or(e.started_at);
                Some(
                    list.into_iter()
                        .max_by_key(pick)
                        .expect("non-empty list has a max"),
                )
            }
            // 读失败：本轮保守跳过，下轮重试。
            Err(_) => continue,
        };
        let expired = match freshest {
            Some(e) => {
                if !matches!(
                    e.state,
                    ExecutionState::Completed | ExecutionState::Failed | ExecutionState::Cancelled
                ) {
                    continue;
                }
                stale(e.ended_at.as_ref(), &path, now, max_age)
            }
            // 空 / 全垃圾行：无可恢复数据，回落 mtime 闸。
            None => stale(None, &path, now, max_age),
        };
        if expired && fsutil::remove_file_resilient(&path) {
            removed += 1;
        }
    }
    removed
}

/// mtime / `ended_at` 按龄判定：锚点缺省回落文件 mtime；无锚点或未来
/// 时间戳（时钟偏斜）保守视为新鲜。
fn stale(
    anchor: Option<&DateTime<Local>>,
    path: &Path,
    now: std::time::SystemTime,
    max_age: std::time::Duration,
) -> bool {
    let anchor_sys = anchor.map(|dt| std::time::SystemTime::from(*dt));
    let fresh = match anchor_sys {
        Some(t) => now.duration_since(t).ok().map(|age| age < max_age),
        None => path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|age| age < max_age),
    };
    !fresh.unwrap_or(true)
}

#[cfg(test)]
mod tests;
