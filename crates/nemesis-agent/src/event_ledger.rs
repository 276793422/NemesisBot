//! 会话事件账本（扩展计划 T6 / D5）。
//!
//! 会话历史（chat_log jsonl）可被合法改写：rewind/redo 截断（E3）、clear
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
//! 写入才是真相源；账本是旁证）。

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

/// 显式路径记账（worker；测试用 [`record_entry_at`] 打临时路径，
/// 生产走 [`ledger_record`]）。
pub fn record_entry_at(
    path: &std::path::Path,
    op: LedgerOp,
    actor: &str,
    rows: Vec<LedgerRow>,
) -> Result<u64, String> {
    let (seq, prev_hash) = read_chain_tail(path)?;
    let body = LedgerBody {
        seq,
        op: op.as_str().to_string(),
        ts: chrono::Local::now().to_rfc3339(),
        actor: actor.to_string(),
        rows,
    };
    let hash = chain_hash(&prev_hash, &body);
    let entry = LedgerEntry {
        prev_hash,
        hash,
        body,
    };
    let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open: {e}"))?;
    writeln!(f, "{}", line).map_err(|e| format!("write: {e}"))?;
    Ok(seq)
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

/// 读链尾：末行的 (seq+1, hash)；空/缺失 = (0, GENESIS)。
fn read_chain_tail(path: &std::path::Path) -> Result<(u64, String), String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok((0, GENESIS_PREV_HASH.to_string())),
    };
    let mut last: Option<LedgerEntry> = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        last = Some(serde_json::from_str(&line).map_err(|e| e.to_string())?);
    }
    Ok(match last {
        Some(e) => (e.body.seq + 1, e.hash),
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
