//! 会话回收站（2026-10-30）：`sessions.delete` 默认从硬删除改为「移入回收站」。
//!
//! 存储真相源 = `<workspace>/logs/trash/sessions/{deleted_at_ms}_{safe_key}/`
//! （路径真相源 [`nemesis_path::resolve_trash_sessions_dir_in_workspace`]），
//! 每个子目录装原 chat_log 三件套的改名副本（`chat.jsonl` / `boundary.jsonl` /
//! `meta.json`）+ `trash.json` 清单。设计要点：
//!
//! - **移入 = rename**：同卷 rename 是元数据操作，三件套字节原样；同键多次
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
//! 冲突语义（拍板②）：恢复目标三件套（jsonl 或 meta）已存在 = 结构化
//! conflict 返回（调用方转提示）；web 键允许带 `new_sid` 改名恢复（新 sid 经
//! [`SessionStore::sanitize_session_id`] 消毒后重验），复合键改名不支持。
//!
//! # 寻址注入
//!
//! 核心函数全部走 [`TrashLocations`]（chat_log 三件套寻址 + 回收站根）。
//! 生产 = [`TrashLocations::production`]（进程单例 path manager，文件名格式
//! 由 chat_log 同源闭包决定）；测试一律 [`TrashLocations::with_dirs`] 注入
//! tempdir——进程单例 home 不可 per-test 重定向（OnceLock 固化，cov 系同款
//! 约束），而真实 home 是在用数据目录，测试写它 = 污染。惯例先例：
//! `nemesis-plugins-wasm::PluginManager::new_with_dirs`。

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
    /// 三件套字节合计。
    #[serde(default)]
    pub bytes: u64,
    /// 来源标记：`single`（单条删除）/ `batch`（批量删除）。
    #[serde(default)]
    pub source: String,
}

// ---------------------------------------------------------------------------
// 寻址注入面
// ---------------------------------------------------------------------------

/// 回收站寻址注入面：chat_log 三件套寻址 + 回收站根。
///
/// 生产走 [`TrashLocations::production`]（与 chat_log 同源寻址，路径真相源
/// 不搬出 chat_log）；测试走 [`TrashLocations::with_dirs`] 注入 tempdir。
#[derive(Clone)]
pub struct TrashLocations {
    trash_root: PathBuf,
    #[allow(clippy::type_complexity)]
    log_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
    #[allow(clippy::type_complexity)]
    meta_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
    #[allow(clippy::type_complexity)]
    boundary_path: Arc<dyn Fn(&str) -> PathBuf + Send + Sync>,
}

impl TrashLocations {
    /// 生产寻址：chat_log 同源函数 + 进程单例回收站根（与 chat_log 同一
    /// 寻址惯例）。
    pub(crate) fn production() -> Self {
        Self {
            trash_root: trash_root(),
            log_path: Arc::new(chat_log::log_path),
            meta_path: Arc::new(chat_log::meta_path),
            boundary_path: Arc::new(chat_log::boundary_path),
        }
    }

    /// 测试注入：回收站根 + session_logs 目录 + boundary 目录。文件名格式与
    /// chat_log 同源（`{safe}.jsonl` / `{safe}.meta.json`，
    /// `safe = sanitize_path_segment`；chat_log.rs::log_path 注释即契约）。
    pub fn with_dirs(
        trash_root: PathBuf,
        session_logs_dir: PathBuf,
        boundary_dir: PathBuf,
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
/// 异常）。目标已存在时 Windows rename 也会失败——恢复冲突检测靠前置检查，
/// 不靠 rename 竞态裁决。重试耗尽诚实 Err（调用方上浮，不静默丢件）。
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

/// 把会话 chat_log 三件套移入回收站，返回条目目录名。
///
/// 三件套全部缺席（从未物化的空会话）= 没有可恢复的东西 → 不建目录，返回
/// `Ok(None)`（删除即完成，handler 报 `trashed: false`）。清单 title 从
/// entry 内 meta 读、message_count 从 chat.jsonl 数、bytes 为三件套合计。
/// 目录 mtime ≈ 删除时刻（此后内容不变），sweeper 第六族按它 TTL 清扫。
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
    if !src_jsonl.exists() && !src_meta.exists() && !src_boundary.exists() {
        return Ok(None);
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let name = entry_dir_name(session_key, now_ms);
    let entry = loc.trash_root().join(&name);
    std::fs::create_dir_all(&entry).map_err(|e| format!("创建回收站目录失败: {e}"))?;

    let mut bytes = 0u64;
    for (from, to) in [
        (&src_jsonl, entry.join("chat.jsonl")),
        (&src_boundary, entry.join("boundary.jsonl")),
        (&src_meta, entry.join("meta.json")),
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
    };
    let raw =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("清单序列化失败: {e}"))?;
    std::fs::write(entry.join("trash.json"), raw).map_err(|e| format!("清单写入失败: {e}"))?;

    Ok(Some(name))
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
                let b: u64 = ["chat.jsonl", "boundary.jsonl", "meta.json"]
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

/// 恢复条目到原键（或 `new_sid` 改名后的新 web 键）。
///
/// 返回两态（都是 `Ok`，handler 按字段转前端）：
/// - `{"restored": <key>, ...}` —— 移回成功（含 project_id/project_path/title
///   投影，handler 据此重挂项目归属或摘除陈旧字段）；
/// - `{"conflict": true, "suggested_sid": ...}` —— 目标已存在（结构化冲突，
///   拍板②：拒绝 + 提示；前端提示界面带 `new_sid` 重试即改名恢复）。
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

    // 冲突检测：目标 jsonl 或 meta 已存在 = 该会话已物化（meta 单独存在 =
    // 建了但没消息的会话，同样拦截）。boundary 是审计 sidecar，clear 截断
    // 保留文件，不参与冲突判定（恢复覆盖之）。
    let conflict = loc.log_path(&target_key).exists() || loc.meta_path(&target_key).exists();
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

    // 项目归属/title 投影在移走 meta 前抓取（meta.json 马上要 rename）。
    let meta = std::fs::read_to_string(entry_dir.join("meta.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<SessionMeta>(&raw).ok());
    let (project_id, project_path, title) = match &meta {
        Some(m) => (
            m.project_id.clone(),
            m.project_path.clone(),
            m.title.clone(),
        ),
        None => (None, None, manifest.as_ref().and_then(|m| m.title.clone())),
    };

    // 移回三件套（boundary/meta 缺席 = 跳过）。
    for (from, to) in [
        (entry_dir.join("chat.jsonl"), loc.log_path(&target_key)),
        (
            entry_dir.join("boundary.jsonl"),
            loc.boundary_path(&target_key),
        ),
        (entry_dir.join("meta.json"), loc.meta_path(&target_key)),
    ] {
        move_file_retry(&from, &to)?;
    }
    // 残留（trash.json 等）整目录清掉。
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
    nemesis_utils::fsutil::remove_dir_all_retry(&entry_dir)
        .map_err(|e| format!("删除回收站条目失败: {e}"))
}

/// 清空回收站，返回清除条目数。个别失败不中断其余（失败明细上浮）。
pub fn trash_purge_all() -> Result<usize, String> {
    trash_purge_all_in(&TrashLocations::production())
}

/// [`trash_purge_all`] 的注入形态（测试用；语义逐字节同源）。
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
        match nemesis_utils::fsutil::remove_dir_all_retry(&path) {
            Ok(()) => purged += 1,
            Err(e) => failures.push(format!(
                "{}: {e}",
                path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
            )),
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
