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
        since: chrono::DateTime<chrono::Utc>,
    ) -> std::io::Result<Vec<TraceEvent>> {
        let all = self.read_all().await?;
        let filtered: Vec<TraceEvent> = all
            .into_iter()
            .filter(|e| {
                chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                    .map(|dt| dt.with_timezone(&chrono::Utc) >= since)
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

        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days as i64);
        let all = self.read_all().await?;
        let original_count = all.len();

        let kept: Vec<TraceEvent> = all
            .into_iter()
            .filter(|e| {
                chrono::DateTime::parse_from_rfc3339(&e.timestamp)
                    .map(|dt| dt.with_timezone(&chrono::Utc) >= cutoff)
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
            }
        }

        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_event(event_type: &str) -> TraceEvent {
        TraceEvent {
            id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            session_key: "test-session".into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            data: serde_json::json!({"test": true}),
        }
    }

    fn make_event_at(event_type: &str, time: chrono::DateTime<chrono::Utc>) -> TraceEvent {
        TraceEvent {
            id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            session_key: "test-session".into(),
            timestamp: time.to_rfc3339(),
            data: serde_json::json!({"test": true}),
        }
    }

    #[tokio::test]
    async fn test_append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        store.append(&make_event("tool_call")).await.unwrap();
        store.append(&make_event("llm_response")).await.unwrap();

        let events = store.read_all().await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type, "tool_call");
        assert_eq!(events[1].event_type, "llm_response");
    }

    #[tokio::test]
    async fn test_count() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        assert_eq!(store.count().await.unwrap(), 0);
        store.append(&make_event("test")).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_clear() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        store.append(&make_event("test")).await.unwrap();
        assert!(path.exists());

        store.clear().await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn test_empty_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let store = TraceStore::new(&path);

        let events = store.read_all().await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_read_traces_since() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        let now = chrono::Utc::now();
        let two_hours_ago = now - chrono::Duration::hours(2);
        let one_hour_ago = now - chrono::Duration::hours(1);

        store.append(&make_event_at("old_event", two_hours_ago)).await.unwrap();
        store.append(&make_event_at("recent_event", one_hour_ago)).await.unwrap();
        store.append(&make_event_at("new_event", now)).await.unwrap();

        // Read only events from the last 90 minutes
        let cutoff = now - chrono::Duration::minutes(90);
        let recent = store.read_traces_since(cutoff).await.unwrap();

        assert_eq!(recent.len(), 2);
        assert!(recent.iter().any(|e| e.event_type == "recent_event"));
        assert!(recent.iter().any(|e| e.event_type == "new_event"));
        assert!(!recent.iter().any(|e| e.event_type == "old_event"));
    }

    #[tokio::test]
    async fn test_read_traces_since_all_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        store.append(&make_event("a")).await.unwrap();
        store.append(&make_event("b")).await.unwrap();

        let long_ago = chrono::Utc::now() - chrono::Duration::days(365);
        let all = store.read_traces_since(long_ago).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_read_traces_since_none_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        let past = chrono::Utc::now() - chrono::Duration::hours(2);
        store.append(&make_event_at("old", past)).await.unwrap();

        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let none = store.read_traces_since(future).await.unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn test_cleanup_removes_old() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        let now = chrono::Utc::now();
        let two_days_ago = now - chrono::Duration::days(2);
        let one_hour_ago = now - chrono::Duration::hours(1);

        store.append(&make_event_at("old_event", two_days_ago)).await.unwrap();
        store.append(&make_event_at("recent_event", one_hour_ago)).await.unwrap();
        store.append(&make_event_at("new_event", now)).await.unwrap();

        let removed = store.cleanup(1).await.unwrap();
        assert_eq!(removed, 1);

        // Verify remaining events
        let remaining = store.read_all().await.unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(!remaining.iter().any(|e| e.event_type == "old_event"));
        assert!(remaining.iter().any(|e| e.event_type == "recent_event"));
        assert!(remaining.iter().any(|e| e.event_type == "new_event"));
    }

    #[tokio::test]
    async fn test_cleanup_removes_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        let past = chrono::Utc::now() - chrono::Duration::days(10);
        store.append(&make_event_at("old1", past)).await.unwrap();
        store.append(&make_event_at("old2", past)).await.unwrap();

        let removed = store.cleanup(5).await.unwrap();
        assert_eq!(removed, 2);
        assert!(!path.exists()); // File deleted when empty
    }

    #[tokio::test]
    async fn test_cleanup_nothing_to_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);

        store.append(&make_event("fresh")).await.unwrap();

        let removed = store.cleanup(30).await.unwrap();
        assert_eq!(removed, 0);

        let remaining = store.read_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    async fn test_cleanup_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.jsonl");
        let store = TraceStore::new(&path);

        let removed = store.cleanup(30).await.unwrap();
        assert_eq!(removed, 0);
    }

    // --- Additional trace_store tests ---

    #[tokio::test]
    async fn test_append_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);
        let event = TraceEvent {
            id: "evt-123".into(),
            event_type: "custom_type".into(),
            session_key: "session-abc".into(),
            timestamp: "2026-05-01T12:00:00Z".into(),
            data: serde_json::json!({"key": "value", "nested": {"a": 1}}),
        };
        store.append(&event).await.unwrap();
        let events = store.read_all().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "evt-123");
        assert_eq!(events[0].event_type, "custom_type");
        assert_eq!(events[0].session_key, "session-abc");
    }

    #[tokio::test]
    async fn test_read_all_ignores_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);
        store.append(&make_event("valid")).await.unwrap();
        // Append malformed data
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "not json").unwrap();
        writeln!(f, "").unwrap();
        let events = store.read_all().await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "valid");
    }

    #[tokio::test]
    async fn test_count_after_multiple_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("traces.jsonl");
        let store = TraceStore::new(&path);
        for i in 0..5 {
            store.append(&make_event(&format!("evt-{}", i))).await.unwrap();
        }
        assert_eq!(store.count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_clear_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.jsonl");
        let store = TraceStore::new(&path);
        store.clear().await.unwrap(); // Should not panic
    }

    #[tokio::test]
    async fn test_read_traces_since_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        let store = TraceStore::new(&path);
        let result = store.read_traces_since(chrono::Utc::now()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_append_many_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("many.jsonl");
        let store = TraceStore::new(&path);
        for i in 0..20 {
            store.append(&make_event(&format!("type-{}", i))).await.unwrap();
        }
        assert_eq!(store.count().await.unwrap(), 20);
    }

    #[tokio::test]
    async fn test_cleanup_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.jsonl");
        let store = TraceStore::new(&path);
        let now = chrono::Utc::now();
        let three_days_ago = now - chrono::Duration::days(3);
        store.append(&make_event_at("old", three_days_ago)).await.unwrap();
        store.append(&make_event("new")).await.unwrap();
        let removed = store.cleanup(2).await.unwrap();
        assert_eq!(removed, 1);
        let remaining = store.read_all().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].event_type, "new");
    }
}
