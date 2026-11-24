//! Trace store - JSONL persistence for trace events.
//!
//! Persists trace events to JSONL files for later analysis by the reflector.
//! Supports time-based filtering and age-based cleanup.

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::trace::TraceEvent;

/// A store that persists trace events to JSONL.
pub struct TraceStore {
    path: PathBuf,
    /// 写互斥（BUG-20261125-004 同族横向修复）：`append`（record_trace_event
    /// 每次工具派发都写）与 `cleanup` 的读→滤→整文件写回共用——重写读在
    /// 锁外时，读→写窗内并发 append 的事件不在快照里，写回即静默丢失。
    /// 旧注释只修了崩溃截断窗（write_file_atomic），丢失窗未修。实例锁
    /// 足够：Forge `init_trace` 持单一实例，append 与 cleanup 同源。
    /// tokio Mutex（guard Send，临界区含 await）。
    write_lock: tokio::sync::Mutex<()>,
}

impl TraceStore {
    /// Create a new trace store at the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Append a trace event to the JSONL file.
    pub async fn append(&self, event: &TraceEvent) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let _w = self.write_lock.lock().await;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        let mut line = serde_json::to_string(event)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }

    /// Read all trace events from the JSONL file.
    pub async fn read_all(&self) -> std::io::Result<Vec<TraceEvent>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&self.path).await?;
        let mut events = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(event) = serde_json::from_str::<TraceEvent>(line) {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Read trace events that occurred at or after the given timestamp.
    ///
    /// Parses each event's `timestamp` field (RFC 3339) and filters by
    /// comparing against `since`. Events with unparseable timestamps are
    /// excluded.
    pub async fn read_traces_since(
        &self,
        since: chrono::DateTime<chrono::Local>,
    ) -> std::io::Result<Vec<TraceEvent>> {
        let all = self.read_all().await?;
        let filtered: Vec<TraceEvent> = all
            .into_iter()
            .filter(|e| {
                chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                    .map(|dt| dt.with_timezone(&chrono::Local) >= since)
                    .unwrap_or(false)
            })
            .collect();
        Ok(filtered)
    }

    /// Count the number of stored events.
    pub async fn count(&self) -> std::io::Result<usize> {
        let events = self.read_all().await?;
        Ok(events.len())
    }

    /// Clear all stored events.
    pub async fn clear(&self) -> std::io::Result<()> {
        // 与 cleanup 写回同锁（BUG-20261125-004 同族横向修复）：否则
        // cleanup 持旧快照 → clear 删文件 → cleanup 写回 = 已清空内容复活。
        let _w = self.write_lock.lock().await;
        if self.path.exists() {
            tokio::fs::remove_file(&self.path).await?;
        }
        Ok(())
    }

    /// Remove traces older than `max_age_days` days.
    ///
    /// Unlike `clear()` which removes all traces, this preserves recent
    /// events. Reads all events, filters out old ones, and rewrites the
    /// file with only the remaining events.
    ///
    /// Returns the number of traces removed.
    pub async fn cleanup(&self, max_age_days: u64) -> std::io::Result<usize> {
        if !self.path.exists() {
            return Ok(0);
        }

        // 入口钳制（R735）：u64→i64 回绕负值 = cutoff 落未来 = 全删事件；
        // 上界单一真相源 time_bounds::MAX_AGE_DAYS。
        let max_age_days = max_age_days.min(nemesis_utils::time_bounds::MAX_AGE_DAYS as u64);
        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days as i64);
        // RMW 全程持锁（BUG-20261125-004 同族横向修复）：read_all 在锁外时，
        // 读→写窗内 record_trace_event 并发 append 的事件不在快照里，写回
        // 即静默丢失（append 频率 = 每次工具派发，丢失概率高）。
        let _w = self.write_lock.lock().await;
        let all = self.read_all().await?;
        let original_count = all.len();

        let kept: Vec<TraceEvent> = all
            .into_iter()
            .filter(|e| {
                chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                    .map(|dt| dt.with_timezone(&chrono::Local) >= cutoff)
                    .unwrap_or(true) // Keep events with unparseable timestamps
            })
            .collect();

        let removed = original_count - kept.len();

        if removed > 0 {
            // 原子重写：cleanup 与 append 并发
            // （record_trace_event 每次工具派发都写同一文件），旧实现
            // write+truncate 原地重写有两个丢失窗口——重写中途 crash 丢
            // 整个 trace 文件；重写期间到达的 append 被 truncate 抹掉。
            // 与 experience_store F- / registry F- 同族，统一走
            // write_file_atomic（唯一临时名 + rename）：崩溃面收敛为
            // 「旧文件或新文件」，不产半截文件。
            if kept.is_empty() {
                tokio::fs::remove_file(&self.path).await?;
            } else {
                let mut content = String::new();
                for event in &kept {
                    content.push_str(&serde_json::to_string(event).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                    })?);
                    content.push('\n');
                }
                nemesis_utils::file::write_file_atomic(
                    &self.path.to_string_lossy(),
                    content.as_bytes(),
                    0o644,
                )
                .map_err(std::io::Error::other)?;
            }
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests;
