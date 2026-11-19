//! Integrity Audit Chain - Layer 8
//! Merkle-based SHA256 audit chain with JSONL persistence, segment rotation,
//! export/load, and sign field support.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// 支持的最高 hash 公式版本。fail-closed 守卫（`verify_chain`）与
/// Dashboard 诊断（nemesis-web logs 的 wrapper cap）**同源消费本常量**
/// ——v2 漂移实录：#31 加 v2 后 web 侧 wrapper 恒 cap >1，v2 事件在
/// Dashboard 链诊断恒算空 hash 报假 mismatch（本次 #3 批顺手根修）。
/// 新增版本必须同时改本常量、`compute_event_hash` 的 `>=` 臂与本守卫。
pub const MAX_SUPPORTED_HASH_VERSION: u8 = 3;

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
    /// Hash formula version. 0 = legacy（prev/timestamp/operation/tool/user/
    /// target/decision 七字段）；1 = v1（追加 source + reason——审计叙述与
    /// 来源进防篡改覆盖）；2 = v2（v1 字段追加 instance_id——单机实例身份
    /// 进防篡改覆盖，W1 #31，2026-11-19）。serde default 让旧链照常加载
    /// 与验证（BUG-20261102-305：旧公式不含 reason，篡改「为何放行/拒绝」
    /// 的取证叙述不破坏链验证）。
    #[serde(default)]
    pub hash_version: u8,
    /// 产出本事件的 bot 实例身份（workspace/identity.json 持久 UUID，
    /// W1 #31）。None = 实例身份未注入（hash 走 v1，字节不变零扰动）；
    /// Some = hash_version 2（v1 字段 + instance_id 进覆盖）。
    #[serde(default)]
    pub instance_id: Option<String>,
    /// 事件发生时的安全档位（#2 审计携带级别，W2 2026-11-20）。None =
    /// 档位未注入（hash 走 v2 及以下公式，字节不变零扰动）；Some =
    /// hash_version 3（v2 字段 + security_level 进覆盖）——审计叙述
    /// 「这条放行/拒绝发生在什么安全姿态下」，事后取证可按档位分层。
    #[serde(default)]
    pub security_level: Option<String>,
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
    /// append 全程串行锁（横扫九期补网）：链不变量是
    /// 「行 i.prev == 行 i-1.hash」——last_hash 的读与写原是两次独立取锁，
    /// 中间算 hash 的窗口无锁，并发 append 各读到同一 prev ⇒ 链分叉
    /// （verify 的 prev_hash != 前一行 hash 必 FAIL）。本锁罩「读 prev →
    /// 算 hash → 写 last → 落盘 → rotation」全程。
    append_lock: parking_lot::Mutex<()>,
    /// #31 单机实例身份（W1）：Some = 新事件走 hash v2（v1 字段 +
    /// instance_id 进覆盖）；None = v1 字节不变（默认，零扰动）。装配点
    /// 注入（[`AuditChain::set_instance_id`]，gateway build_security_plugin）。
    instance_id: parking_lot::Mutex<Option<String>>,
    /// #2 审计携带级别（W2）：Some = 新事件走 hash v3（v2 字段 +
    /// security_level 进覆盖）；None = v2 及以下字节不变（默认）。装配点
    /// 注入（[`AuditChain::set_security_level`]，build_security_plugin）。
    security_level: parking_lot::Mutex<Option<String>>,
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
            instance_id: parking_lot::Mutex::new(None),
            security_level: parking_lot::Mutex::new(None),
        }
    }

    /// #31：实例身份注入（builder 形态，装配点调用；None = 保持 v1）。
    /// Interior Mutex——注入后所有后续 append 的新事件带身份（hash v2）。
    pub fn set_instance_id(&self, instance_id: Option<String>) {
        *self.instance_id.lock() = instance_id;
    }

    /// #2：安全档位注入（builder 形态，装配点调用；None = 不盖章，v2
    /// 及以下公式）。Interior Mutex——注入后所有后续 append 的新事件带
    /// 档位（hash v3）。值应是规范小写（`SecurityLevel::as_str`）。
    pub fn set_security_level(&self, security_level: Option<String>) {
        *self.security_level.lock() = security_level;
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

        // 撕裂尾防护（BUG-20261114-005，BUG-20261113-168 event_ledger 同族）：
        // 上一笔 writeln 撕裂（磁盘满中途落盘/进程死）留下的半行残迹若不
        // 处理，本笔追加会拼接其后成不可解析行——新事件内存链尾已推进但盘上
        // 不可解析 ⇒ 解析视图从下一事件起永久断链（verify 恒红无修复路径）。
        // 必须在 prev_hash 读取**之前**执行（Restored 分支把恢复事件采纳为
        // 链尾，本笔事件要链到它而不是到恢复前的旧链尾）；append_lock 内
        // 调用（串行保证）。
        let storage_path = self.get_current_segment_path();
        match heal_segment_tail(&storage_path)? {
            TailHeal::Restored(ev) => {
                // 残迹是丢换行的完整事件（content 与 "\n" 两段写之间断裂）
                // ——换行已补，事件已在盘上；把它采纳为本进程链尾（本进程
                // 撕裂时 writeln 返回 Err 未推进内存）。prev 不对齐 = 异常
                // 态（并发/外部写入），fail-closed 诚实拒绝。
                let mut lh = self.last_hash.lock();
                if ev.prev_hash != *lh {
                    return Err(format!(
                        "audit chain healed-tail event prev_hash mismatch (expected {}, got {})",
                        *lh, ev.prev_hash
                    ));
                }
                *lh = ev.hash.clone();
                self.event_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.total_events
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            TailHeal::Truncated { dropped } => {
                tracing::warn!(
                    target: "nemesis_security::integrity",
                    dropped,
                    segment = %storage_path.display(),
                    "[AuditChain] 撕裂尾自愈：截掉撕裂残迹字节（append-only 不变量下自身故障只可能在尾部）"
                );
            }
            TailHeal::Clean => {}
        }

        let timestamp = chrono::Local::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();

        let prev_hash = self.last_hash.lock().clone();

        // #31（W1，2026-11-19）：实例身份 Some → hash v2（v1 字段 +
        // instance_id 进覆盖）；None → v1 字节不变（既有部署零扰动）。
        // #2（W2，2026-11-20）：安全档位 Some → hash v3（v2 字段 +
        // security_level 进覆盖）——审计事件盖「发生时的安全姿态」。
        // hash 计算走 compute_event_hash 单一真相源（append 与 verify
        // 同源——公式变更只改一处，防 append/verify 漂移）。
        let instance_id = self.instance_id.lock().clone();
        let security_level = self.security_level.lock().clone();
        let mut event = AuditEvent {
            id,
            timestamp,
            operation: operation.to_string(),
            tool_name: tool_name.to_string(),
            user: user.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            decision: decision.to_string(),
            reason: reason.to_string(),
            hash: String::new(),
            prev_hash: prev_hash.clone(),
            sign,
            hash_version: if security_level.is_some() {
                3
            } else if instance_id.is_some() {
                2
            } else {
                1
            },
            instance_id,
            security_level,
        };
        let hash = Self::compute_event_hash(&event);
        event.hash = hash.clone();

        // 盘先行（横扫角度 A）：磁盘是审计链的权威定义——只有
        // 行真正落盘才推进内存链状态（last_hash/计数器）并返回 Ok。旧的
        // 「先推进内存、落盘 let _ = 吞错、恒返回 Ok」在磁盘写失败时内存与
        // 盘脱节：verify 按盘上行链式校验从此必 FAIL，事发时刻零可见。
        let json = serde_json::to_string(&event)
            .map_err(|e| format!("audit event serialize failed: {e}"))?;
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

    /// append + 失败可观测（单点：漏斗 [`crate::pipeline`]/直连位点共用）。
    ///
    /// `append` 的 Err 契约是「事件未落盘，内存链尾不前进」——磁盘写失败
    /// 分支内部虽有 error!，但 tracing 日志轮转即失（本模块 degraded 登记
    /// 表文档明文「tracing 日志不算留痕」），且 closed/heal 失败分支无任何
    /// 日志。审计链是安全第 8 层台账，静默丢账 = 审计面开洞运维不可见。
    /// 失败路径在此统一：tracing warn + 降级登记表 `audit_chain` kind
    /// （gateway 水泵 → SSE system-alert + `/ready` 降级面，与 chat_log /
    /// turn_wal 同族基建）。成功路径零额外开销。
    /// 返回 `None` = 追加失败（已可观测）；`Some` = 已落盘事件。
    pub(crate) fn append_or_observe(
        &self,
        operation: &str,
        tool_name: &str,
        user: &str,
        source: &str,
        target: &str,
        decision: &str,
        reason: &str,
        sign: Option<String>,
    ) -> Option<AuditEvent> {
        match self.append_with_sign(
            operation, tool_name, user, source, target, decision, reason, sign,
        ) {
            Ok(ev) => Some(ev),
            Err(e) => {
                tracing::warn!(
                    target: "nemesis_security::integrity",
                    operation = %operation,
                    target = %target,
                    error = %e,
                    "[AuditChain] 审计事件未落盘（已降级登记 audit_chain）"
                );
                nemesis_utils::degraded::report(
                    "audit_chain",
                    format!("operation={operation} target={target}: {e}"),
                );
                None
            }
        }
    }

    /// [`Self::append_or_observe`] 的 bool 面——调用方丢弃事件本体、只
    /// 需「是否已落盘」语义的直连位点用（`let _ = chain.append(..)` 直吞
    /// 形态的替代写法，防回归：bool 无 must_use 丢弃告警，观测在链侧完成）。
    pub fn append_observed(
        &self,
        operation: &str,
        tool_name: &str,
        user: &str,
        source: &str,
        target: &str,
        decision: &str,
        reason: &str,
    ) -> bool {
        self.append_or_observe(
            operation, tool_name, user, source, target, decision, reason, None,
        )
        .is_some()
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

    /// Canonical per-event hash computation（统一定义：append 与 verify
    /// 及 Dashboard 诊断同源）。按事件自带 `hash_version` 选公式——0=旧
    /// 七字段公式（存量链）；1=追加 source+reason；2=v2（v1 字段追加
    /// instance_id，#31）；3=v3（v2 字段追加 security_level，#2 W2 审计
    /// 携带级别）；未知版本返回空串（消费方对比恒不等 = fail-closed，
    /// 与 verify_chain 版本守卫同语义）。版本支持上限见
    /// [`MAX_SUPPORTED_HASH_VERSION`]。
    pub fn compute_event_hash(event: &AuditEvent) -> String {
        if event.hash_version > MAX_SUPPORTED_HASH_VERSION {
            return String::new();
        }
        let mut hasher = Sha256::new();
        hasher.update(event.prev_hash.as_bytes());
        hasher.update(event.timestamp.as_bytes());
        hasher.update(event.operation.as_bytes());
        hasher.update(event.tool_name.as_bytes());
        hasher.update(event.user.as_bytes());
        hasher.update(event.target.as_bytes());
        hasher.update(event.decision.as_bytes());
        if event.hash_version >= 1 {
            hasher.update(event.source.as_bytes());
            hasher.update(event.reason.as_bytes());
        }
        if event.hash_version >= 2 {
            hasher.update(event.instance_id.as_deref().unwrap_or("").as_bytes());
        }
        if event.hash_version >= 3 {
            hasher.update(event.security_level.as_deref().unwrap_or("").as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    /// Verify chain integrity from a list of events.
    pub fn verify_chain(events: &[AuditEvent]) -> bool {
        for (i, event) in events.iter().enumerate() {
            if i > 0 && event.prev_hash != events[i - 1].hash {
                return false;
            }
            // 未知 hash_version = 覆盖面契约不可判定 → fail-closed（宁可报
            // 断链也不静默按旧公式放行新版本事件）。3 = v3（security_level，
            // #2 W2 审计携带级别）。compute_event_hash 对超限版本返回空串
            // → 与任何存 hash 不等，双保险。
            if event.hash_version > MAX_SUPPORTED_HASH_VERSION {
                return false;
            }
            let computed = Self::compute_event_hash(event);
            if computed != event.hash {
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
            // 撕裂尾防护（BUG-20261114-005）：断电/崩溃可撕裂任意历史段
            // （写 syscall 完成不保证落盘完成）——解析前先自愈每段尾部，
            // 恢复的事件随本函数既有解析循环自然入账。
            if let Err(e) = heal_segment_tail(&path) {
                tracing::warn!(
                    target: "nemesis_security::integrity",
                    segment = %path.display(),
                    error = %e,
                    "[AuditChain] 段撕裂尾自愈失败（按原样解析）: {e}"
                );
            }
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

// ---------------------------------------------------------------------------
// 撕裂尾自愈（BUG-20261114-005，BUG-20261113-168 event_ledger 同族移植）
// ---------------------------------------------------------------------------

/// `heal_segment_tail` 的结果。
#[derive(Debug)]
enum TailHeal {
    /// 尾字节 = '\n'，无残迹。
    Clean,
    /// 残迹整体恰是完整 AuditEvent（writeln 的 content 与 "\n" 两段写之间
    /// 断裂，只丢换行）——已补写换行恢复，事件零丢失。调用方决定是否采纳
    /// 进内存链尾（append 采纳；load_segments 交解析循环）。
    Restored(Box<AuditEvent>),
    /// 残迹不可解析（半行/拼行）——已截到最后 '\n'，丢弃 `dropped` 字节。
    Truncated { dropped: u64 },
}

/// 段文件撕裂尾自愈。serde_json 转义控制字符 ⇒ 事件行内无裸 `\n`，因此
/// **尾字节 ≠ '\n' ⇔ 存在撕裂残迹**（O(残迹) 向后扫描，非 O(文件)——段
/// 可达 MB 级且 append 在热路径）。语义对齐 event_ledger `read_chain_tail_healed`
/// （append-only 不变量下自身故障只可能发生在尾部；最后 '\n' 之前的中间
/// 损坏不在本防护范围——verify 对其保持红，本函数只触碰尾区字节不吞证据）。
fn heal_segment_tail(path: &Path) -> Result<TailHeal, String> {
    use std::io::{Read, Seek, SeekFrom};
    let size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return Ok(TailHeal::Clean), // 段不存在 = 从未追加
    };
    if size == 0 {
        return Ok(TailHeal::Clean);
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("heal open: {e}"))?;
    f.seek(SeekFrom::End(-1))
        .map_err(|e| format!("heal seek: {e}"))?;
    let mut last = [0u8; 1];
    f.read_exact(&mut last)
        .map_err(|e| format!("heal read: {e}"))?;
    if last[0] == b'\n' {
        return Ok(TailHeal::Clean);
    }
    // 向后分块找最后 '\n'（残迹 = 单事件撕裂，8KB 步进远超所需）。
    const CHUNK: u64 = 8192;
    let mut scan_end = size;
    let mut residue_start = 0u64;
    while scan_end > 0 {
        let start = scan_end.saturating_sub(CHUNK);
        let len = (scan_end - start) as usize;
        f.seek(SeekFrom::Start(start))
            .map_err(|e| format!("heal seek: {e}"))?;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf)
            .map_err(|e| format!("heal read: {e}"))?;
        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            residue_start = start + i as u64 + 1;
            break;
        }
        scan_end = start;
    }
    let residue_len = (size - residue_start) as usize;
    f.seek(SeekFrom::Start(residue_start))
        .map_err(|e| format!("heal seek: {e}"))?;
    let mut residue = vec![0u8; residue_len];
    f.read_exact(&mut residue)
        .map_err(|e| format!("heal read: {e}"))?;
    // ① 残迹整体是完整事件 → 补换行恢复（证据零丢失）。
    if let Ok(s) = std::str::from_utf8(&residue)
        && let Ok(event) = serde_json::from_str::<AuditEvent>(s.trim_end_matches('\r'))
    {
        let mut af = std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|e| format!("heal open: {e}"))?;
        use std::io::Write as _;
        af.write_all(b"\n")
            .map_err(|e| format!("heal newline: {e}"))?;
        let _ = af.sync_all();
        tracing::warn!(
            target: "nemesis_security::integrity",
            segment = %path.display(),
            "[AuditChain] 撕裂尾自愈：尾区为丢换行的完整事件，已补换行恢复"
        );
        return Ok(TailHeal::Restored(Box::new(event)));
    }
    // ② 截到最后 '\n'（残迹随截丢弃，丢弃量可观测）。
    let af = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("heal open: {e}"))?;
    af.set_len(residue_start)
        .map_err(|e| format!("heal truncate: {e}"))?;
    let _ = af.sync_all();
    Ok(TailHeal::Truncated {
        dropped: residue_len as u64,
    })
}

#[cfg(test)]
mod cov_tests;
#[cfg(test)]
mod tests;
