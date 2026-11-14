//! Registry - tracks forge artifacts with a JSON index.
//!
//! Provides CRUD operations and version tracking. The index is optionally
//! persisted as a JSON file.

use parking_lot::Mutex;
use uuid::Uuid;

use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

use crate::types::RegistryConfig;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for unique atomic-write temp filenames (F-). Unique per
/// write so concurrent writes to the same path (e.g. parallel tests sharing the
/// default index) don't collide on the temp file.
static REGISTRY_WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn atomic_tmp_path(index_path: &str) -> PathBuf {
    let n = REGISTRY_WRITE_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("{}.{}.tmp", index_path, n))
}

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
            // 版本 bump = 新一代产物：评估账本
            // （usage_count/success_rate/consecutive_observing_rounds/
            // last_degraded_at/last_efficacy）是 per-version 状态，必须重置
            // ——跨代残留会让重建（Degraded/Archived 重蒸馏、dedup 过期
            // 重建）的新版本带着旧时代的 success_rate 与 last_degraded_at
            // cooldown 免疫进 monitor（未评估先判罚或免罚），stale 实效
            // 快照还会顶替新代展示。deployed_at 盖重部署时刻——这是全仓
            // 唯一写入点，monitor 的 before/after 实效窗（F 线「SPO 迭代
            // 重部署独立实效窗」）由此锚定本代，不再退化为 created_at
            // 全历史。created_at 保留首代出生时间（账本不变量：
            // created=首生，updated=本代）。
            let new_version = Self::increment_version(&existing.version);
            let now = chrono::Local::now().to_rfc3339();
            existing.version = new_version;
            existing.content = artifact.content;
            existing.tool_signature = artifact.tool_signature;
            existing.updated_at = now.clone();
            existing.status = ArtifactStatus::Draft;
            existing.usage_count = 0;
            existing.success_rate = 0.0;
            existing.consecutive_observing_rounds = 0;
            existing.last_degraded_at = None;
            existing.last_efficacy = None;
            existing.deployed_at = Some(now);
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
    pub fn list(
        &self,
        kind_filter: Option<ArtifactKind>,
        status_filter: Option<ArtifactStatus>,
    ) -> Vec<Artifact> {
        let arts = self.artifacts.lock();
        arts.iter()
            .filter(|a| {
                let kind_match = kind_filter.as_ref().is_none_or(|k| a.kind == *k);
                let status_match = status_filter.as_ref().is_none_or(|s| a.status == *s);
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
        // 横扫族 C 四轮（2026-11-06）：同步自动持久化失败不再静默——rename
        // 失败 = 内存态有、盘上 index 陈旧，重启丢注册且 tmp 孤儿（计数器
        // 唯一名不会覆盖）无任何线索。三臂（序列化/tmp 写/rename）失败均
        // WARN 留痕；tmp 写与 rename 失败时都清掉 tmp 残留（resilient，其
        // 内部 WARN 兜底）——计数器唯一名不会被下次保存覆盖，漏清 = 永久
        // 孤儿累积（BUG-20261109-005：tmp 写臂此前漏清）。
        let json = match serde_json::to_string_pretty(&*arts) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(error = %e, "[ForgeRegistry] index serialize failed — auto-persist skipped");
                return;
            }
        };
        let tmp = atomic_tmp_path(&self.config.index_path);
        if let Err(e) = std::fs::write(&tmp, &json) {
            tracing::warn!(
                path = %self.config.index_path,
                error = %e,
                "[ForgeRegistry] index tmp write failed — auto-persist skipped"
            );
            let _ = nemesis_utils::fsutil::remove_file_resilient(&tmp);
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &self.config.index_path) {
            tracing::warn!(
                path = %self.config.index_path,
                error = %e,
                "[ForgeRegistry] index rename failed — on-disk index stale until next save"
            );
            let _ = nemesis_utils::fsutil::remove_file_resilient(&tmp);
        }
    }

    /// Persist the registry index to disk.
    pub async fn save(&self) -> std::io::Result<()> {
        if self.config.index_path.is_empty() {
            return Ok(());
        }
        // Serialize inside the lock scope — the guard must not reach the
        // awaits below (clippy's flow analysis recognizes block scoping,
        // not a bare drop()).
        let json = {
            let arts = self.artifacts.lock();
            serde_json::to_string_pretty(&*arts)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
        };
        // F-: atomic write (unique temp + rename). 计数器唯一名 = 错误路径
        // 漏清即永久孤儿（BUG-20261109-005）——tmp 写与 rename 失败都清残留。
        let tmp = atomic_tmp_path(&self.config.index_path);
        if let Err(e) = tokio::fs::write(&tmp, &json).await {
            let _ = nemesis_utils::fsutil::remove_file_resilient(&tmp);
            return Err(e);
        }
        if let Err(e) = tokio::fs::rename(&tmp, &self.config.index_path).await {
            let _ = nemesis_utils::fsutil::remove_file_resilient(&tmp);
            return Err(e);
        }
        Ok(())
    }

    /// Load the registry index from disk.
    ///
    /// (BUG-20261114-026) 「索引文件不存在」= 全新工作区常态，返回 Ok（空
    /// 注册表），与 AuthStore::load / TrustStore::load / WorkspaceState::load
    /// 的家族约定同口径——缺失不是损坏，构造/重载调用方据 Err 触发的 loud
    /// 留痕只应面向真损坏（撕裂 JSON / IO 错误）。此前缺失 = Err(NotFound)，
    /// 调用方只能 `let _ =` 一刀切吞掉，连带真损坏也零痕迹。
    pub async fn load(&self) -> std::io::Result<()> {
        if self.config.index_path.is_empty() {
            return Ok(());
        }
        if !std::path::Path::new(&self.config.index_path).exists() {
            return Ok(());
        }
        let content = tokio::fs::read_to_string(&self.config.index_path).await?;
        let loaded: Vec<Artifact> = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
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
        if parts.len() == 3
            && let Ok(patch) = parts[2].parse::<u32>()
        {
            return format!("{}.{}.{}", parts[0], parts[1], patch + 1);
        }
        // Fallback: just append ".1"
        format!("{}.1", version)
    }
}

#[cfg(test)]
mod tests;
