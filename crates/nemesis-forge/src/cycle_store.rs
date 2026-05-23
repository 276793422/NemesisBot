//! Cycle store - JSONL persistence for learning cycle records.
//!
//! Each learning cycle produces a summary record that is appended to a JSONL
//! file organized by month: `learning/YYYYMM/YYYYMMDD.jsonl`. Only the summary
//! is stored, not the full skill drafts.
//!
//! Supports:
//! - `read_cycles(since)` - time-filtered reading
//! - `cleanup(max_age_days)` - remove expired files
//! - `load_latest_cycle()` - most recent cycle across all files

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use nemesis_types::forge::LearningCycle;

/// A cycle store persists learning cycle summaries to JSONL files
/// organized by month directory.
///
/// Directory structure: `{base_dir}/YYYYMM/YYYYMMDD.jsonl`
pub struct CycleStore {
    base_dir: PathBuf,
}

impl CycleStore {
    /// Create a new cycle store rooted at `forge_dir/learning`.
    pub fn new(forge_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: forge_dir.into().join("learning"),
        }
    }

    /// Create from an explicit base directory (no "learning" suffix).
    pub fn from_base(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Append a cycle record to today's JSONL file (month-based directory).
    pub async fn append(&self, cycle: &LearningCycle) -> std::io::Result<()> {
        // Parse started_at to determine the date for file naming
        let date_str = &cycle.started_at.get(..10).unwrap_or("19700101");
        let date_str_clean = date_str.replace('-', "");
        let month_part = &date_str_clean[..6];
        let day_part = &date_str_clean;

        let month_dir = self.base_dir.join(month_part);
        tokio::fs::create_dir_all(&month_dir).await?;

        let file_path = month_dir.join(format!("{}.jsonl", day_part));

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?;

        let mut line = serde_json::to_string(cycle).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Read all cycle records, optionally filtered by `since` timestamp.
    ///
    /// If `since` is `None`, returns all records. The filtering is done by
    /// checking the filename date (YYYYMMDD.jsonl) against the `since` date.
    pub async fn read_cycles(&self, since: Option<chrono::DateTime<chrono::Utc>>) -> std::io::Result<Vec<LearningCycle>> {
        let mut results = Vec::new();

        if !self.base_dir.exists() {
            return Ok(results);
        }

        let since_date_str = since.map(|s| s.format("%Y%m%d").to_string());

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

                // Filter by since date using filename
                if let Some(ref since_str) = since_date_str {
                    let date_part = name_str.trim_end_matches(".jsonl");
                    if date_part < since_str.as_str() {
                        continue; // file is older than since
                    }
                }

                let content = tokio::fs::read_to_string(file_entry.path()).await?;
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(cycle) = serde_json::from_str::<LearningCycle>(line) {
                        results.push(cycle);
                    }
                }
            }
        }

        Ok(results)
    }

    /// Read all cycle records from the JSONL files (convenience wrapper).
    pub async fn read_all(&self) -> std::io::Result<Vec<LearningCycle>> {
        self.read_cycles(None).await
    }

    /// Get the latest cycle record across all files.
    pub async fn load_latest_cycle(&self) -> std::io::Result<Option<LearningCycle>> {
        let cycles = self.read_cycles(None).await?;
        Ok(cycles.into_iter().last())
    }

    /// Get the latest cycle record (convenience alias).
    pub async fn get_latest(&self) -> std::io::Result<Option<LearningCycle>> {
        self.load_latest_cycle().await
    }

    /// Get the number of stored cycles.
    pub async fn count(&self) -> std::io::Result<usize> {
        let cycles = self.read_all().await?;
        Ok(cycles.len())
    }

    /// Remove cycle files older than `max_age_days`.
    ///
    /// Walks the month-based directory structure and deletes JSONL files
    /// whose filename date is older than the cutoff.
    pub async fn cleanup(&self, max_age_days: i64) -> std::io::Result<usize> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
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

        Ok(removed)
    }
}

#[cfg(test)]
mod tests;
