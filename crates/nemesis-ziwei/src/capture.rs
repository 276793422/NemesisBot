//! capture —— 使用期数据回流落盘（T2.4，决议 3）。
//!
//! 布局：`<workspace>/ziwei/runs/<profile_id>/capture.jsonl`，每行一条
//! JSON（ts + 来源工具 + 数据快照）。保留策略（§6.5）：7 天 TTL +
//! 每 profile 512 MB 上限，超限删最旧一半（重写文件，尾部新数据保住）。
//! 快照可能带目标业务数据 → 审计只记 hash+size，不落全文（对齐图片管线）。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// 默认保留策略（config 可覆盖，经 ZiweiConfig runs_ttl_days）。
pub const DEFAULT_TTL_DAYS: u64 = 7;
/// 每 profile capture.jsonl 上限（字节）。
pub const MAX_BYTES_PER_PROFILE: u64 = 512 * 1024 * 1024;

/// 使用期捕获存储（每 profile 一个 jsonl）。
pub struct CaptureStore {
    runs_dir: PathBuf,
}

/// 清扫报告（诚实计数）。
#[derive(Debug, Default, PartialEq)]
pub struct SweepReport {
    /// 按 TTL 删除的 profile 目录数。
    pub expired_profiles: usize,
    /// 按容量截断的文件数。
    pub size_truncated: usize,
    /// 清扫后仍存在的 profile 数。
    pub kept: usize,
}

impl CaptureStore {
    pub fn new(workspace: &Path) -> CaptureStore {
        CaptureStore {
            runs_dir: workspace.join("ziwei").join("runs"),
        }
    }

    fn profile_file(&self, profile_id: &str) -> PathBuf {
        self.runs_dir.join(profile_id).join("capture.jsonl")
    }

    /// 追加一条捕获记录（`{"ts":<rfc3339>,"profile":..,"tool":..,..}` 形态
    /// 由调用方组好；这里只补 ts 缺省）。坏 JSON 行不可能是自己的产物——
    /// 调用方传 serde_json::Value 保证。
    pub fn append(&self, profile_id: &str, mut record: serde_json::Value) -> std::io::Result<()> {
        let path = self.profile_file(profile_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if record.get("ts").is_none()
            && let Some(obj) = record.as_object_mut()
        {
            obj.insert(
                "ts".into(),
                serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
            );
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(f, "{record}")?;
        Ok(())
    }

    /// 读回某 profile 的全部记录（WSAPI 会话视图「最近捕获」用）。
    pub fn read_recent(&self, profile_id: &str, limit: usize) -> Vec<serde_json::Value> {
        let path = self.profile_file(profile_id);
        let Ok(f) = std::fs::File::open(&path) else {
            return Vec::new();
        };
        let all: Vec<serde_json::Value> = BufReader::new(f)
            .lines()
            .map_while(|l| l.ok())
            .filter_map(|l| serde_json::from_str(&l).ok())
            .collect();
        let start = all.len().saturating_sub(limit);
        all[start..].to_vec()
    }

    /// 清扫：TTL + 每 profile 容量上限（网关启动时 + 每 10 分钟周期）。
    pub fn sweep(&self, ttl: std::time::Duration, max_bytes: u64) -> SweepReport {
        let mut report = SweepReport::default();
        let Ok(entries) = std::fs::read_dir(&self.runs_dir) else {
            return report;
        };
        let now = std::time::SystemTime::now();
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort(); // read_dir 顺序是平台细节——显式排序（CLAUDE.md 环境速查）
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let file = dir.join("capture.jsonl");
            let meta = match std::fs::metadata(&file) {
                Ok(m) => m,
                Err(_) => continue, // 无 capture.jsonl 的目录不碰（非本 store 资产）
            };
            // TTL：mtime 早于 cutoff → 删整个 profile 目录
            match meta.modified() {
                Ok(mtime) if now.duration_since(mtime).unwrap_or_default() > ttl => {
                    if nemesis_utils::fsutil::remove_dir_all_retry(&dir).is_ok() {
                        report.expired_profiles += 1;
                    }
                    continue;
                }
                Err(_) => continue,
                Ok(_) => {}
            }
            // 容量：超限删最旧一半（行级重写）
            if meta.len() > max_bytes && truncate_keep_newest(&file, max_bytes).is_ok() {
                report.size_truncated += 1;
            }
            report.kept += 1;
        }
        report
    }
}

/// 保留文件尾部（最新）行，使重写后 ≤ max_bytes（至少保留最后一行，
/// 即使它自己超限——单条超大记录不静默丢）。
fn truncate_keep_newest(file: &Path, max_bytes: u64) -> std::io::Result<()> {
    let f = std::fs::File::open(file)?;
    let len = f.metadata()?.len();
    // 从尾部向前找起点：简化实现——逐行读入并从尾部累计（512MB 上限内
    // 内存可承受；超上限场景是滥用，此时诚实占用而非静默丢数据）。
    let lines: Vec<String> = BufReader::new(f).lines().map_while(|l| l.ok()).collect();
    let mut keep_from = lines.len();
    let mut acc = 0u64;
    for (i, line) in lines.iter().enumerate().rev() {
        acc += line.len() as u64 + 1; // +\n
        if acc > max_bytes {
            keep_from = i + 1;
            break;
        }
        keep_from = i;
    }
    if keep_from >= lines.len() {
        if lines.is_empty() {
            return Ok(()); // 空文件无可截
        }
        // 最新一条自己就超限：仍保它（单条超大记录不静默丢，注释承诺）
        keep_from = lines.len() - 1;
    }
    let tmp = file.with_extension("jsonl.tmp");
    {
        let mut out = std::fs::File::create(&tmp)?;
        for line in &lines[keep_from..] {
            writeln!(out, "{line}")?;
        }
        out.flush()?;
    }
    std::fs::rename(&tmp, file)?;
    tracing::info!(
        "[ziwei] capture 截断：{} → {}（保留尾部 {} 行）",
        len,
        max_bytes,
        lines.len() - keep_from
    );
    Ok(())
}

#[cfg(test)]
mod tests;
