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
        let now = chrono::Local::now().to_rfc3339();
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
            existing.updated_at = chrono::Local::now().to_rfc3339();
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
            artifact.updated_at = chrono::Local::now().to_rfc3339();
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
mod tests;
