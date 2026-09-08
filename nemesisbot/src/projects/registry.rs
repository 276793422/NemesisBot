//! 项目注册表（`<workspace>/config/projects.json`；L6++ Phase 1）。
//!
//! **注册不拥有**：删除项目只解除分组，不删会话与项目目录内的任何文件
//! （行业共识，设计档 §2）。纯文件操作：load（缺席=空表首用 / 损坏=Err，
//! 写路径 loud 拒绝——绝不把损坏文件用空表覆盖，那等于静默清空用户数据）+
//! save（原子写 tmp+rename，同 `eval_assessor::save_rules` 形态）+
//! create/remove/rename。
//!
//! 路径 canonicalize 一次（`canonicalize_for_compare`，剥 verbatim + 归一
//! 8.3 短名），与围栏比较层同表示——「构建时 canonicalize 一次」的地基（M2
//! 项目工厂直接消费 entry.path，不再二次 canonicalize）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use nemesis_path::paths::canonicalize_for_compare;
use nemesis_path::resolve_projects_registry_path_in_workspace;

/// id 前缀（`p-{8hex}`）。
const ID_PREFIX: &str = "p-";
/// id 的 hex 段长度。
const ID_HEX_LEN: usize = 8;
/// 项目名称最大长度（防滥用；超限拒绝）。
const NAME_MAX: usize = 100;

/// 单条项目注册。创建时烧入：`path` 不可改（改路径 = 移除后重建，Phase 1
/// 语义）；`name` 可改（rename）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectEntry {
    pub id: String,
    pub name: String,
    /// canonical（剥 verbatim）后的项目目录绝对路径。
    pub path: PathBuf,
    /// RFC3339 创建时间。
    pub created_at: String,
}

/// projects.json 根结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectsFile {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

/// 注册表路径（`<workspace>/config/projects.json`；真相源在 nemesis-path，
/// 本封装为 nemesisbot 侧便捷入口）。
pub fn registry_path(workspace: &Path) -> PathBuf {
    resolve_projects_registry_path_in_workspace(workspace)
}

/// 读注册表。`Ok(None)` = 文件不存在（首次使用）；`Err` = 存在但损坏或不可
/// 读——**写路径必须 loud 拒绝**（用空表覆盖损坏文件 = 静默清空用户数据）。
pub fn try_load_projects(path: &Path) -> Result<Option<ProjectsFile>> {
    let data = match std::fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(anyhow!("读取项目注册表失败 {}: {}", path.display(), e));
        }
    };
    match serde_json::from_str::<ProjectsFile>(&data) {
        Ok(f) => Ok(Some(f)),
        Err(e) => {
            tracing::warn!(
                "[projects] 注册表解析失败 {}: {}（写操作拒绝，避免覆盖损坏文件）",
                path.display(),
                e
            );
            Err(anyhow!(
                "项目注册表已损坏: {}（{e}）；请手动修复或删除该文件后重试",
                path.display()
            ))
        }
    }
}

/// 读注册表（list 语义）：缺席/损坏都给空表 + warn——读路径不炸。
pub fn load_projects_lenient(path: &Path) -> ProjectsFile {
    match try_load_projects(path) {
        Ok(f) => f.unwrap_or_default(),
        Err(_) => ProjectsFile::default(),
    }
}

/// 原子写（同目录 tmp + rename；Windows rename 带 REPLACE_EXISTING，同目录
/// 保证同卷）。rename 失败时清残留 tmp（best-effort）。
pub fn save_projects(path: &Path, file: &ProjectsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(file).context("serialize projects")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &content).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("rename {} -> {}", tmp.display(), path.display())
    })?;
    Ok(())
}

/// `p-{8hex}`：uuid v4 前 4 字节 hex（uuid 已在依赖树——nemesis-web
/// sessions.create 同源形态）。
pub fn new_project_id() -> String {
    let u = uuid::Uuid::new_v4();
    let hex: String = u.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("{ID_PREFIX}{}", &hex[..ID_HEX_LEN])
}

/// 规范化 + 校验项目路径：非空、绝对、存在且为目录、canonical 后不与主
/// workspace 或已有项目重叠（相等或任一为另一的祖先——防围栏嵌套歧义）。
/// 返回 canonical 路径（剥 verbatim，围栏比较层同表示）。
pub fn canonicalize_project_path(
    raw: &str,
    main_workspace: &Path,
    existing: &[ProjectEntry],
) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("项目路径不能为空");
    }
    let p = Path::new(raw);
    if !p.is_absolute() {
        bail!("项目路径必须是绝对路径: {raw}");
    }
    let canon = canonicalize_for_compare(p);
    if !canon.is_dir() {
        bail!("项目目录不存在或不是目录: {}", canon.display());
    }
    let ws = canonicalize_for_compare(main_workspace);
    if paths_overlap(&ws, &canon) {
        bail!(
            "项目目录与主工作区重叠: {}（围栏嵌套歧义，禁止）",
            canon.display()
        );
    }
    for e in existing {
        if paths_overlap(&e.path, &canon) {
            bail!(
                "项目目录与项目「{}」重叠: {} vs {}",
                e.name,
                e.path.display(),
                canon.display()
            );
        }
    }
    Ok(canon)
}

/// 相等或祖先关系（双向组件级 starts_with——canonical 后可直接比较）。
fn paths_overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// 校验项目名（create/rename 共用；单一真相源）。
fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("项目名称不能为空");
    }
    if name.chars().count() > NAME_MAX {
        bail!("项目名称过长（>{NAME_MAX} 字符）");
    }
    Ok(name.to_string())
}

/// 构造新条目（纯函数，不落盘）：名字校验 → 上限校验 → 路径校验 → 烧入
/// id/时间。
pub fn build_new_entry(
    file: &ProjectsFile,
    main_workspace: &Path,
    name: &str,
    raw_path: &str,
    max: usize,
) -> Result<ProjectEntry> {
    let name = validate_name(name)?;
    if file.projects.len() >= max {
        bail!(
            "项目数已达上限（{max}）：每项目一个常驻执行引擎，如需更多请调大 config projects.max"
        );
    }
    let path = canonicalize_project_path(raw_path, main_workspace, &file.projects)?;
    Ok(ProjectEntry {
        id: new_project_id(),
        name,
        path,
        created_at: chrono::Local::now().to_rfc3339(),
    })
}

/// 创建项目（读-校验-写；损坏文件 loud 拒绝）。返回新条目（调用方 spawn loop）。
pub fn create_project(
    path: &Path,
    main_workspace: &Path,
    name: &str,
    raw_path: &str,
    max: usize,
) -> Result<ProjectEntry> {
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let entry = build_new_entry(&file, main_workspace, name, raw_path, max)?;
    file.projects.push(entry.clone());
    save_projects(path, &file)?;
    Ok(entry)
}

/// 移除项目（只解除分组——不删会话/文件；不存在报错）。返回被移除条目
/// （调用方负责 stop loop + 路由摘除）。
pub fn remove_project(path: &Path, id: &str) -> Result<ProjectEntry> {
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let idx = file
        .projects
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    let entry = file.projects.remove(idx);
    save_projects(path, &file)?;
    Ok(entry)
}

/// 重命名（`path` 不可改——Phase 1：改路径 = 移除后重建）。返回更新后条目。
pub fn rename_project(path: &Path, id: &str, new_name: &str) -> Result<ProjectEntry> {
    let new_name = validate_name(new_name)?;
    let mut file = try_load_projects(path)?.unwrap_or_default();
    let entry = file
        .projects
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| anyhow!("项目不存在: {id}"))?;
    entry.name = new_name;
    let updated = entry.clone();
    save_projects(path, &file)?;
    Ok(updated)
}

/// 列出全部（lenient：损坏给空 + warn——读路径不炸）。
pub fn list_projects(path: &Path) -> Vec<ProjectEntry> {
    load_projects_lenient(path).projects
}

/// 按 id 查单条（lenient 语义）。
pub fn find_project(path: &Path, id: &str) -> Option<ProjectEntry> {
    list_projects(path).into_iter().find(|p| p.id == id)
}

#[cfg(test)]
mod tests;
