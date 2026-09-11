//! Chat log module — append-only JSONL log for user-facing chat history.
//!
//! Session files (`sessions/`) serve LLM context recovery (summarization,
//! truncation). This module provides a separate, append-only log that never
//! gets truncated, ensuring the user-facing chat history is always complete.

use chrono::Local;
use nemesis_path::default_path_manager;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::r#loop::FileChange;

/// D3（devtool-upgrade 阶段 5）：chat_log 条目在 `role`/`content` 之外的
/// 可选元数据。jsonl 行 shape 的单一真相源仍是 [`write_chat_entry`]；本
/// struct 是**写入 API** 的单一入口——扩字段只动这里，不再加位置参数
/// （`append_chat_log_full*` 旧签名保留为薄包装，存量调用点不动）。
#[derive(Debug, Clone, Copy, Default)]
pub struct ChatLogMeta<'a> {
    /// `"model"` 字段（assistant 行的「供应商·模型名」徽标）。`None` 不写。
    pub model: Option<&'a str>,
    /// `"cron_job_id"` 字段（定时任务来源标记）。`None` 不写。
    pub cron_job_id: Option<&'a str>,
    /// `"cron_job_name"` 字段。`None` 不写。
    pub cron_job_name: Option<&'a str>,
    /// `"images"` 字段（图片**路径引用**，不落字节）。空切片不写。
    pub images: &'a [String],
    /// D3：`"file_changes"` 字段——本 turn 声明式文件工具的变更清单
    /// （`[{path, kind}]`；AgentLoop 在 dispatch 瀑布经 `preview_all`
    /// 收集，assistant 最终回复落盘时去重随行）。消息↔文件变更映射；
    /// M3 会话级 diff 查看器的数据源。空切片不写——旧条目/无变更解析
    /// 不受影响。
    pub file_changes: &'a [FileChange],
    /// E3（devtool-upgrade 阶段 5）：`"checkpoint_turn"` 字段——本行所属
    /// turn 的 checkpoint 序号（`turn_preamble` begin 的值随 admission 穿针
    /// 到行落盘点）。E3 消息级回退用它把 jsonl 行精确定位回 checkpoint
    /// turn（不靠内容/时间戳猜测）。`None` 不写——无 checkpoint store 的
    /// 行回退时只截断对话不回滚文件。
    pub checkpoint_turn: Option<usize>,
}

/// D3：全字段追加入口（`ChatLogMeta` 携带全部可选元数据）。
pub fn append_chat_log_meta(session_key: &str, role: &str, content: &str, meta: &ChatLogMeta<'_>) {
    write_chat_entry(
        session_key,
        role,
        content,
        meta.model,
        meta.cron_job_id,
        meta.cron_job_name,
        meta.images,
        meta.file_changes,
        meta.checkpoint_turn,
    );
}

/// D3：把本 turn 的 FileChange 流水按 `path` 去重——保留首次出现顺序，
/// `kind` 取**最后一次**声明（同文件先 Create 后 Modify，对消息级展示的
/// 语义就是「该文件被动过」；Create/Delete 的恢复语义由 checkpoint 保留，
/// 不受此投影影响）。AgentLoop drain 时调用（单一投影点，M3 依赖此形状）。
pub fn dedup_file_changes(changes: Vec<FileChange>) -> Vec<FileChange> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out: Vec<FileChange> = Vec::with_capacity(changes.len());
    for c in changes {
        match seen.get(&c.path) {
            Some(&i) => out[i].kind = c.kind,
            None => {
                seen.insert(c.path.clone(), out.len());
                out.push(c);
            }
        }
    }
    out
}

/// Append a chat message to the JSONL log file.
pub fn append_chat_log(session_key: &str, role: &str, content: &str) {
    append_chat_log_full(session_key, role, content, None, None, None);
}

/// Append a chat message with an optional model badge (`provider/name`).
///
/// When `model` is `Some`, an extra `"model"` field is written so the
/// Dashboard can render a "供应商·模型名" badge on the assistant message after
/// a history reload. `None` (user rows, legacy callers) omits the field — old
/// jsonl entries without it parse fine (read side treats missing = no badge).
pub fn append_chat_log_with_model(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
) {
    append_chat_log_full(session_key, role, content, model, None, None);
}

/// Full append: optional model badge AND optional cron origin marker.
///
/// `cron_job_id` / `cron_job_name`: when `Some`, marks this entry as
/// originating from a scheduled (cron) task, so the Dashboard can label it
/// (🕒) and filter "只看定时任务" in the session browser. `None` (the common
/// case) omits the fields — old jsonl entries without them parse fine.
pub fn append_chat_log_full(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
) {
    write_chat_entry(
        session_key,
        role,
        content,
        model,
        cron_job_id,
        cron_job_name,
        &[],
        &[],
        None,
    );
}

/// T6（多模态）：带图片路径引用的追加变体（`append_chat_log_full` 的签名
/// 不动，媒体信息走此函数）。`images` 非空时写入 `"images": [路径...]`
/// 字段——只存**路径引用**，不落 base64 字节（与 SessionStore 的
/// `StoredMessage.image_refs` 同源同语义）；读侧缺字段 = 无图，旧条目解析
/// 不受影响。
#[allow(clippy::too_many_arguments)]
pub fn append_chat_log_full_with_images(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
    images: &[String],
) {
    write_chat_entry(
        session_key,
        role,
        content,
        model,
        cron_job_id,
        cron_job_name,
        images,
        &[],
        None,
    );
}

/// Write core shared by all append variants (single source of truth for the
/// jsonl entry shape). `images` non-empty → extra `"images"` array field.
/// `file_changes` non-empty → extra `"file_changes"` array field (D3).
/// `checkpoint_turn` Some → extra `"checkpoint_turn"` numeric field (E3).
#[allow(clippy::too_many_arguments)]
fn write_chat_entry(
    session_key: &str,
    role: &str,
    content: &str,
    model: Option<&str>,
    cron_job_id: Option<&str>,
    cron_job_name: Option<&str>,
    images: &[String],
    file_changes: &[FileChange],
    checkpoint_turn: Option<usize>,
) {
    let path = log_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("[chat_log] Failed to open {}: {}", path.display(), e);
            return;
        }
    };
    let mut entry = serde_json::json!({
        "role": role,
        "content": content,
        "timestamp": Local::now().to_rfc3339(),
    });
    if let Some(m) = model {
        entry["model"] = serde_json::Value::String(m.to_string());
    }
    if let Some(id) = cron_job_id {
        entry["cron_job_id"] = serde_json::Value::String(id.to_string());
    }
    if let Some(name) = cron_job_name {
        entry["cron_job_name"] = serde_json::Value::String(name.to_string());
    }
    if !images.is_empty() {
        entry["images"] = serde_json::Value::Array(
            images
                .iter()
                .map(|p| serde_json::Value::String(p.clone()))
                .collect(),
        );
    }
    // D3：本 turn 声明式文件工具变更（`[{path, kind}]`）。缺字段 = 旧条目
    // /无变更，读侧解析不受影响（与 model/cron/images 同一宽容读法）。
    if !file_changes.is_empty() {
        entry["file_changes"] = serde_json::to_value(file_changes).unwrap_or(Value::Null);
    }
    // E3：本行所属 turn 的 checkpoint 序号（消息级回退的行→turn 定位锚）。
    // 缺字段 = 无 store 时代的行，回退只截断对话不回滚文件。
    if let Some(t) = checkpoint_turn {
        entry["checkpoint_turn"] = serde_json::Value::from(t);
    }
    if let Err(e) = writeln!(file, "{}", entry) {
        tracing::warn!("[chat_log] Failed to write to {}: {}", path.display(), e);
        return;
    }
    // U20 (sixth batch): lazy FTS index hook — best-effort, failures inside
    // are swallowed (the next full reindex repairs). Timestamp mirrors the
    // entry written above.
    let ts = entry
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    crate::history_search::index_append(session_key, role, content, ts);
}

/// Read chat log with pagination.
///
/// Returns `(page, total_count, has_more, oldest_index)`. `before_index` is the
/// exclusive upper bound — "give me items before this index". `None` means the
/// newest batch. Messages are returned in chronological order (oldest first).
///
/// Uses two-pass approach: first counts lines, then only deserializes the needed
/// range. Avoids loading the entire file into memory.
pub fn read_chat_log(
    session_key: &str,
    limit: usize,
    before_index: Option<usize>,
) -> (Vec<Value>, usize, bool, usize) {
    let path = log_path(session_key);
    // is_file 而非 exists：Linux 上 File::open 对目录成功（O_RDONLY 合法），
    // 而下面 BufReader::lines() 的 count/filter_map 在 read Err（目录 fd 每次
    // read 都 EISDIR，Lines 迭代器不熔断）上会无限自旋。目录占位按缺失处理
    // （Windows 上 open 目录本就失败走同臂）。（2026-09-01 Linux 首跑暴露）
    if !path.is_file() {
        return (Vec::new(), 0, false, 0);
    }

    // Pass 1: Count lines (no deserialization).
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0, false, 0),
    };
    let total = std::io::BufReader::new(file).lines().count();
    if total == 0 {
        return (Vec::new(), 0, false, 0);
    }

    let end = before_index.map(|bi| bi.min(total)).unwrap_or(total);
    let start = end.saturating_sub(limit);

    // Pass 2: Read only lines in [start, end), skip the rest.
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return (Vec::new(), 0, false, 0),
    };
    let page: Vec<Value> = std::io::BufReader::new(file)
        .lines()
        .skip(start)
        .take(end - start)
        .filter_map(|l| l.ok())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        // ROUND-5 FIX: boundary events moved to a sidecar file (see
        // `boundary_path`), so the message jsonl can no longer contain them.
        // The filter stays as a one-line guard for dev machines that ran a
        // batch-3 build with interleaved boundaries — cheap and harmless.
        .filter(|v| v.get("role").and_then(|r| r.as_str()) != Some("boundary"))
        .collect();

    (page, total, start > 0, start)
}

/// Read ONLY the boundary (audit) events of a session — replay/audit tooling
/// reads through here. Round-5 fix: these live in the sidecar file
/// (`logs/boundary/<safe_key>.jsonl`), not the message jsonl.
pub fn read_boundary_events(session_key: &str) -> Vec<Value> {
    let path = boundary_path(session_key);
    if !path.exists() {
        return Vec::new();
    }
    let file = match File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .collect()
}

/// Resolve the JSONL file path for a session key.
fn log_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key))
}

/// Does this session have a chat_log jsonl on disk? (2026-08-25 fork 第三轮)
/// `session_fork::unique_key` consults this so a fork never APPENDS onto a
/// previous fork's surviving jsonl after its store json aged out of the
/// 7-day TTL (append would duplicate the whole prefix).
pub fn chat_log_exists(session_key: &str) -> bool {
    log_path(session_key).exists()
}

/// Write pre-read chat_log rows under `new_key`, VERBATIM (2026-08-25 fork
/// 第三轮). Each row is a complete jsonl `Value` — original timestamps,
/// model badges, cron markers, everything preserved byte-faithfully. This is
/// the fork's jsonl side: the fork dialog counts turns ON these rows, so the
/// copy must be exactly the rows the user picked, not a re-derived
/// projection. Returns the number of lines written.
pub fn write_chat_log_rows(new_key: &str, rows: &[Value]) -> usize {
    if rows.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut written = 0usize;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for v in rows {
            let Ok(line) = serde_json::to_string(v) else {
                continue;
            };
            if writeln!(f, "{}", line).is_ok() {
                written += 1;
            }
        }
    }
    written
}

/// E3（devtool-upgrade 阶段 5）：把会话 jsonl **原位截断**为 `kept` 里
/// 的行（VERBATIM 逐字节保留）。消息级回退/重做（`AgentLoop::rewind_to_message`
/// / `redo_rewind`）的落盘原语。
///
/// 崩溃安全：先写 `.rewinding` 临时文件再 rename 覆盖原文件——中途崩溃
/// 要么原文件完好、要么截断后完好，不会出现半截文件。与 fork 的
/// [`write_chat_log_rows`] 同一宽容边界：不走 `history_search::index_append`
/// （原样重写不加新词），FTS 懒索引下次全量重建时自愈。
///
/// 返回写入行数。`kept` 为空 = 清空文件（保留文件本身，会话仍可用）。
pub fn truncate_chat_log_rows(session_key: &str, kept: &[Value]) -> usize {
    let path = log_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("jsonl.rewinding");
    let mut written = 0usize;
    {
        let Ok(mut f) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
        else {
            tracing::warn!(
                "[chat_log] Failed to open tmp {} for truncate",
                tmp.display()
            );
            return 0;
        };
        for v in kept {
            let Ok(line) = serde_json::to_string(v) else {
                continue;
            };
            if writeln!(f, "{}", line).is_ok() {
                written += 1;
            }
        }
    }
    // Windows 上 std::fs::rename 走 MOVEFILE_REPLACE_EXISTING，可覆盖已存在
    // 目标；失败时保留原文件（会话不损）并清掉 tmp。
    if let Err(e) = fs::rename(&tmp, &path) {
        tracing::warn!(
            "[chat_log] truncate rename failed ({}): {} — 原文件保留",
            path.display(),
            e
        );
        let _ = fs::remove_file(&tmp);
        return 0;
    }
    written
}

/// Z1 (Phase4-d): copy the first `at_turn` COMPLETE user turns of
/// `source_key`'s chat log to `new_key`, lines VERBATIM (original
/// timestamps and extra fields preserved — a fork must not re-stamp
/// history). Uses the same user-turn counting as the SessionStore-side
/// fork cut, so the Dashboard log and the model-context store stay aligned.
/// Returns the number of lines copied. The lazy FTS full-index picks the
/// new file up on first search; no per-line index_append needed here.
///
/// ⚠ SUPERSEDED TWICE — do not re-enable:
/// - Round 2 (2026-08-25 上午, fork 内容错位第一修): disabled in favor of
///   `write_chat_log_from_store` on the then-belief that SessionStore was
///   the single source of truth for turn semantics (see that note below).
/// - Round 3 (2026-08-25 深夜): the self-heal fix made jsonl the single
///   source of truth (store = rebuildable cache that compaction folds and
///   TTL deletes); the round-2 assumption was backwards, and forking off
///   the store produced garbage on real production sessions (store held a
///   truncated, tool-intermediate-polluted history while the user picked a
///   turn by the clean jsonl the UI renders). `fork_session` now reads the
///   rows itself and writes them via `write_chat_log_rows`.
///   Kept (not deleted) per the code-change discipline.
#[allow(dead_code)]
pub fn copy_chat_log_prefix(source_key: &str, new_key: &str, at_turn: usize) -> usize {
    // Whole-log read: fork is a one-shot admin op, not a hot path.
    let (all, _total, _more, _oldest) = read_chat_log(source_key, usize::MAX, None);
    let mut turns = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for v in all {
        if v.get("role").and_then(|r| r.as_str()) == Some("user") {
            turns += 1;
            if turns > at_turn {
                break;
            }
        }
        if let Ok(line) = serde_json::to_string(&v) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for l in &lines {
            let _ = writeln!(f, "{}", l);
        }
    }
    lines.len()
}

/// FIX (2026-08-25): single source of truth for "which rows become visible
/// chat rows" — used by the self-heal rebuild / fork store mapping
/// (`session::projected_messages_from_rows`) AND the turns endpoint's
/// `end_preview` computation (api_handlers). The call sites must never
/// drift apart, or the fork dialog's "分叉末条" preview would disagree with
/// what the fork actually ends on — the exact class of bug this round
/// fixed. Tool/system rows never project (chat_log is the UI bubble
/// source); an `assistant` row with empty/whitespace content is a pure
/// tool_calls intermediate, not a displayable reply.
pub fn is_projected_chat_row(role: &str, content: &str) -> bool {
    match role {
        "user" => true,
        "assistant" => !content.trim().is_empty(),
        _ => false,
    }
}

/// FIX (2026-08-25 分叉内容错位, round 2): generate `new_key`'s chat_log by
/// PROJECTING a SessionStore history prefix — the replacement for
/// `copy_chat_log_prefix` (see its ⚠ SUPERSEDED note for the divergence
/// bug this fixes).
///
/// ⚠ SUPERSEDED (2026-08-25 round 3, fork 第三轮): this function's premise —
/// "SessionStore is the single source of truth for turn semantics" — was
/// inverted by the self-heal fix later the same day: jsonl is the truth,
/// the store is a lossy, compaction-folded, TTL-deleted cache. Projecting
/// the fork's chat_log FROM the store made the fork inherit every store
/// defect (truncated history, tool-intermediate pollution, folded turns),
/// producing garbage forks on real production sessions. `fork_session` now
/// copies the jsonl rows verbatim (`write_chat_log_rows`) and derives the
/// store FROM those rows (`session::projected_messages_from_rows`) — the
/// store→jsonl direction is dead. Kept (not deleted) per the code-change
/// discipline.
///
/// Projection semantics (historical, for when this was live):
/// - only `user` / `assistant` rows are written (chat_log is the UI bubble
///   source; tool/system rows were never logged there);
/// - `assistant` rows with empty/whitespace content are skipped — those are
///   pure tool_calls intermediate messages; the final per-turn reply has
///   content. (A non-empty intermediate reply is real model output and is
///   kept — honest content beats byte-parity with a source log that may
///   itself be stale.)
/// - timestamps come from the stored messages (a fork must not re-stamp
///   history);
/// - model badge / cron markers are NOT carried over (the store does not
///   record them; a missing badge degrades to "no badge" on the read side,
///   which parses fine — an acceptable display-only cost for guaranteed
///   store↔log alignment).
///
/// By construction the new session's two stores agree: the Dashboard's
/// last displayed message == the last user/assistant message of
/// `messages`. The lazy FTS full-index picks the new file up on first
/// search; no per-line index_append needed here (same as the old copy).
/// Returns the number of lines written.
#[allow(dead_code)]
pub fn write_chat_log_from_store(
    new_key: &str,
    messages: &[crate::session::StoredMessage],
) -> usize {
    let mut lines: Vec<String> = Vec::new();
    for m in messages {
        if !is_projected_chat_row(&m.role, &m.content) {
            continue;
        }
        let entry = serde_json::json!({
            "role": m.role,
            "content": m.content,
            "timestamp": m.timestamp,
        });
        if let Ok(line) = serde_json::to_string(&entry) {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return 0;
    }
    let target = log_path(new_key);
    if let Some(parent) = target.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut written = 0usize;
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&target) {
        for l in &lines {
            if writeln!(f, "{}", l).is_ok() {
                written += 1;
            }
        }
    }
    written
}

/// Delete a session's chat log file (JSONL). Used by session management
/// (delete conversation) to clear the user-facing history. Also deletes the
/// boundary-events sidecar (so a re-created session doesn't inherit stale
/// audit rows) and the title meta sidecar (2026-08-25: it used to survive as
/// an orphan — invisible to `sessions.list` which scans jsonl only, but dead
/// bytes on disk; `clear` deliberately KEEPS the meta because the
/// conversation stays alive with its title). No-op if absent.
pub fn delete_chat_log(session_key: &str) {
    let path = log_path(session_key);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("[chat_log] Failed to delete {}: {}", path.display(), e);
    }
    let bpath = boundary_path(session_key);
    if let Err(e) = std::fs::remove_file(&bpath)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("[chat_log] Failed to delete {}: {}", bpath.display(), e);
    }
    let mpath = meta_path(session_key);
    if let Err(e) = std::fs::remove_file(&mpath)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("[chat_log] Failed to delete {}: {}", mpath.display(), e);
    }
}

/// Clear (truncate) a session's chat log, keeping the file. Used by session
/// management "clear" — empties history but the session id stays usable.
/// Also truncates the boundary-events sidecar (same lifecycle).
pub fn clear_chat_log(session_key: &str) {
    let path = log_path(session_key);
    if let Err(e) = fs::write(&path, "") {
        tracing::warn!("[chat_log] Failed to clear {}: {}", path.display(), e);
    }
    let bpath = boundary_path(session_key);
    if bpath.exists()
        && let Err(e) = fs::write(&bpath, "")
    {
        tracing::warn!("[chat_log] Failed to clear {}: {}", bpath.display(), e);
    }
}

/// Path for the sidecar title meta file (`{safe_key}.meta.json`, next to the
/// `.jsonl`). Stores a user-editable conversation title for multi-session
/// management without touching the lazy-created SessionStore.
fn meta_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.meta.json", safe_key))
}

/// Write the conversation title to the sidecar meta file.
///
/// E4 (2026-09-05): read-modify-write UPSERT — preserves `parent` /
/// `forked_at_turn` already recorded by `write_session_parent` (fork writes
/// lineage first, then the fork endpoint writes a title; a blind overwrite
/// would erase the lineage).
pub fn write_session_meta(session_key: &str, title: &str) {
    upsert_meta(session_key, |m| m.title = Some(title.to_string()));
}

/// E7: 手动标题写入（rename / 建会话时用户显式命名）——置 `title_manual`
/// 标记，自动标题（`write_session_meta_auto_title`）永不覆盖。
pub fn write_session_meta_manual(session_key: &str, title: &str) {
    upsert_meta(session_key, |m| {
        m.title = Some(title.to_string());
        m.title_manual = true;
    });
}

/// E7: 会话侧栏的默认占位标题（sessions create 未显式命名时写入）。
/// 自动标题允许覆盖它（占位符不是用户意志）。
pub const DEFAULT_SESSION_TITLE: &str = "新对话";

/// E7: 自动标题写入。只填充「无标题 / 仅默认占位符」的会话；手动改名或
/// 建会话时显式命名的（`title_manual=true`）与已有真实标题的一律不动。
/// 返回是否实际写入。
pub fn write_session_meta_auto_title(session_key: &str, title: &str) -> bool {
    if !auto_title_eligible(session_key) {
        return false;
    }
    upsert_meta(session_key, |m| m.title = Some(title.to_string()));
    true
}

/// E7: 自动标题资格判定——无 meta / 无标题 / 仅默认占位符，且未被手动
/// 改名。生成端（loop 的标题任务）与写入端共用，双端一致。
pub fn auto_title_eligible(session_key: &str) -> bool {
    match read_meta_full(session_key) {
        None => true,
        Some(m) => {
            !m.title_manual
                && m.title
                    .as_deref()
                    .is_none_or(|t| t == DEFAULT_SESSION_TITLE)
        }
    }
}

/// E7: 本会话 chat log 中首条非空 user 消息（截到 `max_chars` 字符，
/// char 边界安全；自动标题的输入）。
pub fn first_user_message(session_key: &str, max_chars: usize) -> Option<String> {
    let data = fs::read_to_string(log_path(session_key)).ok()?;
    for line in data.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let content = v.get("content").and_then(|c| c.as_str()).unwrap_or("");
        let trimmed = content.trim();
        if trimmed.is_empty() {
            continue;
        }
        return Some(trimmed.chars().take(max_chars).collect());
    }
    None
}

/// E4: record fork lineage (`parent` = source session key, `forked_at_turn`
/// = the kept-turn count) in the sidecar meta. Upsert — preserves `title`.
pub fn write_session_parent(session_key: &str, parent: &str, forked_at_turn: usize) {
    upsert_meta(session_key, |m| {
        m.parent = Some(parent.to_string());
        m.forked_at_turn = Some(forked_at_turn);
    });
}

/// Shared read-modify-write for the sidecar meta (single fs read + write).
fn upsert_meta(session_key: &str, f: impl FnOnce(&mut SessionMeta)) {
    let path = meta_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Corrupt / unreadable existing file → start from an empty meta (same
    // warn-and-continue posture as before; nothing readable is lost).
    let mut meta = read_meta_full(session_key).unwrap_or_default();
    f(&mut meta);
    if let Err(e) = fs::write(&path, serde_json::to_string(&meta).unwrap_or_default()) {
        tracing::warn!("[chat_log] failed to write meta {}: {}", path.display(), e);
    }
}

/// L6++（2026-09-08）：项目归属烧入（创建项目会话时一次写入；归属不可变
/// ——无改移 API，改归属 = 新会话，行业共识）。Upsert 语义同
/// write_session_parent（已写过的 title 等字段保留）。
pub fn write_session_project(session_key: &str, project_id: &str, project_path: &str) {
    upsert_meta(session_key, |m| {
        m.project_id = Some(project_id.to_string());
        m.project_path = Some(project_path.to_string());
    });
}

/// L6++（2026-09-08，M4 sessions.delete/clear 的 forget 联动）：摘除项目
/// 归属两字段（title/血缘/manual 标记全部保留）。语义：
/// - 文件缺失 = no-op（delete 路径 meta 已随 delete_chat_log 一并删除）；
/// - 本就无归属 = no-op（不空写文件）；
/// - 有归属 = 读改写摘除（防 owner_of 的 sidecar 兜底把绑定「复活」）。
///
/// 返回是否实际摘除了归属。
pub fn clear_session_project(session_key: &str) -> bool {
    let Some(mut meta) = read_meta_full(session_key) else {
        return false;
    };
    if meta.project_id.is_none() && meta.project_path.is_none() {
        return false;
    }
    meta.project_id = None;
    meta.project_path = None;
    let path = meta_path(session_key);
    if let Err(e) = fs::write(&path, serde_json::to_string(&meta).unwrap_or_default()) {
        tracing::warn!(
            "[chat_log] failed to clear session project {}: {}",
            path.display(),
            e
        );
        return false;
    }
    true
}

/// Read the conversation title from the sidecar meta file, if present.
pub fn read_session_meta(session_key: &str) -> Option<String> {
    read_meta_full(session_key).and_then(|m| m.title)
}

/// Read the full sidecar meta by session key. `None` when absent/unparsable.
fn read_meta_full(session_key: &str) -> Option<SessionMeta> {
    let path = meta_path(session_key);
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// E4: read the full sidecar meta (title + fork lineage). `None` when the
/// file is absent or unparsable. Legacy files (`{"title": ...}` only)
/// deserialize with lineage fields `None` (compatible read).
pub fn read_session_meta_full(session_key: &str) -> Option<SessionMeta> {
    read_meta_full(session_key)
}

/// E4: full sidecar meta (`{safe_key}.meta.json`). All fields optional —
/// pre-E4 files carry only `title`; serde defaults keep them compatible.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SessionMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Source session key this session was forked from (fork_session writes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Turn boundary the fork was taken at (user-turn count kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_at_turn: Option<usize>,
    /// E7: 用户手动命名过（rename / 建会话显式标题）——自动标题永不覆盖。
    #[serde(default, skip_serializing_if = "is_false")]
    pub title_manual: bool,
    /// L6++：项目归属（创建项目会话时烧入，不可变；None = 对话组）。
    /// 真相源 = 本 sidecar；`StoredSession` 的同名字段只是缓存镜像。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// L6++：canonical 项目目录绝对路径（与 project_id 同批写入）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
}

/// E7: `skip_serializing_if` 助手（false 不落盘，兼容旧文件形态）。
fn is_false(b: &bool) -> bool {
    !*b
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
// I3 (U9): boundary events (round-5 fix: sidecar file)
// ---------------------------------------------------------------------------

/// Resolve the boundary-events SIDECAR path (`logs/boundary/<safe_key>.jsonl`).
///
/// Round-5 review fix: boundary events used to be interleaved into the
/// message jsonl — which skewed `read_chat_log` pagination counts (total
/// counted boundary lines, pages underfilled or came back empty with
/// has_more=true), made every NEW session's first line a `turn_start` row
/// (blank title/preview in the Dashboard session list, which reads
/// `lines[0]["content"]`), and rendered empty bubbles in raw readers
/// (`logs.rs` session_detail / scan_session_logs bypass read_chat_log).
/// A separate file fixes all of them at the storage layer. Deliberately NOT
/// inside `session_logs/`: that dir is scanned for `*.jsonl` as sessions —
/// a sidecar there would appear as a phantom session.
fn boundary_path(session_key: &str) -> PathBuf {
    let safe_key = session_key.replace(':', "_");
    default_path_manager()
        .boundary_events_dir()
        .join(format!("{}.jsonl", safe_key))
}

/// Append a turn/step boundary event to the session's boundary sidecar.
/// Lightweight durable markers for replay/audit: `turn_start` / `turn_end`
/// (with a reason) / `llm_request` (model + token estimate) /
/// `steer_injected`. Never contains message bodies.
pub fn append_boundary_event(session_key: &str, kind: &str, detail: &str) {
    let path = boundary_path(session_key);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "[chat_log] boundary event open failed {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    let entry = serde_json::json!({
        "role": "boundary",
        "event": kind,
        "detail": detail,
        "timestamp": Local::now().to_rfc3339(),
    });
    if let Ok(line) = serde_json::to_string(&entry) {
        let _ = writeln!(file, "{}", line);
    }
}

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;

// D3 (devtool-upgrade 阶段 5)：ChatLogMeta 全字段入口 + file_changes 字段
// 写入 + dedup_file_changes 投影测试。
#[cfg(test)]
mod d3_tests;

// E3 (devtool-upgrade 阶段 5)：checkpoint_turn 行标记 + truncate_chat_log_rows
// 原位截断（消息级回退的落盘原语）测试。
#[cfg(test)]
mod e3_tests;
