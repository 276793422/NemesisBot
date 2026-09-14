//! 双向分块传输通道（看板项目档案 goal P3/D1+D3+D6+D7）。
//!
//! 既有集群 RPC 之上的可靠文件传输：worker→master（执行记录回传；P4 起
//! 变更集同路）、master→worker（P4 基线下发复用同一机制）。设计裁决：
//!
//! - **分块 + 逐块确认**：每个 chunk 是一次独立 RPC request/response
//!   （chunk ACK = master 已把该块**先落盘**），禁止一次性大包/base64
//!   整包（1.33x 膨胀 + 内存尖峰）。chunk payload 本身 base64 进 JSON
//!   ——逐块有界（默认 1 MiB，帧上限 16 MB 富余），与「整包」禁令不冲突。
//! - **幂等去重（D3）**：`(task_id, content_hash)` 去重，at-least-once
//!   语义下重复投递无害；去重索引持久化（master 重启不失效）。
//! - **断点续传**：begin 回复 `have`（staging 已收块序号），sender 跳过
//!   已收块——传输中断/进程重启后重推只补缺块。staging 布局确定性
//!   （manifest 决定 seq→(file,offset) 映射），双方无需协商额外状态。
//! - **端到端完整性（D6）**：manifest.json 携带文件清单 + SHA-256；
//!   end 组装后逐文件核验，缺失/校验败 = 诚实报错（不 ACK，sender 保留
//!   本地重试）。
//! - **体积护栏（D4）**：`max_bytes` 超限 = begin 直接诚实拒绝
//!   （`over_limit`），绝不截断。
//! - **路径围栏（D7）**：manifest 声明的相对路径经 [`safe_relative_path`]
//!   单点校验——拒 `..`/绝对路径/盘符/反斜杠/8.3 短名。
//!
//! 本模块只做**传输 + 中立落盘**（`<workspace>/cluster/inbox/<task_id>/`），
//! 不依赖 board；档案投影（records/execution 挂载 + project.json 缺失标记
//! + 决策流卡片）由宿主（gateway）经 [`TransferSink::set_on_landed`] /
//!   [`TransferSink::set_on_overlimit`] 回调接线。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// 协议常量与 action 名（ActionType::Custom 扩展位，query_task_result 先例）
// ---------------------------------------------------------------------------

pub const ACTION_TRANSFER_BEGIN: &str = "transfer_begin";
pub const ACTION_TRANSFER_CHUNK: &str = "transfer_chunk";
pub const ACTION_TRANSFER_END: &str = "transfer_end";
pub const ACTION_TRANSFER_PULL: &str = "transfer_pull";
pub const ACTION_TRANSFER_OVERLIMIT: &str = "transfer_overlimit";

/// 传输类别（`kind`；P3 只用 execution_records，P4 基线/变更集复用通道）。
pub const TRANSFER_KIND_EXECUTION_RECORDS: &str = "execution_records";

/// P4/E2：项目档案基线快照（master 派发时 HEAD 树同步推送 worker）。
pub const TRANSFER_KIND_PROJECT_BASELINE: &str = "project_baseline";

/// 默认 chunk 大小：1 MiB（base64 后 ~1.4 MB JSON，16 MB 帧上限富余；
/// 速率限制 30 req/10s 下有效吞吐 ~3 MiB/s，2 GiB 极限载荷 ~11 分钟——
/// 可接受，正常执行记录是 KB~MB 级）。
pub const DEFAULT_CHUNK_BYTES: usize = 1024 * 1024;

/// chunk 大小环境变量覆盖（UAT「调小块大小」用，免重编；钳到
/// 4 KiB..8 MiB——太小拖死速率限制，太大逼近帧上限）。
const CHUNK_ENV: &str = "NEMESISBOT_TRANSFER_CHUNK_BYTES";

/// chunk 大小：env 覆盖 > 默认。每次调用现读（UAT 进程启动前置即可生效）。
pub fn chunk_bytes() -> usize {
    resolve_chunk_bytes(
        std::env::var(CHUNK_ENV)
            .ok()
            .and_then(|v| v.parse::<usize>().ok()),
    )
}

/// env 原始值 → 生效块大小（纯函数；越界回落默认并 WARN）。
pub fn resolve_chunk_bytes(raw: Option<usize>) -> usize {
    match raw {
        Some(n) if (4096..=8 * 1024 * 1024).contains(&n) => n,
        Some(bad) => {
            tracing::warn!(
                "[Transfer] {CHUNK_ENV}={bad} 越界（4KiB..8MiB），用默认 {DEFAULT_CHUNK_BYTES}"
            );
            DEFAULT_CHUNK_BYTES
        }
        None => DEFAULT_CHUNK_BYTES,
    }
}

/// 8.3 短名组件判定（与 nemesis-board anchor.rs `has_83_short_name`
/// 同语义：`NAME~<数字>` 后跟组件结尾或 `.`；代码独立——nemesis-cluster
/// 不依赖 nemesis-board，语义对齐防漂移）。
fn is_short_name(comp: &str) -> bool {
    let bytes = comp.as_bytes();
    for i in 1..bytes.len() {
        if bytes[i] == b'~' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let after = &bytes[i + 2..];
            let digits = after.iter().take_while(|b| b.is_ascii_digit()).count();
            if digits == after.len() || after[digits] == b'.' {
                return true;
            }
        }
    }
    false
}

/// D7 路径围栏：manifest 声明的相对路径单点校验（语义复用 anchor
/// `resolve_anchor_path`，代码独立——nemesis-cluster 不依赖 nemesis-board）。
/// 拒绝：空串、绝对路径、盘符、反斜杠、`..`/`.` 组件、8.3 短名组件、
/// 非 UTF-8。返回以 `/` 为分隔的相对 PathBuf（落地时 join 到接收目录）。
pub fn safe_relative_path(rel: &str) -> Result<PathBuf, String> {
    let t = rel.trim();
    if t.is_empty() {
        return Err("空路径".into());
    }
    if t.contains('\\') {
        return Err(format!("拒绝反斜杠路径（统一 '/' 分隔）：{t}"));
    }
    if t.contains(':') {
        return Err(format!("拒绝含盘符/冒号组件：{t}"));
    }
    let p = Path::new(t);
    if p.is_absolute() {
        return Err(format!("拒绝绝对路径：{t}"));
    }
    for comp in p.components() {
        match comp {
            std::path::Component::Normal(c) => {
                let s = c
                    .to_str()
                    .ok_or_else(|| format!("非 UTF-8 路径组件：{t}"))?;
                if s == ".." || s == "." {
                    return Err(format!("拒绝特殊路径组件：{t}"));
                }
                if is_short_name(s) {
                    return Err(format!("拒绝 8.3 短名组件：{t}"));
                }
            }
            _ => return Err(format!("拒绝非普通路径组件：{t}")),
        }
    }
    Ok(PathBuf::from(t))
}

/// SHA-256 hex（小写）。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// wire 类型
// ---------------------------------------------------------------------------

/// manifest 文件条目（路径统一 `/` 分隔相对路径）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferFileEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// transfer_begin 载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferBegin {
    pub transfer_id: String,
    pub task_id: String,
    /// [`TRANSFER_KIND_EXECUTION_RECORDS`] 等。
    pub kind: String,
    /// 发送方节点 id（审计/回执用）。
    pub source_node: String,
    pub total_bytes: u64,
    pub chunk_size: usize,
    pub chunk_count: usize,
    pub files: Vec<TransferFileEntry>,
    /// 全量内容指纹（排序清单哈希；去重键的一半）。
    pub content_hash: String,
}

/// transfer_begin 回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferBeginReply {
    /// ok = 可推块；dedup = master 已有同 (task_id, content_hash) 档案，
    /// sender 可删本地；over_limit = 超护栏诚实拒绝。
    pub status: String,
    /// 断点续传：master staging 已收到的块序号（sender 跳过）。
    #[serde(default)]
    pub have: Vec<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// transfer_chunk 载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferChunk {
    pub transfer_id: String,
    /// 全局块序号（manifest 确定性计划的下标）。
    pub seq: usize,
    pub total: usize,
    /// 原始块字节 base64。
    pub data_b64: String,
    /// 原始块字节 SHA-256（逐块完整性）。
    pub sha256: String,
}

/// transfer_end 载荷 / 回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEndReply {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

/// transfer_pull 载荷（master→worker，D5 兜底拉取）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferPull {
    pub task_id: String,
}

/// transfer_pull 回复。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferPullReply {
    /// queued = worker 已（重新）入队并触发推送；no_archive = worker 本地
    /// 无该任务执行记录（诚实回答，master 侧记缺失）。
    pub status: String,
}

/// transfer_overlimit 载荷（worker→master 通知：本地护栏拦下，档案未传）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferOverlimit {
    pub task_id: String,
    pub source_node: String,
    pub total_bytes: u64,
    pub limit: u64,
    pub kind: String,
}

// ---------------------------------------------------------------------------
// sender 侧：目录收集 + 确定性分块计划
// ---------------------------------------------------------------------------

/// 递归收集目录内全部文件（相对 `/` 路径按字典序——确定性，保证同内容
/// 两次收集 content_hash 一致；符号链接跳过）。目录不存在 = Ok(空)。
pub fn collect_dir_files(root: &Path) -> Result<Vec<TransferFileEntry>, String> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    walk(root, root, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<TransferFileEntry>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("读目录 {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let Some(rel_str) = rel.to_str() else {
            return Err(format!("非 UTF-8 路径：{}", path.display()));
        };
        // 围栏自检：收集器自身产物也必须过围栏（畸形路径宁可不传）。
        let normalized = rel_str.replace('\\', "/");
        if safe_relative_path(&normalized).is_err() {
            continue;
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            walk(root, &path, out)?;
        } else if meta.is_file() {
            let data =
                std::fs::read(&path).map_err(|e| format!("读文件 {}: {e}", path.display()))?;
            out.push(TransferFileEntry {
                path: normalized,
                size: data.len() as u64,
                sha256: sha256_hex(&data),
            });
        }
    }
    Ok(())
}

/// 内容指纹：排序清单的 SHA-256（`path:size:sha256\n` 逐行拼接）。
/// 同内容必同指纹（去重键），不同内容几乎必不同。
pub fn content_hash(files: &[TransferFileEntry]) -> String {
    let mut buf = String::new();
    for f in files {
        buf.push_str(&f.path);
        buf.push(':');
        buf.push_str(&f.size.to_string());
        buf.push(':');
        buf.push_str(&f.sha256);
        buf.push('\n');
    }
    sha256_hex(buf.as_bytes())
}

/// 确定性分块计划：manifest（文件顺序 + chunk_size）唯一决定
/// seq → (文件下标, 文件内偏移, 长度)。收发双方各自推导，零额外协商。
/// 空文件（size=0）不产块，end 组装时直接创建空文件。
pub fn plan_chunks(files: &[TransferFileEntry], chunk_size: usize) -> Vec<(usize, u64, usize)> {
    assert!(chunk_size > 0, "chunk_size must be > 0");
    let mut plan = Vec::new();
    for (idx, f) in files.iter().enumerate() {
        let mut off = 0u64;
        while off < f.size {
            let len = ((f.size - off) as usize).min(chunk_size);
            plan.push((idx, off, len));
            off += len as u64;
        }
    }
    plan
}

/// 总块数（plan 长度，供 begin.chunk_count）。
pub fn chunk_count(files: &[TransferFileEntry], chunk_size: usize) -> usize {
    plan_chunks(files, chunk_size).len()
}

/// base64 编解码薄封装（统一 engine，收发两侧不漂移）。
pub fn b64_encode(data: &[u8]) -> String {
    B64.encode(data)
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s).map_err(|e| format!("base64 解码失败: {e}"))
}

// ---------------------------------------------------------------------------
// 接收侧状态机：TransferSink（先落盘后 ACK + 去重 + 核验 + 落地）
// ---------------------------------------------------------------------------

/// 去重索引条目（磁盘形态：`<inbox>/.dedup.json` 的 map 值）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupEntry {
    pub content_hash: String,
    pub landed_at: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

/// 落地回调（宿主接线：档案投影/决策流卡片）。`(task_id, landed_dir)`。
pub type LandedCallback = Arc<dyn Fn(&str, &Path) + Send + Sync>;
/// 超限通知回调（worker→master 通知到卡）。`(payload)`。
pub type OverlimitCallback = Arc<dyn Fn(&TransferOverlimit) + Send + Sync>;

/// master/接收侧传输汇聚点。所有节点都可注册（任一节点都可能是 A 端
/// dispatcher）；board 档案投影回调只由 master 接线。
pub struct TransferSink {
    /// 中立收件箱根：`<workspace>/cluster/inbox`。
    inbox_root: PathBuf,
    /// staging 根：`<inbox>/.staging/<transfer_id>/`。
    staging_root: PathBuf,
    /// 去重索引（内存镜像 + `.dedup.json` 持久化）：task_id → 条目。
    dedup: RwLock<HashMap<String, DedupEntry>>,
    /// D4 体积护栏（字节；0 = 不限）。AtomicU64：gateway 周期从 config
    /// 现读刷新（热生效）。
    max_bytes: AtomicU64,
    on_landed: Mutex<Option<LandedCallback>>,
    on_overlimit: Mutex<Option<OverlimitCallback>>,
}

impl TransferSink {
    /// 创建 sink（构造期同步建目录 + 加载持久化去重索引）。
    pub fn new(workspace: &Path, max_bytes: u64) -> Self {
        let inbox_root = nemesis_path::cluster_dir_in_workspace(workspace).join("inbox");
        let staging_root = inbox_root.join(".staging");
        let _ = std::fs::create_dir_all(&staging_root);
        let dedup = load_dedup(&inbox_root);
        Self {
            inbox_root,
            staging_root,
            dedup: RwLock::new(dedup),
            max_bytes: AtomicU64::new(max_bytes),
            on_landed: Mutex::new(None),
            on_overlimit: Mutex::new(None),
        }
    }

    /// D4 护栏现值刷新（gateway sweep 周期调用；0 = 不限）。
    pub fn set_max_bytes(&self, max_bytes: u64) {
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// 接线落地回调（档案投影）。只在装配期调用一次。
    pub fn set_on_landed(&self, cb: LandedCallback) {
        *self.on_landed.lock().unwrap() = Some(cb);
    }

    /// 接线超限通知回调（决策流卡片）。
    pub fn set_on_overlimit(&self, cb: OverlimitCallback) {
        *self.on_overlimit.lock().unwrap() = Some(cb);
    }

    /// 去重快照（测试/诊断用）。
    pub fn dedup_entry(&self, task_id: &str) -> Option<DedupEntry> {
        self.dedup.read().get(task_id).cloned()
    }

    /// begin：校验 → 去重/超限/续传三分支。
    pub fn begin(&self, req: &TransferBegin) -> TransferBeginReply {
        // 1. manifest 结构校验（路径围栏 + 总量一致 + 计划一致）。
        if let Err(e) = validate_manifest(req) {
            return TransferBeginReply {
                status: "error".into(),
                have: vec![],
                error: Some(e),
            };
        }
        // 2. D4 护栏（接收侧诚实拒绝；0 = 不限）。
        let limit = self.max_bytes();
        if limit > 0 && req.total_bytes > limit {
            return TransferBeginReply {
                status: "over_limit".into(),
                have: vec![],
                error: Some(format!(
                    "传输体积 {} 字节超护栏 {limit}（诚实拒绝，不截断）",
                    req.total_bytes
                )),
            };
        }
        // 3. D3 去重：(task_id, content_hash) 已落地 **且档案实体仍在收件箱**
        //    → 免传。ingest 会把 inbox/<task_id> 移走安置进项目档案——此时
        //    索引虽在、实体已不在 master，必须放行重传：否则 D5 兜底拉取
        //    被 dedup 挡死（worker 推 → dedup → 删发件箱 → sweep 永远补不回
        //    档案，无限空转）。
        if let Some(entry) = self.dedup.read().get(&req.task_id)
            && entry.content_hash == req.content_hash
            && self
                .inbox_root
                .join(&req.task_id)
                .join("landed.json")
                .exists()
        {
            // 去重命中可观测（UAT/运维 grep 定位；信息量 = 免传了多少块）。
            tracing::info!(
                task_id = %req.task_id,
                transfer_id = %req.transfer_id,
                "[Transfer] dedup：同 (task_id, content_hash) 档案已在收件箱，免传"
            );
            return TransferBeginReply {
                status: "dedup".into(),
                have: vec![],
                error: None,
            };
        }
        // 4. staging：同 transfer_id 且同 content_hash = 断点续传（收集已收块）；
        //    否则新建（旧 staging 属不同传输，清掉）。
        let st = self.staging_dir(&req.transfer_id);
        let resume = read_staging_manifest(&st)
            .map(|m| m.content_hash == req.content_hash)
            .unwrap_or(false);
        if resume {
            write_staging_manifest(&st, req);
            let have = list_received_chunks(&st, req.chunk_count);
            return TransferBeginReply {
                status: "ok".into(),
                have,
                error: None,
            };
        }
        if let Err(e) = std::fs::remove_dir_all(&st)
            && st.exists()
        {
            return TransferBeginReply {
                status: "error".into(),
                have: vec![],
                error: Some(format!("清理旧 staging 失败: {e}")),
            };
        }
        if let Err(e) = std::fs::create_dir_all(&st) {
            return TransferBeginReply {
                status: "error".into(),
                have: vec![],
                error: Some(format!("创建 staging 失败: {e}")),
            };
        }
        // begin manifest 落盘（断电/重启后续传依据 + 审计）。
        write_staging_manifest(&st, req);
        TransferBeginReply {
            status: "ok".into(),
            have: vec![],
            error: None,
        }
    }

    /// chunk：**先落盘后 ACK**——staging 块文件写成功才回 Ok；写失败回
    /// Err（RPC error 帧），sender 重试同块。同 seq 重写幂等（覆盖）。
    pub fn chunk(&self, req: &TransferChunk) -> Result<(), String> {
        let data = b64_decode(&req.data_b64)?;
        if sha256_hex(&data) != req.sha256 {
            return Err("chunk sha256 不匹配".into());
        }
        let st = self.staging_dir(&req.transfer_id);
        if !st.exists() {
            return Err("staging 不存在（begin 未到达或已被清理）".into());
        }
        // 块文件名固定宽度：字典序 = 数值序。
        let path = st.join(format!("chunk_{:06}.bin", req.seq));
        std::fs::write(&path, &data).map_err(|e| format!("写 staging 块失败: {e}"))?;
        Ok(())
    }

    /// end：组装 staging → 逐文件 SHA-256 核验（D6）→ 落地 inbox → 更新
    /// 去重索引 → 清 staging → 触发落地回调。任何一步失败 = 诚实 Err
    /// （不 ACK，sender 保留本地重试；staging 保留供诊断/续传）。
    pub fn end(&self, transfer_id: &str) -> Result<TransferEndReply, String> {
        let st = self.staging_dir(transfer_id);
        if !st.exists() {
            // 幂等重放：staging 已清 = 该传输此前已成功落地（end ACK 丢失后
            // sender 重试 end）。`.done/` 标记保存了当时的成功回复——重放之，
            // 不然 sender 拿到 Err 会永远重试（outbox 零丢失语义的死锁）。
            let done = self
                .inbox_root
                .join(".done")
                .join(format!("{}.json", sanitize_transfer_id(transfer_id)));
            if let Ok(raw) = std::fs::read_to_string(&done)
                && let Ok(reply) = serde_json::from_str::<TransferEndReply>(&raw)
            {
                return Ok(reply);
            }
            return Err(format!("staging 不存在（transfer {transfer_id}）"));
        }
        let begin = read_staging_manifest(&st)
            .ok_or_else(|| format!("staging manifest 缺失（transfer {transfer_id}）"))?;

        // 1. 组装 + 核验到临时 out/ 目录（载荷放 files/ 子层，manifest/回执
        //    在落地根——不污染 inbox，核验过了才落地）。
        let out = st.join("out");
        let files_root = out.join("files");
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&files_root).map_err(|e| format!("创建 out 失败: {e}"))?;
        let plan = plan_chunks(&begin.files, begin.chunk_size);
        // 空文件（size=0）不产块，直接创建空文件（plan_chunks 契约）。
        for file in &begin.files {
            if file.size == 0 {
                let rel = safe_relative_path(&file.path)?;
                let dest = files_root.join(&rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("创建目录 {}: {e}", parent.display()))?;
                }
                std::fs::write(&dest, b"").map_err(|e| format!("创建空文件 {}: {e}", file.path))?;
            }
        }
        for (seq, (file_idx, offset, len)) in plan.iter().enumerate() {
            let chunk_path = st.join(format!("chunk_{seq:06}.bin"));
            let data = std::fs::read(&chunk_path)
                .map_err(|e| format!("staging 块 {seq} 缺失或不可读: {e}"))?;
            if data.len() != *len {
                return Err(format!("块 {seq} 长度 {} ≠ 计划 {len}", data.len()));
            }
            let file = &begin.files[*file_idx];
            let rel = safe_relative_path(&file.path)?;
            let dest = files_root.join(&rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目录 {}: {e}", parent.display()))?;
            }
            // 定长偏移写入（乱序块也能拼对——按计划位置落；truncate(false)
            // 显式声明保留已有字节，先到的后位块不被先到的前位块清掉）。
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&dest)
                .map_err(|e| format!("打开 {}: {e}", dest.display()))?;
            use std::io::{Seek, SeekFrom, Write};
            f.seek(SeekFrom::Start(*offset))
                .map_err(|e| format!("seek {}: {e}", dest.display()))?;
            f.write_all(&data)
                .map_err(|e| format!("写 {}: {e}", dest.display()))?;
        }
        // 2. 逐文件 SHA-256 核验（D6 端到端完整性）。
        for file in &begin.files {
            let rel = safe_relative_path(&file.path)?;
            let data = std::fs::read(files_root.join(&rel))
                .map_err(|e| format!("组装后读 {} 失败: {e}", file.path))?;
            if data.len() as u64 != file.size {
                return Err(format!(
                    "文件 {} 长度 {} ≠ manifest {}",
                    file.path,
                    data.len(),
                    file.size
                ));
            }
            let got = sha256_hex(&data);
            if got != file.sha256 {
                return Err(format!(
                    "文件 {} SHA-256 核验失败（期望 {} 实得 {got}）",
                    file.path, file.sha256
                ));
            }
        }
        // 3. 落地：inbox/<task_id>/（已存在则整体替换——同 task_id 新内容
        //    以最新为准）。含 files/ + manifest.json + landed.json 回执。
        let landed = self.inbox_root.join(&begin.task_id);
        if landed.exists()
            && let Err(e) = std::fs::remove_dir_all(&landed)
        {
            return Err(format!("清理旧收件目录失败: {e}"));
        }
        std::fs::create_dir_all(self.inbox_root.parent().unwrap_or(&landed))
            .map_err(|e| format!("创建 inbox 父目录失败: {e}"))?;
        if let Err(e) = std::fs::rename(&out, &landed) {
            // rename 失败（跨卷等）退化为 copy。
            if let Err(e2) = copy_dir_recursive(&out, &landed) {
                return Err(format!("落地失败: rename {e} / copy {e2}"));
            }
            let _ = std::fs::remove_dir_all(&out);
        }
        // 落地内容镜像 manifest + 回执（审计：谁、何时、何指纹）。
        write_staging_manifest(&landed, &begin);
        let receipt = serde_json::json!({
            "transfer_id": transfer_id,
            "task_id": begin.task_id,
            "kind": begin.kind,
            "source_node": begin.source_node,
            "content_hash": begin.content_hash,
            "file_count": begin.files.len(),
            "total_bytes": begin.total_bytes,
            "landed_at": chrono::Local::now().to_rfc3339(),
        });
        std::fs::write(
            landed.join("landed.json"),
            serde_json::to_string_pretty(&receipt).unwrap_or_default(),
        )
        .map_err(|e| format!("写 landed.json 失败: {e}"))?;
        // 4. 去重索引更新 + 持久化。
        self.dedup.write().insert(
            begin.task_id.clone(),
            DedupEntry {
                content_hash: begin.content_hash.clone(),
                landed_at: chrono::Local::now().to_rfc3339(),
                file_count: begin.files.len(),
                total_bytes: begin.total_bytes,
            },
        );
        save_dedup(&self.inbox_root, &self.dedup.read());
        // 5. 清 staging + 写幂等重放标记（end 重试防死锁，见函数头）。
        let done_reply = TransferEndReply {
            status: "ok".into(),
            error: None,
            file_count: Some(begin.files.len()),
            total_bytes: Some(begin.total_bytes),
        };
        let _ = std::fs::remove_dir_all(&st);
        let done_dir = self.inbox_root.join(".done");
        let _ = std::fs::create_dir_all(&done_dir);
        if let Ok(json) = serde_json::to_string(&done_reply) {
            let _ = std::fs::write(
                done_dir.join(format!("{}.json", sanitize_transfer_id(transfer_id))),
                json,
            );
        }
        // 6. 落地回调（档案投影接线；失败不影响 ACK——数据已安全落地）。
        if let Some(cb) = self.on_landed.lock().unwrap().as_ref() {
            cb(&begin.task_id, &landed);
        }
        Ok(done_reply)
    }

    /// worker→master 超限通知入口（D4 诚实失败路径的 master 侧可见性）。
    pub fn note_overlimit(&self, req: &TransferOverlimit) {
        tracing::warn!(
            task_id = %req.task_id,
            total = req.total_bytes,
            limit = req.limit,
            "[Transfer] worker 报告传输超限（档案未回传）"
        );
        if let Some(cb) = self.on_overlimit.lock().unwrap().as_ref() {
            cb(req);
        }
    }

    fn staging_dir(&self, transfer_id: &str) -> PathBuf {
        self.staging_root.join(sanitize_transfer_id(transfer_id))
    }

    /// inbox 根（D5 sweep 复用：master 检查本地已收档案）。
    pub fn inbox_root(&self) -> &Path {
        &self.inbox_root
    }
}

/// transfer_id 安全化（目录名成分；只留文件名字符，其余折叠 `_`）。
/// outbox 条目目录名/发件箱 transfer_id 同源复用（身份不漂移）。
pub fn sanitize_transfer_id(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "transfer".into()
    } else {
        out
    }
}

/// begin 载荷校验：全部路径过围栏 + 总量/块计划一致。
fn validate_manifest(req: &TransferBegin) -> Result<(), String> {
    if req.transfer_id.is_empty() || req.task_id.is_empty() {
        return Err("transfer_id/task_id 不能为空".into());
    }
    if req.chunk_size == 0 {
        return Err("chunk_size 必须为正".into());
    }
    let mut total = 0u64;
    for f in &req.files {
        safe_relative_path(&f.path)?;
        total += f.size;
    }
    if total != req.total_bytes {
        return Err(format!("manifest 总量 {total} ≠ 声明 {}", req.total_bytes));
    }
    let expect = chunk_count(&req.files, req.chunk_size);
    if expect != req.chunk_count {
        return Err(format!("块数 {expect} ≠ 声明 {}", req.chunk_count));
    }
    if req.content_hash.is_empty() {
        return Err("content_hash 不能为空".into());
    }
    Ok(())
}

fn staging_manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

fn write_staging_manifest(dir: &Path, begin: &TransferBegin) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_string_pretty(begin) {
        let _ = std::fs::write(staging_manifest_path(dir), json);
    }
}

fn read_staging_manifest(dir: &Path) -> Option<TransferBegin> {
    let raw = std::fs::read_to_string(staging_manifest_path(dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 列出 staging 已收到的合法块序号（0..chunk_count 内）。
fn list_received_chunks(st: &Path, chunk_count: usize) -> Vec<usize> {
    let mut have = Vec::new();
    for seq in 0..chunk_count {
        if st.join(format!("chunk_{seq:06}.bin")).exists() {
            have.push(seq);
        }
    }
    have
}

/// 递归复制目录（rename 跨卷退化路径；outbox 入队复制同源复用）。
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("readdir {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("readdir entry: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// 去重索引持久化（`.dedup.json`，tmp+rename 原子）。
fn save_dedup(inbox_root: &Path, dedup: &HashMap<String, DedupEntry>) {
    let path = inbox_root.join(".dedup.json");
    let Ok(json) = serde_json::to_string_pretty(dedup) else {
        return;
    };
    let tmp = inbox_root.join(".dedup.json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

fn load_dedup(inbox_root: &Path) -> HashMap<String, DedupEntry> {
    let path = inbox_root.join(".dedup.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
