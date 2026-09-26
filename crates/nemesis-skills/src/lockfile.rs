//! 技能 lockfile（P14）：`<workspace>/skills.lock.json` 逐文件哈希记账。
//!
//! 每技能记录 `{slug, source, commit, files: {path: sha256}, installed_at,
//! verified_state}`；装/卸/更新三路维护；`verify_drift` 做漂移检测
//! （文件被改 = 列出差异，不阻断——审计面，不是执行面）。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::warn;

use nemesis_types::error::{NemesisError, Result};

/// lockfile 文件名（置于 workspace 根）。
pub const LOCKFILE_NAME: &str = "skills.lock.json";

/// 单技能锁定条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockedSkill {
    /// 技能名（目标目录名）。
    pub slug: String,
    /// 来源（`github:<owner>/<repo>` / `registry:<name>/<slug>`）。
    pub source: String,
    /// 锁定的 commit SHA（GitHub 路径；未知为空）。
    #[serde(default)]
    pub commit: String,
    /// 相对路径 -> sha256（hex）。
    pub files: BTreeMap<String, String>,
    /// 安装时间（unix 秒）。
    pub installed_at: i64,
    /// 信任四态（`trust::TrustState::as_str()`）。
    pub verified_state: String,
}

/// lockfile 根结构。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillsLockfile {
    /// 格式版本。
    pub version: u32,
    /// slug -> 条目（BTreeMap 保证序列化顺序稳定，diff 友好）。
    pub skills: BTreeMap<String, LockedSkill>,
}

impl SkillsLockfile {
    /// 空 lockfile。
    pub fn new() -> Self {
        Self {
            version: 1,
            skills: BTreeMap::new(),
        }
    }

    /// workspace 根下的 lockfile 路径。
    pub fn path_for(workspace: &Path) -> std::path::PathBuf {
        workspace.join(LOCKFILE_NAME)
    }

    /// 从 workspace 读取；文件缺失/损坏按空表处理（损坏时 warn，不炸安装流）。
    pub fn load(workspace: &Path) -> Self {
        let path = Self::path_for(workspace);
        if !path.exists() {
            return Self::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<SkillsLockfile>(&data) {
                Ok(file) => file,
                Err(e) => {
                    warn!("skills.lock.json 解析失败，按空表继续: {}", e);
                    Self::new()
                }
            },
            Err(e) => {
                warn!("skills.lock.json 读取失败，按空表继续: {}", e);
                Self::new()
            }
        }
    }

    /// 写回 workspace（原子性要求低：记账文件，直接整写）。
    pub fn save(&self, workspace: &Path) -> Result<()> {
        let path = Self::path_for(workspace);
        let data = serde_json::to_string_pretty(self).map_err(NemesisError::Serialization)?;
        std::fs::write(&path, data).map_err(NemesisError::Io)?;
        Ok(())
    }

    /// 记录/覆盖一条（装与更新共用——更新即覆盖）。
    pub fn record(&mut self, entry: LockedSkill) {
        self.skills.insert(entry.slug.clone(), entry);
    }

    /// 移除一条；返回是否存在。
    pub fn remove(&mut self, slug: &str) -> bool {
        self.skills.remove(slug).is_some()
    }

    /// 查询一条。
    pub fn get(&self, slug: &str) -> Option<&LockedSkill> {
        self.skills.get(slug)
    }

    /// 计算技能目录内全部文件的 sha256（跳过点开头文件，与签名 manifest 同口径；
    /// 按字节哈希，非 UTF-8 文件也能记账）。
    pub fn compute_dir_hashes(skill_dir: &Path) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        Self::walk(skill_dir, skill_dir, &mut out)?;
        Ok(out)
    }

    fn walk(base: &Path, current: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        let entries = std::fs::read_dir(current).map_err(NemesisError::Io)?;
        for entry in entries {
            let entry = entry.map_err(NemesisError::Io)?;
            let path = entry.path();

            // 跳过隐藏文件与 .signature/.skill-origin.json（点开头统一跳过）。
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                Self::walk(base, &path, out)?;
            } else {
                let data = std::fs::read(&path).map_err(NemesisError::Io)?;
                let mut hasher = Sha256::new();
                hasher.update(&data);
                let hash = format!("{:x}", hasher.finalize());
                let relative = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    // 统一正斜杠：GitHub blob 路径与签名 manifest 都用 '/'，
                    // lockfile 在 Windows 上也要产出同形 key（否则漂移检测全假阳）。
                    .replace('\\', "/");
                out.insert(relative, hash);
            }
        }
        Ok(())
    }

    /// 漂移检测：lockfile 记账 vs 磁盘现状。
    pub fn verify_drift(&self, workspace: &Path, slug: &str) -> DriftReport {
        let mut report = DriftReport {
            slug: slug.to_string(),
            missing_files: Vec::new(),
            modified_files: Vec::new(),
            new_files: Vec::new(),
            clean: true,
        };

        let Some(entry) = self.skills.get(slug) else {
            report.clean = false;
            return report;
        };

        let skill_dir = workspace.join("skills").join(slug);
        if !skill_dir.exists() {
            report.missing_files = entry.files.keys().cloned().collect();
            report.clean = false;
            return report;
        }

        let current = match Self::compute_dir_hashes(&skill_dir) {
            Ok(h) => h,
            Err(e) => {
                warn!("drift 检测读取目录失败: {}", e);
                report.clean = false;
                return report;
            }
        };

        for (path, expected) in &entry.files {
            match current.get(path) {
                None => report.missing_files.push(path.clone()),
                Some(actual) => {
                    if actual != expected {
                        report.modified_files.push(path.clone());
                    }
                }
            }
        }
        for path in current.keys() {
            if !entry.files.contains_key(path) {
                report.new_files.push(path.clone());
            }
        }

        report.clean = report.missing_files.is_empty()
            && report.modified_files.is_empty()
            && report.new_files.is_empty();
        report
    }
}

/// 漂移检测结果。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DriftReport {
    /// 技能名。
    pub slug: String,
    /// 记账里有但磁盘上没有的文件。
    pub missing_files: Vec<String>,
    /// 内容哈希与记账不一致的文件。
    pub modified_files: Vec<String>,
    /// 磁盘上有但记账里没有的新文件。
    pub new_files: Vec<String>,
    /// 是否干净（无任何差异）。
    pub clean: bool,
}

impl DriftReport {
    /// 面向用户/模型的差异摘要（clean 时为空串）。
    pub fn summary(&self) -> String {
        if self.clean {
            return String::new();
        }
        let mut lines = vec![format!("技能 '{}' 存在漂移:", self.slug)];
        for f in &self.modified_files {
            lines.push(format!("  已修改: {}", f));
        }
        for f in &self.missing_files {
            lines.push(format!("  已丢失: {}", f));
        }
        for f in &self.new_files {
            lines.push(format!("  新增: {}", f));
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests;
