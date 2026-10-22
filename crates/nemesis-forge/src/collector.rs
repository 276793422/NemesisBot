//! Collector - asynchronously collects tool call experiences.
//!
//! Deduplicates by (tool_name + args hash) and stores up to `max_size` entries
//! in memory. Optionally persists to JSONL.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tracing;

use crate::types::{CollectedExperience, CollectorConfig, Experience};

/// Async collector of tool-call experiences.
pub struct Collector {
    config: CollectorConfig,
    experiences: Mutex<VecDeque<CollectedExperience>>,
    seen_hashes: Mutex<HashSet<String>>,
    persistence_path: Option<PathBuf>,
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
    /// (i.e. not exceeding capacity). The `args` parameter is the original
    /// tool call arguments (JSON object), hashed (key names only) for
    /// deduplication.
    pub async fn record_with_args(&self, experience: Experience, args: &serde_json::Value) -> bool {
        let hash = Self::dedup_hash(&experience.tool_name, args);

        self.persist_and_store(experience, hash).await;
        true
    }

    /// Persist to JSONL + insert into the in-memory store.
    ///
    /// P4 拆分（2026-10-16）：落库与聚合曾是两条路径；聚合线（pattern_counts
    /// /flush/aggregates.jsonl）2026-10-22 整体移除——aggregates 全仓零读者，
    /// 是 write-only 死数据（审计 P2-a）。本函数只承担落盘 + 内存入库。
    async fn persist_and_store(&self, experience: Experience, hash: String) {
        let ce = CollectedExperience {
            experience,
            dedup_hash: hash.clone(),
        };

        // Persist to JSONL (before adding to memory so a crash between the two
        // does not lose data; re-loading will deduplicate anyway).
        if let Some(ref path) = self.persistence_path
            && let Err(e) = Self::append_jsonl(path, &ce).await
        {
            tracing::warn!(path = %path.display(), error = %e, "[Collector] Failed to persist experience");
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
        // 同 experience_store：write_all 不保证落盘，必须 flush 才满足 read-your-writes。
        file.flush().await?;
        Ok(())
    }

    /// Clear all collected experiences.
    pub fn clear(&self) {
        self.experiences.lock().clear();
        self.seen_hashes.lock().clear();
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
}

#[cfg(test)]
mod tests;
