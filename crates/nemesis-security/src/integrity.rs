//! Integrity Audit Chain - Layer 8
//! Merkle-based SHA256 audit chain with JSONL persistence, segment rotation,
//! export/load, and sign field support.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Audit event for the integrity chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub operation: String,
    pub tool_name: String,
    pub user: String,
    pub source: String,
    pub target: String,
    pub decision: String,
    pub reason: String,
    pub hash: String,
    pub prev_hash: String,
    /// Optional Ed25519 signature of the event hash.
    #[serde(default)]
    pub sign: Option<String>,
}

/// Audit chain configuration.
#[derive(Debug, Clone)]
pub struct AuditChainConfig {
    pub enabled: bool,
    pub storage_path: PathBuf,
    pub max_file_size: u64,
    pub verify_on_load: bool,
    /// Max events per segment before rotation.
    pub max_events_per_segment: u64,
    /// Optional signing key for event signatures.
    pub signing_key: Option<String>,
}

impl Default for AuditChainConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_path: PathBuf::from("audit_chain.jsonl"),
            max_file_size: 50 * 1024 * 1024,
            verify_on_load: false,
            max_events_per_segment: 100_000,
            signing_key: None,
        }
    }
}

/// Returns a reasonable default configuration.
///
/// Equivalent to Go's `DefaultAuditChainConfig()`.
pub fn default_audit_chain_config() -> AuditChainConfig {
    AuditChainConfig::default()
}

/// Merkle audit chain with segment rotation and export.
///
/// Thread-safe: all mutable state is protected by locks or atomic counters.
pub struct AuditChain {
    config: AuditChainConfig,
    last_hash: parking_lot::Mutex<String>,
    event_count: std::sync::atomic::AtomicU64,
    segment_count: std::sync::atomic::AtomicU64,
    total_events: std::sync::atomic::AtomicU64,
    /// Whether the chain has been closed.
    closed: std::sync::atomic::AtomicBool,
    /// append 全程串行锁（横扫九期补网，2026-10-14）：链不变量是
    /// 「行 i.prev == 行 i-1.hash」——last_hash 的读与写原是两次独立取锁，
    /// 中间算 hash 的窗口无锁，并发 append 各读到同一 prev ⇒ 链分叉
    /// （verify 的 prev_hash != 前一行 hash 必 FAIL）。本锁罩「读 prev →
    /// 算 hash → 写 last → 落盘 → rotation」全程。
    append_lock: parking_lot::Mutex<()>,
}

impl AuditChain {
    pub fn new(config: AuditChainConfig) -> Self {
        let last_hash = parking_lot::Mutex::new(
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        );
        Self {
            config,
            last_hash,
            event_count: std::sync::atomic::AtomicU64::new(0),
            segment_count: std::sync::atomic::AtomicU64::new(1),
            total_events: std::sync::atomic::AtomicU64::new(0),
            closed: std::sync::atomic::AtomicBool::new(false),
            append_lock: parking_lot::Mutex::new(()),
        }
    }

    /// Append an event to the audit chain.
    ///
    /// Returns an error if the chain has been closed, or if the event could
    /// not be persisted to disk (in which case the in-memory chain tail is
    /// NOT advanced — memory stays consistent with disk).
    pub fn append(
        &self,
        operation: &str,
        tool_name: &str,
        user: &str,
        source: &str,
        target: &str,
        decision: &str,
        reason: &str,
    ) -> Result<AuditEvent, String> {
        self.append_with_sign(
            operation, tool_name, user, source, target, decision, reason, None,
        )
    }

    /// Append an event with an optional signature.
    ///
    /// Returns an error if the chain has been closed, or if the event could
    /// not be persisted to disk (in which case the in-memory chain tail is
    /// NOT advanced — memory stays consistent with disk).
    pub fn append_with_sign(
        &self,
        operation: &str,
        tool_name: &str,
        user: &str,
        source: &str,
        target: &str,
        decision: &str,
        reason: &str,
        sign: Option<String>,
    ) -> Result<AuditEvent, String> {
        if self.is_closed() {
            return Err("audit chain is closed".to_string());
        }
        // 全程临界区（横扫九期）：prev 读取与 last 写入之间不得插入第二个
        // append，否则链分叉（结构体注释）。
        let _chain = self.append_lock.lock();
        let mut hasher = Sha256::new();
        let timestamp = chrono::Local::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();

        let prev_hash = self.last_hash.lock().clone();

        // Hash: prev_hash + timestamp + operation + tool + user + target + decision
        hasher.update(prev_hash.as_bytes());
        hasher.update(timestamp.as_bytes());
        hasher.update(operation.as_bytes());
        hasher.update(tool_name.as_bytes());
        hasher.update(user.as_bytes());
        hasher.update(target.as_bytes());
        hasher.update(decision.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let event = AuditEvent {
            id,
            timestamp,
            operation: operation.to_string(),
            tool_name: tool_name.to_string(),
            user: user.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            decision: decision.to_string(),
            reason: reason.to_string(),
            hash: hash.clone(),
            prev_hash: prev_hash.clone(),
            sign,
        };

        // 盘先行（BUG 2026-10-29 横扫角度 A）：磁盘是审计链的真相源——只有
        // 行真正落盘才推进内存链状态（last_hash/计数器）并返回 Ok。旧的
        // 「先推进内存、落盘 let _ = 吞错、恒返回 Ok」在磁盘写失败时内存与
        // 盘脱节：verify 按盘上行链式校验从此必 FAIL，事发时刻零可见。
        let json = serde_json::to_string(&event)
            .map_err(|e| format!("audit event serialize failed: {e}"))?;
        let storage_path = self.get_current_segment_path();
        if let Err(e) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&storage_path)
            .and_then(|mut f| {
                use std::io::Write;
                writeln!(f, "{json}")
            })
        {
            tracing::error!(
                target: "nemesis_security::integrity",
                segment = %storage_path.display(),
                error = %e,
                "[AuditChain] 审计事件落盘失败：本事件未入链（内存链尾未推进，下个事件重用同一 prev_hash）"
            );
            return Err(format!("audit chain persist failed: {e}"));
        }

        *self.last_hash.lock() = hash;
        let count = self
            .event_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.total_events
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Check if rotation is needed
        if count >= self.config.max_events_per_segment {
            self.rotate_segment();
        }

        Ok(event)
    }

    /// Get the current chain hash.
    pub fn current_hash(&self) -> String {
        self.last_hash.lock().clone()
    }

    /// Get event count for current segment.
    pub fn event_count(&self) -> u64 {
        self.event_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get total event count across all segments.
    pub fn total_event_count(&self) -> u64 {
        self.total_events.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Get current segment number.
    pub fn segment_count(&self) -> u64 {
        self.segment_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Rotate to a new segment.
    fn rotate_segment(&self) {
        self.event_count
            .store(0, std::sync::atomic::Ordering::SeqCst);
        self.segment_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Get the path for the current segment file.
    fn get_current_segment_path(&self) -> PathBuf {
        let seg = self.segment_count.load(std::sync::atomic::Ordering::SeqCst);
        if seg == 1 {
            self.config.storage_path.clone()
        } else {
            let ext = self
                .config
                .storage_path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let stem = self
                .config
                .storage_path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "audit_chain".to_string());
            self.config
                .storage_path
                .parent()
                .map(|p| p.join(format!("{}_seg{:04}{}", stem, seg, ext)))
                .unwrap_or_else(|| PathBuf::from(format!("{}_seg{:04}{}", stem, seg, ext)))
        }
    }

    /// Verify chain integrity from a list of events.
    pub fn verify_chain(events: &[AuditEvent]) -> bool {
        for i in 1..events.len() {
            if events[i].prev_hash != events[i - 1].hash {
                return false;
            }
            // Verify hash computation
            let mut hasher = Sha256::new();
            hasher.update(events[i].prev_hash.as_bytes());
            hasher.update(events[i].timestamp.as_bytes());
            hasher.update(events[i].operation.as_bytes());
            hasher.update(events[i].tool_name.as_bytes());
            hasher.update(events[i].user.as_bytes());
            hasher.update(events[i].target.as_bytes());
            hasher.update(events[i].decision.as_bytes());
            let computed = format!("{:x}", hasher.finalize());
            if computed != events[i].hash {
                return false;
            }
        }
        true
    }

    /// Export the entire chain to a JSON file.
    pub fn export_chain(&self, output_path: &Path) -> Result<(), String> {
        let mut all_events: Vec<AuditEvent> = Vec::new();

        // Read all segment files
        let parent = self.config.storage_path.parent().unwrap_or(Path::new("."));
        let stem = self
            .config
            .storage_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "audit_chain".to_string());

        // Read main file
        if self.config.storage_path.exists()
            && let Ok(content) = std::fs::read_to_string(&self.config.storage_path)
        {
            for line in content.lines() {
                if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                    all_events.push(event);
                }
            }
        }

        // Read segment files
        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut segment_files: Vec<PathBuf> = Vec::new();
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&stem) && name.contains("_seg") {
                    segment_files.push(entry.path());
                }
            }
            segment_files.sort();
            for path in segment_files {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                            all_events.push(event);
                        }
                    }
                }
            }
        }

        let json = serde_json::to_string_pretty(&all_events).map_err(|e| e.to_string())?;
        std::fs::write(output_path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Load chain from an exported JSON file (replaces current chain).
    pub fn load_chain(&self, input_path: &Path) -> Result<u64, String> {
        let content = std::fs::read_to_string(input_path).map_err(|e| e.to_string())?;
        let events: Vec<AuditEvent> = serde_json::from_str(&content).map_err(|e| e.to_string())?;

        if !Self::verify_chain(&events) {
            return Err("chain verification failed".to_string());
        }

        // Update last hash
        if let Some(last) = events.last() {
            *self.last_hash.lock() = last.hash.clone();
        }

        let count = events.len() as u64;
        self.total_events
            .fetch_add(count, std::sync::atomic::Ordering::SeqCst);

        // Write events to storage
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.storage_path)
            .map_err(|e| e.to_string())?;
        use std::io::Write;
        for event in &events {
            let json = serde_json::to_string(event).map_err(|e| e.to_string())?;
            writeln!(f, "{}", json).map_err(|e| e.to_string())?;
        }

        Ok(count)
    }

    /// Load existing segments from disk on startup.
    ///
    /// Reads the main segment file and all rotated segment files,
    /// updates the last hash and counters.
    pub fn load_segments(&self) -> Result<u64, String> {
        let mut total: u64 = 0;
        let mut last_hash =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        // Collect all segment files sorted
        let mut files: Vec<PathBuf> = Vec::new();
        let parent = self.config.storage_path.parent().unwrap_or(Path::new("."));
        let stem = self
            .config
            .storage_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "audit_chain".to_string());

        if self.config.storage_path.exists() {
            files.push(self.config.storage_path.clone());
        }

        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut seg_files: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with(&stem) && name.contains("_seg")
                })
                .map(|e| e.path())
                .collect();
            seg_files.sort();
            files.extend(seg_files);
        }

        for path in files {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        total += 1;
                        last_hash = event.hash.clone();
                    }
                }
            }
        }

        if total > 0 {
            *self.last_hash.lock() = last_hash;
            self.total_events
                .store(total, std::sync::atomic::Ordering::SeqCst);
        }

        Ok(total)
    }

    /// Get a specific event by index from the chain file.
    ///
    /// Index is 0-based. Returns None if index is out of range.
    pub fn get_event(&self, index: usize) -> Option<AuditEvent> {
        let mut count: usize = 0;
        let files = self.collect_segment_files();

        for path in files {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        if count == index {
                            return Some(event);
                        }
                        count += 1;
                    }
                }
            }
        }
        None
    }

    /// Verify a range of events in the chain.
    ///
    /// Checks hash continuity between events[from] to events[to].
    pub fn verify_range(&self, from: usize, to: usize) -> Result<bool, String> {
        let events = self.read_events_range(from, to + 1)?;
        if events.len() < 2 {
            return Ok(true);
        }
        Ok(Self::verify_chain(&events))
    }

    /// Get the number of total stored events (alias for total_event_count).
    pub fn size(&self) -> u64 {
        self.total_event_count()
    }

    /// Close the chain, flushing pending writes and preventing further appends.
    ///
    /// Equivalent to Go's `AuditChain.Close()`. After calling this method,
    /// subsequent `append()` calls will return an error (or be silently ignored
    /// depending on the chain's error handling mode).
    pub fn close(&self) -> Result<(), String> {
        let was_closed = self.closed.swap(true, std::sync::atomic::Ordering::SeqCst);
        if was_closed {
            return Ok(());
        }
        tracing::info!(
            total_events = self.total_event_count(),
            "[Security] Audit chain closed"
        );
        Ok(())
    }

    /// Check if the chain has been closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(std::sync::atomic::Ordering::SeqCst)
    }

    // ---- Private helpers ----

    /// Collect all segment files in order.
    fn collect_segment_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = Vec::new();
        let parent = self.config.storage_path.parent().unwrap_or(Path::new("."));
        let stem = self
            .config
            .storage_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "audit_chain".to_string());

        if self.config.storage_path.exists() {
            files.push(self.config.storage_path.clone());
        }

        if let Ok(entries) = std::fs::read_dir(parent) {
            let mut seg_files: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with(&stem) && name.contains("_seg")
                })
                .map(|e| e.path())
                .collect();
            seg_files.sort();
            files.extend(seg_files);
        }

        files
    }

    /// Read a range of events from disk.
    fn read_events_range(&self, from: usize, to: usize) -> Result<Vec<AuditEvent>, String> {
        let files = self.collect_segment_files();
        let mut events = Vec::new();

        for path in files {
            if events.len() >= to {
                break;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(event) = serde_json::from_str::<AuditEvent>(line) {
                        events.push(event);
                        if events.len() >= to {
                            break;
                        }
                    }
                }
            }
        }

        if from >= events.len() {
            return Ok(Vec::new());
        }
        Ok(events[from..to.min(events.len())].to_vec())
    }
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
