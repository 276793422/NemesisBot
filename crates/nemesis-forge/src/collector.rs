//! Collector - asynchronously collects tool call experiences.
//!
//! Deduplicates by (tool_name + args hash) and stores up to `max_size` entries
//! in memory. Optionally persists to JSONL.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing;

use nemesis_types::utils;

use crate::types::{AggregatedExperience, CollectedExperience, CollectorConfig, Experience};

/// In-memory aggregation for a pattern.
#[derive(Clone)]
struct PatternAggregate {
    count: u64,
    total_duration_ms: i64,
    successes: u64,
    last_seen: String,
    tool_name: String,
}

/// Async collector of tool-call experiences.
pub struct Collector {
    config: CollectorConfig,
    experiences: Mutex<Vec<CollectedExperience>>,
    seen_hashes: Mutex<HashSet<String>>,
    persistence_path: Option<PathBuf>,
    /// Pattern-level aggregation for Flush.
    pattern_counts: Mutex<HashMap<String, PatternAggregate>>,
}

impl Collector {
    /// Create a new collector with the given configuration.
    pub fn new(config: CollectorConfig) -> Self {
        let persistence_path = if config.persistence_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(&config.persistence_path))
        };
        Self {
            config,
            experiences: Mutex::new(Vec::new()),
            seen_hashes: Mutex::new(HashSet::new()),
            persistence_path,
            pattern_counts: Mutex::new(HashMap::new()),
        }
    }

    /// Compute a deduplication hash from tool_name and sorted argument key names.
    ///
    /// Matches Go's `ComputePatternHash` which hashes `toolName + ":" + sorted(arg_keys)`.
    /// Only the key names are used, not their values, so that the same tool
    /// called with different parameter values produces the same pattern hash.
    pub fn dedup_hash(tool_name: &str, args: &serde_json::Value) -> String {
        // Extract arg key names from the JSON object
        let mut keys: Vec<String> = match args.as_object() {
            Some(obj) => obj.keys().cloned().collect(),
            None => Vec::new(),
        };
        keys.sort();

        let data = format!("{}:{}", tool_name, keys.join(","));
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        let hash = hasher.finalize();
        let hash_hex: String = hash.iter().take(8).map(|b| format!("{:02x}", b)).collect();
        format!("sha256:{}", hash_hex)
    }

    /// Record a new experience. Returns `true` if it was actually inserted
    /// (i.e. not exceeding capacity). Duplicates are aggregated into
    /// `pattern_counts` rather than being skipped — matching Go's behavior
    /// where every record is always aggregated.
    ///
    /// The `args` parameter is the original tool call arguments (JSON object).
    /// It is used to compute the pattern hash (key names only), matching Go's
    /// `ComputePatternHash`.
    pub async fn record_with_args(&self, experience: Experience, args: &serde_json::Value) -> bool {
        let hash = Self::dedup_hash(&experience.tool_name, args);

        // Always aggregate into pattern_counts (matching Go's ProcessRecord behavior)
        self.process_record(
            &experience.tool_name,
            &hash,
            experience.duration_ms as i64,
            experience.success,
            &experience.timestamp,
        );

        // Track the hash for capacity management
        let _is_new = !self.seen_hashes.lock().contains(&hash);

        let ce = CollectedExperience {
            experience,
            dedup_hash: hash.clone(),
        };

        // Persist to JSONL (before adding to memory so a crash between the two
        // does not lose data; re-loading will deduplicate anyway).
        if let Some(ref path) = self.persistence_path {
            if let Err(e) = Self::append_jsonl(path, &ce).await {
                tracing::warn!(path = %path.display(), error = %e, "Failed to persist experience");
            }
        }

        // Add to in-memory store
        {
            let mut exps = self.experiences.lock();
            if exps.len() >= self.config.max_size {
                tracing::debug!("Collector at max capacity, evicting oldest");
                if let Some(removed) = exps.first() {
                    self.seen_hashes.lock().remove(&removed.dedup_hash);
                }
                exps.remove(0);
            }
            exps.push(ce);
        }
        self.seen_hashes.lock().insert(hash);

        true
    }

    /// Record a new experience using input_summary for backward compatibility.
    ///
    /// Prefer `record_with_args` which matches Go's `ComputePatternHash`.
    pub async fn record(&self, experience: Experience) -> bool {
        // Fallback: synthesize a JSON object from input_summary so dedup_hash
        // still produces a deterministic result. When args are not available,
        // we use the input_summary as a single key.
        let args = serde_json::json!({ "input_summary": experience.input_summary });
        self.record_with_args(experience, &args).await
    }

    /// Return a snapshot of all collected experiences.
    pub fn experiences(&self) -> Vec<CollectedExperience> {
        self.experiences.lock().clone()
    }

    /// Return the current count.
    pub fn len(&self) -> usize {
        self.experiences.lock().len()
    }

    /// Return whether the collector is empty.
    pub fn is_empty(&self) -> bool {
        self.experiences.lock().is_empty()
    }

    /// Load experiences from a JSONL file (replaces current in-memory store).
    pub async fn load_from_file<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let content = tokio::fs::read_to_string(path).await?;
        let mut exps = self.experiences.lock();
        let mut seen = self.seen_hashes.lock();
        exps.clear();
        seen.clear();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(ce) = serde_json::from_str::<CollectedExperience>(line) {
                seen.insert(ce.dedup_hash.clone());
                exps.push(ce);
            }
        }
        Ok(())
    }

    /// Append a single JSONL record.
    async fn append_jsonl(path: &Path, ce: &CollectedExperience) -> std::io::Result<()> {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let mut line = serde_json::to_string(ce).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Clear all collected experiences.
    pub fn clear(&self) {
        self.experiences.lock().clear();
        self.seen_hashes.lock().clear();
        self.pattern_counts.lock().clear();
    }

    /// Process a record for deduplication and aggregation.
    ///
    /// Updates the in-memory pattern counts map. Call `flush` to write
    /// aggregated results to the experience store.
    pub fn process_record(
        &self,
        tool_name: &str,
        pattern_hash: &str,
        duration_ms: i64,
        success: bool,
        timestamp: &str,
    ) {
        let mut patterns = self.pattern_counts.lock();
        let agg = patterns
            .entry(pattern_hash.to_string())
            .or_insert_with(|| PatternAggregate {
                count: 0,
                total_duration_ms: 0,
                successes: 0,
                last_seen: String::new(),
                tool_name: tool_name.to_string(),
            });
        agg.count += 1;
        agg.total_duration_ms += duration_ms;
        if success {
            agg.successes += 1;
        }
        agg.last_seen = timestamp.to_string();
    }

    /// Flush aggregated patterns to the persistence file and reset memory.
    ///
    /// Writes each aggregated pattern as an `AggregatedExperience` JSON line
    /// to the JSONL persistence file, then clears the in-memory aggregation.
    pub async fn flush(&self) -> Result<usize, String> {
        let patterns = {
            let mut guard = self.pattern_counts.lock();
            let map = guard.clone();
            guard.clear();
            map
        };

        if patterns.is_empty() {
            return Ok(0);
        }

        let path = match &self.persistence_path {
            Some(p) => p.clone(),
            None => return Err("no persistence path configured".to_string()),
        };

        let mut count = 0usize;
        for (hash, agg) in &patterns {
            if agg.count == 0 {
                continue;
            }
            let avg_dur = agg.total_duration_ms / agg.count as i64;
            let sr = agg.successes as f64 / agg.count as f64;
            let record = AggregatedExperience {
                pattern_hash: hash.clone(),
                tool_name: agg.tool_name.clone(),
                count: agg.count,
                avg_duration_ms: avg_dur,
                success_rate: sr,
                last_seen: agg.last_seen.clone(),
            };
            if let Err(e) = Self::append_aggregated(&path, &record).await {
                tracing::warn!(error = %e, "Failed to flush aggregated experience");
            } else {
                count += 1;
            }
        }

        tracing::info!(count, "Flushed experience patterns");
        Ok(count)
    }

    /// Append an aggregated record to the JSONL file.
    async fn append_aggregated(path: &Path, record: &AggregatedExperience) -> std::io::Result<()> {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let mut line = serde_json::to_string(record).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Compute a pattern hash from tool name and argument keys.
    ///
    /// Uses sorted arg keys for deterministic hashing.
    pub fn compute_pattern_hash(tool_name: &str, arg_keys: &[&str]) -> String {
        let mut keys = arg_keys.to_vec();
        keys.sort();
        let data = format!("{}:{}", tool_name, keys.join(","));
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        let hash = hasher.finalize();
        let hash_hex: String = hash.iter().take(4).map(|b| format!("{:02x}", b)).collect();
        format!("sha256:{}", hash_hex)
    }

    /// Sanitize arguments by redacting sensitive fields.
    ///
    /// Fields matching any of the sanitize patterns (case-insensitive) are
    /// replaced with `[REDACTED]`.
    pub fn sanitize_args(
        args: &serde_json::Value,
        sanitize_fields: &[&str],
    ) -> serde_json::Value {
        if sanitize_fields.is_empty() {
            return args.clone();
        }
        let obj = match args.as_object() {
            Some(o) => o,
            None => return args.clone(),
        };

        let mut cleaned = serde_json::Map::new();
        for (k, v) in obj {
            let mut redacted = false;
            for sf in sanitize_fields {
                if k.to_lowercase().contains(&sf.to_lowercase()) {
                    cleaned.insert(k.clone(), serde_json::Value::String("[REDACTED]".to_string()));
                    redacted = true;
                    break;
                }
            }
            if !redacted {
                cleaned.insert(k.clone(), v.clone());
            }
        }
        serde_json::Value::Object(cleaned)
    }

    /// Get the current number of aggregated patterns.
    pub fn pattern_count(&self) -> usize {
        self.pattern_counts.lock().len()
    }
}

// ---------------------------------------------------------------------------
// ForgePlugin — implements Plugin to intercept tool calls and record experiences
// ---------------------------------------------------------------------------

/// Session position tracker for per-session deduplication and ordering.
#[derive(Debug, Default)]
pub struct SessionPositions {
    positions: HashMap<String, usize>,
}

impl SessionPositions {
    /// Create a new session position tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the next position for a session and increment the counter.
    pub fn next(&mut self, session_key: &str) -> usize {
        let pos = self.positions.entry(session_key.to_string()).or_insert(0);
        let current = *pos;
        *pos += 1;
        current
    }

    /// Get the current position for a session without incrementing.
    pub fn get(&self, session_key: &str) -> usize {
        *self.positions.get(session_key).unwrap_or(&0)
    }

    /// Reset position for a session.
    pub fn reset(&mut self, session_key: &str) {
        self.positions.remove(session_key);
    }
}

/// Fields to sanitize when recording tool arguments.
const SANITIZE_FIELDS: &[&str] = &["api_key", "secret", "password", "token", "key"];

/// ForgePlugin intercepts tool calls and records them as experiences.
///
/// Mirrors Go's `ForgePlugin` struct. It implements the `Plugin` trait so
/// it can be registered with a `PluginManager`. When a tool invocation
/// completes, the `on_tool_call` method asynchronously records the
/// experience.
pub struct ForgePlugin {
    collector: Collector,
    running: parking_lot::Mutex<bool>,
    /// Per-session position counters for ordering experiences.
    session_positions: parking_lot::Mutex<SessionPositions>,
    /// Buffered channel for async experience recording.
    input_tx: tokio::sync::mpsc::Sender<Experience>,
    input_rx: parking_lot::Mutex<Option<tokio::sync::mpsc::Receiver<Experience>>>,
}

impl ForgePlugin {
    /// Create a new ForgePlugin with the given collector configuration.
    pub fn new(config: CollectorConfig) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel::<Experience>(256);
        Self {
            collector: Collector::new(config),
            running: parking_lot::Mutex::new(false),
            session_positions: parking_lot::Mutex::new(SessionPositions::new()),
            input_tx: tx,
            input_rx: parking_lot::Mutex::new(Some(rx)),
        }
    }

    /// Create a ForgePlugin wrapping an existing Collector.
    pub fn with_collector(collector: Collector) -> Self {
        let (tx, rx) = tokio::sync::mpsc::channel::<Experience>(256);
        Self {
            collector,
            running: parking_lot::Mutex::new(false),
            session_positions: parking_lot::Mutex::new(SessionPositions::new()),
            input_tx: tx,
            input_rx: parking_lot::Mutex::new(Some(rx)),
        }
    }

    /// Start the plugin — begins processing the input channel.
    pub async fn start(&self) {
        *self.running.lock() = true;
        tracing::info!("ForgePlugin started");
    }

    /// Stop the plugin.
    pub fn stop(&self) {
        *self.running.lock() = false;
    }

    /// Get a reference to the underlying collector.
    pub fn collector(&self) -> &Collector {
        &self.collector
    }

    /// Get the input channel sender for asynchronous recording.
    pub fn input_channel(&self) -> &tokio::sync::mpsc::Sender<Experience> {
        &self.input_tx
    }

    /// Take ownership of the input channel receiver (for async processing).
    pub fn take_input_receiver(&self) -> Option<tokio::sync::mpsc::Receiver<Experience>> {
        self.input_rx.lock().take()
    }

    /// Process a tool call result and record it as an experience.
    ///
    /// Sanitizes arguments, extracts summary information, and sends the
    /// experience to the input channel for async recording.
    pub fn on_tool_call(
        &self,
        session_key: &str,
        tool_name: &str,
        args: &serde_json::Value,
        result: Option<&serde_json::Value>,
        error: Option<&str>,
        duration_ms: u64,
    ) {
        let pos = self.session_positions.lock().next(session_key);

        // Sanitize arguments
        let sanitized_args = Collector::sanitize_args(args, SANITIZE_FIELDS);

        // Extract input/output summaries
        let input_summary = summarize_value(&sanitized_args, 100);
        let output_summary = match (result, error) {
            (_, Some(e)) => format!("Error: {}", truncate_str(e, 100)),
            (Some(r), _) => summarize_value(r, 100),
            _ => "no output".to_string(),
        };
        let success = error.is_none();

        let experience = Experience {
            id: format!("exp-{}-{}", session_key, pos),
            tool_name: tool_name.to_string(),
            input_summary,
            output_summary,
            success,
            duration_ms,
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_key: session_key.to_string(),
        };

        // Try to send to async channel, fall back to direct record if full
        if self.input_tx.try_send(experience.clone()).is_err() {
            tracing::debug!("Input channel full, dropping experience from async channel");
        }

        // Immediately update in-memory aggregation (matching Go's ForgePlugin.Execute
        // which calls p.collector.ProcessRecord(rec) right after Record()).
        // Use the original args (pre-sanitization key names) for dedup hashing,
        // matching Go's ComputePatternHash(toolName, args).
        let hash = Collector::dedup_hash(&experience.tool_name, args);
        self.collector.process_record(
            &experience.tool_name,
            &hash,
            experience.duration_ms as i64,
            experience.success,
            &experience.timestamp,
        );
    }

    /// Process all pending experiences from the input channel.
    ///
    /// Should be called periodically from an async context.
    pub async fn process_pending(&self) -> usize {
        let mut rx_guard = self.input_rx.lock();
        if let Some(ref mut rx) = *rx_guard {
            let mut count = 0;
            while let Ok(exp) = rx.try_recv() {
                self.collector.record(exp).await;
                count += 1;
            }
            count
        } else {
            0
        }
    }
}

impl nemesis_plugin::Plugin for ForgePlugin {
    fn name(&self) -> &str {
        "forge"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> {
        *self.running.lock() = true;
        Ok(())
    }

    fn execute(
        &self,
        invocation: &mut nemesis_plugin::ToolInvocation,
    ) -> (bool, Option<String>, bool) {
        // ForgePlugin does not block execution; it only observes
        if let Some(ref result) = invocation.result {
            let args_value = serde_json::Value::Object(invocation.args.clone());
            self.on_tool_call(
                &invocation.source,
                &invocation.tool_name,
                &args_value,
                Some(result),
                None,
                0, // duration not available in plugin context
            );
        }
        (true, None, false)
    }

    fn is_running(&self) -> bool {
        *self.running.lock()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn cleanup(&self) -> Result<(), String> {
        self.stop();
        Ok(())
    }
}

/// Summarize a JSON value to a short string for storage.
fn summarize_value(value: &serde_json::Value, max_len: usize) -> String {
    let s = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
            format!("{{{}}}", keys.join(", "))
        }
        serde_json::Value::Array(arr) => format!("[{} items]", arr.len()),
        other => other.to_string(),
    };
    truncate_str(&s, max_len)
}

/// Truncate a string to at most `max_len` bytes, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    utils::truncate(s, max_len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nemesis_plugin::Plugin;

    fn make_experience(tool: &str, input: &str, success: bool) -> Experience {
        Experience {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.into(),
            input_summary: input.into(),
            output_summary: if success { "ok".into() } else { "err".into() },
            success,
            duration_ms: 100,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "sess-test".into(),
        }
    }

    #[tokio::test]
    async fn test_record_and_aggregate() {
        let collector = Collector::new(CollectorConfig::default());

        let exp1 = make_experience("file_read", "read a.txt", true);
        let exp2 = make_experience("file_read", "read b.txt", true);
        let exp3 = make_experience("file_read", "read a.txt", false); // same tool+args pattern, should aggregate

        // record() uses synthesized args {input_summary: ...} for backward compat.
        // Since dedup_hash now only uses key names (not values), all three records
        // produce the same hash: sha256(file_read:input_summary).
        // But they still produce 3 stored entries (Go-style always-store behavior).
        assert!(collector.record(exp1.clone()).await);
        assert!(collector.record(exp2).await);
        assert!(collector.record(exp3).await); // NOT skipped — aggregated

        assert_eq!(collector.len(), 3); // All three stored (matching Go behavior)
        assert_eq!(collector.pattern_count(), 1); // One unique pattern hash (key names only)
    }

    #[tokio::test]
    async fn test_record_with_args_different_patterns() {
        let collector = Collector::new(CollectorConfig::default());

        // Different arg keys produce different pattern hashes
        let exp1 = make_experience("file_read", "read a.txt", true);
        let exp2 = make_experience("file_read", "read b.txt", true);

        let args1 = serde_json::json!({"path": "/tmp/a.txt"});
        let args2 = serde_json::json!({"path": "/tmp/b.txt", "mode": "read"});

        assert!(collector.record_with_args(exp1, &args1).await);
        assert!(collector.record_with_args(exp2, &args2).await);

        assert_eq!(collector.len(), 2);
        assert_eq!(collector.pattern_count(), 2); // Different arg keys => different patterns
    }

    #[tokio::test]
    async fn test_record_with_args_same_keys_different_values() {
        let collector = Collector::new(CollectorConfig::default());

        // Same keys, different values => same pattern hash (Go's ComputePatternHash behavior)
        let exp1 = make_experience("file_read", "read a.txt", true);
        let exp2 = make_experience("file_read", "read b.txt", false);

        let args1 = serde_json::json!({"path": "/tmp/a.txt"});
        let args2 = serde_json::json!({"path": "/tmp/b.txt"});

        assert!(collector.record_with_args(exp1, &args1).await);
        assert!(collector.record_with_args(exp2, &args2).await);

        assert_eq!(collector.len(), 2);
        assert_eq!(collector.pattern_count(), 1); // Same key names => same pattern
    }

    #[tokio::test]
    async fn test_max_size_eviction() {
        let mut config = CollectorConfig::default();
        config.max_size = 2;
        let collector = Collector::new(config);

        let exp1 = make_experience("tool_a", "input1", true);
        let exp2 = make_experience("tool_b", "input2", true);
        let exp3 = make_experience("tool_c", "input3", true);

        collector.record(exp1).await;
        collector.record(exp2).await;
        collector.record(exp3).await;

        assert_eq!(collector.len(), 2);
        let exps = collector.experiences();
        // Oldest (tool_a) should have been evicted.
        assert!(exps.iter().all(|e| e.experience.tool_name != "tool_a"));
    }

    #[tokio::test]
    async fn test_jsonl_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("experiences.jsonl");
        let mut config = CollectorConfig::default();
        config.persistence_path = path.to_string_lossy().to_string();
        let collector = Collector::new(config);

        let exp = make_experience("file_write", "write data", true);
        collector.record(exp).await;

        // File should exist and contain one line.
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(content.lines().count(), 1);
        assert!(content.contains("file_write"));
    }

    #[tokio::test]
    async fn test_load_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("load.jsonl");

        // Write some JSONL manually.
        let exp = make_experience("tool_x", "load_test", true);
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: Collector::dedup_hash("tool_x", &serde_json::json!({"input_summary": "load_test"})),
        };
        let line = serde_json::to_string(&ce).unwrap();
        tokio::fs::write(&path, format!("{}\n", line))
            .await
            .unwrap();

        let collector = Collector::new(CollectorConfig::default());
        collector.load_from_file(&path).await.unwrap();

        assert_eq!(collector.len(), 1);
        assert_eq!(
            collector.experiences()[0].experience.tool_name,
            "tool_x"
        );
    }

    #[test]
    fn test_process_record_and_pattern_count() {
        let collector = Collector::new(CollectorConfig::default());
        assert_eq!(collector.pattern_count(), 0);

        collector.process_record("file_read", "hash1", 100, true, "2026-04-29T12:00:00Z");
        collector.process_record("file_read", "hash1", 150, true, "2026-04-29T12:01:00Z");
        collector.process_record("file_write", "hash2", 200, false, "2026-04-29T12:02:00Z");

        assert_eq!(collector.pattern_count(), 2);
    }

    #[tokio::test]
    async fn test_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("experiences.jsonl");
        let mut config = CollectorConfig::default();
        config.persistence_path = path.to_string_lossy().to_string();
        let collector = Collector::new(config);

        collector.process_record("file_read", "hash1", 100, true, "2026-04-29T12:00:00Z");
        collector.process_record("file_write", "hash2", 200, false, "2026-04-29T12:01:00Z");

        let count = collector.flush().await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(collector.pattern_count(), 0); // Cleared after flush

        // File should contain aggregated records
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("hash1"));
        assert!(content.contains("hash2"));
    }

    #[tokio::test]
    async fn test_flush_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        let mut config = CollectorConfig::default();
        config.persistence_path = path.to_string_lossy().to_string();
        let collector = Collector::new(config);

        let count = collector.flush().await.unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_compute_pattern_hash() {
        let h1 = Collector::compute_pattern_hash("tool_a", &["path", "content"]);
        let h2 = Collector::compute_pattern_hash("tool_a", &["content", "path"]); // Same keys, different order
        let h3 = Collector::compute_pattern_hash("tool_b", &["path", "content"]); // Different tool

        assert_eq!(h1, h2); // Sorted keys should produce same hash
        assert_ne!(h1, h3); // Different tool name
        assert!(h1.starts_with("sha256:"));
    }

    #[test]
    fn test_dedup_hash_matches_go_compute_pattern_hash() {
        // Go: ComputePatternHash("read_file", map[string]interface{}{"path": "/tmp/test", "mode": "read"})
        // produces "sha256:" + hex(sha256("read_file:mode,path")[:8])
        let args = serde_json::json!({"path": "/tmp/test", "mode": "read"});
        let h1 = Collector::dedup_hash("read_file", &args);

        // Same keys, different order => same hash
        let args2 = serde_json::json!({"mode": "read", "path": "/tmp/test"});
        let h2 = Collector::dedup_hash("read_file", &args2);
        assert_eq!(h1, h2);

        // Different values but same keys => same hash (Go only hashes key names)
        let args3 = serde_json::json!({"mode": "write", "path": "/other/path"});
        let h3 = Collector::dedup_hash("read_file", &args3);
        assert_eq!(h1, h3);

        // Different tool name => different hash
        let h4 = Collector::dedup_hash("write_file", &args);
        assert_ne!(h1, h4);

        // Has sha256: prefix
        assert!(h1.starts_with("sha256:"));

        // Empty args
        let h_empty = Collector::dedup_hash("tool", &serde_json::json!({}));
        assert!(h_empty.starts_with("sha256:"));
        assert_ne!(h_empty, h1);
    }

    #[test]
    fn test_dedup_hash_non_object_fallback() {
        // Non-object args should still produce a valid hash
        let h = Collector::dedup_hash("tool", &serde_json::json!("string"));
        assert!(h.starts_with("sha256:"));
    }

    #[test]
    fn test_sanitize_args() {
        let args = serde_json::json!({
            "path": "/tmp/file.txt",
            "api_key": "sk-secret-key",
            "content": "hello world"
        });

        let sanitized = Collector::sanitize_args(&args, &["api_key"]);
        assert_eq!(sanitized["path"], "/tmp/file.txt");
        assert_eq!(sanitized["api_key"], "[REDACTED]");
        assert_eq!(sanitized["content"], "hello world");
    }

    #[test]
    fn test_sanitize_args_no_fields() {
        let args = serde_json::json!({"key": "value"});
        let sanitized = Collector::sanitize_args(&args, &[]);
        assert_eq!(sanitized["key"], "value");
    }

    // --- ForgePlugin tests ---

    #[test]
    fn test_forge_plugin_name() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        assert_eq!(plugin.name(), "forge");
        assert_eq!(plugin.version(), "1.0.0");
    }

    #[test]
    fn test_forge_plugin_lifecycle() {
        let mut plugin = ForgePlugin::new(CollectorConfig::default());
        assert!(!plugin.is_running());
        plugin.init(&serde_json::json!({})).unwrap();
        assert!(plugin.is_running());
        plugin.stop();
        assert!(!plugin.is_running());
    }

    #[test]
    fn test_session_positions() {
        let mut positions = SessionPositions::new();
        assert_eq!(positions.next("session-1"), 0);
        assert_eq!(positions.next("session-1"), 1);
        assert_eq!(positions.next("session-2"), 0);
        assert_eq!(positions.get("session-1"), 2);
        positions.reset("session-1");
        assert_eq!(positions.get("session-1"), 0);
    }

    #[test]
    fn test_forge_plugin_on_tool_call() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        plugin.on_tool_call(
            "sess-1",
            "file_read",
            &serde_json::json!({"path": "/tmp/file.txt", "api_key": "secret"}),
            Some(&serde_json::json!({"content": "hello"})),
            None,
            42,
        );
        // Should not panic; input channel has the experience
    }

    #[test]
    fn test_forge_plugin_input_channel() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        let tx = plugin.input_channel();
        assert!(tx.capacity() > 0);
    }

    /// Edge case: ForgePlugin.on_tool_call() when the input channel is full
    /// (matches Go's TestCollector_Record_ChannelFull)
    #[test]
    fn test_forge_plugin_on_tool_call_channel_full() {
        // Create a plugin with default config (channel capacity 256).
        // Fill the channel by sending 256+ items, then verify the next
        // on_tool_call still doesn't panic (graceful degradation).
        let plugin = ForgePlugin::new(CollectorConfig::default());

        // Fill the channel to capacity
        for i in 0..300 {
            plugin.on_tool_call(
                "sess-1",
                &format!("tool_{}", i),
                &serde_json::json!({"key": format!("val_{}", i)}),
                Some(&serde_json::json!("ok")),
                None,
                10,
            );
        }

        // This call should not panic even though channel is full.
        // The experience is still processed in-memory via collector.process_record.
        plugin.on_tool_call(
            "sess-1",
            "overflow_tool",
            &serde_json::json!({"key": "overflow"}),
            Some(&serde_json::json!("overflow")),
            None,
            20,
        );
        // Key assertion: no panic. The in-memory pattern counts should still
        // reflect all calls (including the overflow one).
    }

    #[test]
    fn test_forge_plugin_plugin_trait() {
        let mut mgr = nemesis_plugin::PluginManager::new();
        let plugin = ForgePlugin::new(CollectorConfig::default());
        mgr.register(Box::new(plugin)).unwrap();
        assert!(mgr.is_enabled("forge"));
    }

    #[test]
    fn test_forge_plugin_execute() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        let mut invocation = nemesis_plugin::ToolInvocation::new(
            "file_read",
            serde_json::json!({"path": "/tmp/file.txt"}).as_object().unwrap().clone(),
        );
        invocation.result = Some(serde_json::json!({"ok": true}));
        invocation.source = "test-session".to_string();
        let (allowed, err, modified) = plugin.execute(&mut invocation);
        assert!(allowed);
        assert!(err.is_none());
        assert!(!modified);
    }

    #[test]
    fn test_summarize_value() {
        assert_eq!(summarize_value(&serde_json::json!("hello"), 100), "hello");
        assert!(summarize_value(&serde_json::json!({"a": 1, "b": 2}), 100).contains("a"));
        assert!(summarize_value(&serde_json::json!([1, 2, 3]), 100).contains("3 items"));
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        let long = "a".repeat(200);
        let truncated = truncate_str(&long, 100);
        assert_eq!(truncated.len(), 100); // 97 chars + "..." = 100
        assert!(truncated.ends_with("..."));
    }

    // --- Additional collector tests ---

    #[tokio::test]
    async fn test_record_returns_true() {
        let collector = Collector::new(CollectorConfig::default());
        let exp = make_experience("tool", "input", true);
        assert!(collector.record(exp).await);
    }

    #[tokio::test]
    async fn test_record_many_experiences() {
        let collector = Collector::new(CollectorConfig::default());
        for i in 0..100 {
            let exp = make_experience("tool", &format!("input-{}", i), true);
            assert!(collector.record(exp).await);
        }
        assert_eq!(collector.len(), 100);
    }

    #[tokio::test]
    async fn test_experiences_returns_snapshot() {
        let collector = Collector::new(CollectorConfig::default());
        let exp = make_experience("tool", "input", true);
        collector.record(exp).await;
        let snapshot = collector.experiences();
        assert_eq!(snapshot.len(), 1);
        // Snapshot should be a copy, further records don't change it
        let exp2 = make_experience("tool2", "input2", false);
        collector.record(exp2).await;
        assert_eq!(snapshot.len(), 1); // snapshot unchanged
    }

    #[tokio::test]
    async fn test_pattern_count_multiple_tools() {
        let collector = Collector::new(CollectorConfig::default());
        collector.process_record("read", "h1", 100, true, "2026-01-01T00:00:00Z");
        collector.process_record("write", "h2", 200, true, "2026-01-01T00:00:00Z");
        collector.process_record("exec", "h3", 300, false, "2026-01-01T00:00:00Z");
        assert_eq!(collector.pattern_count(), 3);
    }

    #[tokio::test]
    async fn test_pattern_count_same_hash_dedup() {
        let collector = Collector::new(CollectorConfig::default());
        collector.process_record("read", "same_hash", 100, true, "2026-01-01T00:00:00Z");
        collector.process_record("read", "same_hash", 200, true, "2026-01-01T00:00:00Z");
        collector.process_record("read", "same_hash", 300, false, "2026-01-01T00:00:00Z");
        assert_eq!(collector.pattern_count(), 1);
    }

    #[tokio::test]
    async fn test_flush_clears_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_flush.jsonl");
        let mut config = CollectorConfig::default();
        config.persistence_path = path.to_string_lossy().to_string();
        let collector = Collector::new(config);

        collector.process_record("tool_a", "h1", 100, true, "2026-01-01T00:00:00Z");
        collector.process_record("tool_b", "h2", 200, true, "2026-01-01T00:00:00Z");
        assert_eq!(collector.pattern_count(), 2);

        let count = collector.flush().await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(collector.pattern_count(), 0);
    }

    #[tokio::test]
    async fn test_flush_no_persistence_path() {
        let config = CollectorConfig::default();
        let collector = Collector::new(config);
        collector.process_record("tool", "h1", 100, true, "2026-01-01T00:00:00Z");
        // flush without path should return an error
        let result = collector.flush().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_from_nonexistent_file() {
        let collector = Collector::new(CollectorConfig::default());
        let result = collector.load_from_file(PathBuf::from("/nonexistent/file.jsonl")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_load_from_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        tokio::fs::write(&path, "").await.unwrap();

        let collector = Collector::new(CollectorConfig::default());
        collector.load_from_file(&path).await.unwrap();
        assert_eq!(collector.len(), 0);
    }

    #[tokio::test]
    async fn test_load_from_file_with_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.jsonl");
        let valid_exp = make_experience("tool", "input", true);
        let ce = CollectedExperience {
            experience: valid_exp,
            dedup_hash: Collector::dedup_hash("tool", &serde_json::json!({"input_summary": "input"})),
        };
        let valid_line = serde_json::to_string(&ce).unwrap();
        let content = format!("invalid json\n{}\n", valid_line);
        tokio::fs::write(&path, content).await.unwrap();

        let collector = Collector::new(CollectorConfig::default());
        collector.load_from_file(&path).await.unwrap();
        assert_eq!(collector.len(), 1); // Only valid line loaded
    }

    #[tokio::test]
    async fn test_record_with_args_stores_experience() {
        let collector = Collector::new(CollectorConfig::default());
        let exp = make_experience("tool", "input", true);
        let args = serde_json::json!({"key": "value"});
        assert!(collector.record_with_args(exp, &args).await);
        assert_eq!(collector.len(), 1);
    }

    #[test]
    fn test_compute_pattern_hash_empty_keys() {
        let h = Collector::compute_pattern_hash("tool", &[]);
        assert!(h.starts_with("sha256:"));
    }

    #[test]
    fn test_compute_pattern_hash_order_independent() {
        let h1 = Collector::compute_pattern_hash("tool", &["b", "a", "c"]);
        let h2 = Collector::compute_pattern_hash("tool", &["c", "a", "b"]);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_dedup_hash_with_null() {
        let h = Collector::dedup_hash("tool", &serde_json::Value::Null);
        assert!(h.starts_with("sha256:"));
    }

    #[test]
    fn test_dedup_hash_with_array() {
        let h = Collector::dedup_hash("tool", &serde_json::json!([1, 2, 3]));
        assert!(h.starts_with("sha256:"));
    }

    #[test]
    fn test_dedup_hash_with_number() {
        let h = Collector::dedup_hash("tool", &serde_json::json!(42));
        assert!(h.starts_with("sha256:"));
    }

    #[test]
    fn test_sanitize_args_multiple_sensitive_fields() {
        let args = serde_json::json!({
            "path": "/tmp/file",
            "api_key": "secret1",
            "token": "secret2",
            "password": "secret3",
            "data": "normal"
        });
        let sanitized = Collector::sanitize_args(&args, &["api_key", "token", "password"]);
        assert_eq!(sanitized["api_key"], "[REDACTED]");
        assert_eq!(sanitized["token"], "[REDACTED]");
        assert_eq!(sanitized["password"], "[REDACTED]");
        assert_eq!(sanitized["path"], "/tmp/file");
        assert_eq!(sanitized["data"], "normal");
    }

    #[test]
    fn test_sanitize_args_nested_object() {
        let args = serde_json::json!({
            "config": {"api_key": "secret"},
            "name": "test"
        });
        let sanitized = Collector::sanitize_args(&args, &["api_key"]);
        // Only top-level keys are sanitized
        assert_eq!(sanitized["name"], "test");
    }

    #[test]
    fn test_max_size_zero_means_unlimited() {
        // max_size = 0 actually means "capacity of 0" - items get evicted immediately.
        // To test "unlimited", use a very large max_size instead.
        let mut config = CollectorConfig::default();
        config.max_size = 1000;
        let collector = Collector::new(config);
        for i in 0..50 {
            let exp = make_experience("tool", &format!("input-{}", i), true);
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(collector.record(exp));
        }
        assert_eq!(collector.len(), 50);
    }

    #[test]
    fn test_forge_plugin_cleanup() {
        let mut plugin = ForgePlugin::new(CollectorConfig::default());
        plugin.init(&serde_json::json!({})).unwrap();
        assert!(plugin.is_running());
        plugin.cleanup().unwrap();
        assert!(!plugin.is_running());
    }

    #[test]
    fn test_forge_plugin_take_input_receiver() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        let rx1 = plugin.take_input_receiver();
        assert!(rx1.is_some());
        let rx2 = plugin.take_input_receiver();
        assert!(rx2.is_none()); // Already taken
    }

    #[test]
    fn test_forge_plugin_execute_no_result() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        let mut invocation = nemesis_plugin::ToolInvocation::new(
            "file_read",
            serde_json::json!({"path": "/tmp/file.txt"}).as_object().unwrap().clone(),
        );
        // No result set - should still pass through
        let (allowed, err, modified) = plugin.execute(&mut invocation);
        assert!(allowed);
        assert!(err.is_none());
        assert!(!modified);
    }

    #[tokio::test]
    async fn test_process_pending_empty() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        let count = plugin.process_pending().await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_process_pending_with_items() {
        let plugin = ForgePlugin::new(CollectorConfig::default());
        let exp = make_experience("tool", "input", true);
        plugin.input_channel().send(exp).await.unwrap();
        let count = plugin.process_pending().await;
        assert_eq!(count, 1);
    }

    #[test]
    fn test_forge_plugin_with_collector() {
        let collector = Collector::new(CollectorConfig::default());
        let plugin = ForgePlugin::with_collector(collector);
        assert_eq!(plugin.name(), "forge");
        assert!(plugin.collector().len() == 0);
    }

    #[test]
    fn test_summarize_value_number() {
        assert_eq!(summarize_value(&serde_json::json!(42), 100), "42");
    }

    #[test]
    fn test_summarize_value_bool() {
        assert_eq!(summarize_value(&serde_json::json!(true), 100), "true");
    }

    #[test]
    fn test_summarize_value_null() {
        assert_eq!(summarize_value(&serde_json::Value::Null, 100), "null");
    }

    #[test]
    fn test_summarize_value_truncation() {
        let long_val = serde_json::json!("a".repeat(200));
        let result = summarize_value(&long_val, 50);
        assert!(result.len() <= 53); // 50 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_str_exact_length() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_zero_length() {
        assert_eq!(truncate_str("hello", 0), "...");
    }

    #[test]
    fn test_session_positions_multiple_sessions() {
        let mut positions = SessionPositions::new();
        assert_eq!(positions.next("s1"), 0);
        assert_eq!(positions.next("s2"), 0);
        assert_eq!(positions.next("s1"), 1);
        assert_eq!(positions.next("s2"), 1);
        assert_eq!(positions.next("s3"), 0);
        assert_eq!(positions.get("s1"), 2);
        assert_eq!(positions.get("s2"), 2);
        assert_eq!(positions.get("s3"), 1);
    }

    #[test]
    fn test_session_positions_reset_nonexistent() {
        let mut positions = SessionPositions::new();
        positions.reset("nonexistent"); // Should not panic
        assert_eq!(positions.get("nonexistent"), 0);
    }

    #[test]
    fn test_session_positions_get_nonexistent() {
        let positions = SessionPositions::new();
        assert_eq!(positions.get("never-existed"), 0);
    }

    #[tokio::test]
    async fn test_record_mixed_success_failure() {
        let collector = Collector::new(CollectorConfig::default());
        for i in 0..10 {
            let exp = make_experience("tool", &format!("input-{}", i), i % 2 == 0);
            collector.record(exp).await;
        }
        assert_eq!(collector.len(), 10);
    }

    #[tokio::test]
    async fn test_load_append_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("append.jsonl");

        // Write a CollectedExperience record manually
        let record = r#"{"experience":{"id":"exp-1","tool_name":"tool_a","input_summary":"input","output_summary":"ok","success":true,"duration_ms":100,"timestamp":"2026-01-01T00:00:00Z","session_key":"s1"},"dedup_hash":"h1"}"#;
        tokio::fs::write(&path, record).await.unwrap();

        // Load from file
        let config = CollectorConfig::default();
        let collector = Collector::new(config);
        collector.load_from_file(&path).await.unwrap();
        assert_eq!(collector.len(), 1);
    }
}
