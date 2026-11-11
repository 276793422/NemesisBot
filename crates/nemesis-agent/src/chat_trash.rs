//! 会话回收站（2026-10-30）：`sessions.delete` 默认从硬删除改为「移入回收站」。
//!
//! 存储权威 = `<workspace>/logs/trash/sessions/{deleted_at_ms}_{safe_key}/`
//! （路径真相源 [`nemesis_path::resolve_trash_sessions_dir_in_workspace`]），
//! 每个子目录装原 chat_log 三件套的改名副本（`chat.jsonl` / `boundary.jsonl` /
//! `meta.json`）+ todo 文件副本（`todo.json`，2026-11-05：删除链路此前不感知
//! todo 文件，软删后 live 残留孤儿——BUG 根修四件套化）+ `trash.json` 清单。
//! 设计要点：
//!
//! - **移入 = rename**：同卷 rename 是元数据操作，文件字节原样；同键多次
//!   删除靠 `deleted_at_ms` 前缀并存，不互相覆盖。
//! - **目录名即寻址**：前端/WSAPI 只见条目名（entry），恢复/清除按名操作，
//!   不暴露原键给路径拼接（防注入：entry 名严格校验）。
//! - **清单缺失 = 孤儿目录**：手工拷入/半途失败诚实列出（字段按目录名反推），
//!   web 键（`agent:main:session:` 前缀，uuid 无 `_`）反推无损可恢复；复合
//!   通道键反推有损 → 诚实拒绝恢复。
//! - **logs/ 之下**：自动进 checkpoint 忽略表、logs sweeper 第六族按目录
//!   mtime 整目录 TTL 清扫（目录入站后不再变化，mtime ≈ 删除时刻）、不进
//!   `sessions.list`（只扫 session_logs jsonl）。
//!
//! 冲突语义：恢复目标三件套（jsonl 或 meta）已存在 = 结构化
//! conflict 返回（调用方转提示）；web 键允许带 `new_sid` 改名恢复（新 sid 经
//! [`SessionStore::sanitize_session_id`] 消毒后重验），复合键改名不支持。
//!
//! 恢复事务（2026-11-02 复评）：三段 rename 中途失败会留下「半成品」——
//! 一部分材料已到目标键、其余还在条目目录。恢复在首次 rename 前往条目内写
//! 事务标记（`restore-in-progress.json`，含目标键）；重试按标记识别自己的
//! 半成品，跳过冲突检查续作；换键恢复遇材料已分裂诚实拒绝（防「chat 留原
//! 键、boundary/meta 去新键」的会话分裂假成功）。成功 = 整目录删除（标记
//! 随之消失），失败保留条目 = 可重试。
//!
//! # 寻址注入
//!
//! 核心函数全部走 [`TrashLocations`]（chat_log 三件套寻址 + 回收站根）。
//! 生产 = [`TrashLocations::production`]（进程单例 path manager，文件名格式
//! 由 chat_log 同源闭包决定）；测试一律 [`TrashLocations::with_dirs`] 注入
//! tempdir——进程单例 home 不可 per-test 重定向（OnceLock 固化，cov 系同款
//! 约束），而真实 home 是在用数据目录，测试写它 = 污染。惯例
//! `nemesis-plugins-wasm::PluginManager:new_with_dirs`。

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nemesis_path::{default_path_manager, resolve_trash_sessions_dir_in_workspace};
use serde_json::json;

use crate::chat_log::{self, SessionMeta};
use crate::session::SessionStore;

/// web 会话键前缀（改名恢复只认这个前缀；其余 = 复合通道键）。
const WEB_SESSION_KEY_PREFIX: &str = "agent:main:session:";

/// `trash.json` 清单。全部字段带 default：清单损坏/缺字段不炸列表。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrashManifest {
    /// 被删会话的原始 session_key（恢复寻址真相源）。
    pub original_session_key: String,
    /// 展示用 id（web 键 = session id；复合键 = 键原样）。
    #[serde(default)]
    pub display_id: String,
    /// meta.title 快照（进回收站时刻的名字；恢复后 meta 内 title 照旧）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 删除时刻（毫秒；目录名前缀同源）。
    pub deleted_at_ms: u64,
    /// chat.jsonl 非空行数（≈ 消息条数）。
    #[serde(default)]
    pub message_count: u64,
    /// 四件套字节合计。
    #[serde(default)]
    pub bytes: u64,
    /// 来源标记：`single`（单条删除）/ `batch`（批量删除）。
    #[serde(default)]
    pub source: String,
    /// 条目是否包含 todo 文件（2026-11-05 四件套化新增；旧清单缺字段 =
    /// false 兼容——旧条目无 todo 副本，恢复/清除按 absent 处理）。
    #[serde(default)]
    pub todo: bool,
}

// ---------------------------------------------------------------------------
// 寻址注入面
// ---------------------------------------------------------------------------

/// 回收站寻址注入面：chat_log 三件套 + todo 文件寻址 + 回收站根。
///
/// 生产走 [`TrashLocations::production`]（与 chat_log 同源寻址，路径真相源
/// 不搬出 chat_log；todo 走 [`nemesis_path::session_todo_file_in_workspace`]
/// 单一真相源）；测试走 [`TrashLocations::with_dirs`] 注入 tempdir。
#[derive(Clone)]
pub struct TrashLocations {
    trash_root: PathBuf,
    #[allow(clippy::type_complexity)]
    log_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
    #[allow(clippy::type_complexity)]
    meta_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
    #[allow(clippy::type_complexity)]
    boundary_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
    #[allow(clippy::type_complexity)]
    todo_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
}

impl TrashLocations {
    /// 生产寻址：chat_log 同源函数 + 进程单例回收站根（与 chat_log 同一
    /// 寻址惯例）。
    pub(crate) fn production() -> Self {
        let workspace = default_path_manager().workspace();
        Self {
            trash_root: trash_root(),
            log_path: Arc::new(chat_log::log_path),
            meta_path: Arc::new(chat_log::meta_path),
            boundary_path: Arc::new(chat_log::boundary_path),
            todo_path: Arc::new(move |k: &str| {
                nemesis_path::session_todo_file_in_workspace(&workspace, k)
            }),
        }
    }

    /// 测试注入：回收站根 + session_logs 目录 + boundary 目录 + sessions 目录
    /// （todo 文件挂后者）。文件名格式与 chat_log / nemesis-path 真相源同源
    /// （`{safe}.jsonl` / `{safe}.meta.json` / `todo_{safe}.json`，
    /// `safe = sanitize_path_segment`；chat_log.rs::log_path 注释即契约）。
    pub fn with_dirs(
        trash_root: PathBuf,
        session_logs_dir: PathBuf,
        boundary_dir: PathBuf,
        sessions_dir: PathBuf,
    ) -> Self {
        let logs_dir = session_logs_dir.clone();
        Self {
            trash_root,
            log_path: Arc::new(move |k: &str| {
                logs_dir.join(format!(
                    "{}.jsonl",
                    nemesis_utils::sanitize::sanitize_path_segment(k)
                ))
            }),
            meta_path: Arc::new(move |k: &str| {
                session_logs_dir.join(format!(
                    "{}.meta.json",
                    nemesis_utils::sanitize::sanitize_path_segment(k)
                ))
            }),
            boundary_path: Arc::new(move |k: &str| {
                boundary_dir.join(format!(
                    "{}.jsonl",
                    nemesis_utils::sanitize::sanitize_path_segment(k)
                ))
            }),
            todo_path: Arc::new(move |k: &str| {
                sessions_dir.join(format!(
                    "todo_{}.json",
                    nemesis_utils::sanitize::sanitize_path_segment(k)
                ))
            }),
        }
    }

    pub fn trash_root(&self) -> &Path {
        &self.trash_root
    }

    pub fn log_path(&self, session_key: &str) -> PathBuf {
        (self.log_path)(session_key)
    }

    pub fn meta_path(&self, session_key: &str) -> PathBuf {
        (self.meta_path)(session_key)
    }

    pub fn boundary_path(&self, session_key: &str) -> PathBuf {
        (self.boundary_path)(session_key)
    }

    pub fn todo_path(&self, session_key: &str) -> PathBuf {
        (self.todo_path)(session_key)
    }
}

/// 回收站根目录（进程单例 path manager；与 chat_log 同一寻址惯例）。
fn trash_root() -> PathBuf {
    resolve_trash_sessions_dir_in_workspace(&default_path_manager().workspace())
}

/// 条目目录名：`{deleted_at_ms}_{sanitize_path_segment(key)}`。ms 前缀让
/// 同键多次删除并存；字符串排序即时间排序。
fn entry_dir_name(session_key: &str, deleted_at_ms: u64) -> String {
    format!(
        "{}_{}",
        deleted_at_ms,
        nemesis_utils::sanitize::sanitize_path_segment(session_key)
    )
}

/// 展示 id：web 键剥前缀，其余原样。
fn display_id_of(key: &str) -> String {
    key.strip_prefix(WEB_SESSION_KEY_PREFIX)
        .unwrap_or(key)
        .to_string()
}

/// 条目名校验：非空、无路径分隔符、无 `..`。防注入的第一道闸——前端只见
/// 条目名，恢复/清除把它拼进路径前必须先过这里。
fn valid_entry_name(entry: &str) -> bool {
    !entry.is_empty()
        && entry != "."
        && entry != ".."
        && !entry.contains('/')
        && !entry.contains('\\')
        && !entry.contains("..")
}

/// 孤儿目录名反推（清单缺失时的 best-effort）：`(deleted_at_ms, derived_key)`。
/// web 键反推无损（uuid 段无 `_`）；其余键 naive `_`→`:` 有损（仅用于列表
/// 展示，恢复时拒绝——见 [`trash_restore_in`]）。
fn derive_from_entry_name(entry: &str) -> Option<(u64, String)> {
    let (ms, safe) = entry.split_once('_')?;
    let ms = ms.parse::<u64>().ok()?;
    let key = if let Some(rest) = safe.strip_prefix("agent_main_session_") {
        format!("{WEB_SESSION_KEY_PREFIX}{rest}")
    } else {
        safe.replace('_', ":")
    };
    Some((ms, key))
}

// ---------------------------------------------------------------------------
// 移动原语
// ---------------------------------------------------------------------------

/// 同卷 rename 短重试（Windows 瞬时句柄：AV/索引器/杀软扫打开是常态不是
/// 异常）。std::fs::rename 两平台都覆盖已存在目标（Windows =
/// MoveFileExW + MOVEFILE_REPLACE_EXISTING）；真正失败的形态是目标被
/// share-mode 占用（AV 抓着）——重试等句柄松手，恒占用时耗尽诚实 Err。
/// 恢复冲突检测靠前置检查（rename 会静默覆盖 live 会话 = 吞件），不靠
/// rename 竞态裁决。重试耗尽诚实 Err（调用方上浮，不静默丢件）。
fn move_file_retry(from: &Path, to: &Path) -> Result<(), String> {
    if !from.exists() {
        return Ok(()); // 缺席 = 跳过（boundary/meta 是 lazy 创建）
    }
    let mut last = None;
    for _ in 0..5 {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    Err(format!(
        "rename {} -> {} 重试耗尽: {}",
        from.display(),
        to.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// chat.jsonl 非空行数（read_chat_log 同语义：空行跳过）。
fn count_nonempty_lines(path: &Path) -> u64 {
    match std::fs::File::open(path) {
        Ok(f) => std::io::BufReader::new(f)
            .lines()
            .map_while(Result::ok)
            .filter(|l| !l.trim().is_empty())
            .count() as u64,
        Err(_) => 0,
    }
}

// ---------------------------------------------------------------------------
// 写侧：移入回收站
// ---------------------------------------------------------------------------

/// 把会话 chat_log 三件套 + todo 文件移入回收站，返回条目目录名。
///
/// 四件全部缺席（从未物化的空会话）= 没有可恢复的东西 → 不建目录，返回
/// `Ok(None)`（删除即完成，handler 报 `trashed: false`）。清单 title 从
/// entry 内 meta 读、message_count 从 chat.jsonl 数、bytes 为四件套合计、
/// `todo` 标记 todo 副本在场。目录 mtime ≈ 删除时刻（此后内容不变），
/// sweeper 第六族按它 TTL 清扫。
pub fn trash_chat_log(session_key: &str, source: &str) -> Result<Option<String>, String> {
    trash_chat_log_in(&TrashLocations::production(), session_key, source)
}

/// [`trash_chat_log`] 的注入形态（测试用；语义逐字节同源）。
pub fn trash_chat_log_in(
    loc: &TrashLocations,
    session_key: &str,
    source: &str,
) -> Result<Option<String>, String> {
    let src_jsonl = loc.log_path(session_key);
    let src_meta = loc.meta_path(session_key);
    let src_boundary = loc.boundary_path(session_key);
    let src_todo = loc.todo_path(session_key);
    if !src_jsonl.exists() && !src_meta.exists() && !src_boundary.exists() && !src_todo.exists() {
        return Ok(None);
    }
    // todo 在场标记在移动前抓取（移动后 src_todo 已不存在）。
    let has_todo = src_todo.exists();

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let name = entry_dir_name(session_key, now_ms);
    // 同毫秒双删撞名防覆盖：ms 前缀在快机上不够区分（两次连续删除可落同一
    // 毫秒），create_dir 原子占位 + `-2`/`-3` 换槽位。绝不复用已有目录——
    // 复用会让第二次 move 覆掉第一删的 chat.jsonl（真丢件）。恢复链以
    // trash.json 清单为权威，条目名后缀只影响孤儿反推的展示，无损。
    std::fs::create_dir_all(loc.trash_root()).map_err(|e| format!("创建回收站根目录失败: {e}"))?;
    let entry = {
        let mut n = 1u32;
        loop {
            let candidate = if n == 1 {
                loc.trash_root().join(&name)
            } else {
                loc.trash_root().join(format!("{name}-{n}"))
            };
            match std::fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
                Err(e) => return Err(format!("创建回收站目录失败: {e}")),
            }
        }
    };

    let mut bytes = 0u64;
    for (from, to) in [
        (&src_jsonl, entry.join("chat.jsonl")),
        (&src_boundary, entry.join("boundary.jsonl")),
        (&src_meta, entry.join("meta.json")),
        // todo 副本（2026-11-05 四件套化）：move_file_retry 对缺席源自动
        // Ok 跳过——旧会话没有 todo 文件时本成员无副作用。
        (&src_todo, entry.join("todo.json")),
    ] {
        if from.exists() {
            bytes += std::fs::metadata(from).map(|m| m.len()).unwrap_or(0);
        }
        move_file_retry(from, &to)?;
    }

    // title 从（已移入 entry 的）meta 快照；meta 解析失败不阻塞移入。
    let title = std::fs::read_to_string(entry.join("meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<SessionMeta>(&raw).ok())
        .and_then(|m| m.title);

    let manifest = TrashManifest {
        original_session_key: session_key.to_string(),
        display_id: display_id_of(session_key),
        title,
        deleted_at_ms: now_ms,
        message_count: count_nonempty_lines(&entry.join("chat.jsonl")),
        bytes,
        source: source.to_string(),
        todo: has_todo,
    };
    let raw =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("清单序列化失败: {e}"))?;
    // 原子写（write_file_atomic）：trash.json 是回收站条目的可见性清单，
    // 半截 = 条目从列表静默消失（材料还在 entry 里但读不出清单）。
    nemesis_utils::file::write_file_atomic(
        &entry.join("trash.json").to_string_lossy(),
        raw.as_bytes(),
        0o644,
    )
    .map_err(|e| format!("清单写入失败: {e}"))?;

    // FTS 懒索引：jsonl 已离开 session_logs，摘除索引登记——同键复用（复合
    // 通道键同节点同 chat 重开真实可达）的增量插入不得以旧 DB 行为基线推
    // seq（与 delete_chat_log 的级联清理同族；见 history_search::invalidate_session）。
    #[cfg(feature = "history-search")]
    crate::history_search::invalidate_session(session_key);

    // 返回实际占位条目名：同毫秒双删时第二删落在 `name-2` 槽位，返回循环前
    // 的 `name` 会让调用方拿到第一删的名字——按名 purge/restore 会误指前删
    //（CI Linux 快机同毫秒实证，Windows 慢盘天然跨毫秒测不出来）。
    Ok(Some(
        entry
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(name),
    ))
}

// ---------------------------------------------------------------------------
// 读侧：列表
// ---------------------------------------------------------------------------

/// 列出回收站全部条目（新删在前）。清单缺席的孤儿目录诚实列出（`orphan:
/// true`，字段按目录名反推）。跳过非目录项；不因个别目录读取失败整体失败。
pub fn trash_list() -> Vec<serde_json::Value> {
    trash_list_in(&TrashLocations::production())
}

/// [`trash_list`] 的注入形态（测试用；语义逐字节同源）。
pub fn trash_list_in(loc: &TrashLocations) -> Vec<serde_json::Value> {
    let root = loc.trash_root();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out; // 根目录不存在 = 空回收站
    };
    for e in entries.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let manifest = std::fs::read_to_string(path.join("trash.json"))
            .ok()
            .and_then(|raw| serde_json::from_str::<TrashManifest>(&raw).ok());
        let (key, deleted_at_ms, orphan) = match &manifest {
            Some(m) => (m.original_session_key.clone(), m.deleted_at_ms, false),
            None => match derive_from_entry_name(name) {
                Some((ms, k)) => (k, ms, true),
                None => continue, // 目录名都不成形态：彻底不可解，跳过
            },
        };
        let (message_count, bytes) = match &manifest {
            Some(m) => (m.message_count, m.bytes),
            None => {
                let p = path.join("chat.jsonl");
                let b: u64 = ["chat.jsonl", "boundary.jsonl", "meta.json", "todo.json"]
                    .iter()
                    .map(|f| {
                        std::fs::metadata(path.join(f))
                            .map(|m| m.len())
                            .unwrap_or(0)
                    })
                    .sum();
                (count_nonempty_lines(&p), b)
            }
        };
        let title = match &manifest {
            Some(m) => m.title.clone(),
            None => std::fs::read_to_string(path.join("meta.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<SessionMeta>(&raw).ok())
                .and_then(|m| m.title),
        };
        let source = manifest.map(|m| m.source).unwrap_or_default();
        out.push(json!({
            "entry": name,
            "original_session_key": key,
            "display_id": display_id_of(&key),
            "title": title,
            "deleted_at_ms": deleted_at_ms,
            "message_count": message_count,
            "bytes": bytes,
            "source": if source.is_empty() { "unknown" } else { &source },
            "orphan": orphan,
        }));
    }
    out.sort_by(|a, b| {
        let ka = (
            a["deleted_at_ms"].as_u64().unwrap_or(0),
            a["entry"].as_str().unwrap_or_default(),
        );
        let kb = (
            b["deleted_at_ms"].as_u64().unwrap_or(0),
            b["entry"].as_str().unwrap_or_default(),
        );
        kb.cmp(&ka)
    });
    out
}

// ---------------------------------------------------------------------------
// 恢复 / 清除
// ---------------------------------------------------------------------------

/// 恢复事务标记文件名（复评）：条目目录内、首次 rename 前写入、
/// 成功随整目录删除消失；语义见 [`trash_restore_in`]。
const RESTORE_MARKER_FILE: &str = "restore-in-progress.json";

/// [`RESTORE_MARKER_FILE`] 的内容。只记目标键，不记已移动清单——续作按盘面
/// 事实对账（rename 同卷原子：源消失 = 目标在场），落盘清单反而多一个
/// 「rename 成功但清单未更新」的不一致窗口。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RestoreMarker {
    /// 本次恢复事务的目标键。
    target_key: String,
}

/// 恢复条目到原键（或 `new_sid` 改名后的新 web 键）。
///
/// 返回两态（都是 `Ok`，handler 按字段转前端）：
/// - `{"restored": <key>,...}` —— 移回成功（含 project_id/project_path/title
///   投影，handler 据此重挂项目归属或摘除陈旧字段）；
/// - `{"conflict": true, "suggested_sid":...}` —— 目标已存在（结构化冲突，
///   拒绝 + 提示；前端提示界面带 `new_sid` 重试即改名恢复）。
///
/// 改名恢复**仅限 web 会话键**（`agent:main:session:` 前缀）；复合通道键
/// （cluster B 端 `node/chat`）语义非裸 sid，诚实拒绝。孤儿目录（清单缺失）
/// 反推键是有损的：非 web 键拒绝恢复。
pub fn trash_restore(entry: &str, new_sid: Option<&str>) -> Result<serde_json::Value, String> {
    trash_restore_in(&TrashLocations::production(), entry, new_sid)
}

/// [`trash_restore`] 的注入形态（测试用；语义逐字节同源）。
pub fn trash_restore_in(
    loc: &TrashLocations,
    entry: &str,
    new_sid: Option<&str>,
) -> Result<serde_json::Value, String> {
    if !valid_entry_name(entry) {
        return Err(format!("非法条目名: {entry:?}"));
    }
    let entry_dir = loc.trash_root().join(entry);
    if !entry_dir.is_dir() {
        return Err(format!("回收站条目不存在: {entry}"));
    }

    let manifest = std::fs::read_to_string(entry_dir.join("trash.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<TrashManifest>(&raw).ok());
    let original_key = match &manifest {
        Some(m) => m.original_session_key.clone(),
        None => match derive_from_entry_name(entry) {
            Some((_, k)) if k.starts_with(WEB_SESSION_KEY_PREFIX) => k, // web 键反推无损
            Some((_, k)) => {
                return Err(format!(
                    "孤儿目录 {entry} 反推键 {k:?} 有损（复合通道键不支持孤儿恢复）"
                ));
            }
            None => return Err(format!("回收站条目不可解: {entry}")),
        },
    };

    // 决定目标键：原名 or 改名（web 键限定）。
    let (target_key, renamed) = match new_sid {
        None => (original_key.clone(), false),
        Some(sid) => {
            if !original_key.starts_with(WEB_SESSION_KEY_PREFIX) {
                return Err("复合通道键不支持改名恢复（键语义非裸 session id）".to_string());
            }
            let sanitized = SessionStore::sanitize_session_id(sid);
            if sanitized.is_empty() {
                return Err("新 session id 为空".to_string());
            }
            (format!("{WEB_SESSION_KEY_PREFIX}{sanitized}"), true)
        }
    };

    // 恢复事务标记（复评）：三段 rename 中途失败会把一部分
    // 材料留在目标键（chat 最先移、通常已过去）——不识别的话，重试的冲突
    // 检测把自己的半成品当成「别的会话占位」（conflict:true 永久卡死原名
    // 恢复），用户改道改名恢复更会把剩余材料移到另一个键（chat 留原键、
    // boundary/meta 去新键 = 会话分裂还报成功）。标记在首次 rename 前写入，
    // 成功随整目录删除一起消失；失败保留条目 + 标记 = 可重试。续作三态：
    // - 同目标 = 上次半途失败的重试：目标上的占用是本事务自己的半成品，
    //   跳过冲突检查续作；
    // - 换目标 = 改道：旧目标已有任何一件材料 = 已分裂，改道必然造出会话
    //   分裂，诚实拒绝；旧目标干净 = 上次死在第一个 rename 前，允许改道
    //   （标记下面覆写）；
    // - 无标记 = 全新恢复。标记在但读不出（半截 JSON）同此：标记只在移动
    //   开始前写入（换目标覆写同样先于移动且已核实旧目标干净），写坏 = 死
    //   在移动前、材料全在条目内——按全新处理，覆写坏标记。
    //   （不变量：标记写入永远不进移动循环。）
    let marker_path = entry_dir.join(RESTORE_MARKER_FILE);
    let marker: Option<RestoreMarker> = std::fs::read_to_string(&marker_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok());
    let resuming = match &marker {
        Some(m) if m.target_key == target_key => true,
        Some(m) => {
            let split = [
                loc.log_path(&m.target_key),
                loc.boundary_path(&m.target_key),
                loc.meta_path(&m.target_key),
            ]
            .iter()
            .any(|p| p.exists());
            if split {
                return Err(format!(
                    "条目 {entry} 的恢复曾在目标 {} 中途失败（材料已分裂到该键），\
                     换键恢复会造出会话分裂——请用与上次相同的目标重试完成",
                    m.target_key
                ));
            }
            false
        }
        None => false,
    };

    // 冲突检测：目标 jsonl 或 meta 已存在 = 该会话已物化（meta 单独存在 =
    // 建了但没消息的会话，同样拦截）。boundary 是审计 sidecar，clear 截断
    // 保留文件，不参与冲突判定（恢复覆盖之）。续作路径跳过——目标上的占用
    // 是本事务自己的半成品。
    let conflict =
        !resuming && (loc.log_path(&target_key).exists() || loc.meta_path(&target_key).exists());
    if conflict {
        let display_sid = display_id_of(&target_key);
        let ms = manifest
            .as_ref()
            .map(|m| m.deleted_at_ms)
            .unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            });
        return Ok(json!({
            "conflict": true,
            "entry": entry,
            "original_session_key": original_key,
            "target_session_key": target_key,
            "suggested_sid": format!("{}-restored-{ms}", display_sid),
            "renamed": renamed,
        }));
    }

    // 项目归属/title 投影在移走 meta 前抓取（meta.json 马上要 rename）；
    // 续作时 meta 可能已被上次移到目标键——回落读目标键的（同为本事务移入）。
    let meta = std::fs::read_to_string(entry_dir.join("meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<SessionMeta>(&raw).ok())
        .or_else(|| {
            std::fs::read_to_string(loc.meta_path(&target_key))
                .ok()
                .and_then(|raw| serde_json::from_str::<SessionMeta>(&raw).ok())
        });
    let (project_id, project_path, title) = match &meta {
        Some(m) => (
            m.project_id.clone(),
            m.project_path.clone(),
            m.title.clone(),
        ),
        None => (None, None, manifest.as_ref().and_then(|m| m.title.clone())),
    };

    // 写事务标记（首次恢复 / 改道覆写坏标记），先于任何 rename。
    if !resuming {
        let raw = serde_json::to_string(&RestoreMarker {
            target_key: target_key.clone(),
        })
        .map_err(|e| format!("恢复标记序列化失败: {e}"))?;
        // 原子写（write_file_atomic）：与 trash.json 同面——标记是恢复
        // 事务锚点，读侧虽兜「读不出=全新恢复+覆写」（上方 561-563 注释），
        // 原子化把「半截」形态从窗口里直接消除，协议不依赖时序巧合。
        nemesis_utils::file::write_file_atomic(
            &marker_path.to_string_lossy(),
            raw.as_bytes(),
            0o644,
        )
        .map_err(|e| format!("恢复标记写入失败: {e}"))?;
    }

    // 移回材料（boundary/meta/todo 缺席 = 跳过；续作时已移走的同样按缺席
    // 跳过）。todo 副本只在条目内存在时追加（2026-11-05 四件套化；旧条目
    // 无此文件不追加，expected 完成性验证对「两侧都缺席」成员本就不参与）。
    let mut restore_pairs: Vec<(&str, PathBuf, PathBuf)> = vec![
        (
            "chat.jsonl",
            entry_dir.join("chat.jsonl"),
            loc.log_path(&target_key),
        ),
        (
            "boundary.jsonl",
            entry_dir.join("boundary.jsonl"),
            loc.boundary_path(&target_key),
        ),
        (
            "meta.json",
            entry_dir.join("meta.json"),
            loc.meta_path(&target_key),
        ),
    ];
    let todo_src = entry_dir.join("todo.json");
    if todo_src.exists() {
        // 目标键 live todo 先清：它只可能是已删会话的孤儿残留（BUG① 形态，
        // 2026-11-05 之前旧条目恢复时必撞）或本事务上次半途已移走的半成品
        //（后者 todo_src 已不在条目内，走不进本分支）。restore 语义 = 条目
        // 内容为准（rename 两平台都会覆盖已存在目标——显式清是语义声明与
        // 失败面收窄，不是防覆盖）。todo 不参与冲突判定（meta/jsonl 才是
        // 会话存在性权威）。
        let live = loc.todo_path(&target_key);
        if live.exists() {
            nemesis_utils::fsutil::remove_file_retry(&live)
                .map_err(|e| format!("清理目标键 live todo 失败: {e}"))?;
        }
        restore_pairs.push(("todo.json", todo_src, loc.todo_path(&target_key)));
    }
    // 完成性验证基线：移动前「条目内有 或 目标键已有（续作半成品）」的成员
    // = 期望集合；两侧都缺席 = 本来就 lazy 不存在，不参与验证。
    let expected: Vec<usize> = restore_pairs
        .iter()
        .enumerate()
        .filter(|(_, (_, src, dst))| src.exists() || dst.exists())
        .map(|(i, _)| i)
        .collect();
    for (_, from, to) in &restore_pairs {
        move_file_retry(from, to)?;
    }
    // 完成性验证：期望集合逐一核实在目标键——rename 同卷原子（源消失
    // = 目标在场），缺位 = 语义被破坏，诚实 Err 不报假成功（条目 + 标记保留
    // 可重试）。
    for i in &expected {
        let (name, _, dst) = &restore_pairs[*i];
        if !dst.exists() {
            return Err(format!(
                "恢复完成性校验失败: {name} 未落到目标位置（条目保留，可重试）"
            ));
        }
    }
    // 残留（trash.json / 恢复标记）整目录清掉——标记随之消失，此后同键再删
    // 是全新事务。
    nemesis_utils::fsutil::remove_dir_all_retry(&entry_dir)
        .map_err(|e| format!("清理回收站条目失败: {e}"))?;

    Ok(json!({
        "restored": target_key,
        "display_id": display_id_of(&target_key),
        "renamed": renamed,
        "project_id": project_id,
        "project_path": project_path,
        "title": title,
    }))
}

/// 彻底删除单个条目（整目录物理删除，不可恢复）。
///
/// 2026-11-05 四件套化：删除条目前顺带清原键的 live todo——回收站语义 =
/// 该会话已删，live todo 是 BUG① 的孤儿形态（2026-11-05 之前的旧条目 +
/// 残留），purge 是最后一次清扫机会。todo 清理失败 = 诚实 Err（条目保留
/// 可重试），不删条目装成功。
pub fn trash_purge(entry: &str) -> Result<(), String> {
    trash_purge_in(&TrashLocations::production(), entry)
}

/// [`trash_purge`] 的注入形态（测试用；语义逐字节同源）。
pub fn trash_purge_in(loc: &TrashLocations, entry: &str) -> Result<(), String> {
    if !valid_entry_name(entry) {
        return Err(format!("非法条目名: {entry:?}"));
    }
    let entry_dir = loc.trash_root().join(entry);
    if !entry_dir.is_dir() {
        return Err(format!("回收站条目不存在: {entry}"));
    }
    // 清 live todo：清单在 = 原键可寻址（不按 manifest.todo 闸——旧形态清单
    // 无 todo 字段但 live 残留同样要清）。**同键复用守卫（BUG-20261112-100）**：
    // 「键是 uuid，误清新会话不可能」的前提不成立——复合通道键同节点同 chat
    // 重开真实可达（BUG-20261107-024 同键复用实证），import 亦接受自定义
    // sid。目标键已有在世会话（jsonl/meta 在场，与 trash_restore 冲突检测
    // 同判据）= live todo 归属复用后的**新会话**，purge 旧条目不得清它；
    // 键无在世会话 = 旧纪元孤儿残留，照旧收口。
    let manifest = std::fs::read_to_string(entry_dir.join("trash.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<TrashManifest>(&raw).ok());
    if let Some(m) = &manifest {
        let original_key = &m.original_session_key;
        let reincarnated =
            loc.log_path(original_key).exists() || loc.meta_path(original_key).exists();
        if !reincarnated {
            let live_todo = loc.todo_path(original_key);
            if live_todo.exists() {
                nemesis_utils::fsutil::remove_file_retry(&live_todo)
                    .map_err(|e| format!("清理 live todo 失败（条目保留可重试）: {e}"))?;
            }
        }
    }
    nemesis_utils::fsutil::remove_dir_all_retry(&entry_dir)
        .map_err(|e| format!("删除回收站条目失败: {e}"))
}

/// 清空回收站，返回清除条目数。个别失败不中断其余（失败明细上浮）。
pub fn trash_purge_all() -> Result<usize, String> {
    trash_purge_all_in(&TrashLocations::production())
}

/// [`trash_purge_all`] 的注入形态（测试用；语义逐字节同源）。逐条走
/// [`trash_purge_in`]（2026-11-05：live todo 清扫单一收敛点；条目名来自
/// read_dir 不含路径分隔符，valid_entry_name 校验天然通过）。
pub fn trash_purge_all_in(loc: &TrashLocations) -> Result<usize, String> {
    let root = loc.trash_root();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Ok(0); // 根目录不存在 = 无事可清
    };
    let mut purged = 0usize;
    let mut failures = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };
        match trash_purge_in(loc, &name) {
            Ok(()) => purged += 1,
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    if failures.is_empty() {
        Ok(purged)
    } else {
        Err(format!(
            "{} 个条目清除失败: {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}

#[cfg(test)]
mod trash_tests;
