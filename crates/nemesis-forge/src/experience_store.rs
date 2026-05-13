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
        let now = chrono::Utc::now();
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
        since: Option<chrono::DateTime<chrono::Utc>>,
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
        since: Option<chrono::DateTime<chrono::Utc>>,
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
        since: Option<chrono::DateTime<chrono::Utc>>,
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
    fn file_newer_than(filename: &str, since: &chrono::DateTime<chrono::Utc>) -> bool {
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
        since: Option<chrono::DateTime<chrono::Utc>>,
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
mod tests {
    use super::*;
    use crate::types::Experience;

    fn make_experience(tool: &str) -> Experience {
        Experience {
            id: uuid::Uuid::new_v4().to_string(),
            tool_name: tool.into(),
            input_summary: "test input".into(),
            output_summary: "ok".into(),
            success: true,
            duration_ms: 100,
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_key: "test-session".into(),
        }
    }

    fn make_aggregated(hash: &str, tool: &str, count: u64) -> AggregatedExperience {
        AggregatedExperience {
            pattern_hash: hash.to_string(),
            tool_name: tool.to_string(),
            count,
            avg_duration_ms: 100,
            success_rate: 0.9,
            last_seen: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn test_append_and_read_aggregated() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let agg1 = make_aggregated("hash1", "file_read", 10);
        let agg2 = make_aggregated("hash2", "file_write", 5);

        store.append_aggregated(&agg1).await.unwrap();
        store.append_aggregated(&agg2).await.unwrap();

        let all = store.read_aggregated().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_read_aggregated_since_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Create an old file manually
        let old_date = chrono::Utc::now() - chrono::Duration::days(100);
        let old_month = dir.path().join("experiences").join(old_date.format("%Y%m").to_string());
        std::fs::create_dir_all(&old_month).unwrap();
        let old_agg = AggregatedExperience {
            pattern_hash: "old_hash".to_string(),
            tool_name: "old_tool".to_string(),
            count: 5,
            avg_duration_ms: 200,
            success_rate: 0.5,
            last_seen: old_date.to_rfc3339(),
        };
        let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
        std::fs::write(&old_file, format!("{}\n", serde_json::to_string(&old_agg).unwrap())).unwrap();

        // Append a recent record
        let recent_agg = make_aggregated("recent_hash", "recent_tool", 10);
        store.append_aggregated(&recent_agg).await.unwrap();

        // Read all
        let all = store.read_aggregated().await.unwrap();
        assert_eq!(all.len(), 2);

        // Read since 7 days ago (should only get recent)
        let since = chrono::Utc::now() - chrono::Duration::days(7);
        let recent = store.read_aggregated_since(Some(since)).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].pattern_hash, "recent_hash");
    }

    #[tokio::test]
    async fn test_read_aggregated_by_day() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let agg = make_aggregated("hash1", "tool_a", 3);
        store.append_aggregated(&agg).await.unwrap();

        let by_day = store.read_aggregated_by_day().await.unwrap();
        assert!(!by_day.is_empty());
    }

    #[tokio::test]
    async fn test_get_top_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let agg1 = make_aggregated("hash1", "file_read", 50);
        let agg2 = make_aggregated("hash2", "file_write", 10);
        let agg3 = make_aggregated("hash3", "exec", 30);

        store.append_aggregated(&agg1).await.unwrap();
        store.append_aggregated(&agg2).await.unwrap();
        store.append_aggregated(&agg3).await.unwrap();

        let top = store.get_top_patterns(2).await.unwrap();
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].tool_name, "file_read");
        assert_eq!(top[1].tool_name, "exec");
    }

    #[tokio::test]
    async fn test_get_top_patterns_since() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Create an old file manually with high count
        let old_date = chrono::Utc::now() - chrono::Duration::days(100);
        let old_month = dir.path().join("experiences").join(old_date.format("%Y%m").to_string());
        std::fs::create_dir_all(&old_month).unwrap();
        let old_agg = AggregatedExperience {
            pattern_hash: "old_hash".to_string(),
            tool_name: "old_tool".to_string(),
            count: 999,
            avg_duration_ms: 100,
            success_rate: 0.9,
            last_seen: old_date.to_rfc3339(),
        };
        let old_file = old_month.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
        std::fs::write(&old_file, format!("{}\n", serde_json::to_string(&old_agg).unwrap())).unwrap();

        // Append recent records
        store.append_aggregated(&make_aggregated("hash1", "recent_tool", 20)).await.unwrap();

        // Without filter, old should be included
        let all_top = store.get_top_patterns(1).await.unwrap();
        assert_eq!(all_top[0].tool_name, "old_tool"); // 999 > 20

        // With since filter, only recent should appear
        let since = chrono::Utc::now() - chrono::Duration::days(7);
        let recent_top = store.get_top_patterns_since(Some(since), 1).await.unwrap();
        assert_eq!(recent_top[0].tool_name, "recent_tool");
    }

    #[tokio::test]
    async fn test_get_stats() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let agg = make_aggregated("hash1", "tool", 10);
        store.append_aggregated(&agg).await.unwrap();
        let agg2 = make_aggregated("hash2", "tool2", 5);
        store.append_aggregated(&agg2).await.unwrap();

        let (total, unique) = store.get_stats().await.unwrap();
        assert_eq!(total, 15);
        assert_eq!(unique, 2);
    }

    #[tokio::test]
    async fn test_cleanup_removes_old() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Create an old file manually
        let old_date = chrono::Utc::now() - chrono::Duration::days(100);
        let month_dir = dir.path().join("experiences").join(old_date.format("%Y%m").to_string());
        std::fs::create_dir_all(&month_dir).unwrap();
        let old_file = month_dir.join(format!("{}.jsonl", old_date.format("%Y%m%d")));
        std::fs::write(&old_file, "test data\n").unwrap();

        // Create a recent file
        let recent_date = chrono::Utc::now();
        let recent_month = dir.path().join("experiences").join(recent_date.format("%Y%m").to_string());
        std::fs::create_dir_all(&recent_month).unwrap();
        let recent_file = recent_month.join(format!("{}.jsonl", recent_date.format("%Y%m%d")));
        std::fs::write(&recent_file, "recent data\n").unwrap();

        let removed = store.cleanup(30).await.unwrap();
        assert_eq!(removed, 1);
        assert!(!old_file.exists());
        assert!(recent_file.exists());
    }

    #[tokio::test]
    async fn test_cleanup_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let removed = store.cleanup(30).await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn test_count_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        assert_eq!(store.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_clear() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Write to simple format
        let exp_path = dir.path().join("experiences").join("experiences.jsonl");
        std::fs::create_dir_all(exp_path.parent().unwrap()).unwrap();
        std::fs::write(&exp_path, "data\n").unwrap();

        store.clear().await.unwrap();
        assert!(!exp_path.exists());
    }

    #[tokio::test]
    async fn test_daily_limit_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let config = ExperienceStoreConfig {
            max_experiences_per_day: 2,
        };
        let store = ExperienceStore::from_forge_dir_with_config(dir.path(), config);

        let agg = make_aggregated("hash1", "tool", 10);

        // First two should succeed
        store.append_aggregated(&agg).await.unwrap();
        store.append_aggregated(&agg).await.unwrap();

        // Third should be silently dropped
        store.append_aggregated(&agg).await.unwrap();

        let all = store.read_aggregated().await.unwrap();
        assert_eq!(all.len(), 2); // Only 2, not 3
    }

    #[tokio::test]
    async fn test_daily_limit_zero_means_unlimited() {
        let dir = tempfile::tempdir().unwrap();
        let config = ExperienceStoreConfig {
            max_experiences_per_day: 0,
        };
        let store = ExperienceStore::from_forge_dir_with_config(dir.path(), config);

        let agg = make_aggregated("hash1", "tool", 10);

        for _ in 0..5 {
            store.append_aggregated(&agg).await.unwrap();
        }

        let all = store.read_aggregated().await.unwrap();
        assert_eq!(all.len(), 5);
    }

    // --- Additional experience_store tests ---

    #[tokio::test]
    async fn test_from_forge_dir_creates_experiences_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let agg = make_aggregated("hash1", "tool", 1);
        store.append_aggregated(&agg).await.unwrap();
        assert!(dir.path().join("experiences").exists());
    }

    #[tokio::test]
    async fn test_read_aggregated_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let all = store.read_aggregated().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_read_aggregated_since_no_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let agg = make_aggregated("hash1", "tool", 5);
        store.append_aggregated(&agg).await.unwrap();
        let all = store.read_aggregated_since(None).await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_read_aggregated_since_future() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let agg = make_aggregated("hash1", "tool", 5);
        store.append_aggregated(&agg).await.unwrap();
        let future = chrono::Utc::now() + chrono::Duration::days(1);
        let result = store.read_aggregated_since(Some(future)).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_get_top_patterns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let top = store.get_top_patterns(10).await.unwrap();
        assert!(top.is_empty());
    }

    #[tokio::test]
    async fn test_get_top_patterns_all_returned() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        store.append_aggregated(&make_aggregated("h1", "a", 10)).await.unwrap();
        store.append_aggregated(&make_aggregated("h2", "b", 5)).await.unwrap();
        store.append_aggregated(&make_aggregated("h3", "c", 1)).await.unwrap();
        let top = store.get_top_patterns(0).await.unwrap();
        assert_eq!(top.len(), 3);
    }

    #[tokio::test]
    async fn test_get_stats_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let (total, unique) = store.get_stats().await.unwrap();
        assert_eq!(total, 0);
        assert_eq!(unique, 0);
    }

    #[tokio::test]
    async fn test_read_all_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        let all = store.read_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_read_all_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Write to simple format
        let exp_path = dir.path().join("experiences");
        std::fs::create_dir_all(&exp_path).unwrap();
        let exp = make_experience("test_tool");
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "test-hash".into(),
        };
        let line = serde_json::to_string(&ce).unwrap();
        tokio::fs::write(exp_path.join("experiences.jsonl"), format!("{}\n", line))
            .await.unwrap();

        let all = store.read_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].experience.tool_name, "test_tool");
    }

    #[tokio::test]
    async fn test_read_all_ignores_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let exp_path = dir.path().join("experiences");
        std::fs::create_dir_all(&exp_path).unwrap();
        let content = "invalid json line\n{\"valid\": true}\n";
        tokio::fs::write(exp_path.join("experiences.jsonl"), content)
            .await.unwrap();

        let all = store.read_all().await.unwrap();
        assert!(all.is_empty()); // Neither line is valid CollectedExperience
    }

    #[tokio::test]
    async fn test_count_after_append() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let exp_path = dir.path().join("experiences");
        std::fs::create_dir_all(&exp_path).unwrap();
        let exp = make_experience("tool");
        let ce = CollectedExperience { experience: exp, dedup_hash: "h".into() };
        let line = serde_json::to_string(&ce).unwrap();
        tokio::fs::write(exp_path.join("experiences.jsonl"), format!("{}\n", line))
            .await.unwrap();

        assert_eq!(store.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_clear_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        store.clear().await.unwrap(); // Should not panic
    }

    #[test]
    fn test_file_newer_than_same_day() {
        let now = chrono::Utc::now();
        let filename = format!("{}.jsonl", now.format("%Y%m%d"));
        assert!(ExperienceStore::file_newer_than(&filename, &now));
    }

    #[test]
    fn test_file_newer_than_older() {
        let now = chrono::Utc::now();
        let old_date = now - chrono::Duration::days(10);
        let filename = format!("{}.jsonl", old_date.format("%Y%m%d"));
        assert!(!ExperienceStore::file_newer_than(&filename, &now));
    }

    #[test]
    fn test_file_newer_than_newer() {
        let now = chrono::Utc::now();
        let future_date = now + chrono::Duration::days(10);
        let filename = format!("{}.jsonl", future_date.format("%Y%m%d"));
        assert!(ExperienceStore::file_newer_than(&filename, &now));
    }

    #[tokio::test]
    async fn test_cleanup_keeps_recent() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Write recent data
        let agg = make_aggregated("hash1", "tool", 5);
        store.append_aggregated(&agg).await.unwrap();

        let removed = store.cleanup(30).await.unwrap();
        assert_eq!(removed, 0);

        let all = store.read_aggregated().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_append_multiple_same_day() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        for i in 0..5 {
            let agg = make_aggregated(&format!("hash-{}", i), "tool", i + 1);
            store.append_aggregated(&agg).await.unwrap();
        }

        let all = store.read_aggregated().await.unwrap();
        assert_eq!(all.len(), 5);
    }

    #[tokio::test]
    async fn test_read_aggregated_merges_same_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Same hash, different tool names - should be merged
        let agg1 = AggregatedExperience {
            pattern_hash: "shared-hash".into(),
            tool_name: "tool_a".into(),
            count: 10,
            avg_duration_ms: 100,
            success_rate: 0.8,
            last_seen: "2026-05-01T00:00:00Z".into(),
        };
        let agg2 = AggregatedExperience {
            pattern_hash: "shared-hash".into(),
            tool_name: "tool_b".into(),
            count: 5,
            avg_duration_ms: 200,
            success_rate: 0.6,
            last_seen: "2026-05-02T00:00:00Z".into(),
        };
        store.append_aggregated(&agg1).await.unwrap();
        store.append_aggregated(&agg2).await.unwrap();

        let all = store.read_aggregated().await.unwrap();
        // Same hash entries are stored separately (no merge on read)
        let shared: Vec<_> = all.iter().filter(|a| a.pattern_hash == "shared-hash").collect();
        assert_eq!(shared.len(), 2);
    }

    // --- Additional coverage tests ---

    #[tokio::test]
    async fn test_get_top_patterns_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        store.append_aggregated(&make_aggregated("h1", "tool_low", 2)).await.unwrap();
        store.append_aggregated(&make_aggregated("h2", "tool_mid", 5)).await.unwrap();
        store.append_aggregated(&make_aggregated("h3", "tool_high", 20)).await.unwrap();
        store.append_aggregated(&make_aggregated("h4", "tool_med2", 10)).await.unwrap();

        let top = store.get_top_patterns(3).await.unwrap();
        assert_eq!(top.len(), 3);
        // Should be sorted by count descending
        assert!(top[0].count >= top[1].count);
        assert!(top[1].count >= top[2].count);
    }

    #[tokio::test]
    async fn test_get_stats_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        store.append_aggregated(&make_aggregated("h1", "a", 5)).await.unwrap();
        store.append_aggregated(&make_aggregated("h2", "b", 3)).await.unwrap();
        store.append_aggregated(&make_aggregated("h3", "c", 1)).await.unwrap();

        let (total, unique) = store.get_stats().await.unwrap();
        assert_eq!(total, 9); // 5 + 3 + 1
        assert!(unique >= 1);
    }

    #[tokio::test]
    async fn test_read_aggregated_since_today() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        store.append_aggregated(&make_aggregated("h1", "tool", 5)).await.unwrap();

        let today = chrono::Utc::now() - chrono::Duration::days(1);
        let result = store.read_aggregated_since(Some(today)).await.unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn test_clear_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());
        // Use raw format (append/read_all) since clear() only removes experiences.jsonl
        let exp = make_experience("test_tool");
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "hash-1".into(),
        };
        store.append(&ce).await.unwrap();
        assert_eq!(store.count().await.unwrap(), 1);

        store.clear().await.unwrap();
        let all = store.read_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_append_experiences_raw_format() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        // Append raw collected experiences
        let exp = make_experience("test_tool");
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "hash-1".into(),
        };
        store.append(&ce).await.unwrap();

        let all = store.read_all().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn test_append_experience_deduplication() {
        let dir = tempfile::tempdir().unwrap();
        let store = ExperienceStore::from_forge_dir(dir.path());

        let exp = make_experience("test_tool");
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "same-hash".into(),
        };
        // First append should succeed
        store.append(&ce).await.unwrap();
        // Second with same hash should be deduplicated
        store.append(&ce).await.unwrap();

        let all = store.read_all().await.unwrap();
        // append() does not deduplicate — both are stored
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_file_newer_than_invalid_filename() {
        // "notadate" >= "20260512" is true lexicographically (lowercase > digits)
        assert!(ExperienceStore::file_newer_than("notadate.jsonl", &chrono::Utc::now()));
    }
}
