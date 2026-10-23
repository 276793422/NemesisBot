//! profile 目录存储（总控 §6.1 布局 + §10 存储汇总）。
//!
//! ```text
//! <workspace>/ziwei/profiles/<id>/
//! ├── profile.json
//! ├── anchors.json
//! ├── notes.md
//! ├── snapshots/<ts>/…      （配置期，TTL 30d——清扫在 M3 快照引擎）
//! └── runs/<ts>/capture.jsonl（使用期，TTL 7d——清扫在 M2）
//! ```
//!
//! 写入一律「tmp + rename」原子替换；删除走 `nemesis_utils::fsutil` 韧性
//! 包装（Windows AV/索引器瞬时句柄是常态不是异常）。

use crate::anchors::AnchorSet;
use crate::model::Profile;
use chrono::Utc;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("profile {0:?} 不存在")]
    NotFound(String),
    #[error("profile {0:?} 已存在")]
    AlreadyExists(String),
    #[error("profile 校验失败: {0}")]
    Invalid(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// 列表行（轻量，列表页用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileSummary {
    pub profile_id: String,
    pub name: String,
    pub status: String,
    pub exe_name: String,
    pub machine: String,
    pub tool_count: usize,
    pub updated_at: String,
}

pub struct ProfileStore {
    /// `<workspace>/ziwei/profiles`
    root: PathBuf,
}

impl ProfileStore {
    pub fn new(workspace: &Path) -> Self {
        Self {
            root: workspace.join("ziwei").join("profiles"),
        }
    }

    /// 快照目录根 `<workspace>/ziwei/profiles/<id>/snapshots`。
    pub fn snapshots_dir(&self, id: &str) -> PathBuf {
        self.root.join(id).join("snapshots")
    }

    /// 运行记录目录根 `<workspace>/ziwei/profiles/<id>/runs`。
    pub fn runs_dir(&self, id: &str) -> PathBuf {
        self.root.join(id).join("runs")
    }

    /// profile 目录根 `<workspace>/ziwei/profiles/<id>`（commit 报告面用）。
    pub fn dir(&self, id: &str) -> PathBuf {
        self.profile_dir(id)
    }

    fn profile_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn assert_id(id: &str) -> Result<(), StoreError> {
        // id 是路径组件：只允许 zw-pro- 前缀 + 安全字符，拒绝路径穿越。
        let ok = id.starts_with("zw-pro-")
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if ok {
            Ok(())
        } else {
            Err(StoreError::Invalid(format!("profile_id {id:?} 形态非法")))
        }
    }

    /// 列出全部 profile（按目录扫描；目录缺 profile.json 的跳过并 WARN）。
    pub fn list(&self) -> Result<Vec<ProfileSummary>, StoreError> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        let mut entries: Vec<_> = std::fs::read_dir(&self.root)?.collect::<Result<Vec<_>, _>>()?;
        // 有序消费 read_dir：按文件名字典序（NTFS/ext4 顺序是平台实现细节）。
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let id = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let pf = path.join("profile.json");
            if !pf.exists() {
                tracing::warn!("ziwei profile 目录 {} 缺 profile.json，跳过", id);
                continue;
            }
            match std::fs::read_to_string(&pf)
                .map_err(StoreError::Io)
                .and_then(|s| serde_json::from_str::<Profile>(&s).map_err(StoreError::Json))
            {
                Ok(p) => out.push(ProfileSummary {
                    profile_id: p.profile_id,
                    name: p.name,
                    status: format!("{:?}", p.status).to_lowercase(),
                    exe_name: p.target.exe_name,
                    machine: format!("{:?}", p.target.machine).to_lowercase(),
                    tool_count: p.tools.len(),
                    updated_at: p.last_verified_at.to_rfc3339(),
                }),
                Err(e) => {
                    // 单个 profile 损坏不拖垮列表（诚实标注，不静默吞）。
                    tracing::warn!("ziwei profile {} 解析失败: {}（跳过）", id, e);
                }
            }
        }
        Ok(out)
    }

    pub fn load(&self, id: &str) -> Result<Profile, StoreError> {
        Self::assert_id(id)?;
        let path = self.profile_dir(id).join("profile.json");
        if !path.exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let p: Profile = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        p.validate().map_err(StoreError::Invalid)?;
        Ok(p)
    }

    pub fn load_anchors(&self, id: &str) -> Result<AnchorSet, StoreError> {
        Self::assert_id(id)?;
        let path = self.profile_dir(id).join("anchors.json");
        if !path.exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(serde_json::from_str(&std::fs::read_to_string(&path)?)?)
    }

    /// 保存 profile（+ 可选锚集）。存在性 / 跨文件一致性 / 结构校验全部
    /// 前置，任一失败不落盘（loud）。
    pub fn save(&self, profile: &Profile, anchors: Option<&AnchorSet>) -> Result<(), StoreError> {
        profile.validate().map_err(StoreError::Invalid)?;
        Self::assert_id(&profile.profile_id)?;
        let dir = self.profile_dir(&profile.profile_id);
        let existed = dir.join("profile.json").exists();
        if let Some(set) = anchors {
            if set.profile_id != profile.profile_id {
                return Err(StoreError::Invalid(format!(
                    "anchors.profile_id {:?} 与 profile {:?} 不一致",
                    set.profile_id, profile.profile_id
                )));
            }
            let fn_names: std::collections::HashSet<&str> = profile
                .functions
                .iter()
                .map(|f| f.semantic.as_str())
                .collect();
            for a in &set.anchors {
                crate::anchors::validate_anchor(a).map_err(StoreError::Invalid)?;
                if !fn_names.contains(a.function.as_str()) {
                    return Err(StoreError::Invalid(format!(
                        "锚关联的函数 {:?} 不在 profile.functions 中",
                        a.function
                    )));
                }
            }
        }
        std::fs::create_dir_all(&dir)?;
        if let Some(set) = anchors {
            atomic_write_json(&dir.join("anchors.json"), set)?;
        }
        atomic_write_json(&dir.join("profile.json"), profile)?;
        if !existed {
            tracing::info!(
                "ziwei profile {} 入库（{}）",
                profile.profile_id,
                profile.name
            );
        }
        Ok(())
    }

    /// 保存标注笔记（人读 markdown，自由文本）。
    pub fn save_notes(&self, id: &str, notes: &str) -> Result<(), StoreError> {
        Self::assert_id(id)?;
        let dir = self.profile_dir(id);
        if !dir.join("profile.json").exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        std::fs::create_dir_all(&dir)?;
        atomic_write_bytes(&dir.join("notes.md"), notes.as_bytes())
    }

    pub fn load_notes(&self, id: &str) -> Result<String, StoreError> {
        Self::assert_id(id)?;
        let path = self.profile_dir(id).join("notes.md");
        if !path.exists() {
            return Ok(String::new());
        }
        Ok(std::fs::read_to_string(&path)?)
    }

    /// 删除整个 profile 目录（用户显式动作；走韧性包装）。
    pub fn delete(&self, id: &str) -> Result<(), StoreError> {
        Self::assert_id(id)?;
        let dir = self.profile_dir(id);
        if !dir.exists() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        // Windows 瞬时句柄（AV/索引器）是常态：删除一律走韧性包装。
        nemesis_utils::fsutil::remove_dir_all_retry(&dir)?;
        Ok(())
    }
}

/// 生成新 profile_id：`zw-pro-` + 8 hex（种子 = 名字 + 纳秒时间，sha256 截断）。
pub fn new_profile_id(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(name.as_bytes());
    h.update(nanos.to_le_bytes());
    let digest = h.finalize();
    format!("zw-pro-{}", hex::encode(&digest[..4]))
}

/// 当前时间戳目录名（snapshots/runs 子目录）。
pub fn ts_dir_name() -> String {
    Utc::now().format("%Y%m%d_%H%M%S_%3f").to_string()
}

fn atomic_write_bytes(path: &Path, data: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let mut buf = serde_json::to_string_pretty(value)?;
    buf.push('\n');
    atomic_write_bytes(path, buf.as_bytes())
}
