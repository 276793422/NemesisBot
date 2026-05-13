//! Registry - tracks forge artifacts with a JSON index.
//!
//! Provides CRUD operations and version tracking. The index is optionally
//! persisted as a JSON file.

use parking_lot::Mutex;
use uuid::Uuid;

use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

use crate::types::RegistryConfig;

/// The registry manages a collection of forge artifacts.
pub struct Registry {
    config: RegistryConfig,
    artifacts: Mutex<Vec<Artifact>>,
}

impl Registry {
    /// Create a new empty registry.
    pub fn new(config: RegistryConfig) -> Self {
        Self {
            config,
            artifacts: Mutex::new(Vec::new()),
        }
    }

    /// Add a new artifact to the registry. Returns the artifact ID.
    ///
    /// If an artifact with the same name already exists, its version is
    /// bumped and the content is replaced. Persists to disk automatically
    /// if an index path is configured (mirrors Go's `Registry.Add`).
    pub fn add(&self, mut artifact: Artifact) -> String {
        let mut arts = self.artifacts.lock();

        // Set timestamps (mirrors Go's artifact.CreatedAt = time.Now().UTC())
        let now = chrono::Utc::now().to_rfc3339();
        artifact.created_at = now.clone();
        artifact.updated_at = now;

        // Check for existing artifact with same name and kind
        if let Some(existing) = arts
            .iter_mut()
            .find(|a| a.name == artifact.name && a.kind == artifact.kind)
        {
            // Bump version
            let new_version = Self::increment_version(&existing.version);
            existing.version = new_version;
            existing.content = artifact.content;
            existing.tool_signature = artifact.tool_signature;
            existing.updated_at = chrono::Utc::now().to_rfc3339();
            existing.status = ArtifactStatus::Draft;
            let id = existing.id.clone();
            drop(arts);
            self.save_sync();
            return id;
        }

        // New artifact
        if artifact.id.is_empty() {
            artifact.id = Uuid::new_v4().to_string();
        }
        let id = artifact.id.clone();
        arts.push(artifact);
        drop(arts);
        self.save_sync();
        id
    }

    /// Get an artifact by ID.
    pub fn get(&self, id: &str) -> Option<Artifact> {
        self.artifacts.lock().iter().find(|a| a.id == id).cloned()
    }

    /// Find an artifact by name and kind.
    pub fn find_by_name(&self, name: &str, kind: ArtifactKind) -> Option<Artifact> {
        self.artifacts
            .lock()
            .iter()
            .find(|a| a.name == name && a.kind == kind)
            .cloned()
    }

    /// Update an existing artifact. Returns `true` if the artifact was found.
    /// Persists to disk automatically (mirrors Go's `Registry.Update`).
    pub fn update(&self, id: &str, f: impl FnOnce(&mut Artifact)) -> bool {
        let mut arts = self.artifacts.lock();
        if let Some(artifact) = arts.iter_mut().find(|a| a.id == id) {
            f(artifact);
            artifact.updated_at = chrono::Utc::now().to_rfc3339();
            drop(arts);
            self.save_sync();
            true
        } else {
            false
        }
    }

    /// List all artifacts, optionally filtered by kind and status.
    /// Mirrors Go's `List(artifactType, status)`.
    pub fn list(&self, kind_filter: Option<ArtifactKind>, status_filter: Option<ArtifactStatus>) -> Vec<Artifact> {
        let arts = self.artifacts.lock();
        arts.iter()
            .filter(|a| {
                let kind_match = kind_filter.as_ref().map_or(true, |k| a.kind == *k);
                let status_match = status_filter.as_ref().map_or(true, |s| a.status == *s);
                kind_match && status_match
            })
            .cloned()
            .collect()
    }

    /// Return the count of artifacts, optionally filtered by kind.
    /// Mirrors Go's `Count(artifactType)`.
    pub fn count(&self, kind_filter: Option<ArtifactKind>) -> usize {
        let arts = self.artifacts.lock();
        match kind_filter {
            Some(kind) => arts.iter().filter(|a| a.kind == kind).count(),
            None => arts.len(),
        }
    }

    /// Remove an artifact by ID. Returns `true` if found and removed.
    /// Persists to disk automatically (mirrors Go's `Registry.Delete`).
    pub fn remove(&self, id: &str) -> bool {
        let mut arts = self.artifacts.lock();
        let len_before = arts.len();
        arts.retain(|a| a.id != id);
        let removed = arts.len() < len_before;
        if removed {
            drop(arts);
            self.save_sync();
        }
        removed
    }

    /// Synchronous save — writes to disk without async runtime.
    /// Used internally by `add`, `update`, `remove` for auto-persist.
    fn save_sync(&self) {
        if self.config.index_path.is_empty() {
            return;
        }
        let arts = self.artifacts.lock();
        if let Ok(json) = serde_json::to_string_pretty(&*arts) {
            let _ = std::fs::write(&self.config.index_path, json);
        }
    }

    /// Persist the registry index to disk.
    pub async fn save(&self) -> std::io::Result<()> {
        if self.config.index_path.is_empty() {
            return Ok(());
        }
        let arts = self.artifacts.lock();
        let json = serde_json::to_string_pretty(&*arts).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        tokio::fs::write(&self.config.index_path, json).await?;
        Ok(())
    }

    /// Load the registry index from disk.
    pub async fn load(&self) -> std::io::Result<()> {
        if self.config.index_path.is_empty() {
            return Ok(());
        }
        let content = tokio::fs::read_to_string(&self.config.index_path).await?;
        let loaded: Vec<Artifact> = serde_json::from_str(&content).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        *self.artifacts.lock() = loaded;
        Ok(())
    }

    /// Return the number of artifacts.
    pub fn len(&self) -> usize {
        self.artifacts.lock().len()
    }

    /// Return whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.artifacts.lock().is_empty()
    }

    fn increment_version(version: &str) -> String {
        // Simple semver patch bump: "x.y.z" -> "x.y.(z+1)"
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() == 3 {
            if let Ok(patch) = parts[2].parse::<u32>() {
                return format!("{}.{}.{}", parts[0], parts[1], patch + 1);
            }
        }
        // Fallback: just append ".1"
        format!("{}.1", version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_artifact(name: &str, kind: ArtifactKind) -> Artifact {
        Artifact {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            kind,
            version: "0.1.0".to_string(),
            status: ArtifactStatus::Draft,
            content: "test content".to_string(),
            tool_signature: vec!["tool_a".to_string()],
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        }
    }

    #[test]
    fn test_add_and_get() {
        let registry = Registry::new(RegistryConfig::default());
        let artifact = make_artifact("test-skill", ArtifactKind::Skill);
        let id = registry.add(artifact);
        assert!(!id.is_empty());

        let retrieved = registry.get(&id).unwrap();
        assert_eq!(retrieved.name, "test-skill");
        assert_eq!(retrieved.kind, ArtifactKind::Skill);
        // created_at should be set automatically
        assert!(!retrieved.created_at.is_empty());
    }

    #[test]
    fn test_add_existing_bumps_version() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("my-skill", ArtifactKind::Skill);
        let id1 = registry.add(a1);

        let mut a2 = make_artifact("my-skill", ArtifactKind::Skill);
        a2.content = "updated content".into();
        let id2 = registry.add(a2);

        assert_eq!(id1, id2); // same artifact updated
        assert_eq!(registry.len(), 1);
        let retrieved = registry.get(&id1).unwrap();
        assert_eq!(retrieved.version, "0.1.1");
        assert_eq!(retrieved.content, "updated content");
    }

    #[test]
    fn test_update_and_remove() {
        let registry = Registry::new(RegistryConfig::default());
        let artifact = make_artifact("to-update", ArtifactKind::Script);
        let id = registry.add(artifact);

        // Update
        let updated = registry.update(&id, |a| {
            a.status = ArtifactStatus::Active;
            a.usage_count = 42;
        });
        assert!(updated);
        assert_eq!(registry.get(&id).unwrap().usage_count, 42);

        // Remove
        let removed = registry.remove(&id);
        assert!(removed);
        assert!(registry.get(&id).is_none());
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registry.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        registry.add(make_artifact("persisted", ArtifactKind::Skill));
        registry.add(make_artifact("another", ArtifactKind::Script));

        registry.save().await.unwrap();
        assert!(path.exists());

        let registry2 = Registry::new(RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        });
        registry2.load().await.unwrap();
        assert_eq!(registry2.len(), 2);
    }

    #[test]
    fn test_list_with_status_filter() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("skill1", ArtifactKind::Skill);
        let id1 = registry.add(a1);
        // Set a1 to Active
        registry.update(&id1, |a| a.status = ArtifactStatus::Active);

        let a2 = make_artifact("skill2", ArtifactKind::Skill);
        registry.add(a2);
        // a2 stays Draft

        let a3 = make_artifact("script1", ArtifactKind::Script);
        registry.add(a3);

        // Filter by kind only
        assert_eq!(registry.list(Some(ArtifactKind::Skill), None).len(), 2);
        // Filter by status only
        assert_eq!(registry.list(None, Some(ArtifactStatus::Active)).len(), 1);
        // Filter by both
        assert_eq!(registry.list(Some(ArtifactKind::Skill), Some(ArtifactStatus::Active)).len(), 1);
        // No filter
        assert_eq!(registry.list(None, None).len(), 3);
    }

    #[test]
    fn test_count_with_kind_filter() {
        let registry = Registry::new(RegistryConfig::default());
        registry.add(make_artifact("s1", ArtifactKind::Skill));
        registry.add(make_artifact("s2", ArtifactKind::Skill));
        registry.add(make_artifact("sc1", ArtifactKind::Script));

        assert_eq!(registry.count(None), 3);
        assert_eq!(registry.count(Some(ArtifactKind::Skill)), 2);
        assert_eq!(registry.count(Some(ArtifactKind::Script)), 1);
    }

    // --- Additional registry tests ---

    #[test]
    fn test_add_empty_id_auto_generates() {
        let registry = Registry::new(RegistryConfig::default());
        let mut artifact = make_artifact("auto-id", ArtifactKind::Skill);
        artifact.id = String::new();
        let id = registry.add(artifact);
        assert!(!id.is_empty());
    }

    #[test]
    fn test_add_with_explicit_id_preserved() {
        let registry = Registry::new(RegistryConfig::default());
        let mut artifact = make_artifact("explicit-id", ArtifactKind::Skill);
        artifact.id = "my-custom-id".to_string();
        let id = registry.add(artifact);
        assert_eq!(id, "my-custom-id");
    }

    #[test]
    fn test_get_nonexistent() {
        let registry = Registry::new(RegistryConfig::default());
        assert!(registry.get("does-not-exist").is_none());
    }

    #[test]
    fn test_find_by_name_found() {
        let registry = Registry::new(RegistryConfig::default());
        let artifact = make_artifact("findme", ArtifactKind::Skill);
        registry.add(artifact);
        let found = registry.find_by_name("findme", ArtifactKind::Skill);
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "findme");
    }

    #[test]
    fn test_find_by_name_wrong_kind() {
        let registry = Registry::new(RegistryConfig::default());
        let artifact = make_artifact("findme", ArtifactKind::Skill);
        registry.add(artifact);
        let found = registry.find_by_name("findme", ArtifactKind::Script);
        assert!(found.is_none());
    }

    #[test]
    fn test_find_by_name_nonexistent() {
        let registry = Registry::new(RegistryConfig::default());
        assert!(registry.find_by_name("ghost", ArtifactKind::Skill).is_none());
    }

    #[test]
    fn test_update_nonexistent() {
        let registry = Registry::new(RegistryConfig::default());
        assert!(!registry.update("no-such-id", |_a| {}));
    }

    #[test]
    fn test_remove_nonexistent() {
        let registry = Registry::new(RegistryConfig::default());
        assert!(!registry.remove("no-such-id"));
    }

    #[test]
    fn test_is_empty() {
        let registry = Registry::new(RegistryConfig::default());
        assert!(registry.is_empty());
        registry.add(make_artifact("first", ArtifactKind::Skill));
        assert!(!registry.is_empty());
    }

    #[test]
    fn test_len() {
        let registry = Registry::new(RegistryConfig::default());
        assert_eq!(registry.len(), 0);
        registry.add(make_artifact("a", ArtifactKind::Skill));
        assert_eq!(registry.len(), 1);
        registry.add(make_artifact("b", ArtifactKind::Script));
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn test_increment_version_semver() {
        assert_eq!(Registry::increment_version("1.0.0"), "1.0.1");
        assert_eq!(Registry::increment_version("0.1.0"), "0.1.1");
        assert_eq!(Registry::increment_version("2.3.9"), "2.3.10");
    }

    #[test]
    fn test_increment_version_non_semver() {
        assert_eq!(Registry::increment_version("1.0"), "1.0.1");
        assert_eq!(Registry::increment_version("v1"), "v1.1");
    }

    #[test]
    fn test_add_same_name_different_kind() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("shared-name", ArtifactKind::Skill);
        let a2 = make_artifact("shared-name", ArtifactKind::Script);
        registry.add(a1);
        registry.add(a2);
        assert_eq!(registry.len(), 2);
        assert!(registry.find_by_name("shared-name", ArtifactKind::Skill).is_some());
        assert!(registry.find_by_name("shared-name", ArtifactKind::Script).is_some());
    }

    #[test]
    fn test_add_existing_preserves_id() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("dup", ArtifactKind::Skill);
        let id1 = registry.add(a1);
        let mut a2 = make_artifact("dup", ArtifactKind::Skill);
        a2.content = "new content".into();
        let id2 = registry.add(a2);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_add_existing_resets_to_draft() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("status-test", ArtifactKind::Skill);
        let id = registry.add(a1);
        registry.update(&id, |a| a.status = ArtifactStatus::Active);
        assert_eq!(registry.get(&id).unwrap().status, ArtifactStatus::Active);

        let a2 = make_artifact("status-test", ArtifactKind::Skill);
        registry.add(a2);
        assert_eq!(registry.get(&id).unwrap().status, ArtifactStatus::Draft);
    }

    #[test]
    fn test_update_modifies_updated_at() {
        let registry = Registry::new(RegistryConfig::default());
        let artifact = make_artifact("time-test", ArtifactKind::Skill);
        let id = registry.add(artifact);
        let original_updated = registry.get(&id).unwrap().updated_at.clone();

        // Small delay to ensure timestamp differs
        std::thread::sleep(std::time::Duration::from_millis(10));
        registry.update(&id, |a| a.usage_count = 1);

        let new_updated = registry.get(&id).unwrap().updated_at;
        assert_ne!(original_updated, new_updated);
    }

    #[test]
    fn test_update_multiple_fields() {
        let registry = Registry::new(RegistryConfig::default());
        let artifact = make_artifact("multi", ArtifactKind::Skill);
        let id = registry.add(artifact);

        registry.update(&id, |a| {
            a.status = ArtifactStatus::Active;
            a.usage_count = 100;
            a.version = "2.0.0".to_string();
            a.content = "new content".to_string();
        });

        let updated = registry.get(&id).unwrap();
        assert_eq!(updated.status, ArtifactStatus::Active);
        assert_eq!(updated.usage_count, 100);
        assert_eq!(updated.version, "2.0.0");
        assert_eq!(updated.content, "new content");
    }

    #[test]
    fn test_list_filter_by_kind_and_status() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("skill-active", ArtifactKind::Skill);
        let id1 = registry.add(a1);
        registry.update(&id1, |a| a.status = ArtifactStatus::Active);

        let a2 = make_artifact("skill-draft", ArtifactKind::Skill);
        registry.add(a2);

        let a3 = make_artifact("script-draft", ArtifactKind::Script);
        registry.add(a3);

        // Only active skills
        let active_skills = registry.list(Some(ArtifactKind::Skill), Some(ArtifactStatus::Active));
        assert_eq!(active_skills.len(), 1);
        assert_eq!(active_skills[0].name, "skill-active");
    }

    #[test]
    fn test_list_mixed_kinds() {
        let registry = Registry::new(RegistryConfig::default());
        registry.add(make_artifact("s1", ArtifactKind::Skill));
        registry.add(make_artifact("s2", ArtifactKind::Script));
        registry.add(make_artifact("m1", ArtifactKind::Mcp));

        assert_eq!(registry.list(Some(ArtifactKind::Skill), None).len(), 1);
        assert_eq!(registry.list(Some(ArtifactKind::Script), None).len(), 1);
        assert_eq!(registry.list(Some(ArtifactKind::Mcp), None).len(), 1);
    }

    #[tokio::test]
    async fn test_save_no_path_noop() {
        let registry = Registry::new(RegistryConfig::default());
        registry.add(make_artifact("test", ArtifactKind::Skill));
        // Should succeed without error even without path
        registry.save().await.unwrap();
    }

    #[tokio::test]
    async fn test_load_no_path_noop() {
        let config = RegistryConfig {
            index_path: String::new(), // Empty path means no-op
        };
        let registry = Registry::new(config);
        registry.load().await.unwrap();
    }

    #[tokio::test]
    async fn test_load_from_nonexistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-file.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        let result = registry.load().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_save_load_roundtrip_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        let id = registry.add(make_artifact("persist", ArtifactKind::Skill));
        registry.update(&id, |a| {
            a.status = ArtifactStatus::Active;
            a.usage_count = 10;
        });
        registry.save().await.unwrap();

        let config2 = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry2 = Registry::new(config2);
        registry2.load().await.unwrap();
        assert_eq!(registry2.len(), 1);
        let loaded = registry2.get(&id).unwrap();
        assert_eq!(loaded.name, "persist");
        assert_eq!(loaded.status, ArtifactStatus::Active);
        assert_eq!(loaded.usage_count, 10);
    }

    #[test]
    fn test_remove_then_add_same_name() {
        let registry = Registry::new(RegistryConfig::default());
        let a1 = make_artifact("reborn", ArtifactKind::Skill);
        let id1 = registry.add(a1);
        registry.remove(&id1);
        assert_eq!(registry.len(), 0);

        let a2 = make_artifact("reborn", ArtifactKind::Skill);
        let id2 = registry.add(a2);
        assert_ne!(id1, id2); // New ID since old was removed
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_count_mcp_kind() {
        let registry = Registry::new(RegistryConfig::default());
        registry.add(make_artifact("mcp1", ArtifactKind::Mcp));
        registry.add(make_artifact("mcp2", ArtifactKind::Mcp));
        assert_eq!(registry.count(Some(ArtifactKind::Mcp)), 2);
        assert_eq!(registry.count(None), 2);
    }

    #[tokio::test]
    async fn test_save_creates_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        registry.add(make_artifact("json-test", ArtifactKind::Skill));
        registry.save().await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_list_all_statuses() {
        let registry = Registry::new(RegistryConfig::default());

        // Create artifacts with various statuses
        let _id1 = registry.add(make_artifact("draft", ArtifactKind::Skill));
        let id2 = registry.add(make_artifact("active", ArtifactKind::Skill));
        let id3 = registry.add(make_artifact("observing", ArtifactKind::Skill));
        let id4 = registry.add(make_artifact("degraded", ArtifactKind::Skill));

        registry.update(&id2, |a| a.status = ArtifactStatus::Active);
        registry.update(&id3, |a| a.status = ArtifactStatus::Observing);
        registry.update(&id4, |a| a.status = ArtifactStatus::Degraded);

        assert_eq!(registry.list(None, Some(ArtifactStatus::Draft)).len(), 1);
        assert_eq!(registry.list(None, Some(ArtifactStatus::Active)).len(), 1);
        assert_eq!(registry.list(None, Some(ArtifactStatus::Observing)).len(), 1);
        assert_eq!(registry.list(None, Some(ArtifactStatus::Degraded)).len(), 1);
    }

    #[test]
    fn test_auto_save_on_add_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auto_save.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        registry.add(make_artifact("auto", ArtifactKind::Skill));
        // save_sync should have been called automatically
        assert!(path.exists());
    }

    #[test]
    fn test_auto_save_on_update_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auto_update.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        let id = registry.add(make_artifact("auto-upd", ArtifactKind::Skill));

        // Clear file to verify update writes again
        let _ = std::fs::remove_file(&path);
        registry.update(&id, |a| a.usage_count = 5);
        assert!(path.exists());
    }

    #[test]
    fn test_auto_save_on_remove_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auto_remove.json");
        let config = RegistryConfig {
            index_path: path.to_string_lossy().to_string(),
        };
        let registry = Registry::new(config);
        let id = registry.add(make_artifact("auto-rm", ArtifactKind::Skill));

        registry.remove(&id);
        // File should still exist (just empty array now)
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
        assert!(parsed.is_empty());
    }
}
