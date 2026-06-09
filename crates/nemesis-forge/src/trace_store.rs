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
}

impl TraceStore {
    /// Create a new trace store at the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Append a trace event to the JSONL file.
    pub async fn append(&self, event: &TraceEvent) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        let mut line = serde_json::to_string(event).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
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

        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days as i64);
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
            // Rewrite file with only kept events
            if kept.is_empty() {
                tokio::fs::remove_file(&self.path).await?;
            } else {
                let mut file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&self.path)
                    .await?;
                for event in &kept {
                    let mut line = serde_json::to_string(event).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
                    })?;
                    line.push('\n');
                    file.write_all(line.as_bytes()).await?;
                }
                file.flush().await?;
            }
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests;
