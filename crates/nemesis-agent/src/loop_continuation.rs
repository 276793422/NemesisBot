//! Phase 2 cluster continuation system.
//!
//! When an async tool (e.g. `cluster_rpc`) is invoked, the agent loop cannot
//! wait synchronously for the result. Instead, it saves a "continuation
//! snapshot" (messages + tool call ID + channel/chat context) so that when the
//! async callback arrives, the loop can be resumed exactly where it left off.
//!
//! The save-barrier pattern ensures that `load_continuation` never reads
//! partially-written data: a `ready` `Notify` is closed only after both the
//! in-memory map and the on-disk snapshot have been fully written.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

use crate::context::RequestContext;
use crate::r#loop::{LlmMessage, LlmProvider, Tool};
use crate::session::SessionStore;
use crate::types::ToolCallInfo;

/// Trait for looking up tools by name.
pub trait ToolLookup {
    fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>>;
}

impl ToolLookup for std::collections::HashMap<String, Arc<dyn Tool>> {
    fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.get(name).cloned()
    }
}

impl ToolLookup for parking_lot::RwLock<std::collections::HashMap<String, Arc<dyn Tool>>> {
    fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.read().get(name).cloned()
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// In-memory continuation snapshot.
///
/// `ready` is a [`Notify`] that is **closed** (all waiters woken) once the
/// snapshot data has been fully populated and persisted to disk. Callers that
/// arrive before the data is ready will block on `ready` for up to 5 seconds.
/// `ready_flag` is an [`AtomicBool`] that is set to `true` once the data is
/// ready, providing a non-blocking check that works even if `notify_waiters()`
/// was already called before the waiter registered.
#[derive(Debug)]
pub struct ContinuationData {
    /// LLM message snapshot (up to the assistant's tool_call).
    pub messages: Vec<LlmMessage>,
    /// The tool call ID that triggered the async operation.
    pub tool_call_id: String,
    /// Original channel for sending the final response.
    pub channel: String,
    /// Original chat ID.
    pub chat_id: String,
    /// Session key for persisting the continuation's final reply to chat_log
    /// and session_store. Empty for legacy on-disk snapshots (skips logging).
    pub session_key: String,
    /// G5: 发起 cluster_rpc 的对端节点 ID。A 侧重启恢复（first_start 把
    /// 快照还原成 TaskManager pending 条目）需要它才知道 poll 该问谁。
    /// 旧快照无此字段（空串），恢复时跳过。
    pub peer_id: String,
    /// T6（多模态）：触发本续行的 turn 已过安全闸的图片路径引用（最后一条
    /// user 消息的 `ConversationTurn.image_refs` 同源）。内存里 `messages`
    /// 保留已水合的图片字节（同进程续行字节级无损）；磁盘快照则剥离字节、
    /// 只落引用（见 [`ContinuationSnapshot::image_refs`]），加载时重水合。
    pub image_refs: Vec<String>,
    /// L1（2026-09-04 四轮盲审）：**每条 user 轮**的图片路径引用（与
    /// `messages` 中 user 消息按出现顺序一一对应）。旧版单层 `image_refs`
    /// 只恢复最后一条 user 轮的图——多轮带图会话崩溃恢复后，更早轮的图
    /// 无声丢失（连占位都没有）。磁盘恢复时若非空则按本字段逐轮重水合，
    /// 为空（旧快照）回退 `image_refs` 单层语义。
    pub image_refs_by_user_turn: Vec<Vec<String>>,
    /// Save barrier: notified when data is fully written.
    pub ready: Arc<Notify>,
    /// Non-blocking ready flag: set to true when data is fully written.
    pub ready_flag: Arc<AtomicBool>,
}

/// On-disk continuation snapshot (serialized as JSON).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuationSnapshot {
    pub task_id: String,
    pub messages: String, // JSON-encoded Vec<LlmMessage>
    pub tool_call_id: String,
    pub channel: String,
    pub chat_id: String,
    /// Session key for restoring log persistence after restart. Defaults to
    /// empty for snapshots written by older binaries (skips log write on
    /// resume).
    #[serde(default)]
    pub session_key: String,
    /// G5: 对端节点 ID。`#[serde(default)]` 兼容旧快照（空串 = 恢复时跳过）。
    #[serde(default)]
    pub peer_id: String,
    /// T6（多模态）：图片路径引用。快照**不落图片字节**——`messages` 序列化
    /// 前剥离每条消息的 `images`（base64），只保留路径引用，加载（重启恢复 /
    /// 磁盘回退）时按引用重读本地文件水合回最后一条 user 消息（B 端本机路径
    /// 语义，读不到 → `[图片已失效]` 占位；不做跨节点传输）。`#[serde(default)]`
    /// 兼容旧快照（空 = 无图）。
    #[serde(default)]
    pub image_refs: Vec<String>,
    /// L1（2026-09-04 四轮盲审）：每条 user 轮的图片路径引用（顺序与
    /// `messages` 里的 user 消息一一对应）。非空时磁盘恢复按本字段逐轮重水合
    /// （多轮带图会话更早轮的图不再丢失）；空 = 旧快照，回退 `image_refs`
    /// 单层语义。`#[serde(default)]` 兼容旧快照。
    #[serde(default)]
    pub image_refs_by_user_turn: Vec<Vec<String>>,
    pub created_at: String,
}

/// Result from a continuation tool execution.
#[derive(Debug, Clone)]
pub struct ContinuationToolResult {
    /// Content for the LLM to consume.
    pub for_llm: String,
    /// Content for the user to see immediately (if not silent).
    pub for_user: String,
    /// Whether the tool result should be silently passed to the LLM only.
    pub silent: bool,
    /// Whether this tool result is from an async operation.
    pub is_async: bool,
    /// Task ID for async tools.
    pub task_id: Option<String>,
    /// Error message, if any.
    pub error: Option<String>,
}

impl Default for ContinuationToolResult {
    fn default() -> Self {
        Self {
            for_llm: String::new(),
            for_user: String::new(),
            silent: true,
            is_async: false,
            task_id: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// T6（多模态）：快照图片引用重水合
// ---------------------------------------------------------------------------

/// 把快照携带的图片路径引用重水合进 `messages` 的**最后一条 user 消息**
/// （触发续行的那个 turn）。B 端本机路径语义：按引用重读本地文件；读不到 /
/// 不是图片 → `[图片已失效: <path>]` 占位行追加进 content（诚实降级，不静默、
/// 不炸），不做跨节点字节传输。`refs` 为空（无图 turn / 旧快照）时是 no-op。
/// 已存在同文占位行则跳过（save→load 多次往返不重复堆叠）。
fn rehydrate_last_user_images(messages: &mut [LlmMessage], refs: &[String]) {
    if refs.is_empty() {
        return;
    }
    let Some(last_user) = messages
        .iter_mut()
        .rev()
        .find(|m| m.role == "user" && m.tool_call_id.is_none())
    else {
        return;
    };
    let (images, placeholders) = crate::image_attach::hydrate_image_refs(refs);
    for p in placeholders {
        if !last_user.content.contains(&p) {
            last_user.content.push('\n');
            last_user.content.push_str(&p);
        }
    }
    last_user.images = images;
}

/// L1（2026-09-04 四轮盲审）：从水合后的消息列表按 user 轮派生图片路径引用
/// （`images[].path`，每条 user 消息一项，顺序一致）。保存快照时调用——
/// 传入的 messages 是 build_messages 产物（已水合），派生即快照真相，
/// 不需要调用方额外穿参。
pub(crate) fn derive_image_refs_by_user_turn(messages: &[LlmMessage]) -> Vec<Vec<String>> {
    messages
        .iter()
        .filter(|m| m.role == "user" && m.tool_call_id.is_none())
        .map(|m| m.images.iter().map(|i| i.path.clone()).collect())
        .collect()
}

/// L1（2026-09-04 四轮盲审）：按 user 轮逐条重水合（磁盘恢复路径）。
/// `refs_by_turn[i]` 对应第 i 条 user 消息；条目为空 = 该轮无图（跳过）；
/// 快照条目数少于 user 消息数（手改/损坏）时多出的轮不恢复（诚实缺省）。
/// 占位行与 [`rehydrate_last_user_images`] 同款去重（多次往返不堆叠）。
fn rehydrate_images_by_user_turn(messages: &mut [LlmMessage], refs_by_turn: &[Vec<String>]) {
    if refs_by_turn.is_empty() {
        return;
    }
    let mut turn_idx = 0usize;
    for m in messages.iter_mut() {
        if m.role != "user" || m.tool_call_id.is_some() {
            continue;
        }
        let Some(refs) = refs_by_turn.get(turn_idx) else {
            break; // 快照引用少于 user 轮数：剩余轮诚实缺省
        };
        turn_idx += 1;
        if refs.is_empty() {
            continue;
        }
        let (images, placeholders) = crate::image_attach::hydrate_image_refs(refs);
        for p in placeholders {
            if !m.content.contains(&p) {
                m.content.push('\n');
                m.content.push_str(&p);
            }
        }
        m.images = images;
    }
}

/// F-F（2026-09-04 四轮盲审）：vision=no 模型的**消息级**投影（续行恢复
/// 路径对应 loop.rs 的 turn 级 `project_turns_for_no_vision`）。
///
/// 为什么需要：T10 的 vision=no 投影发生在 build_messages（turn 视图），
/// 续行快照恢复却绕过 build_messages 直接拿 `messages` 调 provider——
/// 内存快照保留了已水合字节、磁盘恢复又按引用重水合，vision=no 模型
/// 接管续行时请求带图 → provider 4xx（正常轮被投影保护、恢复轮裸奔）。
/// 在 `handle_cluster_continuation` 调 LLM 前按 active 模型的 vision
/// 解析结果统一投影：带图消息清空 `images` 并追加占位文本（最后一条
/// user 轮「未发送」、其余「已省略」，与 turn 级投影同语义同措辞）；
/// 已含同文占位则不重复堆叠。
pub fn project_messages_for_no_vision(messages: &mut [LlmMessage]) {
    let last_user_pos = messages
        .iter()
        .rposition(|m| m.role == "user" && m.tool_call_id.is_none());
    for (i, m) in messages.iter_mut().enumerate() {
        if m.images.is_empty() {
            continue;
        }
        m.images.clear();
        let note = if Some(i) == last_user_pos {
            "[图片未发送: 当前模型 vision=no（不支持图像输入），图片已忽略]"
        } else {
            "[图片已省略: 当前模型仅支持文本]"
        };
        if m.content.contains(note) {
            continue;
        }
        if m.content.trim().is_empty() {
            m.content.push_str(note);
        } else {
            m.content.push('\n');
            m.content.push_str(note);
        }
    }
}

// ---------------------------------------------------------------------------
// ContinuationStore -- persists snapshots to disk
// ---------------------------------------------------------------------------

/// Manages on-disk continuation snapshots under `{workspace}/cluster/rpc_cache/`.
pub struct ContinuationStore {
    base_dir: PathBuf,
}

impl ContinuationStore {
    /// Create a new store rooted at the given workspace directory.
    pub fn new(workspace: &std::path::Path) -> Self {
        // 路径唯一真相源 = nemesis-path（与 nemesis-cluster ContinuationStore
        // 同一目录，两侧禁止各自 join）。
        let base_dir = nemesis_path::resolve_cluster_rpc_cache_dir_in_workspace(workspace);
        Self { base_dir }
    }

    /// Ensure the storage directory exists.
    fn ensure_dir(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.base_dir)
    }

    /// Save a continuation snapshot to disk.
    pub fn save(&self, snapshot: &ContinuationSnapshot) -> std::io::Result<()> {
        self.ensure_dir()?;
        let path = self.snapshot_path(&snapshot.task_id);
        let json = serde_json::to_string_pretty(snapshot).map_err(std::io::Error::other)?;
        std::fs::write(&path, json)?;
        Ok(())
    }

    /// Load a continuation snapshot from disk.
    pub fn load(&self, task_id: &str) -> std::io::Result<ContinuationSnapshot> {
        let path = self.snapshot_path(task_id);
        let json = std::fs::read_to_string(&path)?;
        serde_json::from_str(&json).map_err(std::io::Error::other)
    }

    /// Delete a continuation snapshot from disk.
    pub fn delete(&self, task_id: &str) {
        let path = self.snapshot_path(task_id);
        if path.exists()
            && let Err(e) = std::fs::remove_file(&path)
        {
            warn!(
                "[Continuation] Failed to delete continuation snapshot {}: {}",
                task_id, e
            );
        }
    }

    /// List all pending task IDs on disk.
    ///
    /// Scans the cache directory for `.json` files and returns their task IDs
    /// (matching Go's `ListPending` which scans disk on startup).
    pub fn list_pending(&self) -> Vec<String> {
        let mut task_ids = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return task_ids;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "json").unwrap_or(false)
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                task_ids.push(stem.to_string());
            }
        }
        task_ids
    }

    /// Recover all continuation snapshots from disk into the manager's
    /// in-memory map.
    ///
    /// Scans the cache directory for `.json` files, deserializes each one,
    /// and populates the in-memory continuations so they can be resumed.
    /// Returns the number of snapshots recovered.
    pub fn recover_to_manager(&self, manager: &ContinuationManager) -> usize {
        let task_ids = self.list_pending();
        let mut recovered = 0;

        for task_id in &task_ids {
            // Skip if already in memory
            if manager.has_continuation_sync(task_id) {
                continue;
            }

            match self.load(task_id) {
                Ok(snapshot) => {
                    let mut messages: Vec<LlmMessage> = match serde_json::from_str(
                        &snapshot.messages,
                    ) {
                        Ok(m) => m,
                        Err(e) => {
                            warn!(
                                "[Continuation] Failed to deserialize messages for snapshot {}: {}",
                                task_id, e
                            );
                            continue;
                        }
                    };
                    // T6（多模态）：磁盘快照不带图片字节（保存时已剥离）。
                    // L1：优先按每轮引用逐 user 轮重水合（多轮带图会话更早
                    // 轮的图不再丢）；空（旧快照）回退单层 image_refs 语义。
                    if snapshot.image_refs_by_user_turn.is_empty() {
                        rehydrate_last_user_images(&mut messages, &snapshot.image_refs);
                    } else {
                        rehydrate_images_by_user_turn(
                            &mut messages,
                            &snapshot.image_refs_by_user_turn,
                        );
                    }

                    // Create ready continuation data (loaded from disk, so already complete)
                    let ready = Arc::new(Notify::new());
                    let ready_flag = Arc::new(AtomicBool::new(true));

                    let cont_data = Arc::new(ContinuationData {
                        messages,
                        tool_call_id: snapshot.tool_call_id,
                        channel: snapshot.channel,
                        chat_id: snapshot.chat_id,
                        session_key: snapshot.session_key,
                        peer_id: snapshot.peer_id,
                        image_refs: snapshot.image_refs,
                        image_refs_by_user_turn: snapshot.image_refs_by_user_turn,
                        ready,
                        ready_flag,
                    });

                    manager.insert_continuation_sync(task_id.clone(), cont_data);
                    recovered += 1;
                    info!(
                        "[Continuation] Recovered continuation snapshot from disk: task_id={}",
                        task_id
                    );
                }
                Err(e) => {
                    warn!(
                        "[Continuation] Failed to load continuation snapshot {}: {}",
                        task_id, e
                    );
                }
            }
        }

        if recovered > 0 {
            info!(
                "[Continuation] Recovered {} continuation snapshots from disk",
                recovered
            );
        }

        recovered
    }

    /// List task IDs whose disk snapshot is older than `max_age` (mtime-based).
    ///
    /// 2026-08-25: snapshots whose callback never arrives (peer died, task
    /// lost mid-flight) used to accumulate in `cluster/rpc_cache/` forever —
    /// the write side had no retention counterpart. The caller
    /// (`ContinuationManager::cleanup_old_snapshots`) evicts both the file
    /// and the in-memory twin. Mtime (not the in-file `created_at` field) is
    /// the clock: every save rewrites the file, so mtime == last write.
    pub fn stale_task_ids(&self, max_age: Duration) -> Vec<String> {
        let cutoff = std::time::SystemTime::now() - max_age;
        let mut stale = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.base_dir) else {
            return stale;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().map(|e| e == "json").unwrap_or(false) {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if modified < cutoff
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
            {
                stale.push(stem.to_string());
            }
        }
        stale
    }

    fn snapshot_path(&self, task_id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", task_id))
    }
}

// ---------------------------------------------------------------------------
// ContinuationManager -- in-memory + disk dual-write
// ---------------------------------------------------------------------------

/// Manages continuation snapshots with the save-barrier pattern.
///
/// This is the main entry point for the Phase 2 continuation system.
/// It holds an in-memory cache of active continuations and an optional
/// disk store for persistence across restarts.
pub struct ContinuationManager {
    /// In-memory continuation data: task_id -> data.
    continuations: Mutex<HashMap<String, Arc<ContinuationData>>>,
    /// Optional disk store for persistence.
    disk_store: Option<ContinuationStore>,
    /// Timeout for waiting on the save barrier.
    barrier_timeout: Duration,
}

impl ContinuationManager {
    /// Create a new continuation manager without disk persistence.
    pub fn new() -> Self {
        Self {
            continuations: Mutex::new(HashMap::new()),
            disk_store: None,
            barrier_timeout: Duration::from_secs(5),
        }
    }

    /// Create a new continuation manager with disk persistence.
    ///
    /// Automatically scans the disk cache directory for any persisted
    /// continuation snapshots and loads them into memory (matching Go's
    /// `ListPending` disk scan on startup).
    pub fn with_disk_store(workspace: &std::path::Path) -> Self {
        let disk_store = ContinuationStore::new(workspace);
        let manager = Self {
            continuations: Mutex::new(HashMap::new()),
            disk_store: Some(disk_store),
            barrier_timeout: Duration::from_secs(5),
        };
        // Recover any pending snapshots from disk
        if let Some(ref store) = manager.disk_store {
            store.recover_to_manager(&manager);
        }
        manager
    }

    /// Set the barrier timeout (default: 5 seconds).
    pub fn set_barrier_timeout(&mut self, timeout: Duration) {
        self.barrier_timeout = timeout;
    }

    /// Save a continuation snapshot (memory + disk dual-write).
    ///
    /// T6（多模态）：无图调用的兼容入口（签名不动），等价于
    /// [`Self::save_continuation_with_images`] 传空引用。
    ///
    /// This method implements the save-barrier pattern:
    /// 1. Create `ContinuationData` with an open `ready` Notify.
    /// 2. Insert into the in-memory map (loaders will see the entry but wait on `ready`).
    /// 3. Persist to disk.
    /// 4. Close the `ready` Notify (waking any waiting loaders).
    pub async fn save_continuation(
        &self,
        task_id: &str,
        messages: Vec<LlmMessage>,
        tool_call_id: &str,
        channel: &str,
        chat_id: &str,
        session_key: &str,
        peer_id: &str,
    ) {
        self.save_continuation_with_images(
            task_id,
            messages,
            tool_call_id,
            channel,
            chat_id,
            session_key,
            peer_id,
            &[],
        )
        .await;
    }

    /// T6（多模态）：带图片路径引用的续行快照保存（memory + disk 双写）。
    ///
    /// 内存侧 `ContinuationData.messages` 原样保存（同进程续行字节级无损，
    /// 已水合的图片字节保留）；磁盘侧 `ContinuationSnapshot.messages` 序列化
    /// 前剥离每条消息的 `images`（base64 字节不落盘），只落 `image_refs`
    /// 路径引用，加载时重水合。
    #[allow(clippy::too_many_arguments)]
    pub async fn save_continuation_with_images(
        &self,
        task_id: &str,
        messages: Vec<LlmMessage>,
        tool_call_id: &str,
        channel: &str,
        chat_id: &str,
        session_key: &str,
        peer_id: &str,
        image_refs: &[String],
    ) {
        let ready = Arc::new(Notify::new());
        let ready_flag = Arc::new(AtomicBool::new(false));

        // L1（2026-09-04 四轮盲审）：按 user 轮派生图片引用快照字段——
        // messages 是 build_messages 产物（已水合），从 `images[].path`
        // 派生即快照真相，无需调用方额外穿参。
        let image_refs_by_user_turn = derive_image_refs_by_user_turn(&messages);

        let cont_data = Arc::new(ContinuationData {
            messages: messages.clone(),
            tool_call_id: tool_call_id.to_string(),
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            session_key: session_key.to_string(),
            peer_id: peer_id.to_string(),
            image_refs: image_refs.to_vec(),
            image_refs_by_user_turn: image_refs_by_user_turn.clone(),
            ready: ready.clone(),
            ready_flag: ready_flag.clone(),
        });

        // Step 1: Insert into memory (ready not yet notified).
        {
            let mut conts = self.continuations.lock().await;
            conts.insert(task_id.to_string(), cont_data);
        }

        // Step 2: Persist to disk (T6: strip image bytes, keep path refs).
        if let Some(ref store) = self.disk_store {
            let stripped: Vec<LlmMessage> = messages
                .iter()
                .map(|m| {
                    let mut m = m.clone();
                    if !m.images.is_empty() {
                        m.images = Vec::new();
                    }
                    m
                })
                .collect();
            let messages_json = serde_json::to_string(&stripped).unwrap_or_else(|e| {
                warn!(
                    "[Continuation] Failed to serialize messages for continuation: {}",
                    e
                );
                "[]".to_string()
            });
            let snapshot = ContinuationSnapshot {
                task_id: task_id.to_string(),
                messages: messages_json,
                tool_call_id: tool_call_id.to_string(),
                channel: channel.to_string(),
                chat_id: chat_id.to_string(),
                session_key: session_key.to_string(),
                peer_id: peer_id.to_string(),
                image_refs: image_refs.to_vec(),
                image_refs_by_user_turn,
                created_at: chrono::Local::now().to_rfc3339(),
            };
            if let Err(e) = store.save(&snapshot) {
                warn!(
                    "[Continuation] Failed to persist continuation snapshot to disk: {}",
                    e
                );
            }
        }

        // Step 3: Mark as ready and notify waiters.
        ready_flag.store(true, Ordering::Release);
        ready.notify_waiters();
        info!(
            "[Continuation] Continuation snapshot saved (memory + disk): task_id={}",
            task_id
        );
    }

    /// Load a continuation snapshot, trying memory first (with save-barrier wait),
    /// then falling back to disk.
    pub async fn load_continuation(&self, task_id: &str) -> Option<ContinuationData> {
        // Try memory with save-barrier.
        if let Some(data) = self.wait_for_continuation(task_id).await {
            return Some(data);
        }

        // Fall back to disk.
        self.try_load_from_disk(task_id).await
    }

    /// Wait for a continuation to be ready in memory.
    ///
    /// If the entry exists but `ready` hasn't been notified yet, we wait
    /// up to `barrier_timeout` for the data to be populated.
    /// If the entry doesn't exist at all, we retry with short sleeps
    /// until the timeout expires (covers the race where the callback
    /// arrives before the snapshot is registered).
    async fn wait_for_continuation(&self, task_id: &str) -> Option<ContinuationData> {
        let deadline = tokio::time::Instant::now() + self.barrier_timeout;

        loop {
            {
                let conts = self.continuations.lock().await;
                if let Some(data) = conts.get(task_id) {
                    // Entry exists. Check if already ready (non-blocking).
                    if data.ready_flag.load(Ordering::Acquire) {
                        return Some(ContinuationData {
                            messages: data.messages.clone(),
                            tool_call_id: data.tool_call_id.clone(),
                            channel: data.channel.clone(),
                            chat_id: data.chat_id.clone(),
                            session_key: data.session_key.clone(),
                            peer_id: data.peer_id.clone(),
                            image_refs: data.image_refs.clone(),
                            image_refs_by_user_turn: data.image_refs_by_user_turn.clone(),
                            ready: data.ready.clone(),
                            ready_flag: data.ready_flag.clone(),
                        });
                    }

                    let ready = data.ready.clone();
                    let ready_flag = data.ready_flag.clone();
                    drop(conts); // Release lock before awaiting.

                    // Double-check after releasing lock (save might have completed).
                    if ready_flag.load(Ordering::Acquire) {
                        let conts = self.continuations.lock().await;
                        return conts.get(task_id).map(|arc| ContinuationData {
                            messages: arc.messages.clone(),
                            tool_call_id: arc.tool_call_id.clone(),
                            channel: arc.channel.clone(),
                            chat_id: arc.chat_id.clone(),
                            session_key: arc.session_key.clone(),
                            peer_id: arc.peer_id.clone(),
                            image_refs: arc.image_refs.clone(),
                            image_refs_by_user_turn: arc.image_refs_by_user_turn.clone(),
                            ready: arc.ready.clone(),
                            ready_flag: arc.ready_flag.clone(),
                        });
                    }

                    // Wait for ready with remaining timeout.
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() {
                        warn!(
                            "[Continuation] Continuation ready timeout, falling back to disk: task_id={}",
                            task_id
                        );
                        return None;
                    }

                    // Use tokio::select! to wait with a timeout.
                    let notified = tokio::select! {
                        _ = ready.notified() => true,
                        _ = tokio::time::sleep(remaining) => false,
                    };

                    if notified || ready_flag.load(Ordering::Acquire) {
                        // Data is ready. Read it.
                        let conts = self.continuations.lock().await;
                        return conts.get(task_id).map(|arc| ContinuationData {
                            messages: arc.messages.clone(),
                            tool_call_id: arc.tool_call_id.clone(),
                            channel: arc.channel.clone(),
                            chat_id: arc.chat_id.clone(),
                            session_key: arc.session_key.clone(),
                            peer_id: arc.peer_id.clone(),
                            image_refs: arc.image_refs.clone(),
                            image_refs_by_user_turn: arc.image_refs_by_user_turn.clone(),
                            ready: arc.ready.clone(),
                            ready_flag: arc.ready_flag.clone(),
                        });
                    } else {
                        warn!(
                            "[Continuation] Continuation ready timeout, falling back to disk: task_id={}",
                            task_id
                        );
                        return None;
                    }
                }
            }

            // Entry doesn't exist yet. Short sleep and retry.
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }

            let sleep_duration = remaining.min(Duration::from_millis(10));
            tokio::time::sleep(sleep_duration).await;
        }
    }

    /// Try to load a continuation snapshot from disk (restart recovery path).
    async fn try_load_from_disk(&self, task_id: &str) -> Option<ContinuationData> {
        let store = self.disk_store.as_ref()?;
        let snapshot = store.load(task_id).ok()?;

        let mut messages: Vec<LlmMessage> = serde_json::from_str(&snapshot.messages).ok()?;
        // T6（多模态）：磁盘快照不带图片字节，按路径引用重水合（失效 → 占位）。
        // L1：优先按每轮引用逐 user 轮重水合；空（旧快照）回退单层语义。
        if snapshot.image_refs_by_user_turn.is_empty() {
            rehydrate_last_user_images(&mut messages, &snapshot.image_refs);
        } else {
            rehydrate_images_by_user_turn(&mut messages, &snapshot.image_refs_by_user_turn);
        }

        let ready_flag = Arc::new(AtomicBool::new(true)); // Already ready from disk
        Some(ContinuationData {
            messages,
            tool_call_id: snapshot.tool_call_id,
            channel: snapshot.channel,
            chat_id: snapshot.chat_id,
            session_key: snapshot.session_key,
            peer_id: snapshot.peer_id,
            image_refs: snapshot.image_refs,
            image_refs_by_user_turn: snapshot.image_refs_by_user_turn,
            ready: Arc::new(Notify::new()),
            ready_flag,
        })
    }

    /// Remove a continuation from memory and disk.
    /// Mirrors Go's cleanup in `handleClusterContinuation` which deletes
    /// both the in-memory map entry and the disk snapshot.
    pub async fn remove_continuation(&self, task_id: &str) {
        {
            let mut conts = self.continuations.lock().await;
            conts.remove(task_id);
        }
        // Delete the disk snapshot as well to prevent unbounded disk growth.
        // Mirrors Go's: store.Delete(taskID).
        if let Some(ref store) = self.disk_store {
            store.delete(task_id);
        }
    }

    /// Check whether a continuation exists in memory.
    pub async fn has_continuation(&self, task_id: &str) -> bool {
        let conts = self.continuations.lock().await;
        conts.contains_key(task_id)
    }

    /// TTL cleanup: remove continuations older than `max_age` from BOTH the
    /// disk store and the in-memory map (2026-08-25).
    ///
    /// A continuation waiting longer than the TTL is dead by construction —
    /// the outer RPC timeout ceiling is 60 minutes, so nothing legitimately
    /// in-flight can be days old; what's left is a callback that never comes
    /// (peer died, task lost) and would otherwise leak on disk forever and,
    /// after a restart, get re-recovered into memory by `recover_to_manager`.
    /// Mirrors the SessionStore 7-day TTL (`cleanup_old_sessions`) and the
    /// nemesis-cluster `ContinuationStore::cleanup_old` semantics (same
    /// directory, other implementation). No-op without a disk store.
    /// Returns the number of continuations removed.
    pub async fn cleanup_old_snapshots(&self, max_age: Duration) -> usize {
        let Some(store) = self.disk_store.as_ref() else {
            return 0;
        };
        let stale = store.stale_task_ids(max_age);
        for task_id in &stale {
            self.continuations.lock().await.remove(task_id);
            store.delete(task_id);
        }
        stale.len()
    }

    /// Check whether a continuation exists in memory (synchronous).
    ///
    /// Uses `try_lock()` — never blocks the thread, never touches the tokio
    /// runtime, so it is safe to call from a sync fn (`with_disk_store` at
    /// gateway startup) even though that runs on the async gateway thread.
    /// The prior `blocking_lock()` panicked there ("Cannot block the current
    /// thread from within a runtime"). At boot the map is uncontended, so
    /// `try_lock` succeeds; on the rare contended miss it returns `false` and
    /// the async wait path falls back to the disk store (source of truth).
    pub fn has_continuation_sync(&self, task_id: &str) -> bool {
        match self.continuations.try_lock() {
            Ok(g) => g.contains_key(task_id),
            Err(_) => false,
        }
    }

    /// Insert a continuation into the in-memory map (synchronous).
    ///
    /// Used during disk recovery at startup (`recover_to_manager` at boot).
    /// See `has_continuation_sync` for why `try_lock()` (not `blocking_lock()`)
    /// is the correct sync entry here. A failed acquisition is skipped — the
    /// entry remains on disk and is picked up lazily by the async wait path.
    pub fn insert_continuation_sync(&self, task_id: String, data: Arc<ContinuationData>) {
        if let Ok(mut g) = self.continuations.try_lock() {
            g.insert(task_id, data);
        }
    }
}

impl Default for ContinuationManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// handle_cluster_continuation -- the core continuation handler
// ---------------------------------------------------------------------------

/// Persist a continuation's final assistant reply to BOTH stores, store-first.
///
/// Order matters (2026-08-25 self-heal regression fix): `get_or_create` may
/// REBUILD a missing store entry by replaying the jsonl chat_log
/// (`SessionStore::rebuild_from_chat_log`). If the chat_log append ran FIRST,
/// the rebuild would already contain this final reply and `add_message` would
/// append it a SECOND time — a duplicated assistant turn in the model's
/// context. Store-first mirrors the normal AgentLoop turn-end path (store
/// persist, then chat_log append), so rebuild and append can never see the
/// same row twice. Extracted from `handle_cluster_continuation` step 6 so the
/// ordering contract is unit-testable in isolation.
pub(crate) fn persist_final_reply(
    store: Option<&SessionStore>,
    session_key: &str,
    model: &str,
    final_content: &str,
) {
    if let Some(store) = store {
        store.get_or_create(session_key);
        store.add_message(session_key, "assistant", final_content);
        if let Err(e) = store.save(session_key) {
            warn!(
                "[Continuation] Failed to persist session history for {}: {}",
                session_key, e
            );
        }
    }
    crate::chat_log::append_chat_log_with_model(
        session_key,
        "assistant",
        final_content,
        Some(model),
    );
}

/// Handle a cluster continuation callback.
///
/// This function:
/// 1. Loads the continuation snapshot.
/// 2. Retrieves the task result.
/// 3. Appends the real tool result to the messages.
/// 4. Runs the LLM + tool loop to completion.
/// 5. Publishes the final response.
pub async fn handle_cluster_continuation<T: ToolLookup>(
    manager: &ContinuationManager,
    task_id: &str,
    task_response: &str,
    task_failed: bool,
    task_error: Option<&str>,
    provider: &dyn LlmProvider,
    model: &str,
    tools: &T,
    outbound_tx: &tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,
    observer_manager: Option<Arc<nemesis_observer::Manager>>,
    session_store: Option<&SessionStore>,
    vision_supported: bool,
) {
    // Generate trace_id for observer event correlation.
    let trace_id = format!(
        "continuation-{}-{}",
        task_id,
        chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let start_time = std::time::Instant::now();

    // Emit conversation_start observer event.
    if let Some(ref mgr) = observer_manager {
        let event = crate::loop_executor::ObserverEvent::ConversationStart {
            trace_id: trace_id.clone(),
            session_key: format!("continuation-{}", task_id),
            channel: String::new(),
            chat_id: String::new(),
            sender_id: "continuation".to_string(),
            content: format!("cluster_continuation:{}", task_id),
        }
        .to_conversation_event();
        mgr.emit_sync(event).await;
    }
    // 1. Load continuation snapshot.
    let cont_data = match manager.load_continuation(task_id).await {
        Some(data) => data,
        None => {
            // This branch is hit by dashboard-initiated peer_chat tasks:
            // the web handler tasks_submit sends PeerChat RPC directly
            // without going through the cluster_rpc tool, so no
            // continuation snapshot was ever saved. The result is
            // delivered through the TaskManager path (gateway Route 3),
            // so silently skip. Real cluster_rpc continuations survive
            // crashes via disk fallback (try_load_from_disk), so a miss
            // here means there genuinely was nothing to resume.
            debug!(
                "[Continuation] No continuation for task_id={} (likely dashboard-initiated peer_chat, skipping)",
                task_id
            );
            return;
        }
    };

    // 2. Build tool result content from task response.
    let tool_result_content = if task_failed {
        format!(
            "Error: {}",
            task_error.unwrap_or("Task failed with unknown error")
        )
    } else {
        task_response.to_string()
    };

    // 3. Remove the continuation now that we have the data.
    manager.remove_continuation(task_id).await;

    // 4. Build messages: snapshot + real tool result.
    let mut messages = cont_data.messages.clone();
    messages.push(LlmMessage {
        role: "tool".to_string(),
        content: tool_result_content,
        tool_calls: None,
        tool_call_id: Some(cont_data.tool_call_id.clone()),
        reasoning_content: None,
        images: Vec::new(),
    });

    // F-F（2026-09-04 四轮盲审）：vision=no 模型接管续行时，恢复路径
    // （内存快照的已水合字节 / 磁盘重水合）绕过了 build_messages 的 T10
    // 投影——正常轮被保护、恢复轮裸奔 → provider 4xx。调 LLM 前按 active
    // 模型的 vision 解析结果统一投影（supported / 默认放行 = 零改动）。
    if !vision_supported {
        project_messages_for_no_vision(&mut messages);
    }

    // 5. Run the continuation LLM + tool loop.
    let max_iterations = 20;
    let mut final_content = String::new();

    for iteration in 1..=max_iterations {
        debug!(
            "[Continuation] Continuation LLM iteration {}/{}: task_id={}",
            iteration, max_iterations, task_id
        );

        // Emit LLM request observer event.
        if let Some(ref mgr) = observer_manager {
            let msg_values: Vec<serde_json::Value> = messages
                .iter()
                .filter_map(|m| serde_json::to_value(m).ok())
                .collect();
            let event = crate::loop_executor::ObserverEvent::LlmRequest {
                trace_id: trace_id.clone(),
                round: iteration as u32,
                model: model.to_string(),
                messages_count: messages.len(),
                tools_count: 0,
                messages: msg_values,
                tools: vec![],
                provider_name: String::new(),
                api_key: String::new(),
                api_base: String::new(),
            }
            .to_conversation_event();
            let mgr = Arc::clone(mgr);
            tokio::spawn(async move { mgr.emit(event).await });
        }

        let round_start = std::time::Instant::now();
        let mut response = match provider.chat(model, messages.clone(), None, vec![]).await {
            Ok(resp) => resp,
            Err(e) => {
                warn!("[Continuation] Continuation LLM call failed: {}", e);
                final_content = format!("[LLM error: {}]", e);
                break;
            }
        };

        // Emit LLM response observer event.
        let round_duration = round_start.elapsed();
        if let Some(ref mgr) = observer_manager {
            let tc_values: Vec<serde_json::Value> = response
                .tool_calls
                .iter()
                .filter_map(|tc| serde_json::to_value(tc).ok())
                .collect();
            let tc_count = response.tool_calls.len();
            let event = crate::loop_executor::ObserverEvent::LlmResponse {
                trace_id: trace_id.clone(),
                round: iteration as u32,
                duration_ms: round_duration.as_millis() as u64,
                has_tool_calls: !response.tool_calls.is_empty(),
                content: response.content.clone(),
                tool_calls: tc_values,
                tool_calls_count: tc_count,
                finish_reason: if response.finished {
                    Some("stop".to_string())
                } else {
                    None
                },
                usage: response.usage.clone(),
                raw_request_body: response.raw_request_body.take(),
                raw_response_body: response.raw_response_body.take(),
            }
            .to_conversation_event();
            let mgr = Arc::clone(mgr);
            tokio::spawn(async move { mgr.emit(event).await });
        }

        if response.tool_calls.is_empty() {
            final_content = response.content.clone();
            break;
        }

        // Build assistant message with tool calls.
        let assistant_msg = LlmMessage {
            role: "assistant".to_string(),
            content: response.content.clone(),
            tool_calls: Some(response.tool_calls.clone()),
            tool_call_id: None,
            reasoning_content: response.reasoning_content.clone(),
            images: Vec::new(),
        };
        messages.push(assistant_msg);

        // Execute tool calls.
        for tc in &response.tool_calls {
            let tool_start = std::time::Instant::now();
            let tool_result =
                execute_tool_for_continuation(tools, tc, &cont_data.channel, &cont_data.chat_id)
                    .await;
            let tool_duration = tool_start.elapsed();

            // Emit tool call observer event.
            if let Some(ref mgr) = observer_manager {
                let result_str = tool_result
                    .error
                    .clone()
                    .unwrap_or_else(|| tool_result.for_llm.clone());
                let event = crate::loop_executor::ObserverEvent::ToolCall {
                    trace_id: trace_id.clone(),
                    tool_name: tc.name.clone(),
                    success: tool_result.error.is_none(),
                    duration_ms: tool_duration.as_millis() as u64,
                    round: iteration as u32,
                    arguments: tc.arguments.clone(),
                    result: result_str,
                }
                .to_conversation_event();
                let mgr = Arc::clone(mgr);
                tokio::spawn(async move { mgr.emit(event).await });
            }

            // Send ForUser content if not silent.
            if !tool_result.silent && !tool_result.for_user.is_empty() {
                let outbound = nemesis_types::channel::OutboundMessage {
                    channel: cont_data.channel.clone(),
                    chat_id: cont_data.chat_id.clone(),
                    content: tool_result.for_user.clone(),
                    message_type: String::new(),
                    meta: Default::default(),
                };
                if let Err(e) = outbound_tx.send(outbound).await {
                    warn!(
                        "[Continuation] Failed to send continuation tool output: {}",
                        e
                    );
                }
            }

            // Handle nested async: save a new continuation.
            if tool_result.is_async
                && let Some(ref nested_task_id) = tool_result.task_id
            {
                // G5: 嵌套异步的对端 ID —— 从 __ASYNC__:{task_id}:{target_id}:{name}
                // marker 里抠第 3 段；抠不到（老格式）回退继承当前快照的 peer_id。
                let nested_peer = tool_result
                    .for_llm
                    .strip_prefix("__ASYNC__:")
                    .and_then(|rest| rest.split(':').nth(1))
                    .unwrap_or(&cont_data.peer_id)
                    .to_string();
                manager
                    .save_continuation_with_images(
                        nested_task_id,
                        messages.clone(),
                        &tc.id,
                        &cont_data.channel,
                        &cont_data.chat_id,
                        &cont_data.session_key,
                        &nested_peer,
                        &cont_data.image_refs,
                    )
                    .await;
            }

            // Determine content for LLM.
            let content_for_llm = if tool_result.for_llm.is_empty() {
                tool_result.error.unwrap_or_default()
            } else {
                tool_result.for_llm
            };

            messages.push(LlmMessage {
                role: "tool".to_string(),
                content: content_for_llm,
                tool_calls: None,
                tool_call_id: Some(tc.id.clone()),
                reasoning_content: None,
                images: Vec::new(),
            });
        }
    }

    // 6. Send final response.
    if !final_content.is_empty() {
        // Persist final reply to chat_log and session_store before sending
        // outbound. Skip when session_key is empty (legacy on-disk snapshots
        // saved before this field existed).
        if !cont_data.session_key.is_empty() {
            persist_final_reply(session_store, &cont_data.session_key, model, &final_content);
        }

        let outbound = nemesis_types::channel::OutboundMessage {
            channel: cont_data.channel.clone(),
            chat_id: cont_data.chat_id.clone(),
            content: final_content.clone(),
            message_type: String::new(),
            meta: nemesis_types::channel::OutboundMeta {
                model: Some(model.to_string()),
            },
        };
        if let Err(e) = outbound_tx.send(outbound).await {
            warn!(
                "[Continuation] Failed to send continuation final response: {}",
                e
            );
        }

        info!(
            "[Continuation] Continuation response sent: task_id={}, content_len={}, target_channel={}",
            task_id,
            final_content.len(),
            cont_data.channel
        );
    }

    // Emit conversation_end observer event.
    let duration_ms = start_time.elapsed().as_millis() as u64;
    if let Some(ref mgr) = observer_manager {
        let event = crate::loop_executor::ObserverEvent::ConversationEnd {
            trace_id: trace_id.clone(),
            session_key: format!("continuation-{}", task_id),
            total_rounds: max_iterations as u32,
            duration_ms,
            content: final_content.clone(),
            channel: cont_data.channel.clone(),
            chat_id: cont_data.chat_id.clone(),
        }
        .to_conversation_event();
        mgr.emit_sync(event).await;
    }
}

/// Execute a single tool call during continuation processing.
async fn execute_tool_for_continuation<T: ToolLookup>(
    tools: &T,
    tc: &ToolCallInfo,
    channel: &str,
    chat_id: &str,
) -> ContinuationToolResult {
    let context = RequestContext::new(channel, chat_id, "continuation", "continuation_session");

    match tools.get_tool(&tc.name) {
        Some(tool) => match tool.execute(&tc.arguments, &context).await {
            Ok(output) => ContinuationToolResult {
                for_llm: output,
                ..Default::default()
            },
            Err(e) => ContinuationToolResult {
                error: Some(e.clone()),
                ..Default::default()
            },
        },
        None => ContinuationToolResult {
            error: Some(format!("Unknown tool '{}'", tc.name)),
            ..Default::default()
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
