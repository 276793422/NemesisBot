//! Experience store - JSONL persistence for collected experiences.
//!
//! Provides read/write access to the experience JSONL file used by the
//! reflector for statistical analysis (flat `experiences.jsonl`, plus
//! date-based cleanup).
//!
//! 注：聚合读写面（`AggregatedExperience` / `read_aggregated` /
//! `get_top_patterns` 家族 / 每日限额）2026-10-22 整体移除——全链零生产
//! 调用，是 write-only 死数据（审计 P2-a）。经验分析消费 `read_all` /
//! `read_recent`。

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::types::CollectedExperience;

/// An experience store that persists to JSONL with date-based organization.
pub struct ExperienceStore {
    base_dir: PathBuf,
}

impl ExperienceStore {
    /// Create a new experience store rooted at the given directory.
    ///
    /// Files are organized as: `{base_dir}/YYYYMM/YYYYMMDD.jsonl`
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: path.into(),
        }
    }

    /// Create an experience store from a forge directory (adds "experiences" subdirectory).
    pub fn from_forge_dir(forge_dir: &std::path::Path) -> Self {
        Self {
            base_dir: forge_dir.join("experiences"),
        }
    }

    /// Append a collected experience (simple mode) to the JSONL file.
    pub async fn append(&self, exp: &CollectedExperience) -> std::io::Result<()> {
        if let Some(parent) = self.base_dir.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::create_dir_all(&self.base_dir).await?;

        let file_path = self.base_dir.join("experiences.jsonl");
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?;

        let mut line = serde_json::to_string(exp)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }

    /// Remove experience files older than the specified number of days.
    pub async fn cleanup(&self, max_age_days: i64) -> std::io::Result<usize> {
        let cutoff = chrono::Local::now() - chrono::Duration::days(max_age_days);
        let cutoff_str = cutoff.format("%Y%m%d").to_string();
        let mut removed = 0usize;

        if !self.base_dir.exists() {
            return Ok(0);
        }

        let mut month_entries = tokio::fs::read_dir(&self.base_dir).await?;
        while let Some(month_entry) = month_entries.next_entry().await? {
            if !month_entry.file_type().await?.is_dir() {
                continue;
            }
            let mut file_entries = tokio::fs::read_dir(month_entry.path()).await?;
            while let Some(file_entry) = file_entries.next_entry().await? {
                let name = file_entry.file_name();
                let name_str = name.to_string_lossy();
                if !name_str.ends_with(".jsonl") {
                    continue;
                }
                // Extract date from filename (YYYYMMDD.jsonl)
                let date_part = name_str.trim_end_matches(".jsonl");
                if date_part < cutoff_str.as_str() {
                    tokio::fs::remove_file(file_entry.path()).await?;
                    removed += 1;
                }
            }
        }

        // Also trim the flat experiences.jsonl by age. The loop above only
        // descends YYYYMM/ subdirs (is_dir gate at the top), so the flat file
        // at base_dir — where the live collector actually writes — was never
        // cleaned and grew unbounded. Trim old CollectedExperience entries by
        // timestamp and atomically rewrite. Unparseable lines (e.g. legacy
        // aggregate pollution from before F-D1) are dropped during the
        // rewrite. (F-D2)
        let flat = self.base_dir.join("experiences.jsonl");
        if flat.exists() {
            let cutoff_dt = chrono::Local::now() - chrono::Duration::days(max_age_days);
            if let Ok(content) = tokio::fs::read_to_string(&flat).await {
                let total: usize = content.lines().filter(|l| !l.trim().is_empty()).count();
                let mut kept = 0usize;
                let mut new_content = String::new();
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    // Non-JSON lines (legacy aggregate pollution) are dropped.
                    if let Ok(ce) = serde_json::from_str::<CollectedExperience>(line) {
                        let young = chrono::DateTime::parse_from_rfc3339(&ce.experience.timestamp)
                            .map(|t| t.with_timezone(&chrono::Local) >= cutoff_dt)
                            .unwrap_or(true); // unparseable ts → keep (don't lose data)
                        if young {
                            new_content.push_str(line);
                            new_content.push('\n');
                            kept += 1;
                        }
                    }
                }
                // Atomic rewrite (temp + rename) so a crash can't truncate the
                // file. BUG-20261025-59：唯一临时名权威 helper——旧固定
                // `experiences.jsonl.tmp` 与 Collector 对同一活文件的 append
                // 共享目录，双写者同名互踩可产损坏内容被 rename 进活文件；
                // 且旧名被残留垃圾占用时重写被静默吞掉。清理是 best-effort：
                // 失败 warn 不上浮（与整个 cleanup 的容忍姿态一致）。
                if let Err(e) = nemesis_utils::file::write_file_atomic(
                    &flat.to_string_lossy(),
                    new_content.as_bytes(),
                    0o644,
                ) {
                    tracing::warn!(error = %e, "[ExperienceStore] flat trim rewrite failed");
                }
                removed += total.saturating_sub(kept);
            }
        }

        Ok(removed)
    }

    /// Read all experiences from the simple JSONL file (flat format).
    pub async fn read_all(&self) -> std::io::Result<Vec<CollectedExperience>> {
        let file_path = self.base_dir.join("experiences.jsonl");
        if !file_path.exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(&file_path).await?;
        let mut experiences = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(exp) = serde_json::from_str::<CollectedExperience>(line) {
                experiences.push(exp);
            }
        }
        Ok(experiences)
    }

    /// Read up to the `limit` most-recent experiences from the flat file.
    /// Reads the file once but **parses only the last `limit` lines**, bounding
    /// parse cost regardless of file size. Used by the reflection cycle so a
    /// large (age-bounded) file doesn't make each 6h tick increasingly slow.
    /// (F-P2)
    pub async fn read_recent(&self, limit: usize) -> std::io::Result<Vec<CollectedExperience>> {
        let file_path = self.base_dir.join("experiences.jsonl");
        if !file_path.exists() {
            return Ok(Vec::new());
        }
        let content = tokio::fs::read_to_string(&file_path).await?;
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = lines.len().saturating_sub(limit.max(1));
        let mut experiences = Vec::new();
        for line in &lines[start..] {
            if let Ok(exp) = serde_json::from_str::<CollectedExperience>(line) {
                experiences.push(exp);
            }
        }
        Ok(experiences)
    }

    /// Count the number of stored experiences (simple format).
    pub async fn count(&self) -> std::io::Result<usize> {
        let exps = self.read_all().await?;
        Ok(exps.len())
    }

    /// Remove all experiences from the store.
    pub async fn clear(&self) -> std::io::Result<()> {
        let file_path = self.base_dir.join("experiences.jsonl");
        if file_path.exists() {
            tokio::fs::remove_file(&file_path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
