//! worker 侧持久化发件箱（看板项目档案 goal P3/D2）。
//!
//! 任务任一终态（交付成功/FAIL/取消/超时）经 [`super::rpc::peer_chat_handler::TaskResultPersister::on_task_terminal`]
//! 钩子入队：把 `workspace/logs/cluster_logs/{source_node}/{ts}_{task_id}/`
//! 执行记录**先复制落盘**到 `<workspace>/cluster/outbox/<task_id>/`（防日志
//! 轮换丢原件），后台推送循环再分块推给 master；master 落盘 ACK（end ok）
//! 才删本地。**绝不依赖 worker LLM 主动拉取**（用户红线）。
//!
//! - **零丢失闭环**：推送失败/掉线 → 留 `pending` 周期重试（15s tick +
//!   Notify 即时踢）；worker 重启 → [`TransferOutbox::sweep_startup`]
//!   把 `pushing` 重置回 `pending` 续推，并对「有执行残留但无发件箱记录」
//!   的任务目录补入队（崩溃兜底）。
//! - **幂等**：master 侧 `(task_id, content_hash)` 去重（D3）+ end 幂等
//!   重放标记——at-least-once 语义下重复投递无害。
//! - **D4 体积护栏（worker 侧同适用）**：载荷超 `max_bytes`（现读 provider，
//!   热生效）→ 本地标记 `over_limit` + 向 master 发 `transfer_overlimit`
//!   通知（决策流出卡），**绝不截断**。护栏调大后启动清扫自动重新武装。
//! - **transfer_id 确定性**：`{task_id}-{content_hash12}-{chunk_size}`——
//!   同内容同块大小跨重启得到同一 id（master staging 断点续传依据）；
//!   块大小变更 = 新传输（旧 staging 不复用，防跨界拼接腐坏）。
//!
//! 同构先例 = task_result_store（磁盘模式 + confirm 双删 + 启动清扫）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::changeset;
use crate::transfer::{
    ACTION_TRANSFER_BEGIN, ACTION_TRANSFER_CHUNK, ACTION_TRANSFER_END, ACTION_TRANSFER_OVERLIMIT,
    TRANSFER_KIND_EXECUTION_RECORDS, TransferBegin, TransferBeginReply, TransferChunk,
    TransferEndReply, TransferOverlimit, b64_encode, chunk_bytes, chunk_count, collect_dir_files,
    content_hash, copy_dir_recursive, plan_chunks, sanitize_transfer_id, sha256_hex,
};

/// 推送循环空转周期（无 kick 时的兜底节拍；失败重试的天然退避）。
const PUSH_TICK_SECS: u64 = 15;

/// cluster_logs 任务目录名 `{ts}_{task_id}` 的 ts 前缀定长：
/// `%Y-%m-%d_%H-%M-%S-%3f`（23 字符）+ `_`（1）= 24。task_id 从下标 24 起。
/// （ts 格式由 cluster_request_logger_observer 决定；变更须同步这里。）
const TASK_DIR_TS_PREFIX: usize = 24;

// ---------------------------------------------------------------------------
// 传输抽象（可注入：单测用 fake，生产用 RpcClient）
// ---------------------------------------------------------------------------

/// 单次 RPC 调用抽象（worker → master 方向）。
#[async_trait::async_trait]
pub trait TransferTransport: Send + Sync {
    /// 发一次请求并等待回复（对端 error 帧 / 传输失败 = Err）。
    async fn call(
        &self,
        peer: &str,
        action: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String>;
}

/// 生产实现：既有集群 RPC 客户端（AEAD 鉴权 + 速率限制 30req/10s 自 pacing）。
pub struct RpcTransferTransport {
    rpc: Arc<crate::rpc::client::RpcClient>,
    self_node_id: String,
}

impl RpcTransferTransport {
    pub fn new(rpc: Arc<crate::rpc::client::RpcClient>, self_node_id: String) -> Self {
        Self { rpc, self_node_id }
    }
}

#[async_trait::async_trait]
impl TransferTransport for RpcTransferTransport {
    async fn call(
        &self,
        peer: &str,
        action: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let request = crate::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: crate::rpc_types::ActionType::Custom(action.to_string()),
            payload,
            // 发送方身份必须显式填：RpcClient 不代填（T37 真机教训——留空
            // 对端 `_rpc.from` 拿空串，master 无法定位回执/记账）。
            source: self.self_node_id.clone(),
            target: Some(peer.to_string()),
        };
        let resp = self
            .rpc
            .call_with_timeout(peer, request, timeout)
            .await
            .map_err(|e| format!("[Transfer] RPC {action} → {peer} 失败: {e}"))?;
        if let Some(err) = resp.error {
            return Err(format!("[Transfer] {action} 对端错误: {err}"));
        }
        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }
}

// ---------------------------------------------------------------------------
// 条目（磁盘形态：`<outbox>/<task_id>/entry.json`）
// ---------------------------------------------------------------------------

/// 通用目录推送结果（[`push_transfer_dir`]；outbox 与 P4 基线推送共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushDirOutcome {
    /// master/对端已落盘确认（或 dedup——对端已有同档）。
    Delivered,
    /// 目录为空 → 无档可传。
    NothingToSend,
    /// 超 D4 护栏（不发 overlimit 通知——调用方决定语义：outbox 标记
    /// over_limit + 出卡；基线推送 = 派发诚实停车）。
    Overlimit { total_bytes: u64, limit: u64 },
}

// ---------------------------------------------------------------------------

/// 发件箱条目。entry.json 在场 = 载荷复制完整（enqueue 先写 payload 后写
/// entry.json）；状态机 `pending → pushing → (删) | over_limit`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub task_id: String,
    /// 推送目标（派发来源节点 id；cluster_logs device 目录名同源）。
    pub source_node: String,
    /// pending | pushing | over_limit。
    pub state: String,
    pub created_at: String,
    pub attempts: u32,
    /// 入队时载荷总量（over_limit 重武装判断用）。
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// 单条推送结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushOutcome {
    /// master 已落盘确认（或 dedup——已有同档）→ 删本地。
    Delivered,
    /// 载荷为空（无执行记录）→ 无档可传，删本地。
    NothingToSend,
    /// 超 D4 护栏 → 标记 over_limit，不删不传。
    Overlimit,
}

// ---------------------------------------------------------------------------
// TransferOutbox
// ---------------------------------------------------------------------------

/// worker 侧发件箱。状态全在磁盘（entry.json），无内存镜像——重启即恢复。
pub struct TransferOutbox {
    /// `<workspace>/cluster/outbox`。
    outbox_root: PathBuf,
    /// `<workspace>/logs/cluster_logs`（入队源；nemesis-path 唯一拼接点）。
    logs_root: PathBuf,
    self_node_id: String,
    transport: Arc<dyn TransferTransport>,
    /// D4 护栏现读 provider（gateway 从 config 现读注入，热生效；0=不限）。
    max_bytes: Box<dyn Fn() -> u64 + Send + Sync>,
    kick: tokio::sync::Notify,
}

impl TransferOutbox {
    pub fn new(
        workspace: &Path,
        self_node_id: String,
        transport: Arc<dyn TransferTransport>,
        max_bytes: Box<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        let outbox_root = nemesis_path::cluster_dir_in_workspace(workspace).join("outbox");
        let _ = std::fs::create_dir_all(&outbox_root);
        Self {
            outbox_root,
            logs_root: nemesis_path::resolve_cluster_logs_dir_in_workspace(workspace),
            self_node_id,
            transport,
            max_bytes,
            kick: tokio::sync::Notify::new(),
        }
    }

    pub fn outbox_root(&self) -> &Path {
        &self.outbox_root
    }

    /// 传输通路（P4/E2：master 基线推送复用节点同一 RPC 通路，不另装配）。
    pub fn transport(&self) -> &Arc<dyn TransferTransport> {
        &self.transport
    }

    /// 即时踢推送循环（入队后调用；循环不在等也在下个 tick 兜底）。
    pub fn kick(&self) {
        self.kick.notify_one();
    }

    // -- 入队 ---------------------------------------------------------------

    /// 任务终态入队：复制 `cluster_logs/{source_node}/…_{task_id}` 到发件箱。
    /// 幂等（已有条目 = no-op）。成功后即时踢推送循环。
    pub fn enqueue(&self, task_id: &str, source_node: &str) -> Result<(), String> {
        self.enqueue_with_changeset(task_id, source_node, None)
    }

    /// 任务终态入队（P4/E3 扩展）：执行记录之外，把 `changeset_dir`（变更集，
    /// `changeset.json` + `files/`，见 [`crate::changeset`]）搭进同一载荷——
    /// 同一次分块传输原子落地，master 收到回执时变更集必已随行（保序：
    /// 合并不可能抢在交付前）。`None` = 等价 [`Self::enqueue`]（纯记录交付）。
    pub fn enqueue_with_changeset(
        &self,
        task_id: &str,
        source_node: &str,
        changeset_dir: Option<&Path>,
    ) -> Result<(), String> {
        if task_id.is_empty() || source_node.is_empty() {
            return Err("task_id/source_node 为空，无法入队".into());
        }
        // 变更集先验（破坏目录前拒绝——半套变更集不进载荷，也不丢执行记录）。
        if let Some(cs) = changeset_dir
            && !cs.join(changeset::CHANGESET_MANIFEST_NAME).exists()
        {
            return Err(format!(
                "变更集清单缺失（{}）——半套变更集不入队",
                cs.display()
            ));
        }
        let dir = self.entry_dir(task_id);
        if dir.join("entry.json").exists() {
            return Ok(()); // 幂等
        }
        let Some(src) = self.find_task_dir(source_node, task_id) else {
            return Err(format!(
                "cluster_logs 无该任务执行记录（{source_node}/…_{task_id}）"
            ));
        };
        // payload 先落，entry.json 最后写（在场 = 完整）。
        let _ = std::fs::remove_dir_all(&dir);
        let payload = dir.join("payload");
        std::fs::create_dir_all(&dir).map_err(|e| format!("建发件箱目录失败: {e}"))?;
        copy_dir_recursive(&src, &payload)?;
        if let Some(cs) = changeset_dir {
            copy_dir_recursive(cs, &payload.join(changeset::CHANGESET_DIR_NAME))?;
        }
        let files = collect_dir_files(&payload)?;
        let total: u64 = files.iter().map(|f| f.size).sum();
        let entry = OutboxEntry {
            task_id: task_id.to_string(),
            source_node: source_node.to_string(),
            state: "pending".into(),
            created_at: chrono::Local::now().to_rfc3339(),
            attempts: 0,
            total_bytes: total,
            last_error: None,
        };
        self.write_entry(&entry)?;
        tracing::info!(
            task_id = %task_id,
            source_node = %source_node,
            files = files.len(),
            total = total,
            with_changeset = changeset_dir.is_some(),
            "[Transfer] 执行记录入发件箱"
        );
        self.kick.notify_one();
        Ok(())
    }

    /// D5 兜底拉取（master → worker 的 transfer_pull 落地处）：全设备目录
    /// 找该任务执行记录，找到即入队 + 踢循环。返回 wire 状态（queued /
    /// no_archive，诚实回答）。
    pub fn request_pull(&self, task_id: &str) -> &'static str {
        if task_id.is_empty() {
            return "no_archive";
        }
        let Ok(devices) = std::fs::read_dir(&self.logs_root) else {
            return "no_archive";
        };
        for dev in devices.flatten() {
            let dev_name = dev.file_name().to_string_lossy().to_string();
            if !dev.path().is_dir() {
                continue;
            }
            let Ok(tasks) = std::fs::read_dir(dev.path()) else {
                continue;
            };
            for t in tasks.flatten() {
                let name = t.file_name().to_string_lossy().to_string();
                if t.path().is_dir() && task_dir_matches(&name, task_id) {
                    return match self.enqueue(task_id, &dev_name) {
                        Ok(()) => {
                            tracing::info!(
                                task_id = %task_id,
                                device = %dev_name,
                                "[Transfer] master 兜底拉取：已入队重推"
                            );
                            "queued"
                        }
                        Err(_) => "no_archive",
                    };
                }
            }
        }
        "no_archive"
    }

    // -- 启动清扫（崩溃兜底）------------------------------------------------

    /// worker 重启后调用一次：① `pushing`（推送中途死掉）重置回 `pending`
    /// 续推；② `over_limit` 护栏已调大则重新武装；③ 补入队「有执行残留但
    /// 无发件箱记录」的 cluster_logs 任务目录（崩溃窗口兜底——终态钩子没来
    /// 及执行的）。完成后踢一次循环。
    pub fn sweep_startup(&self) {
        let mut rearmed = 0usize;
        let mut enqueued = 0usize;
        // ①/② 条目重武装。
        for (mut entry, dir) in self.list_entries() {
            let limit = (self.max_bytes)();
            let rearm = match entry.state.as_str() {
                "pushing" => {
                    entry.last_error = Some("worker 重启中断续推".into());
                    true
                }
                "over_limit" if limit == 0 || entry.total_bytes <= limit => {
                    entry.last_error = Some("护栏调大重新武装".into());
                    true
                }
                _ => false,
            };
            if rearm {
                entry.state = "pending".into();
                if self.write_entry_at(&dir, &entry).is_ok() {
                    rearmed += 1;
                }
            }
        }
        // ③ 补入队 cluster_logs 残留。
        if let Ok(devices) = std::fs::read_dir(&self.logs_root) {
            for dev in devices.flatten() {
                let dev_name = dev.file_name().to_string_lossy().to_string();
                if !dev.path().is_dir() {
                    continue;
                }
                if dev_name == "_unknown" {
                    // 无主记录（source 未知）不可寻址——补入队只会永久重试
                    // 失败。诚实跳过（D5 反向兜底拉取仍可救：master 知道
                    // task_id 时主动来拉）。
                    continue;
                }
                let Ok(tasks) = std::fs::read_dir(dev.path()) else {
                    continue;
                };
                for t in tasks.flatten() {
                    if !t.path().is_dir() {
                        continue;
                    }
                    let name = t.file_name().to_string_lossy().to_string();
                    let Some(task_id) = derive_task_id_from_dir(&name) else {
                        continue;
                    };
                    if self.entry_dir(&task_id).join("entry.json").exists() {
                        continue;
                    }
                    match self.enqueue(&task_id, &dev_name) {
                        Ok(()) => enqueued += 1,
                        Err(e) => tracing::warn!(
                            task_id = %task_id,
                            device = %dev_name,
                            "[Transfer] 启动清扫补入队失败: {e}"
                        ),
                    }
                }
            }
        }
        if rearmed + enqueued > 0 {
            tracing::info!(
                rearmed,
                enqueued,
                "[Transfer] 发件箱启动清扫：中断续推 + 残留补入队"
            );
        }
        self.kick.notify_one();
    }

    // -- 推送循环 -----------------------------------------------------------

    /// 起后台推送循环（kick 即时 / 15s 兜底节拍；进程生命周期常驻）。
    pub fn spawn_push_loop(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = this.kick.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(PUSH_TICK_SECS)) => {}
                }
                this.process_once().await;
            }
        })
    }

    /// 一轮推送：串行处理全部 `pending` 条目（单循环任务内串行，天然免并发
    /// 竞态 + 对速率限制友好）。pub 供单测直接驱动（不起循环）。
    pub async fn process_once(&self) {
        for (mut entry, dir) in self.list_entries() {
            if entry.state != "pending" {
                continue;
            }
            entry.state = "pushing".into();
            if self.write_entry_at(&dir, &entry).is_err() {
                continue;
            }
            match self.attempt_push(&entry).await {
                Ok(PushOutcome::Delivered) | Ok(PushOutcome::NothingToSend) => {
                    // 双删：master 已落盘（或 dedup 已有）→ 删本地。
                    let _ = std::fs::remove_dir_all(&dir);
                    tracing::info!(
                        task_id = %entry.task_id,
                        "[Transfer] 执行记录回传完成，本地发件箱清空"
                    );
                }
                Ok(PushOutcome::Overlimit) => {
                    entry.state = "over_limit".into();
                    let limit = (self.max_bytes)();
                    entry.last_error =
                        Some(format!("载荷 {} 字节超护栏 {limit}", entry.total_bytes));
                    let _ = self.write_entry_at(&dir, &entry);
                    tracing::warn!(
                        task_id = %entry.task_id,
                        "[Transfer] 载荷超护栏：诚实标记 over_limit（不截断不丢弃）"
                    );
                }
                Err(e) => {
                    // 留 pending，下轮（≥1 tick 后）重试；attempts 记账观测。
                    entry.state = "pending".into();
                    entry.attempts += 1;
                    entry.last_error = Some(e.clone());
                    let _ = self.write_entry_at(&dir, &entry);
                    tracing::warn!(
                        task_id = %entry.task_id,
                        attempt = entry.attempts,
                        "[Transfer] 推送失败（保留重试）: {e}"
                    );
                }
            }
        }
    }

    /// 推送单个条目：D4 护栏（+ overlimit 通知）→ [`push_transfer_dir`]
    /// 全序列（begin dedup/have → 逐块 → end）。
    async fn attempt_push(&self, entry: &OutboxEntry) -> Result<PushOutcome, String> {
        let payload = self.entry_dir(&entry.task_id).join("payload");
        let limit = (self.max_bytes)();
        match push_transfer_dir(
            self.transport.as_ref(),
            &entry.source_node,
            &entry.task_id,
            TRANSFER_KIND_EXECUTION_RECORDS,
            &self.self_node_id,
            &payload,
            limit,
        )
        .await?
        {
            PushDirOutcome::Delivered => Ok(PushOutcome::Delivered),
            PushDirOutcome::NothingToSend => Ok(PushOutcome::NothingToSend),
            PushDirOutcome::Overlimit {
                total_bytes: total, ..
            } => {
                // D4 worker 侧护栏：通知 master 出卡（best effort——master
                // 不在场时本地 over_limit 标记仍保真）。
                let note = TransferOverlimit {
                    task_id: entry.task_id.clone(),
                    source_node: self.self_node_id.clone(),
                    total_bytes: total,
                    limit,
                    kind: TRANSFER_KIND_EXECUTION_RECORDS.into(),
                };
                let _ = self
                    .transport
                    .call(
                        &entry.source_node,
                        ACTION_TRANSFER_OVERLIMIT,
                        serde_json::to_value(&note).unwrap_or_default(),
                        Duration::from_secs(30),
                    )
                    .await;
                Ok(PushOutcome::Overlimit)
            }
        }
    }

    // -- 磁盘布局辅助 -------------------------------------------------------

    fn entry_dir(&self, task_id: &str) -> PathBuf {
        self.outbox_root.join(sanitize_transfer_id(task_id))
    }

    fn list_entries(&self) -> Vec<(OutboxEntry, PathBuf)> {
        let mut out = Vec::new();
        let Ok(dirs) = std::fs::read_dir(&self.outbox_root) else {
            return out;
        };
        for d in dirs.flatten() {
            if !d.path().is_dir() {
                continue;
            }
            match std::fs::read_to_string(d.path().join("entry.json"))
                .map_err(|e| e.to_string())
                .and_then(|raw| {
                    serde_json::from_str::<OutboxEntry>(&raw).map_err(|e| e.to_string())
                }) {
                Ok(entry) => out.push((entry, d.path())),
                Err(e) => tracing::warn!(
                    dir = %d.path().display(),
                    "[Transfer] 发件箱条目损坏（跳过）: {e}"
                ),
            }
        }
        out.sort_by(|a, b| a.0.created_at.cmp(&b.0.created_at));
        out
    }

    fn write_entry(&self, entry: &OutboxEntry) -> Result<(), String> {
        self.write_entry_at(&self.entry_dir(&entry.task_id), entry)
    }

    /// entry.json 原子写（tmp + rename；读侧以在场为完整标记）。
    fn write_entry_at(&self, dir: &Path, entry: &OutboxEntry) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败: {e}"))?;
        let json = serde_json::to_string_pretty(entry).map_err(|e| e.to_string())?;
        let tmp = dir.join(".entry.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("写 entry 失败: {e}"))?;
        std::fs::rename(&tmp, dir.join("entry.json")).map_err(|e| format!("entry 落盘失败: {e}"))
    }

    /// 定位 `cluster_logs/{source_node}/…_{task_id}` 执行记录目录（多轮
    /// 执行取字典序最大 = 最新）。
    fn find_task_dir(&self, source_node: &str, task_id: &str) -> Option<PathBuf> {
        let device = self.logs_root.join(sanitize_transfer_id(source_node));
        let mut best: Option<PathBuf> = None;
        let Ok(tasks) = std::fs::read_dir(&device) else {
            return None;
        };
        for t in tasks.flatten() {
            if !t.path().is_dir() {
                continue;
            }
            let name = t.file_name().to_string_lossy().to_string();
            if task_dir_matches(&name, task_id) {
                let take_newer = match &best {
                    Some(b) => b.to_string_lossy() < t.path().to_string_lossy(),
                    None => true,
                };
                if take_newer {
                    best = Some(t.path());
                }
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------
// 通用分块目录推送（outbox 执行记录与 P4/E2 基线推送的单一真相源）
// ---------------------------------------------------------------------------

/// `dir` 全量分块推给 `peer`：begin（dedup/have 续传/对端 over_limit 拒收）
/// → 逐文件逐块（内存上界 = chunk_size）→ end。`max_bytes`（0=不限）本地
/// 预检。对端 error 帧 / 传输失败 = Err（调用方按自身语义重试或停车）。
pub async fn push_transfer_dir(
    transport: &dyn TransferTransport,
    peer: &str,
    task_id: &str,
    kind: &str,
    source_node: &str,
    dir: &Path,
    max_bytes: u64,
) -> Result<PushDirOutcome, String> {
    let files = collect_dir_files(dir)?;
    if files.is_empty() {
        return Ok(PushDirOutcome::NothingToSend);
    }
    let total: u64 = files.iter().map(|f| f.size).sum();
    if max_bytes > 0 && total > max_bytes {
        return Ok(PushDirOutcome::Overlimit {
            total_bytes: total,
            limit: max_bytes,
        });
    }
    let hash = content_hash(&files);
    let chunk = chunk_bytes();
    // 确定性 transfer_id：同内容同块大小跨重启同 id（续传依据）；
    // 块大小变更 = 新 id = 全新 staging（防旧块跨界拼接腐坏）。
    let transfer_id = format!(
        "{}-{}-{}",
        sanitize_transfer_id(task_id),
        &hash[..hash.len().min(12)],
        chunk
    );
    let begin = TransferBegin {
        transfer_id: transfer_id.clone(),
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        source_node: source_node.to_string(),
        total_bytes: total,
        chunk_size: chunk,
        chunk_count: chunk_count(&files, chunk),
        content_hash: hash,
        files: files.clone(),
    };
    let reply_val = transport
        .call(
            peer,
            ACTION_TRANSFER_BEGIN,
            serde_json::to_value(&begin).map_err(|e| e.to_string())?,
            Duration::from_secs(60),
        )
        .await?;
    let reply: TransferBeginReply =
        serde_json::from_value(reply_val).map_err(|e| format!("begin 回复解析失败: {e}"))?;
    match reply.status.as_str() {
        "dedup" => return Ok(PushDirOutcome::Delivered), // 对端已有同档
        "over_limit" => {
            return Ok(PushDirOutcome::Overlimit {
                total_bytes: total,
                limit: 0, // 对端未回传护栏值——只标记事实
            });
        }
        "ok" => {}
        other => return Err(format!("begin 异常状态 {other}: {:?}", reply.error)),
    }
    // 逐文件开句柄、逐块 seek 读（内存上界 = chunk_size，不整包进内存）。
    // have 里的块跳过（断点续传）。
    let plan = plan_chunks(&begin.files, begin.chunk_size);
    let chunk_timeout = Duration::from_secs(120);
    let end_timeout = Duration::from_secs(600); // 对端组装大档需要时间
    for (file_idx, file) in begin.files.iter().enumerate() {
        let seqs: Vec<usize> = (0..plan.len()).filter(|&s| plan[s].0 == file_idx).collect();
        if seqs.iter().all(|s| reply.have.contains(s)) {
            continue;
        }
        let path = dir.join(&file.path);
        let mut handle =
            std::fs::File::open(&path).map_err(|e| format!("打开 {}: {e}", file.path))?;
        use std::io::{Read, Seek, SeekFrom};
        for &seq in &seqs {
            if reply.have.contains(&seq) {
                continue;
            }
            let (_, off, len) = plan[seq];
            let mut buf = vec![0u8; len];
            handle
                .seek(SeekFrom::Start(off))
                .map_err(|e| format!("seek {}: {e}", file.path))?;
            handle
                .read_exact(&mut buf)
                .map_err(|e| format!("读 {}: {e}", file.path))?;
            let req = TransferChunk {
                transfer_id: transfer_id.clone(),
                seq,
                total: plan.len(),
                data_b64: b64_encode(&buf),
                sha256: sha256_hex(&buf),
            };
            transport
                .call(
                    peer,
                    ACTION_TRANSFER_CHUNK,
                    serde_json::to_value(&req).map_err(|e| e.to_string())?,
                    chunk_timeout,
                )
                .await?;
        }
    }
    let end_val = transport
        .call(
            peer,
            ACTION_TRANSFER_END,
            json!({ "transfer_id": transfer_id }),
            end_timeout,
        )
        .await?;
    let end: TransferEndReply =
        serde_json::from_value(end_val).map_err(|e| format!("end 回复解析失败: {e}"))?;
    if end.status != "ok" {
        return Err(format!("end 未确认: {:?}", end.error));
    }
    Ok(PushDirOutcome::Delivered)
}

// ---------------------------------------------------------------------------
// 纯函数辅助
// ---------------------------------------------------------------------------

/// 任务目录名是否属于该任务（后缀 `_` + 原样 / 安全化两种形态都认——
/// RequestLogger 与本 crate 的 sanitize 实现不同源，真实 task id（uuid）
/// 下两者同为恒等）。
pub(crate) fn task_dir_matches(dir_name: &str, task_id: &str) -> bool {
    let s = sanitize_transfer_id(task_id);
    dir_name.ends_with(&format!("_{task_id}")) || dir_name.ends_with(&format!("_{s}"))
}

/// 目录名 → task_id（剥 24 字符 ts 前缀；形态不符 = None，宁缺毋错键）。
fn derive_task_id_from_dir(dir_name: &str) -> Option<String> {
    let b = dir_name.as_bytes();
    if dir_name.len() > TASK_DIR_TS_PREFIX && b[TASK_DIR_TS_PREFIX - 1] == b'_' {
        Some(dir_name[TASK_DIR_TS_PREFIX..].to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// 传输 handler 工厂（节点侧接收入口——master 收 begin/chunk/end/overlimit，
// worker 收 pull；全员同注册，按角色自然分流）
// ---------------------------------------------------------------------------

/// 在 Cluster 上注册分块传输通道的全部 5 个 handler。要求 cluster 已运行
/// （与 register_task_recovery_handlers 同款约束，失败由调用方诚实记 warn）。
pub fn register_transfer_handlers(
    cluster: &crate::cluster::Cluster,
    sink: Arc<crate::transfer::TransferSink>,
    outbox: Option<Arc<TransferOutbox>>,
) -> Result<(), String> {
    let sink_begin = sink.clone();
    cluster.register_rpc_handler(
        crate::transfer::ACTION_TRANSFER_BEGIN,
        Box::new(move |payload| {
            let req: TransferBegin = serde_json::from_value(payload)
                .map_err(|e| format!("transfer_begin 载荷解析失败: {e}"))?;
            let reply = sink_begin.begin(&req);
            serde_json::to_value(&reply).map_err(|e| e.to_string())
        }),
    )?;
    let sink_chunk = sink.clone();
    cluster.register_rpc_handler(
        crate::transfer::ACTION_TRANSFER_CHUNK,
        Box::new(move |payload| {
            let req: TransferChunk = serde_json::from_value(payload)
                .map_err(|e| format!("transfer_chunk 载荷解析失败: {e}"))?;
            sink_chunk.chunk(&req)?;
            Ok(json!({ "status": "ok" }))
        }),
    )?;
    let sink_end = sink.clone();
    cluster.register_rpc_handler(
        crate::transfer::ACTION_TRANSFER_END,
        Box::new(move |payload| {
            let transfer_id = payload
                .get("transfer_id")
                .and_then(|v| v.as_str())
                .ok_or("transfer_end 缺 transfer_id")?;
            let reply = sink_end.end(transfer_id)?;
            serde_json::to_value(&reply).map_err(|e| e.to_string())
        }),
    )?;
    // transfer_overlimit（worker 本地护栏拦下时的通知；master 出卡走
    // TransferSink::set_on_overlimit 接线的回调）。
    let sink_ol = sink;
    cluster.register_rpc_handler(
        crate::transfer::ACTION_TRANSFER_OVERLIMIT,
        Box::new(move |payload| {
            let req: TransferOverlimit = serde_json::from_value(payload)
                .map_err(|e| format!("transfer_overlimit 载荷解析失败: {e}"))?;
            sink_ol.note_overlimit(&req);
            Ok(json!({ "status": "noted" }))
        }),
    )?;
    // transfer_pull（master 兜底拉取；worker 侧落地为入队重推）。
    if let Some(ob) = outbox {
        cluster.register_rpc_handler(
            crate::transfer::ACTION_TRANSFER_PULL,
            Box::new(move |payload| {
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .ok_or("transfer_pull 缺 task_id")?;
                let status = ob.request_pull(task_id);
                Ok(json!({ "status": status }))
            }),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
