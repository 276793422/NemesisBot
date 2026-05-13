//! Episodic memory store for conversation episodes.
//!
//! An "episode" is a single message within a conversation session.
//! `FileEpisodicStore` persists episodes as JSONL (one JSON object per line)
//! inside a directory organised by session key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

// ---------------------------------------------------------------------------
// Episode data type
// ---------------------------------------------------------------------------

/// A single conversation episode (one message in a session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Session key grouping related episodes together.
    pub session_key: String,
    /// Message role: "user", "assistant", "system", etc.
    pub role: String,
    /// Message content.
    pub content: String,
    /// When this episode was created.
    pub timestamp: DateTime<Utc>,
    /// Arbitrary metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Episode {
    /// Create a new episode with auto-generated ID and current timestamp.
    pub fn new(session_key: String, role: String, content: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            session_key,
            role,
            content,
            timestamp: Utc::now(),
            metadata: HashMap::new(),
            tags: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Async interface for an episodic memory backend.
#[async_trait]
pub trait EpisodicStore: Send + Sync {
    /// Append an episode to the store.
    async fn append(&self, episode: Episode) -> Result<String, String>;

    /// Retrieve all episodes for a given session, ordered by timestamp.
    async fn get_session(&self, session_key: &str) -> Result<Vec<Episode>, String>;

    /// Get the most recent N episodes for a session.
    async fn get_recent(&self, session_key: &str, limit: usize) -> Result<Vec<Episode>, String>;

    /// Search episodes by text query across all sessions.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<Episode>, String>;

    /// Delete all episodes for a session. Returns count of deleted episodes.
    async fn delete_session(&self, session_key: &str) -> Result<usize, String>;

    /// Remove episodes older than the given number of days. Returns count removed.
    async fn cleanup(&self, older_than_days: usize) -> Result<usize, String>;

    /// List known session keys.
    async fn list_sessions(&self) -> Result<Vec<String>, String>;

    /// Return the number of sessions stored.
    async fn session_count(&self) -> Result<usize, String>;

    /// Return the total number of episodes across all sessions.
    async fn episode_count(&self) -> Result<usize, String>;
}

// ---------------------------------------------------------------------------
// FileEpisodicStore
// ---------------------------------------------------------------------------

/// JSONL-backed episodic store. Each session is a separate `.jsonl` file
/// inside the configured `data_dir`.
pub struct FileEpisodicStore {
    data_dir: PathBuf,
}

impl FileEpisodicStore {
    /// Create a new store rooted at `data_dir`. The directory is created
    /// lazily on first write.
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// Sanitise a session key so it can be used as a file name.
    fn session_file(&self, session_key: &str) -> PathBuf {
        let safe_name = session_key.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        self.data_dir.join(format!("{safe_name}.jsonl"))
    }
}

#[async_trait]
impl EpisodicStore for FileEpisodicStore {
    async fn append(&self, episode: Episode) -> Result<String, String> {
        // Ensure directory exists.
        fs::create_dir_all(&self.data_dir)
            .await
            .map_err(|e| format!("Failed to create episodic dir: {e}"))?;

        let path = self.session_file(&episode.session_key);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| format!("Failed to open episode file: {e}"))?;

        let mut line = serde_json::to_string(&episode)
            .map_err(|e| format!("Failed to serialize episode: {e}"))?;
        line.push('\n');

        file.write_all(line.as_bytes())
            .await
            .map_err(|e| format!("Failed to write episode: {e}"))?;

        file.flush().await.map_err(|e| format!("Flush failed: {e}"))?;

        Ok(episode.id)
    }

    async fn get_session(&self, session_key: &str) -> Result<Vec<Episode>, String> {
        let path = self.session_file(session_key);
        if !path.exists() {
            return Ok(Vec::new());
        }

        let data = fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read session file: {e}"))?;

        let mut episodes: Vec<Episode> = data
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        episodes.sort_by_key(|e| e.timestamp);
        Ok(episodes)
    }

    async fn delete_session(&self, session_key: &str) -> Result<usize, String> {
        let path = self.session_file(session_key);
        if !path.exists() {
            return Ok(0);
        }

        let data = fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read session file: {e}"))?;

        let count = data.lines().filter(|l| !l.trim().is_empty()).count();

        fs::remove_file(&path)
            .await
            .map_err(|e| format!("Failed to delete session file: {e}"))?;

        Ok(count)
    }

    async fn get_recent(&self, session_key: &str, limit: usize) -> Result<Vec<Episode>, String> {
        let mut episodes = self.get_session(session_key).await?;
        let limit = if limit == 0 { 10 } else { limit };
        if episodes.len() > limit {
            let start = episodes.len() - limit;
            episodes = episodes.split_off(start);
        }
        Ok(episodes)
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<Episode>, String> {
        let limit = if limit == 0 { 20 } else { limit };
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        let sessions = self.list_sessions().await?;
        for session_key in &sessions {
            let episodes = self.get_session(session_key).await?;
            for ep in &episodes {
                if ep.content.to_lowercase().contains(&query_lower) {
                    results.push(ep.clone());
                    if results.len() >= limit {
                        return Ok(results);
                    }
                }
            }
        }

        // Also search tags if we haven't hit the limit yet
        if results.len() < limit {
            let mut seen_ids: std::collections::HashSet<String> =
                results.iter().map(|e| e.id.clone()).collect();
            for session_key in &sessions {
                let episodes = self.get_session(session_key).await?;
                for ep in &episodes {
                    if seen_ids.contains(&ep.id) {
                        continue;
                    }
                    if ep
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
                    {
                        results.push(ep.clone());
                        seen_ids.insert(ep.id.clone());
                        if results.len() >= limit {
                            return Ok(results);
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    async fn cleanup(&self, older_than_days: usize) -> Result<usize, String> {
        let cutoff = chrono::Utc::now()
            - chrono::Duration::days(older_than_days as i64);

        let sessions = self.list_sessions().await?;
        let mut total_removed = 0;

        for session_key in &sessions {
            let episodes = self.get_session(session_key).await?;
            let mut remaining = Vec::new();
            for ep in &episodes {
                if ep.timestamp > cutoff {
                    remaining.push(ep.clone());
                }
            }

            let removed = episodes.len() - remaining.len();
            if removed > 0 {
                total_removed += removed;
                if remaining.is_empty() {
                    // Delete the whole session file
                    self.delete_session(session_key).await?;
                } else {
                    // Rewrite the session file with remaining episodes
                    let path = self.session_file(session_key);
                    let mut lines = Vec::new();
                    for ep in &remaining {
                        let line = serde_json::to_string(ep)
                            .map_err(|e| format!("Failed to serialize episode: {e}"))?;
                        lines.push(line);
                    }
                    let content = lines.join("\n") + "\n";
                    fs::write(&path, content)
                        .await
                        .map_err(|e| format!("Failed to rewrite session file: {e}"))?;
                }
            }
        }

        Ok(total_removed)
    }

    async fn session_count(&self) -> Result<usize, String> {
        let sessions = self.list_sessions().await?;
        Ok(sessions.len())
    }

    async fn episode_count(&self) -> Result<usize, String> {
        let sessions = self.list_sessions().await?;
        let mut total = 0;
        for session_key in &sessions {
            let episodes = self.get_session(session_key).await?;
            total += episodes.len();
        }
        Ok(total)
    }

    async fn list_sessions(&self) -> Result<Vec<String>, String> {
        if !self.data_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = fs::read_dir(&self.data_dir)
            .await
            .map_err(|e| format!("Failed to read episodic dir: {e}"))?;

        let mut sessions = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| format!("Dir entry error: {e}"))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    sessions.push(stem.to_string());
                }
            }
        }

        sessions.sort();
        Ok(sessions)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_new_has_valid_id() {
        let ep = Episode::new(
            "sess-1".to_string(),
            "user".to_string(),
            "hello".to_string(),
        );
        assert_eq!(ep.id.len(), 36);
        assert_eq!(ep.session_key, "sess-1");
        assert_eq!(ep.role, "user");
        assert_eq!(ep.content, "hello");
        assert!(ep.metadata.is_empty());
        assert!(ep.tags.is_empty());
    }

    #[tokio::test]
    async fn file_store_append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        let ep1 = Episode::new("s1".into(), "user".into(), "hi there".into());
        let ep2 = Episode::new("s1".into(), "assistant".into(), "hello!".into());

        store.append(ep1.clone()).await.unwrap();
        store.append(ep2.clone()).await.unwrap();

        let episodes = store.get_session("s1").await.unwrap();
        assert_eq!(episodes.len(), 2);
        assert_eq!(episodes[0].content, "hi there");
        assert_eq!(episodes[1].content, "hello!");
    }

    #[tokio::test]
    async fn file_store_list_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("alpha".into(), "user".into(), "a".into()))
            .await
            .unwrap();
        store
            .append(Episode::new("beta".into(), "user".into(), "b".into()))
            .await
            .unwrap();

        let sessions = store.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
        // Sorted alphabetically.
        assert_eq!(sessions[0], "alpha");
        assert_eq!(sessions[1], "beta");
    }

    #[tokio::test]
    async fn file_store_delete_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("doom".into(), "user".into(), "bye".into()))
            .await
            .unwrap();

        let count = store.delete_session("doom").await.unwrap();
        assert_eq!(count, 1);

        let episodes = store.get_session("doom").await.unwrap();
        assert!(episodes.is_empty());

        // Deleting non-existent session returns 0.
        let count2 = store.delete_session("doom").await.unwrap();
        assert_eq!(count2, 0);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[tokio::test]
    async fn episode_serialization_roundtrip() {
        let ep = Episode::new("sess-1".into(), "user".into(), "hello world".into());
        let json = serde_json::to_string(&ep).unwrap();
        let deserialized: Episode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, ep.id);
        assert_eq!(deserialized.session_key, "sess-1");
        assert_eq!(deserialized.role, "user");
        assert_eq!(deserialized.content, "hello world");
    }

    #[tokio::test]
    async fn file_store_get_recent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        for i in 0..10 {
            store
                .append(Episode::new(
                    "sess-recent".into(),
                    "user".into(),
                    format!("message {}", i),
                ))
                .await
                .unwrap();
        }

        let recent = store.get_recent("sess-recent", 3).await.unwrap();
        assert_eq!(recent.len(), 3);
        // Should be the last 3 messages
        assert!(recent[0].content.contains("message 7"));
        assert!(recent[1].content.contains("message 8"));
        assert!(recent[2].content.contains("message 9"));
    }

    #[tokio::test]
    async fn file_store_get_recent_zero_defaults_to_ten() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        for i in 0..15 {
            store
                .append(Episode::new("sess-zerolim".into(), "user".into(), format!("m{}", i)))
                .await
                .unwrap();
        }

        let recent = store.get_recent("sess-zerolim", 0).await.unwrap();
        assert_eq!(recent.len(), 10); // defaults to 10
    }

    #[tokio::test]
    async fn file_store_get_recent_more_than_total() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("sess-small".into(), "user".into(), "only one".into()))
            .await
            .unwrap();

        let recent = store.get_recent("sess-small", 10).await.unwrap();
        assert_eq!(recent.len(), 1);
    }

    #[tokio::test]
    async fn file_store_search_by_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("s1".into(), "user".into(), "The Eiffel Tower is in Paris".into()))
            .await
            .unwrap();
        store
            .append(Episode::new("s2".into(), "user".into(), "The Colosseum is in Rome".into()))
            .await
            .unwrap();
        store
            .append(Episode::new("s3".into(), "user".into(), "Big Ben is in London".into()))
            .await
            .unwrap();

        let results = store.search("Eiffel", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].content.contains("Eiffel"));
    }

    #[tokio::test]
    async fn file_store_search_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("s1".into(), "user".into(), "RUST programming".into()))
            .await
            .unwrap();

        let results = store.search("rust", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn file_store_search_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        for i in 0..10 {
            store
                .append(Episode::new(
                    format!("s{}", i),
                    "user".into(),
                    "common keyword here".into(),
                ))
                .await
                .unwrap();
        }

        let results = store.search("keyword", 3).await.unwrap();
        assert!(results.len() <= 3);
    }

    #[tokio::test]
    async fn file_store_search_by_tags() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        let mut ep = Episode::new("s1".into(), "user".into(), "some content".into());
        ep.tags.push("important-tag".into());
        store.append(ep).await.unwrap();

        let results = store.search("important-tag", 10).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn file_store_session_count() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store.append(Episode::new("a".into(), "user".into(), "a".into())).await.unwrap();
        store.append(Episode::new("b".into(), "user".into(), "b".into())).await.unwrap();
        store.append(Episode::new("c".into(), "user".into(), "c".into())).await.unwrap();

        let count = store.session_count().await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn file_store_episode_count() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        for i in 0..5 {
            store.append(Episode::new("s1".into(), "user".into(), format!("msg {}", i))).await.unwrap();
        }
        store.append(Episode::new("s2".into(), "user".into(), "other".into())).await.unwrap();

        let count = store.episode_count().await.unwrap();
        assert_eq!(count, 6);
    }

    #[tokio::test]
    async fn file_store_cleanup_old_episodes() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        // Old episode
        let mut old = Episode::new("s-old".into(), "user".into(), "old content".into());
        old.timestamp = chrono::Utc::now() - chrono::Duration::days(10);
        store.append(old).await.unwrap();

        // Recent episode
        store
            .append(Episode::new("s-recent".into(), "user".into(), "recent content".into()))
            .await
            .unwrap();

        let removed = store.cleanup(5).await.unwrap();
        assert_eq!(removed, 1);

        let sessions = store.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions.contains(&"s-recent".to_string()));
    }

    #[tokio::test]
    async fn file_store_cleanup_nothing_old() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("s1".into(), "user".into(), "fresh".into()))
            .await
            .unwrap();

        let removed = store.cleanup(365).await.unwrap();
        assert_eq!(removed, 0);
    }

    #[tokio::test]
    async fn file_store_get_session_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        let episodes = store.get_session("nonexistent").await.unwrap();
        assert!(episodes.is_empty());
    }

    #[tokio::test]
    async fn file_store_list_sessions_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        let sessions = store.list_sessions().await.unwrap();
        assert!(sessions.is_empty());
    }

    #[tokio::test]
    async fn file_store_session_file_sanitization() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        // Session key with special characters
        store
            .append(Episode::new("sess/with:chars*?".into(), "user".into(), "sanitized".into()))
            .await
            .unwrap();

        let sessions = store.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
        // The file should exist with sanitized name
        assert!(!sessions[0].contains('/'));
    }

    #[tokio::test]
    async fn file_store_episodes_ordered_by_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store
            .append(Episode::new("s1".into(), "user".into(), "first".into()))
            .await
            .unwrap();
        store
            .append(Episode::new("s1".into(), "assistant".into(), "second".into()))
            .await
            .unwrap();
        store
            .append(Episode::new("s1".into(), "user".into(), "third".into()))
            .await
            .unwrap();

        let episodes = store.get_session("s1").await.unwrap();
        assert_eq!(episodes.len(), 3);
        assert_eq!(episodes[0].content, "first");
        assert_eq!(episodes[1].content, "second");
        assert_eq!(episodes[2].content, "third");
    }

    #[tokio::test]
    async fn file_store_multiple_roles() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        store.append(Episode::new("s1".into(), "system".into(), "system prompt".into())).await.unwrap();
        store.append(Episode::new("s1".into(), "user".into(), "user query".into())).await.unwrap();
        store.append(Episode::new("s1".into(), "assistant".into(), "response".into())).await.unwrap();

        let episodes = store.get_session("s1").await.unwrap();
        assert_eq!(episodes[0].role, "system");
        assert_eq!(episodes[1].role, "user");
        assert_eq!(episodes[2].role, "assistant");
    }

    #[tokio::test]
    async fn file_store_episode_with_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileEpisodicStore::new(dir.path());

        let mut ep = Episode::new("s1".into(), "user".into(), "with meta".into());
        ep.metadata.insert("source".into(), "api".into());
        ep.metadata.insert("version".into(), "1.0".into());
        store.append(ep).await.unwrap();

        let episodes = store.get_session("s1").await.unwrap();
        assert_eq!(episodes[0].metadata.get("source").unwrap(), "api");
    }
}
