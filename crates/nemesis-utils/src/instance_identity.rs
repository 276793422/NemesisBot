//! 单机实例身份（W1 #31，2026-11-19 安全图谱批）。
//!
//! 同一台机器上可能并存多个 bot 实例（`--local` 多实例部署）。实例身份
//! 是把审计/日志归属到「哪一个实例」的锚点：`<workspace>/identity.json`
//! 持久一个 UUID v4（workspace 级——同 workspace 的多个进程共享同一
//! 身份，符合「单机实例 = 单个工作空间」的部署语义），首个进程生成、
//! 之后全部复用。
//!
//! 消费面：gateway 装配（nemesisbot security_setup）→ 审计链
//! （hash_version 2，见 nemesis-security integrity.rs）+ 启动日志。
//!
//! 诚实边界：并发首创建（两个进程同时首启）last-write-wins，两进程
//! 当次身份可能不同——下次启动收敛到胜者（幂等读优先）。文件损坏 →
//! 视为不存在重新生成（内容不可信时旧身份无意义）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// identity.json 盘上形态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceIdentity {
    /// 实例唯一标识（UUID v4，workspace 级持久）。
    pub instance_id: String,
    /// 创建时间（RFC3339，本地时区——只作展示，不参与身份）。
    #[serde(default)]
    pub created_at: String,
}

/// `<root>/identity.json` 路径。
pub fn identity_path(root: &Path) -> PathBuf {
    root.join("identity.json")
}

/// 读取或创建实例身份：文件存在且可解析且非空 id → 复用；否则生成新
/// UUID 落盘（`create_dir_all` + 覆盖写）。
pub fn read_or_create(root: &Path) -> std::io::Result<InstanceIdentity> {
    let path = identity_path(root);
    if let Ok(s) = std::fs::read_to_string(&path)
        && let Ok(id) = serde_json::from_str::<InstanceIdentity>(s.trim())
        && !id.instance_id.trim().is_empty()
    {
        return Ok(id);
    }
    let identity = InstanceIdentity {
        instance_id: uuid::Uuid::new_v4().to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
    };
    let json = serde_json::to_string_pretty(&identity)
        .map_err(|e| std::io::Error::other(format!("identity serialize failed: {e}")))?;
    std::fs::create_dir_all(root)?;
    std::fs::write(&path, json)?;
    Ok(identity)
}

/// 解析或回落：任何 fs 失败（root 不可写/磁盘满等）→ 瞬态 UUID（进程内
/// 一致、重启即变）——身份生成**绝不阻塞/炸掉启动**，诚实降级 + WARN。
pub fn resolve_or_fallback(root: &Path) -> InstanceIdentity {
    const EPHEMERAL_NOTE: &str = "identity.json 不可用，使用瞬态实例身份（重启即变）";
    read_or_create(root).unwrap_or_else(|e| {
        tracing::warn!(error = %e, root = %root.display(), "[instance_identity] {EPHEMERAL_NOTE}");
        InstanceIdentity {
            instance_id: uuid::Uuid::new_v4().to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
        }
    })
}

#[cfg(test)]
mod tests;
