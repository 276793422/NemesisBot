//! 会话事件账本（扩展计划）。
//!
//! 会话历史（chat_log jsonl）可被合法改写：rewind/redo 截断、clear
//! 清空、fork 复制。改写本身不可篡改审计——被删/被改的已提交内容直接消失，
//! 事后无法回答「这个会话在 t 时刻是什么样、删掉的部分原文是什么」。本
//! 账本在 chat_log 三个写路径（append / truncate / fork）旁路落一条
//! hash 链 JSONL：`{workspace}/logs/event_ledger/{session_key}.jsonl`。
//!
//! 链形态与 nemesis-security AuditChain 同源（prev_hash + SHA-256 串联），
//! 但按会话分文件（AuditChain 是全局链）：任一行被改/删/插，从该行起全链
//! 断裂，[`ledger_verify`] 重算即红。
//!
//! **内容策略**（磁盘开销 vs 恢复语义的取舍，刻意非对称）：
//! - `append` 行只存 `content_sha256`（chat_log 本体就是 append 记录，账本
//!   职责是完整性证据，不复制正文）；
//! - `truncate` / `fork` 行带**全文**——truncate 的被删行全文入账是本设计
//!   的硬要求（永不丢已提交数据：rewind 删掉的对话可从账本找回原文）。
//!
//! 记账是 best-effort：账本写失败只 warn，绝不阻断聊天主流程（chat_log
//! 写入才为权威依据；账本是旁证）。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// 创世 prev_hash（64 个 '0'——与主流 hash 链惯例一致，全 0 表示无前驱）。
pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// 账本操作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LedgerOp {
    /// chat_log 追加一行（role+sha，不带正文）。
    Append,
    /// chat_log 截断重写（被删行全文入账）。
    Truncate,
    /// fork 复制行到新会话（sha-only）。
    Fork,
}

impl LedgerOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            LedgerOp::Append => "append",
            LedgerOp::Truncate => "truncate",
            LedgerOp::Fork => "fork",
        }
    }
}

/// 账本行。`content` 只在 truncate（被删行恢复）/fork 需要全文时携带。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerRow {
    pub role: String,
    pub content_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl LedgerRow {
    /// 从 chat_log 行构造：sha 必带；`with_content=true` 时全文入账。
    pub fn from_chat_row(v: &serde_json::Value, with_content: bool) -> Self {
        let role = v
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        let content = v
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let content_sha256 = sha256_hex(content.as_bytes());
        let content = if with_content { Some(content) } else { None };
        Self {
            role,
            content_sha256,
            content,
        }
    }
}

/// 链体（hash 计算的序列化单元——字段顺序即 struct 声明顺序，确定性）。
#[derive(Debug, Serialize, Deserialize)]
struct LedgerBody {
    seq: u64,
    op: String,
    ts: String,
    actor: String,
    rows: Vec<LedgerRow>,
}

/// 一条账本记录（jsonl 行）。
#[derive(Debug, Serialize, Deserialize)]
struct LedgerEntry {
    #[serde(flatten)]
    body: LedgerBody,
    prev_hash: String,
    hash: String,
}

/// [`ledger_verify`] 的统计结果。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LedgerStats {
    pub events: u64,
    pub appends: u64,
    pub truncates: u64,
    pub forks: u64,
    pub rows_total: u64,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// 账本文件路径（与 chat_log 同键同消毒规则，落到 sessions_log_dir 的
/// 兄弟目录 `event_ledger/`）。
pub fn ledger_path(session_key: &str) -> PathBuf {
    let safe_key = nemesis_utils::sanitize::sanitize_path_segment(session_key);
    nemesis_path::default_path_manager()
        .sessions_log_dir()
        .parent()
        .map(|p| p.join("event_ledger"))
        .unwrap_or_else(|| {
            // sessions_log_dir 无父目录（退化路径）——原样落同目录，不丢账。
            nemesis_path::default_path_manager().sessions_log_dir()
        })
        .join(format!("{}.jsonl", safe_key))
}

/// 记账入口（chat_log 三个写路径调用）。best-effort：失败只 warn。
pub fn ledger_record(session_key: &str, op: LedgerOp, actor: &str, rows: Vec<LedgerRow>) {
    let path = ledger_path(session_key);
    if let Err(e) = record_entry_at(&path, op, actor, rows) {
        tracing::warn!(
            "[event_ledger] record failed for {} ({}): {}",
            session_key,
            op.as_str(),
            e
        );
    }
}

/// 同会话并发记账源真实存在—— 注记承认的「spawned
/// 续行 final vs 主循环下一条 user 行」双写形态，加 snooze sweeper 唤醒
/// 行（gateway 常驻任务，不经 history gate）。「读链尾 → 追加」两步若不
/// 串行，两方并发读到同一 (seq, prev_hash) → 重复 seq → ledger_verify 从
/// 断点起永久红（append-only 无修复路径），完整性证据工具自身产生永久
/// 假阳性。按账本文件路径分锁（不同会话互不阻塞）；parking_lot 无毒化，
/// 文件 IO 全程持锁（临界区 = 一次读尾 + 单行 append，量级与原实现持平）。
static LEDGER_IO_LOCKS: std::sync::OnceLock<
    dashmap::DashMap<PathBuf, std::sync::Arc<parking_lot::Mutex<()>>>,
> = std::sync::OnceLock::new();

fn ledger_io_lock(path: &std::path::Path) -> std::sync::Arc<parking_lot::Mutex<()>> {
    LEDGER_IO_LOCKS
        .get_or_init(dashmap::DashMap::new)
        .entry(path.to_path_buf())
        .or_default()
        .clone()
}

/// 显式路径记账（worker；测试用 [`record_entry_at`] 打临时路径，
/// 生产走 [`ledger_record`]）。
pub fn record_entry_at(
    path: &std::path::Path,
    op: LedgerOp,
    actor: &str,
    rows: Vec<LedgerRow>,
) -> Result<u64, String> {
    // 读尾 → 构造 → 追加全程持同路径锁（见
    // LEDGER_IO_LOCKS 注记）。
    let io_lock = ledger_io_lock(path);
    let _io = io_lock.lock();
    let (seq, prev_hash) = read_chain_tail_healed(path)?;
    let line = build_entry_line(seq, &prev_hash, op, actor, rows)?;
    append_entry_line(path, &line)?;
    Ok(seq)
}

/// 由链尾坐标构造一条待追加的 jsonl 行（拆出供测试确定性红证旧无锁交错）。
fn build_entry_line(
    seq: u64,
    prev_hash: &str,
    op: LedgerOp,
    actor: &str,
    rows: Vec<LedgerRow>,
) -> Result<String, String> {
    let body = LedgerBody {
        seq,
        op: op.as_str().to_string(),
        ts: chrono::Local::now().to_rfc3339(),
        actor: actor.to_string(),
        rows,
    };
    let hash = chain_hash(prev_hash, &body);
    let entry = LedgerEntry {
        prev_hash: prev_hash.to_string(),
        hash,
        body,
    };
    serde_json::to_string(&entry).map_err(|e| e.to_string())
}

/// 单行追加（OpenOptions append；目录惰性创建）。
fn append_entry_line(path: &std::path::Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open: {e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("write: {e}"))
}

/// 全链重算验证。任何一行被改/删/插（或行内 sha 与自带正文矛盾）→ Err。
pub fn ledger_verify(path: &std::path::Path) -> Result<LedgerStats, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut stats = LedgerStats::default();
    let mut running = GENESIS_PREV_HASH.to_string();
    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| format!("read line {}: {e}", idx + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: LedgerEntry =
            serde_json::from_str(&line).map_err(|e| format!("line {}: 解析失败: {e}", idx + 1))?;
        if entry.body.seq != stats.events {
            return Err(format!(
                "line {}: seq 断裂（期望 {}，实际 {}）",
                idx + 1,
                stats.events,
                entry.body.seq
            ));
        }
        if entry.prev_hash != running {
            return Err(format!("line {}: prev_hash 与前驱断裂", idx + 1));
        }
        let recomputed = chain_hash(&entry.prev_hash, &entry.body);
        if recomputed != entry.hash {
            return Err(format!("line {}: hash 不匹配（内容被篡改）", idx + 1));
        }
        for row in &entry.body.rows {
            if let Some(c) = &row.content
                && sha256_hex(c.as_bytes()) != row.content_sha256
            {
                return Err(format!("line {}: 行内正文与 sha 不一致", idx + 1));
            }
        }
        match entry.body.op.as_str() {
            "append" => stats.appends += 1,
            "truncate" => stats.truncates += 1,
            "fork" => stats.forks += 1,
            other => return Err(format!("line {}: 未知 op '{other}'", idx + 1)),
        }
        stats.rows_total += entry.body.rows.len() as u64;
        stats.events += 1;
        running = entry.hash;
    }
    Ok(stats)
}

/// 读链尾：末好行的 (seq+1, hash)；空/缺失 = (0, GENESIS)。
///
/// **撕裂尾自愈**（BUG-20261113-168）：追加是裸 `writeln!`（write_fmt 对
/// 裸 File 可能拆成多次 write syscall，大行 write_all 还有部分写循环），
/// 磁盘满/进程死/断电可留下半行或丢换行；丢换行的下一笔追加还会把新行拼
/// 到残行上，连原本完整的好行一起不可解析。原实现对坏行 `?` 直接 Err——
/// 追加从此永久失败（[`ledger_record`] 只 warn，best-effort 契约下零可
/// 观测静默停摆），[`ledger_verify`] 也永久红，无修复路径。修复语义：
/// append-only 不变量下自身故障只可能发生在尾部——**最后一个可解析完整行
/// 之后的一切字节**（无换行残行/拼行/坏行）都是崩溃残迹，截断到最后好行
/// 终点并 WARN；**最后好行之前若还有不可解析完整行 = 中间损坏**（篡改/
/// 介质损坏），拒绝自动修复、原样 Err——verify 的红是对篡改的诚实报警，
/// 静默截断会吃掉证据。诚实边界：链无外部锚，本自愈只针对崩溃残迹——
/// 能写文件的对读本可重写/删除整个文件（原设计缺文件即回落 GENESIS），
/// 这不是对抗性防线。
fn read_chain_tail_healed(path: &std::path::Path) -> Result<(u64, String), String> {
    let content = match std::fs::read(path) {
        Ok(c) => c,
        // 缺失 = 从未有账（与原 open Err 同口径回落 GENESIS）；其他读错误诚实报。
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((0, GENESIS_PREV_HASH.to_string()));
        }
        Err(e) => return Err(format!("read: {e}")),
    };
    // 逐完整行（\n 结尾）字节级扫描：跟踪最后可解析非空行的 (seq, hash)
    // 与字节终点（追加锚点，含换行——锚点不含换行会让下一笔 writeln 拼行，
    // 重建撕裂形态）。坏行先记账，出现更晚的好行即定性中间损坏。
    // Option 区分「无好行」（截到空 = 链重启 GENESIS，下一笔 seq=0）与
    // 「末条目 seq=0」（下一笔 seq=1）——裸 u64 初值 0 会混淆两态。
    let mut last_good: Option<(u64, String)> = None;
    let mut good_end = 0usize;
    let mut saw_bad_line = false;
    let mut line_no = 0usize;
    let mut pos = 0usize;
    while pos < content.len() {
        let Some(nl) = content[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| pos + i)
        else {
            break; // 无换行尾巴：残行，归入尾部自愈区
        };
        let line = &content[pos..nl];
        pos = nl + 1;
        line_no += 1;
        if line.iter().all(|&b| b.is_ascii_whitespace()) {
            continue; // 空白行跳过（与 ledger_verify 同口径），不推进锚点
        }
        let parsed = std::str::from_utf8(line)
            .ok()
            .and_then(|s| serde_json::from_str::<LedgerEntry>(s).ok());
        match parsed {
            Some(entry) => {
                if saw_bad_line {
                    return Err(format!(
                        "line {line_no}: 其之前存在不可解析中间损坏（最后好行之前的坏行 = 疑似篡改/介质损坏），拒绝自动修复"
                    ));
                }
                last_good = Some((entry.body.seq, entry.hash));
                good_end = pos;
            }
            None => saw_bad_line = true,
        }
    }
    // 尾部自愈：锚点之后的字节 = 崩溃残迹区。两种形态分治：
    // ① 残迹整体恰是一个完整 LedgerEntry（writeln 的 content 与 "\n" 是
    //    两次独立 write_all——format_args 模板两段——崩溃卡在中间 = 好行
    //    只丢了换行）：补写 "\n" 恢复该行，证据零丢失；
    // ② 其余（半行 / 拼行 / 空白 / 坏行）：截断到锚点并 WARN（拼行形态下
    //    被拼坏的好行字节随残迹一并截掉——账本是 best-effort 旁证，
    //    chat_log 本体才是真相源，WARN 带丢弃字节数可观测）。
    // 两种分支都只许在无中间坏行时执行（坏行之后的可解析尾条目 = 篡改
    // 形态，恢复/截断都会吃掉坏行证据）。
    if content.len() > good_end {
        let tail = &content[good_end..];
        // 残迹整体恰是一个完整 LedgerEntry（writeln 的 content 与 "\n" 是
        // 两次独立 write_all——format_args 模板两段——崩溃卡在中间 = 好行
        // 只丢了换行）→ 补写 "\n" 恢复该行，证据零丢失。可达性注记：
        // 此态下 saw_bad_line 必为 false——坏行字节必落在尾区内，尾区整体
        // 解析必然失败，不会进入本分支；「坏完整行之后接好行」的篡改形态
        // 由上方扫描循环的中间损坏 Err 承接。
        let restored = std::str::from_utf8(tail)
            .ok()
            .and_then(|s| serde_json::from_str::<LedgerEntry>(s).ok());
        if let Some(entry) = restored {
            use std::io::Write as _;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(|e| format!("heal open: {e}"))?;
            f.write_all(b"\n")
                .map_err(|e| format!("heal newline: {e}"))?;
            let _ = f.sync_all();
            tracing::warn!(
                "[event_ledger] 撕裂尾自愈：尾区为丢换行的完整条目，已补换行恢复（seq {}）{}",
                entry.body.seq,
                path.display()
            );
            return Ok((entry.body.seq + 1, entry.hash));
        }
        let dropped = content.len() - good_end;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| format!("heal open: {e}"))?;
        f.set_len(good_end as u64)
            .map_err(|e| format!("heal truncate: {e}"))?;
        let _ = f.sync_all();
        tracing::warn!(
            "[event_ledger] 撕裂尾自愈：截掉 {dropped} 字节崩溃残迹 {}",
            path.display()
        );
    }
    Ok(match last_good {
        Some((seq, hash)) => (seq + 1, hash),
        // 无好行（空文件/残迹全截）：链重启，下一笔从 GENESIS seq 0 起。
        None => (0, GENESIS_PREV_HASH.to_string()),
    })
}

/// 链 hash：SHA-256(prev_hash bytes ‖ body 规范序列化)。
fn chain_hash(prev_hash: &str, body: &LedgerBody) -> String {
    let body_bytes = serde_json::to_vec(body).expect("LedgerBody 只含字符串/数值，序列化不可失败");
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(&body_bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests;
