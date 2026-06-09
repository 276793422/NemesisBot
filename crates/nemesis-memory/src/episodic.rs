//! Episodic memory store for conversation episodes.
//!
//! An "episode" is a single message within a conversation session.
//! `FileEpisodicStore` persists episodes as JSONL (one JSON object per line)
//! inside a directory organised by session key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Local};
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
    pub timestamp: DateTime<Local>,
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
            timestamp: Local::now(),
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

    /// Delete a single episode by ID across all sessions. Returns true if found.
    async fn delete_by_id(&self, id: &str) -> Result<bool, String>;

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

    async fn delete_by_id(&self, id: &str) -> Result<bool, String> {
        let sessions = self.list_sessions().await?;
        for session_key in &sessions {
            let episodes = self.get_session(session_key).await?;
            let mut remaining = Vec::new();
            let mut found = false;
            for ep in &episodes {
                if ep.id == id {
                    found = true;
                } else {
                    remaining.push(ep.clone());
                }
            }
            if found {
                if remaining.is_empty() {
                    self.delete_session(session_key).await?;
                } else {
                    let path = self.session_file(session_key);
                    let mut out = String::new();
                    for ep in &remaining {
                        out.push_str(&serde_json::to_string(ep).unwrap_or_default());
                        out.push('\n');
                    }
                    fs::write(&path, out)
                        .await
                        .map_err(|e| format!("Failed to rewrite session file: {e}"))?;
                }
                return Ok(true);
            }
        }
        Ok(false)
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
        let cutoff = chrono::Local::now()
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
mod tests;
