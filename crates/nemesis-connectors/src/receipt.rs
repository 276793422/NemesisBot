//! 统一回执（感知层核心信物，PLAN §3.5 / §5.1）。
//!
//! 每个关键动作一条带编号凭条，append-only jsonl 落
//! `<workspace>/data/receipts/receipts-YYYYMMDD.jsonl`（UTC 日轮换）。
//! 这是行动账本（LogsView「行动账本」tab）、`connectors.receipts.list`
//! （WSAPI）与 `connector receipts`（CLI）的**同一数据源，单点不双写**。
//!
//! 回执记「动作的账」：args 只存 digest（sha256 前 16 hex）不存原文——
//! 凭据/敏感参数永不落回执（与审计链 hash-only 同纪律）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 回执主结构。`risk`/`class`/`outcome` 存字符串形态（与清单/管线词汇一致，
/// 跨 crate 不拖类型依赖）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    /// `RCPT-<millis>-<seq>`，进程内单调。
    pub receipt_id: String,
    /// RFC3339 UTC。
    pub ts: String,
    pub connector: String,
    pub op: String,
    /// low | medium | high | critical
    pub risk: String,
    /// read | write | exec | network
    pub class: String,
    /// executed | denied_approval | budget_blocked | disabled | error
    pub outcome: String,
    /// sha256(canonical args json) 前 16 hex（args 原文不落盘）。
    pub args_digest: String,
    /// 人话结果摘要（消费方截断，存层不截）。
    pub summary: String,
    /// 回执主键字段名（如 `order_id`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_consumed: Option<BudgetConsumed>,
    /// 审批卡引用（有审批的 op 必带）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// 是否可撤销（决定回执卡上的「可撤销」提示）。
    /// default：撤销动词（2026-10-14）之前落盘的历史行没有本字段——
    /// 缺 default 会把全部存量合法回执行解析成损坏行静默跳过（IT 实证）。
    #[serde(default)]
    pub undoable: bool,
}

/// 本次执行消耗的预算快照（凭条上的「额度余量」素材）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetConsumed {
    pub ops_last_hour: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finance_orders_today: Option<u32>,
}

fn receipt_seq() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed) + 1
}

/// 生成回执编号（`now_ms` 由调用方传，保证可测）。
pub fn new_receipt_id(now_ms: i64) -> String {
    format!("RCPT-{}-{:04}", now_ms, receipt_seq())
}

/// args digest（canonical JSON 字节序，同 args 同 digest）。
pub fn args_digest(args: &serde_json::Value) -> String {
    let mut h = Sha256::new();
    h.update(args.to_string().as_bytes());
    hex::encode(h.finalize())[..16].to_string()
}

/// 回执目录：`<workspace>/data/receipts/`。
pub fn receipts_dir(workspace: &Path) -> PathBuf {
    workspace.join("data").join("receipts")
}

fn day_file_name(date: chrono::NaiveDate) -> String {
    format!("receipts-{}.jsonl", date.format("%Y%m%d"))
}

/// 追加一条回执（append-only；目录按需创建）。
pub fn append_receipt(workspace: &Path, r: &Receipt) -> Result<(), String> {
    let dir = receipts_dir(workspace);
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建回执目录失败: {e}"))?;
    let path = dir.join(day_file_name(Utc::now().date_naive()));
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开回执文件失败 {}: {e}", path.display()))?;
    let line = serde_json::to_string(r).map_err(|e| format!("序列化回执失败: {e}"))?;
    writeln!(f, "{line}").map_err(|e| format!("写回执失败: {e}"))?;
    Ok(())
}

/// 列出回执（新→旧），可按 connector 过滤，最多 `limit` 条。
/// 损坏行 warn + 跳过（不中断流水读取）。
pub fn list_receipts(
    workspace: &Path,
    limit: usize,
    connector: Option<&str>,
) -> Result<Vec<Receipt>, String> {
    let dir = receipts_dir(workspace);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    // 文件名降序 = 日期新→旧（read_dir 显式排序纪律）。
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("读回执目录失败: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("receipts-") && n.ends_with(".jsonl"))
        .collect();
    names.sort();
    names.reverse();

    let mut out = Vec::new();
    for name in names {
        let path = dir.join(&name);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("读回执文件失败 {}: {e}", path.display()))?;
        for line in text.lines().rev() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Receipt>(line) {
                Ok(r) => {
                    if connector.map(|c| r.connector == c).unwrap_or(true) {
                        out.push(r);
                        if out.len() >= limit {
                            return Ok(out);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("[Receipts] 跳过损坏行 {}: {e}", path.display());
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
