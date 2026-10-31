//! B9（VM 项目工作副本）gateway 同步引擎。
//!
//! 职责（实施计划 §4.2 ③）：把项目目录的工作树与 VM 内 `/workspace`
//! 对齐——开工全量打包进 VM（`bulk_write` 分块 tar）+ 基线账本落盘
//! （`<workspace>/logs/vm_sync/<project_id>/baseline.json`，内容 =
//! guest 侧 `hash_tree` 读回的真实指纹，不是宿主侧自说自话）。
//!
//! 硬设计条件对照：
//! - ⑦ **路径翻译单点**：全模块的相对路径一律 `/` 分隔（tar 条目/guest
//!   协议/基线账本），宿主侧 join 归一交给 `Path::join`——VM 侧恒
//!   `/workspace` 相对，无第二处翻译点；
//! - 排除规则单点：[`SyncExclusion`]——git 项目用 `.gitignore`（实作 =
//!   `git ls-files --cached --others --exclude-standard`，git 缺席/失败时
//!   诚实回落 walk+默认排除表并留痕），非 git 用默认排除表；
//!   [`SyncExclusion::admits`] 在两种来源的选择结果上**再过滤一道**
//!   （构建产物类目录即便被 git 跟踪也不进工作副本）；
//! - ⑤ **每轮成本 O(变更量)**：开工闸（[`open_work_gate`]）宿主侧变更
//!   检测 = git 项目 `git status --porcelain`（亚秒）/ 非 git 项目
//!   mtime+size 元数据快检（不读内容）；VM 侧嫌疑文件指纹 =
//!   `hash_tree` 定点 paths 查询（B9-2 起）；全量传输只发生在首次开工
//!   （[`full_sync`]）与手动强制重扫（B9-3 `vm.resync`）。
//!   commit 闸（[`commit_project`]）每轮 turn 末做一次 VM 树全指纹——
//!   这是刻意以正确性换优化：dir-mtime 粗筛会漏「原文件原地改写」（目录
//!   mtime 不动），commit 漏记 = 基线账本失真 = 后续误报冲突；VM 树受
//!   排除表约束（target/node_modules 不进工作副本），规模可控。
//!
//! 诚实边界：首次同步分钟级（大项目一次性成本）；symlink/硬链接不进
//! 工作副本（计数上报）；文件数超 [`MAX_SYNC_FILES`] = 诚实报错停车；
//! 非 git 项目的手改若在两次消息之间只改内容不换名，靠 mtime 快检兜底
//! （保存动作必更新 mtime——按「通常够大」的风险已收敛为平台普遍行为）。

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use sha2::{Digest, Sha256};

use nemesis_utils::sanitize::sanitize_path_segment;

use crate::agents::vm_runtime::VmSupervisor;

/// 宿主侧打包分块大小（guest 收块闸 8MiB 的半档——JSON b64 膨胀 ~1.37x
/// 后单帧 ~5.6MB，留余量）。
const HOST_CHUNK_BYTES: usize = 4 * 1024 * 1024;
/// 单项目工作副本文件数诚实上限（超限停车报错，不静默截断——半棵树当
/// 工作副本是正确性事故）。
pub const MAX_SYNC_FILES: usize = 200_000;
/// 开工闸嫌疑面阈值：嫌疑变更超过它 = 增量对齐成本逼近全量（B9-1 旧
/// 基线缺 host_meta 的首个开工轮 / 大重构），诚实走全量重同步。
const SUSPECT_FULL_RESYNC_THRESHOLD: usize = 20_000;
/// 定点查询/删除的单次路径分块（guest 单帧 8MiB 收块闸下的保守档）。
const VM_QUERY_CHUNK: usize = 5_000;

// ---------------------------------------------------------------------------
// 拉回方向（guest→host）宿主侧防御闸（BUG-20261101-262）
//
// [`pull_files_to_host`] 的 `chunks` 头/单块字节/总 payload 全部来自 VM 侧
// wire 声明——本函数自己的注释曾主张「chunk 分次传输天然有界」，但分次
// 传输只约束单帧形状，不约束帧数×帧大小的总量：谎报 chunks 数（或单块
// 塞超额数据）即可让宿主 `buf` 无界增长直至 OOM。与 BUG-20261029 zip 撒谎
// size 同族——当时只修了 `with_capacity` 预分配面，累积面漏网。
//
// 块数/单块上限镜像 guest `bulk.rs` 协议常数（`MAX_TOTAL_CHUNKS` /
// `BULK_CHUNK_MAX_BYTES`，guest 侧自闸不构成宿主防御——契约常数跨信任
// 边界镜像，与本仓其它协议镜像同款）；总 payload 上限按工作副本排除
// 构建产物（target/node_modules 不进）后的合理量级取 2 GiB，超限与
// [`MAX_SYNC_FILES`] 同哲学：诚实报错停车，不静默截断半棵树。
const PULL_CHUNK_COUNT_MAX: u64 = 200_000;
const PULL_CHUNK_MAX_BYTES: usize = 8 * 1024 * 1024;
const PULL_TOTAL_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// 单 tar 条目解包内存上限（对齐 selfupdate 256MB 条目先例）——
/// [`extract_tar_to_host`] 曾裸 `read_to_end`，违反 zip_util
/// `read_entry_capped` 单一真相源纪律（2026-10-29 建立）。
const PULL_ENTRY_MAX_BYTES: usize = 256 * 1024 * 1024;

/// 默认排除表（非 git 项目 + git 项目顶滤共用；双向生效的「表」侧）。
/// `.browser-profile/` 是 P5-R1 guest 内 chromium 的 user-data-dir（登录态
/// 常驻的锚，overlay 承载跨睡眠/重启）——cookies/缓存不进项目工作副本
/// （体积 + 运行中 chromium 文件竞争双考虑）；迁移面按 D-P6-1 可迁移形态
/// 预埋（随 agent home 打包，P6-R2 定谳）。
pub const DEFAULT_EXCLUDE_PREFIXES: [&str; 13] = [
    ".git/",
    "target/",
    "node_modules/",
    "dist/",
    "build/",
    "__pycache__/",
    ".venv/",
    "venv/",
    ".next/",
    ".idea/",
    ".vscode/",
    "coverage/",
    ".browser-profile/",
];

/// 同步完成报告（可观测面：WSAPI/日志/审计消费）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct FullSyncReport {
    pub files: usize,
    pub bytes: u64,
    /// v1 打包只含普通文件（symlink 选择阶段已不进清单），恒 0——字段
    /// 先立形状，B9-2 commit 闸的增量报告复用本结构时消费。
    #[allow(dead_code)]
    pub skipped_symlinks: usize,
    pub selection: &'static str,
    /// B9-3 `vm.sync.status` 消费（现仅 gate debug 日志间接可见）。
    #[allow(dead_code)]
    pub synced_at_ms: u64,
}

/// 开工闸结果（B9-2 增量对齐版；`open_work_gate` 返回）：
/// - `Fresh`：首次开工 / 手动强制重扫 / 嫌疑面超阈值兜底 —— 全量同步；
/// - `Ready`：基线在册且宿主零变更（零传输直通）；
/// - `Aligned`：宿主变更已批量推进进 VM（无冲突）；
/// - `Conflicts`：存在双改冲突 —— **什么都不推**（半对齐状态等用户裁决，
///   见 plan §4：禁止在裁决前把一侧改动覆盖掉）。
#[derive(Debug, serde::Serialize)]
pub enum GateSync {
    Fresh(FullSyncReport),
    Ready {
        synced_at_ms: u64,
    },
    Aligned {
        pushed: Vec<String>,
        deleted: Vec<String>,
    },
    Conflicts(Vec<ConflictFile>),
}

/// 三方对比冲突条目（宿主≠基线 && VM≠基线；审批卡 / B9-3 WSAPI 消费）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConflictFile {
    pub rel: String,
    /// 宿主当前指纹（文件缺席时为空串——宿主删 + VM 改的冲突形态）。
    pub host_sha: String,
    /// VM 当前指纹（VM 已删时为空串——宿主改 + VM 删的冲突形态）。
    pub vm_sha: String,
    pub baseline_sha: String,
}

/// 宿主侧检测到的单条变更（开工闸宿主清单条目；存在性以盘面 stat 为准）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostChangeKind {
    Upsert,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostChange {
    pub rel: String,
    pub kind: HostChangeKind,
}

/// 三方对比单文件裁决（纯函数；单测钉真值表）。
pub(crate) enum ChangeKind {
    /// 宿主没动（commit 闸域）或删除已在两侧共识——无需动作。
    Skip,
    /// 宿主→VM 单向推进。
    Push,
    /// VM 侧文件删除（宿主已删且 VM 未改）。
    DeleteOnVm,
    /// 双改冲突（交审批裁决）。
    Conflict,
}

/// 宿主变更 → 三方裁决（b/v = 基线/VM 指纹，None = 该侧无此文件）。
/// 嫌疑清单本身意味着「宿主动了」，故不比较宿主指纹——开工闸裁决只需
/// 「VM 是否偏离基线」。
pub(crate) fn classify_host_change(
    kind: &HostChangeKind,
    b: Option<&str>,
    v: Option<&str>,
) -> ChangeKind {
    match kind {
        HostChangeKind::Upsert => match (b, v) {
            (_, Some(v)) if Some(v) == b => ChangeKind::Push, // VM 未动：安全推
            (None, None) => ChangeKind::Push,                 // 双侧都无：纯新增
            _ => ChangeKind::Conflict,                        // VM 偏离基线
        },
        HostChangeKind::Delete => match (b, v) {
            (None, _) => ChangeKind::Skip, // 基线无此条：忽略
            (Some(_), Some(v)) if Some(v) == b => ChangeKind::DeleteOnVm,
            (Some(_), None) => ChangeKind::DeleteOnVm, // VM 也没有：幂等删（清账本）
            (Some(_), Some(_)) => ChangeKind::Conflict, // 宿主删 + VM 改
        },
    }
}

/// 基线树 vs VM 当前树（commit 闸 diff；纯函数）。返回（changed：VM 上
/// 变更/新增的 rel；deleted：基线有、VM 无的 rel——均排序）。
pub(crate) fn diff_trees(
    baseline: &BTreeMap<String, String>,
    current: &BTreeMap<String, String>,
) -> (Vec<String>, Vec<String>) {
    let changed: Vec<String> = current
        .iter()
        .filter(|(rel, sha)| baseline.get(*rel).map(|b| b != *sha).unwrap_or(true))
        .map(|(rel, _)| rel.clone())
        .collect();
    let deleted: Vec<String> = baseline
        .keys()
        .filter(|rel| !current.contains_key(*rel))
        .cloned()
        .collect();
    (changed, deleted)
}

/// 宿主侧单文件元数据（非 git 项目开工闸的 mtime+size 快检依据；不读
/// 内容——数十万文件也在秒级）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostFileMeta {
    pub mtime_ms: u64,
    pub size: u64,
}

/// commit 闸报告（turn 末回传落宿主的结果；B9-3 WSAPI / Dashboard 消费，
/// 全文落 `last_commit.json`）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitReport {
    pub project_id: String,
    pub committed_at_ms: u64,
    pub changed: Vec<String>,
    pub deleted: Vec<String>,
    /// 回传 payload 字节数（tar 解包前内容总量）。
    pub bytes: u64,
    /// 因挂起（冲突裁决②「保留 VM 版」停车语义）跳过回传的文件清单
    /// （H3 可观测面——不回传覆盖宿主，等用户手动合并后由开工闸解挂）。
    #[serde(default)]
    pub parked_skipped: Vec<String>,
}

/// 基线账本（serde 结构；`files` = rel(/)→sha256，内容 = guest 读回）。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BaselineManifest {
    pub project_id: String,
    pub synced_at_ms: u64,
    pub is_git: bool,
    pub selection: String,
    /// 最近一次**全量**同步的 payload 字节（增量推送不记账——对账以
    /// `files` 表为准，此字段只做量级展示）。
    pub bytes: u64,
    pub skipped_symlinks: usize,
    pub files: BTreeMap<String, String>,
    /// 宿主侧元数据快照（B9-2 起：非 git 项目开工闸 mtime 快检 + git
    /// 项目回落检测的依据）。`#[serde(default)]` 兼容 B9-1 旧基线（缺
    /// 键 = 空表 → 全量嫌疑，由嫌疑阈值兜底走重同步）。
    #[serde(default)]
    pub host_meta: BTreeMap<String, HostFileMeta>,
}

/// 单文件同步条目（rel 用 `/` 分隔；mode 是目标权限位——git index 模式/
/// unix 实际位/缺省 755，Windows 宿主打包时按它覆写 header，不读本地
/// 文件元数据——NTFS 表达不了 exec 位）。
#[derive(Clone)]
struct FileSync {
    rel: String,
    mode: u32,
}

static CHUNK_COUNTER: AtomicU64 = AtomicU64::new(0);
/// 每项目在飞闸（并发开工请求串行化；条目按项目数有界，不清扫）。
static INFLIGHT: std::sync::LazyLock<
    std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 基线账本路径（唯一落点）
// ---------------------------------------------------------------------------

/// 基线目录：`<workspace>/logs/vm_sync/<sanitized project_id>/`。
pub fn baseline_dir(main_workspace: &Path, project_id: &str) -> PathBuf {
    main_workspace
        .join("logs")
        .join("vm_sync")
        .join(sanitize_path_segment(project_id))
}

fn baseline_path(main_workspace: &Path, project_id: &str) -> PathBuf {
    baseline_dir(main_workspace, project_id).join("baseline.json")
}

fn load_baseline(main_workspace: &Path, project_id: &str) -> Option<BaselineManifest> {
    let raw = std::fs::read_to_string(baseline_path(main_workspace, project_id)).ok()?;
    match serde_json::from_str(&raw) {
        Ok(m) => Some(m),
        Err(e) => {
            // 坏基线 = 无基线（重新全量），但别无声。
            tracing::warn!("[vm_sync] 基线解析失败（按无基线重全量）: {e}");
            None
        }
    }
}

fn last_commit_path(main_workspace: &Path, project_id: &str) -> PathBuf {
    baseline_dir(main_workspace, project_id).join("last_commit.json")
}

// ---------------------------------------------------------------------------
// 挂起表（H3：冲突裁决②「保留 VM 版」的停车语义）
// ---------------------------------------------------------------------------

/// 冲突裁决②（保留 VM 版）落下的挂起条目：宿主手改挂起——不推进 VM、
/// 不回传覆盖；此后宿主重新改动（指纹 ≠ 挂起时）= 手动合并完成，自动
/// 解挂按正常变更推进 VM；宿主收敛到 VM 版（或两侧都没了）= 无事解挂。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DivergedEntry {
    /// 挂起时的宿主指纹（空串 = 挂起时宿主缺文件）。
    pub host_sha: String,
    /// 挂起时的 VM 指纹。
    pub vm_sha: String,
    pub parked_at_ms: u64,
}

/// rel → 挂起条目。
pub type DivergedMap = BTreeMap<String, DivergedEntry>;

fn diverged_path(main_workspace: &Path, project_id: &str) -> PathBuf {
    baseline_dir(main_workspace, project_id).join("diverged.json")
}

/// 挂起表读取（缺文件 = 无挂起；坏 JSON = warn + 空表——挂起状态丢失
/// 回退旧冲突语义可接受，不能让整个开工/commit 闸炸）。
pub(crate) fn load_diverged(main_workspace: &Path, project_id: &str) -> DivergedMap {
    let Ok(raw) = std::fs::read_to_string(diverged_path(main_workspace, project_id)) else {
        return BTreeMap::new();
    };
    match serde_json::from_str(&raw) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("[vm_sync] 挂起表解析失败（按无挂起处理）: {e}");
            BTreeMap::new()
        }
    }
}

/// 整表写回（pub(crate)：manager 冲突取消臂恢复前置前快照消费——与
/// [`load_diverged`] 对称）。
pub(crate) async fn save_diverged(
    main_workspace: &Path,
    project_id: &str,
    map: &DivergedMap,
) -> Result<(), String> {
    let json = serde_json::to_string(map).map_err(|e| format!("挂起表序列化失败: {e}"))?;
    write_json_atomic(&diverged_path(main_workspace, project_id), &json).await
}

/// 整表恢复（过在飞闸）：manager 冲突取消/环境变化臂恢复前置前快照的
/// 唯一入口。直接调 [`save_diverged`] 会绕过 INFLIGHT 闸——与 commit 的
/// 挂起表过滤读、后续 park 的读改写形成丢更新竞态（「commit 刚过滤完
/// → 取消臂抹表 → 保护丢失」）。恢复语义 = 整表覆盖快照，所以闸内
/// load-modify-save 不适用，直接闸内写。
pub(crate) async fn restore_diverged_snapshot(
    main_workspace: &Path,
    project_id: &str,
    map: &DivergedMap,
) -> Result<(), String> {
    let gate = inflight_gate(project_id)?;
    let _guard = gate.lock().await;
    save_diverged(main_workspace, project_id, map).await
}

/// 整表快照（过在飞闸）：manager 冲突裁决**前置 park 之前**取前置快照用。
/// 无闸 [`load_diverged`] 有原子性（rename）但无隔离性——双轮并发下可能
/// 漏掉刚并发 park 的条目，取消臂恢复陈旧快照会抹掉别的轮次条目（取消
/// 轮无权动他轮凭据）。闸内读与 park/restore/commit 同序，快照不漏。
pub(crate) async fn snapshot_diverged(main_workspace: &Path, project_id: &str) -> DivergedMap {
    let gate = inflight_gate(project_id);
    match gate {
        Ok(g) => {
            let _guard = g.lock().await;
            load_diverged(main_workspace, project_id)
        }
        // 闸构造失败（首用初始化失败，实际不可达）——降级无闸读，快照
        // 陈旧的风险回到极窄双轮窗口，好过裁决流程整体失败。
        Err(_) => load_diverged(main_workspace, project_id),
    }
}

/// 冲突裁决②（保留 VM 版）落挂起表（幂等：同 rel 重挂以最新指纹覆盖）。
/// 独立持在飞闸——与开工/commit 对挂起表的读写互斥。返回挂起表总数。
pub async fn park_diverged_files(
    main_workspace: &Path,
    project_id: &str,
    entries: Vec<(String, String, String)>, // (rel, host_sha, vm_sha)
) -> Result<usize, String> {
    let gate = inflight_gate(project_id)?;
    let _guard = gate.lock().await;
    let mut map = load_diverged(main_workspace, project_id);
    let now = now_ms();
    for (rel, host_sha, vm_sha) in entries {
        map.insert(
            rel,
            DivergedEntry {
                host_sha,
                vm_sha,
                parked_at_ms: now,
            },
        );
    }
    let n = map.len();
    save_diverged(main_workspace, project_id, &map).await?;
    Ok(n)
}

/// 挂起文件去留判定（纯函数；单测钉 H3 语义）。
pub(crate) enum DivergedVerdict {
    /// 宿主没再动（与挂起时同指纹，含「当时缺现在仍缺」）——继续挂起。
    KeepParked,
    /// 宿主收敛到 VM 版（或两侧都没了）——解挂，无需动作。
    ReleaseConverged,
    /// 宿主重新改动 = 手动合并完成——解挂，按正常变更分类推进。
    ReleaseMerged,
}

pub(crate) fn diverged_verdict(
    entry: &DivergedEntry,
    host_sha: Option<&str>,
    vm_sha: Option<&str>,
) -> DivergedVerdict {
    // 收敛：宿主当前内容 == VM 当前内容（或两侧都没了——无事可做）。
    if let (Some(h), Some(v)) = (host_sha, vm_sha)
        && h == v
    {
        return DivergedVerdict::ReleaseConverged;
    }
    if host_sha.is_none() && vm_sha.is_none() {
        return DivergedVerdict::ReleaseConverged;
    }
    // 宿主没再动——继续挂起（不推进、不冲突卡、不回传覆盖）。
    if !entry.host_sha.is_empty() && host_sha == Some(entry.host_sha.as_str()) {
        return DivergedVerdict::KeepParked;
    }
    if entry.host_sha.is_empty() && host_sha.is_none() {
        return DivergedVerdict::KeepParked;
    }
    // 其余 = 宿主重新改动（含宿主删除）= 用户合并意志，解挂按正常变更走。
    DivergedVerdict::ReleaseMerged
}

/// git check-ignore 批量过滤（commit 侧 gitignore 双向生效）。`None` =
/// git 不可用/调用失败/超时（诚实降级：调用方不过滤 + warn）；`Some` =
/// 命中忽略规则的 rel 集合（可为空）。
async fn git_check_ignore(
    root: &Path,
    rels: &[String],
) -> Option<std::collections::HashSet<String>> {
    if rels.is_empty() {
        return Some(std::collections::HashSet::new());
    }
    if !git_available().await {
        return None;
    }
    let mut payload = Vec::new();
    for r in rels {
        payload.extend_from_slice(r.as_bytes());
        payload.push(0);
    }
    let mut child = tokio::process::Command::new("git")
        .current_dir(root)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // 10s 超时臂 = wait future 被 drop：不开 kill_on_drop 会泄漏挂死
        // 的 git 进程（慢机/句柄风暴下的僵尸源）。
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    use tokio::io::AsyncWriteExt;
    if child.stdin.as_mut()?.write_all(&payload).await.is_err() {
        return None;
    }
    drop(child.stdin.take()); // EOF：git 开始裁决
    let out = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    // 退出码 0=有命中 1=无命中（都是成功语义）；128=用法错（降级）。
    if out.status.code() == Some(128) {
        return None;
    }
    let mut ignored = std::collections::HashSet::new();
    for seg in out.stdout.split(|b| *b == 0) {
        if seg.is_empty() {
            continue;
        }
        ignored.insert(String::from_utf8_lossy(seg).replace('\\', "/"));
    }
    Some(ignored)
}

/// JSON 原子写（唯一临时名 + 改名；Windows 侧 rename 覆盖已存在目标可行）。
/// 临时名带 pid+序号：此前固定 `*.json.tmp` 依赖「所有调用方持 INFLIGHT
/// 闸」的隐式不变式（同路径双写方互踩临时文件 = 后写方 rename 走先写方
/// 的半截内容），这里把安全性收进函数自身、不靠调用方纪律（与 vm_runtime
/// save_record 同款）。写/改名失败臂清理孤儿 tmp。
async fn write_json_atomic(path: &Path, json: &str) -> Result<(), String> {
    static WRITE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp = path.with_file_name(format!(
        "{name}.{}.{}.tmp",
        std::process::id(),
        WRITE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    tokio::fs::write(&tmp, json).await.map_err(|e| {
        let _ = std::fs::remove_file(&tmp); // 错误清理臂：同步 fs（async 版未 await 即 drop 等于没清）
        format!("写临时文件失败: {e}")
    })?;
    tokio::fs::rename(&tmp, path).await.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("原子改名失败: {e}")
    })?;
    Ok(())
}

async fn persist_baseline(
    main_workspace: &Path,
    project_id: &str,
    manifest: &BaselineManifest,
) -> Result<(), String> {
    let json =
        serde_json::to_string_pretty(manifest).map_err(|e| format!("基线序列化失败: {e}"))?;
    write_json_atomic(&baseline_path(main_workspace, project_id), &json).await
}

// ---------------------------------------------------------------------------
// 排除规则 + 文件选择（单点）
// ---------------------------------------------------------------------------

/// 排除规则与文件选择单一真相源。git 探测 = root/.git 存在（目录或
/// worktree 文件都算）。
pub(crate) struct SyncExclusion {
    pub is_git: bool,
}

impl SyncExclusion {
    pub fn detect(root: &Path) -> Self {
        Self {
            is_git: root.join(".git").exists(),
        }
    }

    /// rel（`/` 分隔）是否被 admit。前缀匹配 + 精确目录名匹配（文件与
    /// 排除目录同名边缘形态）。
    pub fn admits(&self, rel: &str) -> bool {
        for p in DEFAULT_EXCLUDE_PREFIXES {
            let dir = p.trim_end_matches('/');
            if rel.starts_with(p) || rel == dir {
                return false;
            }
        }
        true
    }
}

/// 选择工作副本文件清单（排序确定性；exec 位随条目携带）。
///
/// git 项目：`git ls-files -z --cached --others --exclude-standard`
/// （跟踪 + 未跟踪未忽略；`.gitignore` 即排除语义），exec 位从
/// `git ls-files -s -z --cached` 的 index 模式读（100755→755，未跟踪
/// 755）。git 探针/调用失败 = 诚实回落 walk+默认排除表（WARN 留痕，
/// 基线 selection=walk）——工具在场 ≠ 可用（环境敏感代码纪律）。
async fn select_files(root: &Path) -> Result<(Vec<FileSync>, bool, &'static str), String> {
    let ex = SyncExclusion::detect(root);
    if ex.is_git {
        match select_files_git(root).await {
            Ok(mut files) => {
                files.retain(|f| ex.admits(&f.rel));
                return Ok((files, true, "git"));
            }
            Err(e) => {
                tracing::warn!(
                    "[vm_sync] git 选择失败，回落 walk+默认排除表（基线 selection=walk 留痕）: {e}"
                );
            }
        }
    }
    let files = tokio::task::spawn_blocking({
        let root = root.to_path_buf();
        move || select_files_walk(&root)
    })
    .await
    .map_err(|e| format!("walk 任务失败: {e}"))??;
    let is_git = ex.is_git;
    Ok((files, is_git, "walk"))
}

/// git 轻量探针（工具自身实跑 `git --version`——在场 ≠ 可用）。
async fn git_available() -> bool {
    let ok = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("git")
            .arg("--version")
            .output(),
    )
    .await;
    matches!(ok, Ok(Ok(out)) if out.status.success())
}

/// 从 `git ls-files -s -z --cached` 输出解析 index 模式表：
/// `"<mode> <sha> <stage>\t<path>"`（z 分隔）。
fn parse_index_modes(raw: &[u8]) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for entry in raw.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let Some(tab) = entry.iter().position(|b| *b == b'\t') else {
            continue;
        };
        let (meta, path) = entry.split_at(tab);
        let path = String::from_utf8_lossy(&path[1..]).replace('\\', "/");
        let meta = String::from_utf8_lossy(meta);
        let mode = meta
            .split_whitespace()
            .next()
            .and_then(|m| u32::from_str_radix(m, 8).ok())
            .unwrap_or(0o644);
        map.insert(path, mode & 0o777);
    }
    map
}

async fn select_files_git(root: &Path) -> Result<Vec<FileSync>, String> {
    if !git_available().await {
        return Err("git 不可用（--version 探针失败）".to_string());
    }
    let run = |args: &[&str]| {
        let root = root.to_path_buf();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(60),
                tokio::process::Command::new("git")
                    .current_dir(&root)
                    .args(&args)
                    .output(),
            )
            .await
            .map_err(|_| "git 调用超时（60s）".to_string())?
            .map_err(|e| format!("git 调用失败: {e}"))
        }
    };
    let list = run(&[
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ])
    .await
    .and_then(|o| {
        if o.status.success() {
            Ok(o.stdout)
        } else {
            Err(format!(
                "git ls-files 失败: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ))
        }
    })?;
    let modes_out = run(&["ls-files", "-s", "-z", "--cached"]).await?;
    let modes = if modes_out.status.success() {
        parse_index_modes(&modes_out.stdout)
    } else {
        HashMap::new() // exec 位表失败不致命：全部按 755（guest 可读可执行）
    };

    let mut files = Vec::new();
    for entry in list.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let rel = String::from_utf8_lossy(entry).replace('\\', "/");
        if rel.is_empty() {
            continue;
        }
        let mode = modes.get(&rel).copied().unwrap_or(0o755);
        files.push(FileSync { rel, mode });
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    files.dedup_by(|a, b| a.rel == b.rel);
    // 盘面是唯一真相（与 detect_host_changes/stat_classify 同语义）：
    // `--cached` 会列出已从盘面删除但未 stage 的 index 幽灵条目——带进
    // 打包清单就是 pack_tar「清单与盘面失配」诚实报错，一个未 stage 的
    // 普通删除把首次全量同步卡死开工。stat 后消失的条目按删除语义处理
    // （下轮 git status 同样不再列它，双方一致）。stat 循环入阻塞线程，
    // 20 万文件量级不占执行器。
    let root2 = root.to_path_buf();
    let files = tokio::task::spawn_blocking(move || {
        files.retain(|f| root2.join(&f.rel).is_file());
        files
    })
    .await
    .map_err(|e| format!("盘面过滤任务失败: {e}"))?;
    Ok(files)
}

/// 非 git（/git 回落）walk：排序读目录、排除目录整枝剪、unix 实际权限
/// 位（非 unix 缺省 755）。symlink 不跟（file_type 不解引用）。
fn select_files_walk(root: &Path) -> Result<Vec<FileSync>, String> {
    let ex = SyncExclusion::detect(root);
    let mut files = Vec::new();
    walk(root, root, &ex, &mut files)?;
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

fn walk(
    root: &Path,
    dir: &Path,
    ex: &SyncExclusion,
    out: &mut Vec<FileSync>,
) -> Result<(), String> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("枚举 {} 失败: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let path = ent.path();
        let ft = ent
            .file_type()
            .map_err(|e| format!("stat {} 失败: {e}", path.display()))?;
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if !ex.admits(&rel) {
            continue;
        }
        if ft.is_dir() {
            walk(root, &path, ex, out)?;
        } else if ft.is_file() {
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::metadata(&path)
                    .map(|m| m.permissions().mode() & 0o777)
                    .unwrap_or(0o755)
            };
            #[cfg(not(unix))]
            let mode = 0o755;
            out.push(FileSync { rel, mode });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 打包（mode 覆写 + 单趟 sha256）
// ---------------------------------------------------------------------------

/// 打包文件清单到临时 tar（不压缩；header mode 按FileSync.mode 覆写——
/// Windows 宿主读不到 exec 位）。返回（临时文件路径, rel→sha256 表,
/// payload 字节数）。
async fn pack_tar(
    root: &Path,
    files: &[FileSync],
) -> Result<(PathBuf, BTreeMap<String, String>, u64), String> {
    let root = root.to_path_buf();
    let files = files.to_vec();
    let id = format!(
        "{}-{}",
        now_ms(),
        CHUNK_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let out = std::env::temp_dir().join(format!("nemesis-vm-sync-{id}.tar"));
    let out_for_task = out.clone();
    // 错误清理臂：打包中途失败（stat 失配/IO/JoinError）时半截 tar 已由
    // File::create 落盘——不留系统 temp 成孤儿（唯一名会随失败反复累积）。
    let packed =
        tokio::task::spawn_blocking(move || pack_tar_blocking(&root, &files, &out_for_task)).await;
    match packed {
        Ok(Ok((hashes, payload_bytes))) => Ok((out, hashes, payload_bytes)),
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&out);
            Err(e)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            Err(format!("打包任务失败: {e}"))
        }
    }
}

fn pack_tar_blocking(
    root: &Path,
    files: &[FileSync],
    out: &Path,
) -> Result<(BTreeMap<String, String>, u64), String> {
    let f = std::fs::File::create(out).map_err(|e| format!("创建打包临时文件失败: {e}"))?;
    let mut builder = tar::Builder::new(std::io::BufWriter::new(f));
    let mut hashes = BTreeMap::new();
    let mut payload_bytes = 0u64;
    for fs_entry in files {
        let src = root.join(&fs_entry.rel);
        let meta = std::fs::metadata(&src)
            .map_err(|e| format!("stat {} 失败（清单与盘面失配？）: {e}", fs_entry.rel))?;
        if !meta.is_file() {
            return Err(format!(
                "{} 已不是普通文件（选择与打包之间盘面变化）",
                fs_entry.rel
            ));
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(meta.len());
        header.set_mode(fs_entry.mode); // 刻意：不读本地元数据（NTFS 无 exec 位）
        header.set_mtime(
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        );
        header.set_cksum();
        let src_f =
            std::fs::File::open(&src).map_err(|e| format!("打开 {} 失败: {e}", fs_entry.rel))?;
        let mut tee = HashingReader {
            inner: src_f,
            hasher: Sha256::new(),
        };
        builder
            .append_data(&mut header, fs_entry.rel.as_str(), &mut tee)
            .map_err(|e| format!("打包 {} 失败: {e}", fs_entry.rel))?;
        hashes.insert(fs_entry.rel.clone(), tee.finish_hex());
        payload_bytes += meta.len();
    }
    // finish 只写 tar 尾块；BufWriter 靠 drop 收尾会把最后 ≤8KiB 的 flush
    // 错误静默吞掉——声明成功但同步包可能截断。into_inner 内含 finish 并
    // 交还 writer，显式 flush 报错后才算打包成功。
    let mut f = builder
        .into_inner()
        .map_err(|e| format!("收尾 tar 失败: {e}"))?;
    f.flush().map_err(|e| format!("tar 落盘失败: {e}"))?;
    Ok((hashes, payload_bytes))
}

/// 读透镜像：内容流过 tar 打包的同时算 sha256（单趟双用）。
struct HashingReader<R: Read> {
    inner: R,
    hasher: sha2::Sha256,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.hasher.update(&buf[..n]);
        }
        Ok(n)
    }
}

impl<R: Read> HashingReader<R> {
    fn finish_hex(self) -> String {
        format!("{:x}", self.hasher.finalize())
    }
}

// ---------------------------------------------------------------------------
// 开工全量同步
// ---------------------------------------------------------------------------

/// 开工闸主入口（B9-2）：基线缺失 / `force_full` = 全量同步；否则宿主
/// 变更检测（git status 或 mtime 快检，O(变更量)）→ 嫌疑文件三方对比 →
/// 无冲突批量推进 / 有冲突**非冲突文件照常推进**后原样停车交裁决
/// （H2 文件粒度隔离：冲突文件的裁决不拖累其他手改的吸收）。挂起表
/// （H3）中的文件全程绕行：不推进、不冲突卡、不回传覆盖。并发开工由
/// per-project 在飞闸串行化（与 commit 闸共用同一把锁——开工对齐和
/// turn 末回传不会交叠）。
pub async fn open_work_gate(
    main_workspace: &Path,
    project_id: &str,
    project_path: &Path,
    sup: &VmSupervisor,
    agent_id: &str,
    force_full: bool,
) -> Result<GateSync, String> {
    let gate = inflight_gate(project_id)?;
    let _guard = gate.lock().await;

    let Some(baseline) = load_baseline(main_workspace, project_id).filter(|_| !force_full) else {
        let report = full_sync(main_workspace, project_id, project_path, sup, agent_id).await?;
        return Ok(GateSync::Fresh(report));
    };

    // 宿主变更清单（开工闸第 1 步；git status 或 mtime 快检）。
    let changes = detect_host_changes(project_path, &baseline).await?;
    if changes.is_empty() {
        return Ok(GateSync::Ready {
            synced_at_ms: baseline.synced_at_ms,
        });
    }
    // 嫌疑面过大 = 增量对齐成本逼近全量（如 B9-1 旧基线缺 host_meta 的
    // 首个开工轮）：诚实走全量重同步。但全量（clean 清场）会覆盖 VM 未
    // 同步改动（M3 守卫）——先对账，VM 有偏离就诚实停车（清场走显式
    // resync，调用方负责披露丢失语义），VM 干净才自动全量。
    if changes.len() > SUSPECT_FULL_RESYNC_THRESHOLD {
        let vm_tree = vm_tree_shas(sup, agent_id)
            .await
            .map_err(|e| format!("全量前对账失败: {e}"))?;
        let (vm_changed, vm_deleted) = diff_trees(&baseline.files, &vm_tree);
        if !vm_changed.is_empty() || !vm_deleted.is_empty() {
            return Err(format!(
                "宿主嫌疑变更 {} 条超阈值 {SUSPECT_FULL_RESYNC_THRESHOLD}，且 VM 存在未同步改动\
                 （改 {} 删 {}）——自动全量重同步会覆盖 VM 侧改动，已停车。请先让项目会话完成\
                 一轮回传，或确认放弃 VM 侧改动后执行显式重同步。",
                changes.len(),
                vm_changed.len(),
                vm_deleted.len()
            ));
        }
        tracing::warn!(
            "[vm_sync] 宿主嫌疑变更 {} 条超阈值 {SUSPECT_FULL_RESYNC_THRESHOLD}（VM 无偏离），走全量重同步",
            changes.len()
        );
        let report = full_sync(main_workspace, project_id, project_path, sup, agent_id).await?;
        return Ok(GateSync::Fresh(report));
    }

    // 嫌疑文件三方对比（第 2 步）：全部嫌疑条目（含 Delete——Delete 也
    // 要看 VM 有没有偏离）定点查 VM 指纹；缺席 = None（VM 已删）。
    let rels: Vec<String> = changes.iter().map(|c| c.rel.clone()).collect();
    let vm_shas = query_vm_shas(sup, agent_id, &rels).await?;

    // 挂起表预检（H3 停车语义）：挂起中的文件不进分类——不推进、不冲突
    // 卡；宿主重新改动 = 手动合并完成自动解挂，收敛 = 无事解挂。
    let mut diverged = load_diverged(main_workspace, project_id);
    let mut diverged_dirty = false;
    let mut parked_count = 0usize;
    let mut effective: Vec<HostChange> = Vec::with_capacity(changes.len());
    for ch in &changes {
        if let Some(entry) = diverged.get(&ch.rel).cloned() {
            let host_sha = hash_host_file(project_path, &ch.rel).await;
            let vm_sha = vm_shas.get(&ch.rel).cloned().flatten();
            match diverged_verdict(&entry, host_sha.as_deref(), vm_sha.as_deref()) {
                DivergedVerdict::KeepParked => {
                    parked_count += 1;
                    continue;
                }
                verdict => {
                    diverged.remove(&ch.rel);
                    diverged_dirty = true;
                    if matches!(verdict, DivergedVerdict::ReleaseMerged) {
                        tracing::info!(
                            project = %project_id,
                            file = %ch.rel,
                            "[vm_sync] 挂起文件宿主已重新改动（手动合并完成），解挂按正常变更推进"
                        );
                    }
                }
            }
        }
        effective.push(ch.clone());
    }
    if diverged_dirty {
        save_diverged(main_workspace, project_id, &diverged).await?;
    }
    if parked_count > 0 {
        tracing::info!(
            project = %project_id,
            count = parked_count,
            "[vm_sync] {} 个挂起文件保持停车（保留 VM 版语义，宿主手改不推进不覆盖）",
            parked_count
        );
    }

    let mut pushes = Vec::new();
    let mut deletes_on_vm = Vec::new();
    let mut conflicts: Vec<String> = Vec::new();
    let mut aligned: Vec<String> = Vec::new(); // M1 短路命中的（三方已一致）
    for ch in &effective {
        let b = baseline.files.get(&ch.rel).map(String::as_str);
        let v = vm_shas.get(&ch.rel).cloned().flatten();
        // 对齐短路（M1）：VM 已在基线（v==b）时补算宿主内容指纹——内容
        // 也没变（git 项目未提交改动的常态：status 相对 index 永远 M；
        // 非 git 项目 mtime 被触摸）= 三方已一致，跳过重推。O(变更量)
        // 本地哈希换掉每轮重复传输。
        if ch.kind == HostChangeKind::Upsert
            && b.is_some()
            && v.as_deref() == b
            && hash_host_file(project_path, &ch.rel).await.as_deref() == b
        {
            aligned.push(ch.rel.clone());
            continue;
        }
        match classify_host_change(&ch.kind, b, v.as_deref()) {
            ChangeKind::Skip => {}
            ChangeKind::Push => pushes.push(ch.rel.clone()),
            ChangeKind::DeleteOnVm => deletes_on_vm.push(ch.rel.clone()),
            ChangeKind::Conflict => conflicts.push(ch.rel.clone()),
        }
    }

    // 批量推进（H2：冲突在场也照常执行非冲突文件——先推后删，推送中途
    // 失败 = 下轮重推幂等——tar 定点覆盖）。
    let pushed_shas = if pushes.is_empty() {
        BTreeMap::new()
    } else {
        push_files_to_vm(sup, agent_id, project_path, &pushes, baseline.is_git).await?
    };
    if !deletes_on_vm.is_empty() {
        delete_on_vm(sup, agent_id, &deletes_on_vm).await?;
    }

    // 基线增量刷新（files += 推进条目、-= 删除条目；host_meta 同步维护）
    // ——冲突在场也照常落盘：已推进文件的基线前移，下轮不重推。
    let mut files = baseline.files.clone();
    let mut host_meta = baseline.host_meta.clone();
    for (rel, sha) in &pushed_shas {
        files.insert(rel.clone(), sha.clone());
    }
    for rel in &deletes_on_vm {
        files.remove(rel);
        host_meta.remove(rel);
    }
    if !pushes.is_empty() {
        for (rel, m) in scan_meta_for(project_path, &pushes).await? {
            host_meta.insert(rel, m);
        }
    }
    // M1 短路命中的也补刷 host_meta——mtime 项目里被触摸过的文件若不刷，
    // 每轮都会重新进嫌疑清单、每轮重新哈希（假阳性永不收敛）。
    if !aligned.is_empty() {
        for (rel, m) in scan_meta_for(project_path, &aligned).await? {
            host_meta.insert(rel, m);
        }
    }
    persist_baseline(
        main_workspace,
        project_id,
        &BaselineManifest {
            project_id: baseline.project_id,
            synced_at_ms: now_ms(),
            is_git: baseline.is_git,
            selection: baseline.selection,
            bytes: baseline.bytes,
            skipped_symlinks: baseline.skipped_symlinks,
            files,
            host_meta,
        },
    )
    .await?;

    // 冲突条目交裁决（非冲突文件已对齐完毕）；宿主版指纹补算给审批卡。
    if !conflicts.is_empty() {
        let mut cfs = Vec::with_capacity(conflicts.len());
        for rel in &conflicts {
            cfs.push(ConflictFile {
                rel: rel.clone(),
                host_sha: hash_host_file(project_path, rel).await.unwrap_or_default(),
                vm_sha: vm_shas.get(rel).cloned().flatten().unwrap_or_default(),
                baseline_sha: baseline.files.get(rel).cloned().unwrap_or_default(),
            });
        }
        return Ok(GateSync::Conflicts(cfs));
    }

    Ok(GateSync::Aligned {
        pushed: pushed_shas.keys().cloned().collect(),
        deleted: deletes_on_vm,
    })
}

/// per-project 在飞闸条目（open_work_gate / commit_project / 冲突裁决
/// 共用——同一项目同一时刻只允许一个对齐动作）。
fn inflight_gate(project_id: &str) -> Result<Arc<tokio::sync::Mutex<()>>, String> {
    let mut m = INFLIGHT.lock().map_err(|_| "在飞闸锁中毒".to_string())?;
    Ok(Arc::clone(
        m.entry(project_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    ))
}

/// 宿主侧变更检测：git 项目 = `git status --porcelain=v1 -z
/// --untracked-files=all`（O(变更量)，亚秒）+ 逐路径 stat 定存在性；
/// git 不可用/失败 = 回落 mtime+size 元数据快检 vs 基线 host_meta；
/// 非 git 项目恒走元数据快检。
async fn detect_host_changes(
    project_path: &Path,
    baseline: &BaselineManifest,
) -> Result<Vec<HostChange>, String> {
    let ex = SyncExclusion {
        is_git: baseline.is_git,
    };
    if baseline.is_git && git_available().await {
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            tokio::process::Command::new("git")
                .current_dir(project_path)
                .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
                .output(),
        )
        .await;
        match out {
            Ok(Ok(o)) if o.status.success() => {
                let paths = parse_status_z(&o.stdout, &ex);
                let root = project_path.to_path_buf();
                return tokio::task::spawn_blocking(move || Ok(stat_classify(&root, paths)))
                    .await
                    .map_err(|e| format!("stat 任务失败: {e}"))?;
            }
            Ok(Ok(o)) => {
                tracing::warn!(
                    "[vm_sync] git status 失败，回落 mtime 快检: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
            }
            Ok(Err(e)) => tracing::warn!("[vm_sync] git status 调用失败，回落 mtime 快检: {e}"),
            Err(_) => tracing::warn!("[vm_sync] git status 超时（60s），回落 mtime 快检"),
        }
    }
    let current = scan_host_meta(project_path).await?;
    Ok(diff_host_meta(&baseline.host_meta, &current))
}

/// `git status --porcelain=v1 -z --untracked-files=all` 输出解析 → 变更
/// 路径集合。R/C 双段条目（`XY new\0old\0`）= 旧删新增两路都收。存在性
/// 归约交给调用方 stat 盘面（`git rm --cached` 等形态下 index 态与盘面
/// 态可能相反——盘面是唯一真相）。
pub(crate) fn parse_status_z(raw: &[u8], ex: &SyncExclusion) -> Vec<String> {
    let mut set = std::collections::BTreeSet::new();
    let mut entries = raw.split(|b| *b == 0).filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let path = String::from_utf8_lossy(&entry[3..]).replace('\\', "/");
        if ex.admits(&path) {
            set.insert(path);
        }
        if (entry[0] == b'R' || entry[0] == b'C')
            && let Some(old) = entries.next()
        {
            let old = String::from_utf8_lossy(old).replace('\\', "/");
            if ex.admits(&old) {
                set.insert(old);
            }
        }
    }
    set.into_iter().collect()
}

/// git 清单的最终态归约：逐路径 stat 盘面——普通文件 = Upsert，缺席 =
/// Delete，其余（目录/特殊文件）不进工作副本。
fn stat_classify(root: &Path, paths: Vec<String>) -> Vec<HostChange> {
    paths
        .into_iter()
        .filter_map(|rel| match std::fs::metadata(root.join(&rel)) {
            Ok(m) if m.is_file() => Some(HostChange {
                rel,
                kind: HostChangeKind::Upsert,
            }),
            Ok(_) => None,
            Err(_) => Some(HostChange {
                rel,
                kind: HostChangeKind::Delete,
            }),
        })
        .collect()
}

/// 宿主元数据快扫（mtime+size，不读内容；排除表过滤；排序确定性——
/// spawn_blocking 里跑，避免卡执行器）。
async fn scan_host_meta(root: &Path) -> Result<BTreeMap<String, HostFileMeta>, String> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || scan_host_meta_blocking(&root))
        .await
        .map_err(|e| format!("元数据快扫任务失败: {e}"))?
}

fn scan_host_meta_blocking(root: &Path) -> Result<BTreeMap<String, HostFileMeta>, String> {
    let ex = SyncExclusion::detect(root);
    let mut out = BTreeMap::new();
    walk_meta(root, root, &ex, &mut out)?;
    Ok(out)
}

fn walk_meta(
    root: &Path,
    dir: &Path,
    ex: &SyncExclusion,
    out: &mut BTreeMap<String, HostFileMeta>,
) -> Result<(), String> {
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .map_err(|e| format!("读取目录 {} 失败: {e}", dir.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("枚举 {} 失败: {e}", dir.display()))?;
    entries.sort_by_key(|e| e.file_name());
    for ent in entries {
        let path = ent.path();
        let ft = ent
            .file_type()
            .map_err(|e| format!("stat {} 失败: {e}", path.display()))?;
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if !ex.admits(&rel) {
            continue;
        }
        if ft.is_dir() {
            walk_meta(root, &path, ex, out)?;
        } else if ft.is_file() {
            let meta = std::fs::metadata(&path)
                .map_err(|e| format!("stat {} 失败: {e}", path.display()))?;
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            out.insert(
                rel,
                HostFileMeta {
                    mtime_ms,
                    size: meta.len(),
                },
            );
        }
    }
    Ok(())
}

/// 基线元数据 vs 当前元数据：mtime/size 不一致或新增 = Upsert；基线有
/// 现无 = Delete。基线侧空表（B9-1 旧基线缺 host_meta）→ 全量嫌疑（由
/// 嫌疑阈值兜底走重同步）。
pub(crate) fn diff_host_meta(
    baseline: &BTreeMap<String, HostFileMeta>,
    current: &BTreeMap<String, HostFileMeta>,
) -> Vec<HostChange> {
    let mut changes = Vec::new();
    for (rel, cur) in current {
        match baseline.get(rel) {
            None => changes.push(HostChange {
                rel: rel.clone(),
                kind: HostChangeKind::Upsert,
            }),
            Some(b) if b.mtime_ms != cur.mtime_ms || b.size != cur.size => {
                changes.push(HostChange {
                    rel: rel.clone(),
                    kind: HostChangeKind::Upsert,
                })
            }
            Some(_) => {}
        }
    }
    for rel in baseline.keys() {
        if !current.contains_key(rel) {
            changes.push(HostChange {
                rel: rel.clone(),
                kind: HostChangeKind::Delete,
            });
        }
    }
    changes.sort_by(|a, b| a.rel.cmp(&b.rel));
    changes
}

/// VM 侧定点指纹查询（hash_tree paths 过滤；分块防单帧过大）。缺席路径
/// guest 静默省略 → 宿主归约为 None（VM 已删）。
async fn query_vm_shas(
    sup: &VmSupervisor,
    agent_id: &str,
    rels: &[String],
) -> Result<BTreeMap<String, Option<String>>, String> {
    let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
    for chunk in rels.chunks(VM_QUERY_CHUNK) {
        let args = serde_json::json!({ "mode": "files", "paths": chunk });
        let raw = sup
            .internal_call(agent_id, "hash_tree", args)
            .await
            .map_err(|e| format!("hash_tree 定点查询失败: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("hash_tree 响应解析失败: {e}"))?;
        if v["truncated"].as_bool() == Some(true) {
            return Err("hash_tree 定点查询截断（单块超 guest 上限）".to_string());
        }
        for f in v["files"]
            .as_array()
            .ok_or_else(|| "hash_tree 响应缺 files 数组".to_string())?
        {
            let path = f["path"]
                .as_str()
                .ok_or_else(|| "hash_tree 条目缺 path".to_string())?;
            let sha = f["sha256"]
                .as_str()
                .ok_or_else(|| "hash_tree 条目缺 sha256".to_string())?;
            out.insert(path.to_string(), Some(sha.to_string()));
        }
    }
    for rel in rels {
        out.entry(rel.clone()).or_insert(None);
    }
    Ok(out)
}

/// VM 树全指纹（排除表同源；正确性优先于增量——commit 与 M3 全量前
/// 对账共用，单一实现）。truncated = 诚实报错（调用方各自决定不落基线
/// / 停车，错误信息带语境由调用方包裹）。
async fn vm_tree_shas(
    sup: &VmSupervisor,
    agent_id: &str,
) -> Result<BTreeMap<String, String>, String> {
    let args = serde_json::json!({
        "mode": "files",
        "exclude_prefixes": DEFAULT_EXCLUDE_PREFIXES,
    });
    let raw = sup
        .internal_call(agent_id, "hash_tree", args)
        .await
        .map_err(|e| format!("hash_tree 读回失败: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("hash_tree 响应解析失败: {e}"))?;
    if v["truncated"].as_bool() == Some(true) {
        return Err("hash_tree 截断（VM 树超上限）".to_string());
    }
    let mut out = BTreeMap::new();
    for f in v["files"]
        .as_array()
        .ok_or_else(|| "hash_tree 响应缺 files 数组".to_string())?
    {
        let path = f["path"]
            .as_str()
            .ok_or_else(|| "hash_tree 条目缺 path".to_string())?;
        let sha = f["sha256"]
            .as_str()
            .ok_or_else(|| "hash_tree 条目缺 sha256".to_string())?;
        out.insert(path.to_string(), sha.to_string());
    }
    Ok(out)
}

/// 宿主→VM 定点推送（增量对齐 / 冲突宿主版覆盖共用）。exec 位：git 项目
/// 查 index 模式表（tracked 文件保位；guest 解包按 header 覆写权限），非
/// git 恒 755。推送后定点指纹复核。返回 rel→sha。
async fn push_files_to_vm(
    sup: &VmSupervisor,
    agent_id: &str,
    project_path: &Path,
    rels: &[String],
    is_git: bool,
) -> Result<BTreeMap<String, String>, String> {
    if rels.is_empty() {
        return Ok(BTreeMap::new());
    }
    let modes: HashMap<String, u32> = if is_git {
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            tokio::process::Command::new("git")
                .current_dir(project_path)
                .args(["ls-files", "-s", "-z", "--cached"])
                .output(),
        )
        .await
        .map_err(|_| "git ls-files 超时（60s）".to_string())?
        .map_err(|e| format!("git ls-files 失败: {e}"))?;
        if out.status.success() {
            parse_index_modes(&out.stdout)
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };
    let files: Vec<FileSync> = rels
        .iter()
        .map(|rel| FileSync {
            rel: rel.clone(),
            mode: modes.get(rel).copied().unwrap_or(0o755),
        })
        .collect();
    let (tmp_tar, hashes, _bytes) = pack_tar(project_path, &files).await?;
    let result = send_tar_chunks(sup, agent_id, &tmp_tar, false).await;
    let _ = std::fs::remove_file(&tmp_tar);
    result?;

    // 定点复核（增量推送的完整性闸——只对推过的路径核指纹）。
    let vm_shas = query_vm_shas(sup, agent_id, rels).await?;
    for (rel, sha) in &hashes {
        if vm_shas.get(rel).cloned().flatten().as_deref() != Some(sha.as_str()) {
            return Err(format!("推送复核失败：{rel} 指纹不一致"));
        }
    }
    Ok(hashes)
}

/// VM 侧定点删除（宿主删除传播；幂等——missing 计数不报错）。
async fn delete_on_vm(sup: &VmSupervisor, agent_id: &str, rels: &[String]) -> Result<(), String> {
    for chunk in rels.chunks(VM_QUERY_CHUNK) {
        let args = serde_json::json!({ "paths": chunk });
        let raw = sup
            .internal_call(agent_id, "bulk_delete", args)
            .await
            .map_err(|e| format!("bulk_delete 失败: {e}"))?;
        let v: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("bulk_delete 响应解析失败: {e}"))?;
        tracing::debug!(
            "[vm_sync] VM 侧删除 {}/{} 个（missing {}）",
            v["deleted"].as_u64().unwrap_or(0),
            chunk.len(),
            v["missing"].as_u64().unwrap_or(0)
        );
    }
    Ok(())
}

/// 定点 stat 补扫（推进条目的 host_meta 维护；缺席文件静默不进表）。
async fn scan_meta_for(
    root: &Path,
    rels: &[String],
) -> Result<BTreeMap<String, HostFileMeta>, String> {
    let root = root.to_path_buf();
    let rels = rels.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut out = BTreeMap::new();
        for rel in rels {
            if let Ok(meta) = std::fs::metadata(root.join(&rel))
                && meta.is_file()
            {
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                out.insert(
                    rel,
                    HostFileMeta {
                        mtime_ms,
                        size: meta.len(),
                    },
                );
            }
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("元数据补扫任务失败: {e}"))?
}

/// 宿主单文件指纹（冲突审批卡消费；读不到 = None → 空串）。
async fn hash_host_file(root: &Path, rel: &str) -> Option<String> {
    let p = root.join(rel);
    tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(&p).ok()?;
        let mut h = Sha256::new();
        std::io::copy(&mut f, &mut h).ok()?;
        Some(format!("{:x}", h.finalize()))
    })
    .await
    .ok()
    .flatten()
}

/// 冲突裁决①（用宿主版覆盖）：定点推 host→VM + 基线增量刷新。审批卡
/// 逐文件调用（文件粒度隔离——单文件失败不牵连其他裁决）。
pub async fn apply_conflict_host_wins(
    main_workspace: &Path,
    project_id: &str,
    project_path: &Path,
    sup: &VmSupervisor,
    agent_id: &str,
    rel: &str,
) -> Result<(), String> {
    let gate = inflight_gate(project_id)?;
    let _guard = gate.lock().await;
    let Some(mut baseline) = load_baseline(main_workspace, project_id) else {
        return Err("基线缺失（先完成一次开工全量同步）".to_string());
    };
    let rels = vec![rel.to_string()];
    let shas = push_files_to_vm(sup, agent_id, project_path, &rels, baseline.is_git).await?;
    for (r, sha) in &shas {
        baseline.files.insert(r.clone(), sha.clone());
    }
    for (r, m) in scan_meta_for(project_path, &rels).await? {
        baseline.host_meta.insert(r, m);
    }
    baseline.synced_at_ms = now_ms();
    persist_baseline(main_workspace, project_id, &baseline).await?;
    // 防御性解挂：该文件若在挂起表（如先选②后重裁决改选①），①的语义
    // 是宿主版落定——挂起条目必须清掉，否则 commit 侧会继续拦它的回传。
    let mut div = load_diverged(main_workspace, project_id);
    if div.remove(rel).is_some() {
        save_diverged(main_workspace, project_id, &div).await?;
    }
    Ok(())
}

/// 全量同步：选文件 → 打包 → 分块 bulk_write（首次带 clean）→ hash_tree
/// 读回对账 → 落基线。任何一步失败 = 诚实报错（基线不落，下次开工重来）。
async fn full_sync(
    main_workspace: &Path,
    project_id: &str,
    project_path: &Path,
    sup: &VmSupervisor,
    agent_id: &str,
) -> Result<FullSyncReport, String> {
    let started = now_ms();
    if !project_path.is_dir() {
        return Err(format!(
            "项目目录不存在或不是目录: {}",
            project_path.display()
        ));
    }
    let (files, is_git, selection) = select_files(project_path).await?;
    if files.len() > MAX_SYNC_FILES {
        return Err(format!(
            "项目文件数 {} 超过工作副本上限 {MAX_SYNC_FILES}（诚实停车：不静默截断半棵树）",
            files.len()
        ));
    }

    let (tmp_tar, expected, payload_bytes) = pack_tar(project_path, &files).await?;
    let result = send_and_verify(sup, agent_id, &tmp_tar, &expected, payload_bytes).await;
    let _ = std::fs::remove_file(&tmp_tar); // 成败都清打包临时件
    let skipped_symlinks = 0usize; // v1 打包只含普通文件（symlink 选择阶段已不进清单）
    let report = FullSyncReport {
        files: files.len(),
        bytes: payload_bytes,
        skipped_symlinks,
        selection,
        synced_at_ms: started,
    };
    result?;

    // 宿主元数据快照（开工闸后续快检依据；O(N) 元数据枚举，不读内容）。
    let host_meta = scan_host_meta(project_path).await?;

    // 基线落盘（内容 = 宿主打包时指纹——已与 guest 读回逐文件对账相等）。
    let manifest = BaselineManifest {
        project_id: project_id.to_string(),
        synced_at_ms: started,
        is_git,
        selection: selection.to_string(),
        bytes: payload_bytes,
        skipped_symlinks,
        files: expected,
        host_meta,
    };
    persist_baseline(main_workspace, project_id, &manifest).await?;
    // 全量同步 = 宿主整体覆盖 VM：原挂起文件的宿主版已随全量进 VM 且基
    // 线已含——挂起表清空（陈旧条目留着会永久拦掉未来同类变更，H3）。
    // 清空前检查：非空挂起 = 用户「保留 VM 版」的裁决将被宿主版覆盖。
    // 显式 resync 是披露过的语义，但坏基线 → 自动全量是隐式路径——
    // loud warn 点名，不留无声覆盖。已在 INFLIGHT 闸内，直读安全。
    let pending = load_diverged(main_workspace, project_id);
    if !pending.is_empty() {
        tracing::warn!(
            project = %project_id,
            count = pending.len(),
            files = ?pending.keys().take(8).collect::<Vec<_>>(),
            "[vm_sync] 全量同步将丢弃全部挂起裁决（这些文件的 VM 版本被宿主版本覆盖）"
        );
    }
    save_diverged(main_workspace, project_id, &BTreeMap::new()).await?;
    Ok(report)
}

/// 分块发送 + guest 读回对账（传输完整性闸：数量 + 逐文件 sha256）。
async fn send_and_verify(
    sup: &VmSupervisor,
    agent_id: &str,
    tmp_tar: &Path,
    expected: &BTreeMap<String, String>,
    payload_bytes: u64,
) -> Result<(), String> {
    send_tar_chunks(sup, agent_id, tmp_tar, true).await?;

    // 读回对账：数量 + 逐文件 sha256（传输损坏 = 诚实报错，不落基线）。
    let args = serde_json::json!({
        "mode": "files",
        "exclude_prefixes": DEFAULT_EXCLUDE_PREFIXES,
    });
    let raw = sup
        .internal_call(agent_id, "hash_tree", args)
        .await
        .map_err(|e| format!("hash_tree 读回失败: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("hash_tree 响应解析失败: {e}"))?;
    if v["truncated"].as_bool() == Some(true) {
        return Err("hash_tree 截断（VM 树超上限）——不落基线".to_string());
    }
    let got = v["files"]
        .as_array()
        .ok_or_else(|| "hash_tree 响应缺 files 数组".to_string())?;
    let mut got_map: HashMap<String, String> = HashMap::with_capacity(got.len());
    for f in got {
        let path = f["path"]
            .as_str()
            .ok_or_else(|| "hash_tree 条目缺 path".to_string())?;
        let sha = f["sha256"]
            .as_str()
            .ok_or_else(|| "hash_tree 条目缺 sha256".to_string())?
            .to_string();
        got_map.insert(path.to_string(), sha);
    }
    if got_map.len() != expected.len() {
        let mut extra: Vec<&String> = got_map.keys().collect();
        let missing: Vec<&String> = expected
            .keys()
            .filter(|k| !got_map.contains_key(*k))
            .collect();
        extra.retain(|k| !expected.contains_key(*k));
        extra.sort();
        return Err(format!(
            "同步对账失败：文件数宿主 {} ≠ VM {}（VM 多余: {:?}… / VM 缺失: {:?}…；payload={payload_bytes}B）",
            expected.len(),
            got_map.len(),
            &extra[..extra.len().min(5)],
            &missing[..missing.len().min(5)],
        ));
    }
    for (rel, sha) in expected {
        if got_map.get(rel).map(String::as_str) != Some(sha.as_str()) {
            return Err(format!(
                "同步对账失败：{rel} 指纹不一致（传输损坏或 VM 侧篡改）"
            ));
        }
    }
    Ok(())
}

/// tar 分块上传（bulk_write；`clean` = 首块清场标志——只有全量首装是
/// true，增量推送/commit 恒 false）。不含对账——对账由调用方按语义选
/// （全量 = 全树核对；增量 = 定点复核）。
async fn send_tar_chunks(
    sup: &VmSupervisor,
    agent_id: &str,
    tmp_tar: &Path,
    clean: bool,
) -> Result<(), String> {
    let tar_len = tokio::fs::metadata(tmp_tar)
        .await
        .map_err(|e| format!("打包产物 stat 失败: {e}"))?
        .len();
    let total = (tar_len.div_ceil(HOST_CHUNK_BYTES as u64)).max(1);
    let transfer_id = format!(
        "in-{}-{}",
        now_ms(),
        CHUNK_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut file = tokio::fs::File::open(tmp_tar)
        .await
        .map_err(|e| format!("打开打包产物失败: {e}"))?;
    use tokio::io::AsyncReadExt;
    for seq in 0..total {
        let mut buf = vec![0u8; HOST_CHUNK_BYTES];
        let mut filled = 0usize;
        while filled < buf.len() {
            let n = file
                .read(&mut buf[filled..])
                .await
                .map_err(|e| format!("读打包产物失败（seq={seq}）: {e}"))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            return Err(format!(
                "打包产物提前 EOF（seq={seq}/{total}，盘面被并发修改？）"
            ));
        }
        buf.truncate(filled);
        let data = base64::engine::general_purpose::STANDARD.encode(&buf);
        let args = serde_json::json!({
            "transfer": transfer_id,
            "seq": seq,
            "total": total,
            "data": data,
            "clean": seq == 0 && clean,
        });
        sup.internal_call(agent_id, "bulk_write", args)
            .await
            .map_err(|e| format!("bulk_write 块 {seq}/{total} 失败: {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// commit 闸（B9-2：turn 末 / 手动——VM→宿主回传 + 基线刷新）
// ---------------------------------------------------------------------------

/// commit 闸：VM 树全指纹 → 与基线 diff → 变更文件 bulk_read 回传落宿主
/// → VM 侧删除同步删宿主（**明示清单，不静默**——`deleted` 随报告上抛）
/// → 基线刷新 → `last_commit.json` 落盘。
///
/// 返回 `None` = 无基线（未开工过，没有可 commit 的工作副本）。零变更
/// 也返回 `Some`（changed/deleted 皆空——`last_commit.json` 照常刷新，
/// Dashboard 可见「最近检查时间」）。estop 期间照常运行：commit 是
/// 「保护方向」的基础设施对齐（internal_call 无 estop 闸，plan §4）。
pub async fn commit_project(
    main_workspace: &Path,
    project_id: &str,
    project_path: &Path,
    sup: &VmSupervisor,
    agent_id: &str,
) -> Result<Option<CommitReport>, String> {
    let gate = inflight_gate(project_id)?;
    let _guard = gate.lock().await;

    let started = now_ms();
    let Some(baseline) = load_baseline(main_workspace, project_id) else {
        return Ok(None); // 未开工过：没有工作副本
    };
    if !project_path.is_dir() {
        return Err(format!("项目目录不存在: {}", project_path.display()));
    }

    // VM 树全指纹（排除表同源；正确性优先于增量——见模块头注释）。
    let mut current = vm_tree_shas(sup, agent_id)
        .await
        .map_err(|e| format!("hash_tree 截断（VM 树超上限）——不提交: {e}"))?;
    let (changed, deleted) = diff_trees(&baseline.files, &current);
    let mut report = CommitReport {
        project_id: project_id.to_string(),
        committed_at_ms: started,
        changed,
        deleted,
        bytes: 0,
        parked_skipped: Vec::new(),
    };
    if report.changed.is_empty() && report.deleted.is_empty() {
        write_json_atomic(
            &last_commit_path(main_workspace, project_id),
            &serde_json::to_string_pretty(&report).map_err(|e| format!("序列化失败: {e}"))?,
        )
        .await?;
        return Ok(Some(report));
    }

    // H3 挂起过滤（停车语义的 commit 侧）：挂起中的文件不回传、不删宿主
    // ——「保留 VM 版」裁决不等于「VM 随后的改动可以覆盖宿主」，宿主
    // 手改区在用户合并完成（重新改动宿主文件 → 开工闸解挂推进）前是
    // 禁区。基线仍刷成 VM 树（挂起条目随 VM 走），解挂后三方对比照常。
    let diverged = load_diverged(main_workspace, project_id);
    if !diverged.is_empty() {
        let mut parked: Vec<String> = Vec::new();
        report.changed.retain(|rel| {
            if diverged.contains_key(rel) {
                parked.push(rel.clone());
                false
            } else {
                true
            }
        });
        report.deleted.retain(|rel| {
            if diverged.contains_key(rel) {
                parked.push(rel.clone());
                false
            } else {
                true
            }
        });
        if !parked.is_empty() {
            parked.sort();
            parked.dedup();
            report.parked_skipped = parked;
            tracing::info!(
                "[vm_sync] commit 挂起过滤：{} 个停车文件不回传（等宿主手动合并后自动解挂）",
                report.parked_skipped.len()
            );
        }
    }

    // M2 自定义 gitignore 过滤（VM→宿主方向遵守项目自己的忽略规则——
    // DEFAULT_EXCLUDE_PREFIXES 之外的动态层）。被忽略的变更条目不回传、
    // 不进基线（宿主与基线双双无视 → 后续轮次不会把它误判成宿主删除
    // 反复纠缠）；删除条目不过滤（删除是收敛方向，照常传播）。git 不可
    // 用 / 规则读取失败 = 诚实降级（warn，不过滤）。
    if baseline.is_git && !report.changed.is_empty() {
        match git_check_ignore(project_path, &report.changed).await {
            Some(ignored) if !ignored.is_empty() => {
                let n = ignored.len();
                report.changed.retain(|rel| !ignored.contains(rel));
                for rel in &ignored {
                    current.remove(rel);
                }
                tracing::info!("[vm_sync] commit gitignore 过滤：{n} 个被忽略文件不回传");
            }
            Some(_) => {}
            None => {
                tracing::warn!(
                    "[vm_sync] git check-ignore 不可用，跳过自定义忽略规则过滤（诚实降级）"
                );
            }
        }
    }
    if report.changed.is_empty() && report.deleted.is_empty() {
        // 全过滤早退（H3 停车 / gitignore），但基线仍要刷成 VM 树——上方
        // 注释承诺的「挂起条目随 VM 走，解挂后三方对比照常」在这里兑现。
        // 不刷的后果（真机实测）：停车轮 commit 全被过滤 → 基线停在停车前
        // 旧值 → 宿主手动合并完成 → 开工闸 ReleaseMerged 解挂 → 三方对比
        // 对旧基线仍是「VM 偏离」→ 立刻再判 Conflict 再弹卡，「按正常变
        // 更推进」永远走不到。host_meta 不动：本路径无 changed/deleted 可
        // 记账。
        persist_baseline(
            main_workspace,
            project_id,
            &BaselineManifest {
                project_id: baseline.project_id,
                synced_at_ms: now_ms(),
                is_git: baseline.is_git,
                selection: baseline.selection,
                bytes: baseline.bytes,
                skipped_symlinks: baseline.skipped_symlinks,
                files: current,
                host_meta: baseline.host_meta,
            },
        )
        .await?;
        write_json_atomic(
            &last_commit_path(main_workspace, project_id),
            &serde_json::to_string_pretty(&report).map_err(|e| format!("序列化失败: {e}"))?,
        )
        .await?;
        return Ok(Some(report));
    }

    // 变更文件回传宿主（bulk_read 组装 tar → safe_rel 逐条目落盘）。
    // 前置跨平台校验闸——大小写臂（非法组件臂=pull 阶段 safe_rel，symlink
    // 臂=选择期排除）：VM 侧（Linux）合法共存的大小写变体路径，回传大小写
    // 不敏感宿主（Windows NTFS / macOS APFS 默认）会静默互覆——碰撞即停车
    // 不猜；大小写敏感宿主（Linux ext4）warn 放行（彼处合法共存）。
    {
        let existing: Vec<String> = baseline.host_meta.keys().cloned().collect();
        let collisions = case_gate_pairs(&report.changed, &existing, &report.deleted);
        if !collisions.is_empty() {
            let detail = collisions
                .iter()
                .map(|(a, b)| format!("{a} ↔ {b}"))
                .collect::<Vec<_>>()
                .join("; ");
            if host_fs_case_insensitive() {
                return Err(format!(
                    "大小写冲突，拒绝提交（宿主文件系统大小写不敏感，以下路径对回传会互相覆盖；\
                     请在 VM 内合并为单一大小写后重试）: {detail}"
                ));
            }
            tracing::warn!(
                "[vm_sync] commit 变更集存在大小写碰撞（宿主大小写敏感，放行）: {detail}"
            );
        }
    }
    if !report.changed.is_empty() {
        report.bytes = pull_files_to_host(sup, agent_id, project_path, &report.changed).await?;
    }
    // VM 侧删除 → 同步删宿主（404 幂等；清单随 CommitReport 明示上抛）。
    if !report.deleted.is_empty() {
        let root = project_path.to_path_buf();
        let deleted = report.deleted.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            for rel in &deleted {
                match std::fs::remove_file(root.join(rel)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("删除宿主文件 {rel} 失败: {e}")),
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("删除任务失败: {e}"))??;
    }

    // 基线刷新：files = VM 当前树（对账已过）；host_meta 补扫变更/剔除删除。
    let mut host_meta = baseline.host_meta;
    for rel in &report.deleted {
        host_meta.remove(rel);
    }
    if !report.changed.is_empty() {
        for (rel, m) in scan_meta_for(project_path, &report.changed).await? {
            host_meta.insert(rel, m);
        }
    }
    persist_baseline(
        main_workspace,
        project_id,
        &BaselineManifest {
            project_id: baseline.project_id,
            synced_at_ms: now_ms(),
            is_git: baseline.is_git,
            selection: baseline.selection,
            bytes: baseline.bytes,
            skipped_symlinks: baseline.skipped_symlinks,
            files: current,
            host_meta,
        },
    )
    .await?;
    write_json_atomic(
        &last_commit_path(main_workspace, project_id),
        &serde_json::to_string_pretty(&report).map_err(|e| format!("序列化失败: {e}"))?,
    )
    .await?;
    Ok(Some(report))
}

/// wire 声明的 chunk 计数闸（BUG-20261101-262）：0（非空清单的诚实 tar
/// 至少含 1 条目）与超镜像上限（guest `MAX_TOTAL_CHUNKS`）都诚实拒。
fn validate_pull_chunk_count(chunks: u64) -> Result<(), String> {
    if chunks == 0 {
        return Err("bulk_read 声明 chunks=0（非空清单不可能）".to_string());
    }
    if chunks > PULL_CHUNK_COUNT_MAX {
        return Err(format!(
            "bulk_read 声明 chunks={chunks} 超上限 {PULL_CHUNK_COUNT_MAX}（wire 谎报？）"
        ));
    }
    Ok(())
}

/// 单块/累计 payload 预算闸（BUG-20261101-262）：extend 前调用，单块超
/// guest `BULK_CHUNK_MAX_BYTES` 镜像或累计超 [`PULL_TOTAL_MAX_BYTES`] 都
/// 诚实拒收——不先收进 `buf` 再判（判定在分配累积面之前）。
fn check_pull_chunk_budget(buf_len: usize, chunk_len: usize) -> Result<(), String> {
    if chunk_len > PULL_CHUNK_MAX_BYTES {
        return Err(format!(
            "单块 {chunk_len} 字节超上限 {PULL_CHUNK_MAX_BYTES}"
        ));
    }
    let total = buf_len as u64 + chunk_len as u64;
    if total > PULL_TOTAL_MAX_BYTES {
        return Err(format!(
            "拉回 payload 累计 {total} 字节超总量上限 {PULL_TOTAL_MAX_BYTES}"
        ));
    }
    Ok(())
}

/// VM→宿主 定点回传（commit 闸；bulk_read 打包 → 分块拉取组装 → tar
/// 解包落盘）。返回 payload 字节数。
async fn pull_files_to_host(
    sup: &VmSupervisor,
    agent_id: &str,
    project_path: &Path,
    rels: &[String],
) -> Result<u64, String> {
    if rels.is_empty() {
        return Ok(0);
    }
    let args = serde_json::json!({ "paths": rels });
    let raw = sup
        .internal_call(agent_id, "bulk_read", args)
        .await
        .map_err(|e| format!("bulk_read 失败: {e}"))?;
    let head: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("bulk_read 响应解析失败: {e}"))?;
    let transfer = head["transfer"]
        .as_str()
        .ok_or_else(|| "bulk_read 响应缺 transfer".to_string())?
        .to_string();
    let chunks = head["chunks"]
        .as_u64()
        .ok_or_else(|| "bulk_read 响应缺 chunks".to_string())?;
    if !head["missing"].as_array().is_none_or(|m| m.is_empty()) {
        // 清单里的路径在 VM 缺席 = 基线与 VM 树在打包窗口内被并发改动
        // （commit 持锁期间不该发生）——诚实报错重试，不落半份。
        return Err(format!(
            "bulk_read 缺席 {} 个请求路径（VM 树并发变化？）",
            head["missing"].as_array().map(|m| m.len()).unwrap_or(0)
        ));
    }
    // 不按响应头 `bytes` 预分配（BUG-20261029 zip 撒谎 size 同族横扫补漏）：
    // head 声明来自 VM 侧 wire，谎报/损坏值 `with_capacity` 即成无中生有的
    // 巨额分配（u64::MAX 直接 capacity overflow abort）。本路径对路径已做
    // fail-closed 防御（safe_rel/symlink 闸），size 头没有理由盲信——按实
    // 到 chunk 增长。
    //
    // 「分次传输天然有界」不成立（BUG-20261101-262）：chunk 循环的帧数与
    // 单帧大小同样来自 VM 侧 wire，分次只约束单帧形状、不约束总量——
    // 谎报 chunks 数即可让宿主 buf 无界增长直至 OOM。guest 自限
    // （MAX_TOTAL_CHUNKS/BULK_CHUNK_MAX_BYTES）活在 guest 进程内，不是
    // 宿主防御。以下闸按上列 PULL_* 常数诚实拒收（与 MAX_SYNC_FILES
    // 同哲学：超限停车报错，不静默截断半棵树）。
    validate_pull_chunk_count(chunks)?;
    let mut buf = Vec::new();
    for seq in 0..chunks {
        let args = serde_json::json!({ "transfer": transfer, "seq": seq });
        let chunk_raw = sup
            .internal_call(agent_id, "bulk_read_chunk", args)
            .await
            .map_err(|e| format!("bulk_read_chunk {seq}/{chunks} 失败: {e}"))?;
        let cv: serde_json::Value = serde_json::from_str(&chunk_raw)
            .map_err(|e| format!("bulk_read_chunk 响应解析失败: {e}"))?;
        let data = cv["data"]
            .as_str()
            .ok_or_else(|| "bulk_read_chunk 响应缺 data".to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("bulk_read_chunk {seq} base64 解码失败: {e}"))?;
        check_pull_chunk_budget(buf.len(), bytes.len())
            .map_err(|e| format!("bulk_read_chunk {seq}: {e}"))?;
        buf.extend_from_slice(&bytes);
    }
    let root = project_path.to_path_buf();
    let payload_len = buf.len() as u64;
    tokio::task::spawn_blocking(move || extract_tar_to_host(&buf, &root))
        .await
        .map_err(|e| format!("解包任务失败: {e}"))??;
    Ok(payload_len)
}

/// tar 字节 → 宿主工作树（commit 回读；safe_rel 逐条目防御闸；unix 侧
/// exec 位按 header 归一 755/644）。**只写不删**——删除面由调用方按
/// deleted 清单显式处理（明示语义，不藏在 tar 里）。
#[allow(dead_code)]
/// 组件 symlink 闸（宿主侧）：`rel` 的任一祖先/末端组件若已存在且是
/// symlink，拒绝落盘（fail-closed，点名路径）。宿主工作区是真实开发
/// 目录——pnpm 的 node_modules 全链接、用户手建链接都常见；VM 树回拉的
/// 同名真实文件若顺着链接写入，会写到工作区外（pnpm 全局 store 等）。
/// 与 guest 侧 `extract_tar_safe` 的组件闸同族防御（2026-10-20 复查批
/// 同修）。调用前 rel 已过 [`safe_rel`]（无 `..`/绝对/前缀组件，`/` 分隔）。
fn ensure_no_symlink_components_host(root: &Path, rel: &str) -> Result<(), String> {
    let mut cur = root.to_path_buf();
    for comp in rel.split('/') {
        if comp.is_empty() {
            continue; // safe_rel 已拒空组件；防御性跳过重复斜杠
        }
        cur.push(comp);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(format!(
                    "路径组件含 symlink，拒绝回拉落盘（fail-closed）: {} —— 请先清理宿主工作区同名链接",
                    cur.display()
                ));
            }
            // 已存在为普通目录：继续查下级；尚不存在：后续 create 会建；
            // 已存在为普通文件但还有下级组件：交给 create_dir_all 的诚实 IO 错误。
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("stat {} 失败: {e}", cur.display())),
        }
    }
    Ok(())
}

pub(crate) fn extract_tar_to_host(bytes: &[u8], root: &Path) -> Result<(), String> {
    extract_tar_to_host_with_caps(bytes, root, PULL_ENTRY_MAX_BYTES)
}

/// [`extract_tar_to_host`] 的可注入条目上限变体（测试用小 cap 钉行为；
/// 生产包装传 [`PULL_ENTRY_MAX_BYTES`]）。条目读走
/// `nemesis_utils::zip_util::read_entry_capped` 单一真相源（16KB 分块
/// 流式、超 cap 即拒——解压炸弹在撑爆内存前被拦），不再裸 `read_to_end`
/// （BUG-20261101-262 straggler）。
pub(crate) fn extract_tar_to_host_with_caps(
    bytes: &[u8],
    root: &Path,
    entry_cap: usize,
) -> Result<(), String> {
    let mut ar = tar::Archive::new(bytes);
    let entries = ar.entries().map_err(|e| format!("tar 遍历失败: {e}"))?;
    for ent in entries {
        let mut ent = ent.map_err(|e| format!("tar 条目读取失败: {e}"))?;
        let rel_raw = ent
            .path()
            .map_err(|e| format!("tar 条目路径读取失败: {e}"))?
            .to_string_lossy()
            .replace('\\', "/");
        let rel = safe_rel(&rel_raw)?;
        if !ent.header().entry_type().is_file() {
            continue; // 目录/链接条目忽略：父目录按需创建；symlink 不进工作副本
        }
        // 组件 symlink 闸在落盘前统一执行（std::fs::write 会跟随链接）。
        ensure_no_symlink_components_host(root, &rel)?;
        let dst = root.join(&rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败（{rel}）: {e}"))?;
        }
        let content = nemesis_utils::zip_util::read_entry_capped(&mut ent, entry_cap)
            .map_err(|e| format!("读条目 {rel} 失败: {e}"))?;
        std::fs::write(&dst, &content).map_err(|e| format!("写宿主文件 {rel} 失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = ent.header().mode().unwrap_or(0o644) & 0o777;
            let norm = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
            let _ = std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(norm));
        }
    }
    Ok(())
}

/// commit 闸的 AgentLoop 挂点：turn 末（最终回复已记史、Done 事件前）
/// 做一次 VM→宿主回传 + 基线刷新。恒 `Stop`（反馈不属于本闸职责——失败
/// 只 WARN，不影响用户收到回复，下轮重试；estop 期间照常——「保护方向」
/// 的基础设施对齐，plan §4）。全项目 loop 无差别注册，触发期自检：
/// supervisor / agent_id / VM 档都从 agents 全局表**按需解析**（VM 档是
/// 运行期属性——spawn 期 supervisor 未装配、后来切档/切回都自然跟随；
/// 非 VM 档项目零开销穿越）。
pub struct VmCommitHook {
    pub project_id: String,
    pub project_path: PathBuf,
    pub main_workspace: PathBuf,
}

#[async_trait::async_trait]
impl nemesis_agent::hooks::LifecycleHook for VmCommitHook {
    fn name(&self) -> String {
        "vm_commit".to_string()
    }

    async fn on_turn_end(
        &self,
        _end: &nemesis_agent::hooks::HookTurnEnd,
    ) -> nemesis_agent::hooks::TurnEndDecision {
        let Some(mgr) = crate::agents::manager() else {
            return nemesis_agent::hooks::TurnEndDecision::Stop;
        };
        let Some(rec) = mgr.agent_by_project(&self.project_id) else {
            return nemesis_agent::hooks::TurnEndDecision::Stop;
        };
        if rec.runtime_backend != nemesis_taskstore::agents::runtime_backend::VM {
            return nemesis_agent::hooks::TurnEndDecision::Stop;
        }
        let Some(sup) = mgr.vm_supervisor() else {
            return nemesis_agent::hooks::TurnEndDecision::Stop;
        };
        match commit_project(
            &self.main_workspace,
            &self.project_id,
            &self.project_path,
            sup.as_ref(),
            &rec.agent_id,
        )
        .await
        {
            Ok(Some(r)) if !r.changed.is_empty() || !r.deleted.is_empty() => {
                tracing::info!(
                    project = %self.project_id,
                    changed = r.changed.len(),
                    deleted = r.deleted.len(),
                    bytes = r.bytes,
                    "[vm_sync] commit 闸：VM 变更已回传宿主（{} 改 / {} 删）",
                    r.changed.len(),
                    r.deleted.len()
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    project = %self.project_id,
                    "[vm_sync] commit 闸失败（不影响本轮回复，下轮重试）: {e}"
                );
            }
        }
        nemesis_agent::hooks::TurnEndDecision::Stop
    }
}

// ---------------------------------------------------------------------------
// tar 条目名安全闸（宿主侧同水位防线；guest bulk.rs 另有一份）
// ---------------------------------------------------------------------------

/// 拒绝对路径组件（`..`/绝对路径/Windows 前缀）——基线里的 rel 都来自
/// 本模块选择，此处是 B9-2 commit 回读时的防御闸（`pull_files_to_host`
/// 消费；单测钉语义）。
pub(crate) fn safe_rel(rel: &str) -> Result<String, String> {
    if rel.is_empty() {
        return Err("相对路径为空".to_string());
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(format!("绝对路径拒绝: {rel}"));
    }
    for c in p.components() {
        match c {
            Component::Normal(_) => {}
            other => return Err(format!("非法路径组件 ({other:?}) 拒绝: {rel}")),
        }
    }
    Ok(rel.replace('\\', "/"))
}

/// 宿主文件系统是否默认大小写不敏感（Windows NTFS/FAT、macOS APFS 默认
/// 不敏感；Linux ext4 等默认敏感——碰撞在彼处合法共存，只 warn 不停车）。
fn host_fs_case_insensitive() -> bool {
    cfg!(windows) || cfg!(target_os = "macos")
}

/// 大小写碰撞检测（commit 校验闸纯函数）：changed（本次回传集）∪ existing
/// （宿主既有清单，剔除 deleted——「删 foo.rs + 加 Foo.rs」是合法大小写
/// 改名不是碰撞）里，casefold 相同而 byte 不同的路径对；至少一端在
/// changed 里才上报（两端都是既有文件的历史碰撞不由本笔提交停车）。
/// 返回 `[(a, b)]`，a 为组内字典序最小者。
fn case_gate_pairs(
    changed: &[String],
    existing: &[String],
    deleted: &[String],
) -> Vec<(String, String)> {
    let changed_set: std::collections::HashSet<&String> = changed.iter().collect();
    let deleted_set: std::collections::HashSet<&String> = deleted.iter().collect();
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for r in changed
        .iter()
        .chain(existing.iter().filter(|e| !deleted_set.contains(e)))
    {
        groups.entry(r.to_lowercase()).or_default().push(r.clone());
    }
    let mut out = Vec::new();
    for (_, mut members) in groups {
        members.sort();
        members.dedup();
        if members.len() < 2 {
            continue;
        }
        let first = &members[0];
        for other in &members[1..] {
            if changed_set.contains(first) || changed_set.contains(other) {
                out.push((first.clone(), other.clone()));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// B9-3 手动面状态报告（WSAPI vm.sync.status / CLI / Dashboard 同步卡消费）
// ---------------------------------------------------------------------------

/// 项目同步状态（只读：不开闸、不落盘、不触碰 VM 实例——status 永远
/// 不该把睡眠的实例拽起来）。
#[derive(Debug, serde::Serialize)]
pub struct SyncStatusReport {
    pub project_id: String,
    /// 基线在册 = 至少开工同步过一次（false = 尚未开工，宿主待同步清单
    /// 无法增量判定，状态卡应提示先发消息触发开工同步）。
    pub has_baseline: bool,
    pub synced_at_ms: Option<u64>,
    pub selection: Option<String>,
    pub baseline_files: Option<usize>,
    /// 宿主侧待推进（开工闸口径：git status / mtime 快检嫌疑面）。
    pub host_pending: Vec<String>,
    /// 宿主侧待删除（基线有 / 盘面无）。
    pub host_pending_deletes: Vec<String>,
    /// 上次 commit 报告原样回放（last_commit.json；无 = 从未 commit）。
    pub last_commit: Option<serde_json::Value>,
}

/// 组装 [`SyncStatusReport`]。基线缺席时宿主清单恒空（诚实：不是「无
/// 变更」，是「无从对账」——由 has_baseline=false 表达）。
pub(crate) async fn sync_status_report(
    main_workspace: &Path,
    project_id: &str,
    project_path: &Path,
) -> Result<SyncStatusReport, String> {
    let baseline = load_baseline(main_workspace, project_id);
    let (host_pending, host_pending_deletes) = match &baseline {
        Some(b) => {
            let changes = detect_host_changes(project_path, b).await?;
            let mut up = Vec::new();
            let mut del = Vec::new();
            for c in changes {
                if c.kind == HostChangeKind::Upsert {
                    up.push(c.rel);
                } else {
                    del.push(c.rel);
                }
            }
            (up, del)
        }
        None => (Vec::new(), Vec::new()),
    };
    let last_commit = std::fs::read_to_string(last_commit_path(main_workspace, project_id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    Ok(SyncStatusReport {
        project_id: project_id.to_string(),
        has_baseline: baseline.is_some(),
        synced_at_ms: baseline.as_ref().map(|b| b.synced_at_ms),
        selection: baseline.as_ref().map(|b| b.selection.clone()),
        baseline_files: baseline.as_ref().map(|b| b.files.len()),
        host_pending,
        host_pending_deletes,
        last_commit,
    })
}

#[cfg(test)]
mod tests;
