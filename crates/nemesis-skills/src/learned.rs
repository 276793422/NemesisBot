//! 学得技能 manifest（N3，2026-10-13 机会项）：`<workspace>/skills/.learned.json`。
//!
//! `skill_manage`（agent 程序性记忆）创建的技能**不进** `skills.lock.json`
//! ——那是 registry 安装记账（带 source/commit/逐文件哈希）。学得技能只有
//! 本地目录，没有上游来源，锁不出有意义的条目。本 manifest 是学得技能的
//! 轻量账本：`{name, created_at_ms, description}`，供
//! WSAPI `skills.learned_list` / `skills.learned_remove` 管理面与 learn
//! 技能提示回查。文件缺失 = 空 manifest（首个学得技能落盘时创建）；
//! 损坏 = 备份 .bak 后按空表继续（对齐 `lockfile` 口径）。
//!
//! `learned_remove` 的护栏 = **manifest 成员资格**：只删 manifest 里记着
//! 的技能目录——防止管理面误删 registry 装的技能（那些走 `skills.uninstall`）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

/// manifest 文件名（置于 `<workspace>/skills/` 下；点前缀 + 文件形态，
/// `installed()` 等目录扫描天然跳过）。
pub const LEARNED_MANIFEST_NAME: &str = ".learned.json";

/// 单条学得技能记录。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LearnedEntry {
    /// 技能名（= `skills/<name>/` 目录名）。
    pub name: String,
    /// 创建时间（unix 毫秒）。
    pub created_at_ms: i64,
    /// 从 SKILL.md frontmatter `description:` 提取的一句话描述（缺失为空）。
    #[serde(default)]
    pub description: String,
}

/// manifest 根结构。BTreeMap 保证序列化顺序稳定（diff 友好，对齐 lockfile）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LearnedManifest {
    /// 格式版本。
    pub version: u32,
    /// name -> 条目。
    pub entries: BTreeMap<String, LearnedEntry>,
}

/// workspace 根下 manifest 路径。
pub fn learned_manifest_path(workspace: &Path) -> PathBuf {
    workspace.join("skills").join(LEARNED_MANIFEST_NAME)
}

impl LearnedManifest {
    /// 空 manifest。
    pub fn new() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
        }
    }

    /// 从 workspace 读取；文件缺失/损坏按空表处理（损坏时备份 .bak 再降级，
    /// 对齐 `lockfile::SkillsLockfile::load` 口径——不备份就丢排查证据）。
    pub fn load(workspace: &Path) -> Self {
        let path = learned_manifest_path(workspace);
        if !path.exists() {
            return Self::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<LearnedManifest>(&data) {
                Ok(file) => file,
                Err(e) => {
                    let bak = path.with_extension("json.bak");
                    match std::fs::copy(&path, &bak) {
                        Ok(_) => warn!(
                            "{} 解析失败，已备份为 {} 后按空表继续: {}",
                            LEARNED_MANIFEST_NAME,
                            bak.display(),
                            e
                        ),
                        Err(be) => warn!(
                            "{} 解析失败，备份也失败（{}），按空表继续: {}",
                            LEARNED_MANIFEST_NAME, be, e
                        ),
                    }
                    Self::new()
                }
            },
            Err(e) => {
                warn!("{} 读取失败，按空表继续: {}", LEARNED_MANIFEST_NAME, e);
                Self::new()
            }
        }
    }

    /// 写回 workspace（原子写：tmp + rename；Windows 下 `std::fs::rename`
    /// 带 MOVEFILE_REPLACE_EXISTING，可覆盖已存在文件）。
    pub fn save(&self, workspace: &Path) -> Result<(), String> {
        let path = learned_manifest_path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create skills dir: {}", e))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("failed to serialize learned manifest: {}", e))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, data).map_err(|e| format!("failed to write manifest tmp: {}", e))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("failed to commit learned manifest: {}", e))?;
        Ok(())
    }

    /// 记录/覆盖一条（created_at_ms 保留已有条目原值——patch/重装不刷新创建时间）。
    pub fn upsert(&mut self, name: &str, description: &str, created_at_ms: i64) {
        let entry = match self.entries.get(name) {
            Some(existing) => {
                let mut e = existing.clone();
                e.description = description.to_string();
                e
            }
            None => LearnedEntry {
                name: name.to_string(),
                created_at_ms,
                description: description.to_string(),
            },
        };
        self.entries.insert(name.to_string(), entry);
    }

    /// 移除一条；返回是否存在。
    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    /// 查询一条。
    pub fn get(&self, name: &str) -> Option<&LearnedEntry> {
        self.entries.get(name)
    }
}

/// 便捷一次成型：load → upsert → save。给 `skill_manage create` 成功路径用，
/// 失败不影响 create 本身（调用方决定降级策略）。
pub fn record_learned(workspace: &Path, name: &str, description: &str) -> Result<(), String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let mut manifest = LearnedManifest::load(workspace);
    manifest.upsert(name, description, now_ms);
    manifest.save(workspace)
}

/// 便捷一次成型：load → remove → save。返回移除前是否存在。
pub fn remove_learned(workspace: &Path, name: &str) -> Result<bool, String> {
    let mut manifest = LearnedManifest::load(workspace);
    let existed = manifest.remove(name);
    if existed {
        manifest.save(workspace)?;
    }
    Ok(existed)
}

/// 从 SKILL.md 内容提取 frontmatter `description:` 一句话（去包裹引号）。
/// 无 frontmatter / 无该键 = 空串。只在首个 frontmatter 块内找。
pub fn extract_description(content: &str) -> String {
    let mut in_frontmatter = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            if in_frontmatter {
                // frontmatter 结束，还没找到 = 没有。
                break;
            }
            in_frontmatter = true;
            continue;
        }
        if in_frontmatter && let Some(rest) = trimmed.strip_prefix("description:") {
            let desc = rest.trim();
            // 去常见包裹引号（YAML 双引号/单引号形态）。
            let desc = desc
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| desc.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(desc);
            return desc.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests;
