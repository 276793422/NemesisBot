//! Artifact exporter - writes artifacts to the workspace.
//!
//! Exports forge artifacts (skills, scripts, MCP modules) to the appropriate
//! workspace directories. Supports:
//! - `export()` - write content to forge directory
//! - `export_artifact()` - export a registered artifact with manifest
//! - `export_all()` - batch export all active artifacts
//! - `copy_skill_to_workspace()` - copy skill with -forge suffix

use std::path::{Path, PathBuf};

use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

/// Export manifest describing an exported artifact's metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportManifest {
    /// Artifact ID.
    pub id: String,
    /// Artifact type (skill, script, mcp).
    #[serde(rename = "type")]
    pub kind: String,
    /// Artifact name.
    pub name: String,
    /// Artifact version.
    pub version: String,
    /// Export timestamp (ISO 8601).
    pub exported_at: String,
    /// List of exported files (relative paths).
    pub files: Vec<String>,
}

/// Export target configuration.
#[derive(Clone)]
pub struct ExportConfig {
    /// Root workspace directory.
    pub workspace: PathBuf,
    /// Forge data directory.
    pub forge_dir: PathBuf,
    /// Registry reference for looking up artifacts.
    pub registry: Option<std::sync::Arc<crate::registry::Registry>>,
}

impl ExportConfig {
    /// Create export config from workspace root.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        let workspace = workspace.into();
        let forge_dir = workspace.join("forge");
        Self {
            workspace,
            forge_dir,
            registry: None,
        }
    }

    /// Create export config with a registry reference.
    pub fn with_registry(
        workspace: impl Into<PathBuf>,
        registry: std::sync::Arc<crate::registry::Registry>,
    ) -> Self {
        let workspace = workspace.into();
        let forge_dir = workspace.join("forge");
        Self {
            workspace,
            forge_dir,
            registry: Some(registry),
        }
    }
}

/// Exporter writes artifact content to the appropriate workspace directory.
pub struct Exporter {
    config: ExportConfig,
}

impl Exporter {
    /// Create a new exporter with the given configuration.
    pub fn new(config: ExportConfig) -> Self {
        Self { config }
    }

    /// Get the target directory for an artifact type.
    pub fn artifact_dir(&self, kind: ArtifactKind, name: &str) -> PathBuf {
        match kind {
            ArtifactKind::Skill => self.config.forge_dir.join("skills").join(name),
            ArtifactKind::Script => self.config.forge_dir.join("scripts").join(name),
            ArtifactKind::Mcp => self.config.forge_dir.join("mcp").join(name),
        }
    }

    /// Export an artifact's content to disk.
    pub async fn export(
        &self,
        kind: ArtifactKind,
        name: &str,
        content: &str,
    ) -> std::io::Result<PathBuf> {
        let dir = self.artifact_dir(kind, name);
        tokio::fs::create_dir_all(&dir).await?;

        let path = match kind {
            ArtifactKind::Skill => dir.join("SKILL.md"),
            ArtifactKind::Script => dir.join(name),
            ArtifactKind::Mcp => dir.join("server.py"),
        };

        tokio::fs::write(&path, content).await?;
        tracing::info!(path = %path.display(), kind = ?kind, "[Exporter] Artifact exported");
        Ok(path)
    }

    /// Export a single registered artifact to a target directory.
    ///
    /// Creates a subdirectory `{target_dir}/{name}-{version}/` containing
    /// the artifact content and a `forge-manifest.json` metadata file.
    /// Also copies any project structure files (`requirements.txt`, `go.mod`,
    /// `README.md`) and a `tests/` subdirectory if present.
    ///
    /// Matches Go's `Exporter.ExportArtifact()`.
    pub async fn export_artifact(
        &self,
        artifact: &Artifact,
        target_dir: &Path,
    ) -> std::io::Result<PathBuf> {
        // Create target subdirectory: {targetDir}/{name}-{version}/
        let export_dir = target_dir.join(format!("{}-{}", artifact.name, artifact.version));
        tokio::fs::create_dir_all(&export_dir).await?;

        let mut files = Vec::new();

        // Write artifact main content
        let main_filename = match artifact.kind {
            ArtifactKind::Skill => "SKILL.md",
            ArtifactKind::Script => &artifact.name,
            ArtifactKind::Mcp => "server.py",
        };

        let main_path = export_dir.join(main_filename);
        tokio::fs::write(&main_path, &artifact.content).await?;
        files.push(main_filename.to_string());

        // Copy project structure files from artifact directory
        let artifact_dir = self.artifact_dir(artifact.kind, &artifact.name);
        let project_files = ["requirements.txt", "go.mod", "README.md"];
        for pf in &project_files {
            let src = artifact_dir.join(pf);
            if src.exists() {
                let dst = export_dir.join(pf);
                if let Ok(data) = tokio::fs::read(&src).await {
                    tokio::fs::write(&dst, &data).await.ok();
                    files.push(pf.to_string());
                }
            }
        }

        // Copy tests/ subdirectory if it exists
        let tests_dir = artifact_dir.join("tests");
        if tests_dir.is_dir() {
            let test_target = export_dir.join("tests");
            tokio::fs::create_dir_all(&test_target).await.ok();
            let copied = copy_dir_recursive(&tests_dir, &test_target).await;
            for f in copied {
                files.push(format!("tests/{}", f));
            }
        }

        // Generate forge-manifest.json
        let manifest = ExportManifest {
            id: artifact.id.clone(),
            kind: format!("{:?}", artifact.kind).to_lowercase(),
            name: artifact.name.clone(),
            version: artifact.version.clone(),
            exported_at: chrono::Local::now().to_rfc3339(),
            files,
        };

        let manifest_data = serde_json::to_string_pretty(&manifest)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let manifest_path = export_dir.join("forge-manifest.json");
        tokio::fs::write(&manifest_path, manifest_data).await?;

        Ok(export_dir)
    }

    /// Export all active artifacts to a target directory.
    ///
    /// Iterates all registered artifacts with `Active` status and exports
    /// each one. Returns the count of successfully exported artifacts.
    ///
    /// Matches Go's `Exporter.ExportAll()`.
    pub async fn export_all(&self, target_dir: &Path) -> std::io::Result<usize> {
        let registry = match self.config.registry {
            Some(ref r) => r,
            None => return Ok(0),
        };

        let artifacts = registry.list(None, None);
        if artifacts.is_empty() {
            return Ok(0);
        }

        tokio::fs::create_dir_all(target_dir).await?;

        let mut count = 0;
        for a in &artifacts {
            if a.status != ArtifactStatus::Active {
                continue;
            }
            if self.export_artifact(a, target_dir).await.is_ok() {
                count += 1;
            }
        }

        Ok(count)
    }

    /// Copy a skill artifact to the workspace skills directory (with -forge suffix).
    pub async fn copy_skill_to_workspace(
        &self,
        name: &str,
        content: &str,
    ) -> std::io::Result<PathBuf> {
        let dir = self
            .config
            .workspace
            .join("skills")
            .join(format!("{}-forge", name));
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join("SKILL.md");
        tokio::fs::write(&path, content).await?;
        Ok(path)
    }

    /// Get the forge directory path.
    pub fn forge_dir(&self) -> &Path {
        &self.config.forge_dir
    }

    /// Get the workspace directory path.
    pub fn workspace(&self) -> &Path {
        &self.config.workspace
    }
}

/// Recursively copy all files from `src_dir` to `dst_dir`, returning relative paths.
fn copy_dir_recursive<'a>(
    src_dir: &'a Path,
    dst_dir: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + 'a>> {
    Box::pin(async move {
        let mut files = Vec::new();
        if !src_dir.is_dir() {
            return files;
        }

        let mut entries = match tokio::fs::read_dir(src_dir).await {
            Ok(e) => e,
            Err(_) => return files,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();

            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                // Recurse into subdirectory
                let sub_src = src_dir.join(&name_str);
                let sub_dst = dst_dir.join(&name_str);
                tokio::fs::create_dir_all(&sub_dst).await.ok();
                let sub_files = copy_dir_recursive(&sub_src, &sub_dst).await;
                for f in sub_files {
                    files.push(format!("{}/{}", name_str, f));
                }
            } else {
                let src_path = entry.path();
                let dst_path = dst_dir.join(&name_str);
                if let Ok(data) = tokio::fs::read(&src_path).await {
                    if tokio::fs::write(&dst_path, &data).await.is_ok() {
                        files.push(name_str);
                    }
                }
            }
        }

        files
    })
}

#[cfg(test)]
mod tests;
