//! Collector - asynchronously collects tool call experiences.
//!
//! Deduplicates by (tool_name + args hash) and stores up to `max_size` entries
//! in memory. Optionally persists to JSONL.

use std::collections::{HashMap, HashSet, VecDeque};
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
    experiences: Mutex<VecDeque<CollectedExperience>>,
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
            let path = PathBuf::from(&config.persistence_path);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            Some(path)
        };
        tracing::debug!(
            persistence = persistence_path.as_ref().map(|p| p.display().to_string()),
            max_size = config.max_size,
            "[Forge/Collector] Created"
        );
        Self {
            config,
            experiences: Mutex::new(VecDeque::new()),
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
                tracing::warn!(path = %path.display(), error = %e, "[Collector] Failed to persist experience");
            }
        }

        // Add to in-memory store
        {
            let mut exps = self.experiences.lock();
            if exps.len() >= self.config.max_size {
                tracing::debug!("[Collector] At max capacity, evicting oldest");
                if let Some(removed) = exps.front() {
                    self.seen_hashes.lock().remove(&removed.dedup_hash);
                }
                exps.pop_front();
            }
            exps.push_back(ce);
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
        self.experiences.lock().iter().cloned().collect()
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
                exps.push_back(ce);
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
        let mut line = serde_json::to_string(ce)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
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

        // Aggregates go to a SEPARATE file. Writing them into experiences.jsonl
        // polluted it with a different JSON schema (AggregatedExperience) that
        // ExperienceStore::read_all can't parse — it silently skipped every
        // aggregate line, making aggregation write-only and corrupting the file
        // the reflector reads. (F-D1)
        let path = match &self.persistence_path {
            Some(p) => p.with_file_name("aggregates.jsonl"),
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
                tracing::warn!(error = %e, "[Collector] Failed to flush aggregated experience");
            } else {
                count += 1;
            }
        }

        tracing::info!(count, "[Collector] Flushed experience patterns");
        Ok(count)
    }

    /// Append an aggregated record to the JSONL file.
    async fn append_aggregated(path: &Path, record: &AggregatedExperience) -> std::io::Result<()> {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let mut line = serde_json::to_string(record)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
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
    pub fn sanitize_args(args: &serde_json::Value, sanitize_fields: &[&str]) -> serde_json::Value {
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
                    cleaned.insert(
                        k.clone(),
                        serde_json::Value::String("[REDACTED]".to_string()),
                    );
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
        tracing::info!("[Collector] ForgePlugin started");
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
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: session_key.to_string(),
        };

        // Try to send to async channel, fall back to direct record if full
        if self.input_tx.try_send(experience.clone()).is_err() {
            tracing::debug!(
                "[Collector] Input channel full, dropping experience from async channel"
            );
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
mod tests;
