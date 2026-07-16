//! Experience store - JSONL persistence for collected experiences.
//!
//! Provides read/write access to the experience JSONL file used by the
//! reflector for statistical analysis. Supports date-based directory
//! structure (YYYYMM/YYYYMMDD.jsonl), aggregation, top patterns, and cleanup.
//!
//! Matches Go's ExperienceStore with:
//! - `read_aggregated(since)` / `read_aggregated_by_day(since)` - time-filtered reading
//! - `get_top_patterns(since, top_n)` - time-filtered top patterns
//! - `append_aggregated` with optional daily limit
//! - `cleanup(max_age_days)` - remove expired files

use std::collections::HashMap;
use std::path::PathBuf;

use tokio::io::AsyncWriteExt;

use crate::types::{AggregatedExperience, CollectedExperience};

/// Configuration for the experience store (mirrors Go's reference to ForgeConfig).
#[derive(Debug, Clone)]
pub struct ExperienceStoreConfig {
    /// Maximum number of aggregated experiences to write per day (0 = unlimited).
    pub max_experiences_per_day: usize,
}

impl Default for ExperienceStoreConfig {
    fn default() -> Self {
        Self {
            max_experiences_per_day: 500,
        }
    }
}

/// An experience store that persists to JSONL with date-based organization.
pub struct ExperienceStore {
    base_dir: PathBuf,
    config: ExperienceStoreConfig,
}

impl ExperienceStore {
    /// Create a new experience store rooted at the given directory.
    ///
    /// Files are organized as: `{base_dir}/YYYYMM/YYYYMMDD.jsonl`
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: path.into(),
            config: ExperienceStoreConfig::default(),
        }
    }

    /// Create a new experience store with custom config.
    pub fn with_config(path: impl Into<PathBuf>, config: ExperienceStoreConfig) -> Self {
        Self {
            base_dir: path.into(),
            config,
        }
    }

    /// Create an experience store from a forge directory (adds "experiences" subdirectory).
    pub fn from_forge_dir(forge_dir: &std::path::Path) -> Self {
        Self {
            base_dir: forge_dir.join("experiences"),
            config: ExperienceStoreConfig::default(),
        }
    }

    /// Create an experience store from a forge directory with custom config.
    pub fn from_forge_dir_with_config(
        forge_dir: &std::path::Path,
        config: ExperienceStoreConfig,
    ) -> Self {
        Self {
            base_dir: forge_dir.join("experiences"),
            config,
        }
    }

    /// Append an aggregated experience to today's JSONL file.
    ///
    /// If `max_experiences_per_day` is set and the daily limit has been reached,
    /// the record is silently skipped (matching Go's behavior).
    pub async fn append_aggregated(&self, record: &AggregatedExperience) -> std::io::Result<()> {
        let now = chrono::Local::now();
        let month_dir = self.base_dir.join(now.format("%Y%m").to_string());
        tokio::fs::create_dir_all(&month_dir).await?;

        let file_path = month_dir.join(format!("{}.jsonl", now.format("%Y%m%d")));

        // Check daily limit (matching Go's behavior)
        if self.config.max_experiences_per_day > 0 {
            let count = self.count_lines_in_file(&file_path).await;
            if count >= self.config.max_experiences_per_day {
                return Ok(()); // Skip silently
            }
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .await?;

        let mut line = serde_json::to_string(record).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        Ok(())
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

        let mut line = serde_json::to_string(exp).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        line.push('\n');
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// Read all aggregated experiences from JSONL files (all time).
    pub async fn read_aggregated(&self) -> std::io::Result<Vec<AggregatedExperience>> {
        self.read_aggregated_since(None).await
    }

    /// Read aggregated experiences since the given time.
    ///
    /// The filtering is done at the file level by checking the filename date
    /// (YYYYMMDD.jsonl) against the `since` date. Files whose name-date is
    /// before `since` are skipped entirely.
    pub async fn read_aggregated_since(
        &self,
        since: Option<chrono::DateTime<chrono::Local>>,
    ) -> std::io::Result<Vec<AggregatedExperience>> {
        let mut results = Vec::new();
        self.walk_jsonl_files_since(&mut results, since).await?;
        Ok(results)
    }

    /// Read aggregated experiences grouped by day (all time).
    ///
    /// Returns a map of date string (YYYY-MM-DD) to a list of aggregated
    /// experiences for that day.
    pub async fn read_aggregated_by_day(
        &self,
    ) -> std::io::Result<HashMap<String, Vec<AggregatedExperience>>> {
        self.read_aggregated_by_day_since(None).await
    }

    /// Read aggregated experiences grouped by day, filtered since the given time.
    pub async fn read_aggregated_by_day_since(
        &self,
        since: Option<chrono::DateTime<chrono::Local>>,
    ) -> std::io::Result<HashMap<String, Vec<AggregatedExperience>>> {
        let records = self.read_aggregated_since(since).await?;
        let mut grouped: HashMap<String, Vec<AggregatedExperience>> = HashMap::new();

        for r in records {
            // Extract date from last_seen (assume ISO 8601 format)
            let day = r.last_seen.get(..10).unwrap_or("unknown").to_string();
            grouped.entry(day).or_default().push(r);
        }

        Ok(grouped)
    }

    /// Get the top N patterns by count (all time).
    ///
    /// Merges records by pattern_hash, sorts by count descending,
    /// and returns the top N.
    pub async fn get_top_patterns(&self, top_n: usize) -> std::io::Result<Vec<AggregatedExperience>> {
        self.get_top_patterns_since(None, top_n).await
    }

    /// Get the top N patterns by count since the given time.
    pub async fn get_top_patterns_since(
        &self,
        since: Option<chrono::DateTime<chrono::Local>>,
        top_n: usize,
    ) -> std::io::Result<Vec<AggregatedExperience>> {
        let records = self.read_aggregated_since(since).await?;

        // Merge by pattern hash
        let mut merged: HashMap<String, AggregatedExperience> = HashMap::new();
        for r in records {
            let entry = merged
                .entry(r.pattern_hash.clone())
                .or_insert_with(|| r.clone());
            entry.count += r.count;
            entry.avg_duration_ms = (entry.avg_duration_ms + r.avg_duration_ms) / 2;
            entry.success_rate = (entry.success_rate + r.success_rate) / 2.0;
            if r.last_seen > entry.last_seen {
                entry.last_seen = r.last_seen.clone();
            }
        }

        // Sort by count descending
        let mut sorted: Vec<AggregatedExperience> = merged.into_values().collect();
        sorted.sort_by(|a, b| b.count.cmp(&a.count));

        if top_n > 0 && sorted.len() > top_n {
            sorted.truncate(top_n);
        }

        Ok(sorted)
    }

    /// Get summary statistics (all time).
    ///
    /// Returns (total_records, unique_patterns).
    pub async fn get_stats(&self) -> std::io::Result<(usize, usize)> {
        let records = self.read_aggregated().await?;
        let total: u64 = records.iter().map(|r| r.count).sum();
        let unique = records.len();
        Ok((total as usize, unique))
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
        // timestamp and atomically rewrite. Legacy AggregatedExperience lines
        // (pollution from before F-D1) are dropped during the rewrite.
        // (F-D2)
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
                    match serde_json::from_str::<CollectedExperience>(line) {
                        Ok(ce) => {
                            let young = chrono::DateTime::parse_from_rfc3339(
                                &ce.experience.timestamp,
                            )
                            .map(|t| t.with_timezone(&chrono::Local) >= cutoff_dt)
                            .unwrap_or(true); // unparseable ts → keep (don't lose data)
                            if young {
                                new_content.push_str(line);
                                new_content.push('\n');
                                kept += 1;
                            }
                        }
                        Err(_) => {} // drop legacy aggregate pollution
                    }
                }
                // Atomic rewrite (temp + rename) so a crash can't truncate the file.
                let tmp = flat.with_extension("jsonl.tmp");
                if tokio::fs::write(&tmp, new_content.as_bytes()).await.is_ok() {
                    let _ = tokio::fs::rename(&tmp, &flat).await;
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

    /// Check if a JSONL filename (YYYYMMDD.jsonl) is newer than or equal to
    /// the given `since` time.
    fn file_newer_than(filename: &str, since: &chrono::DateTime<chrono::Local>) -> bool {
        let name = filename.trim_end_matches(".jsonl");
        let since_str = since.format("%Y%m%d").to_string();
        // Compare as string dates: YYYYMMDD format sorts lexicographically
        name >= since_str.as_str()
    }

    /// Count lines in a file (non-empty lines only).
    async fn count_lines_in_file(&self, path: &PathBuf) -> usize {
        if !path.exists() {
            return 0;
        }
        let content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(_) => return 0,
        };
        content.lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// Walk all JSONL files in the date-based directory structure, optionally
    /// filtering by `since` date using the filename.
    async fn walk_jsonl_files_since(
        &self,
        results: &mut Vec<AggregatedExperience>,
        since: Option<chrono::DateTime<chrono::Local>>,
    ) -> std::io::Result<()> {
        if !self.base_dir.exists() {
            return Ok(());
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

                // Apply time filter on filename
                if let Some(ref since_time) = since {
                    if !Self::file_newer_than(&name_str, since_time) {
                        continue;
                    }
                }

                let content = tokio::fs::read_to_string(file_entry.path()).await?;
                for line in content.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if let Ok(rec) = serde_json::from_str::<AggregatedExperience>(line) {
                        results.push(rec);
                    }
                }
            }
        }
        Ok(())
    }

    /// Walk all JSONL files (no time filter - kept for backward compat).
    #[allow(dead_code)]
    async fn walk_jsonl_files(
        &self,
        results: &mut Vec<AggregatedExperience>,
    ) -> std::io::Result<()> {
        self.walk_jsonl_files_since(results, None).await
    }
}

#[cfg(test)]
mod tests;
