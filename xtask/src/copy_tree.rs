//! 仓库 → 装配复制体的递增同步复制（`assembly/`，git 忽略）。
//!
//! mtime 保持：每个复制后的文件 `set_modified(源 mtime)`——下游 cargo 以
//! mtime+size 做 fingerprint，若按复制时刻写时间戳，共享 `assembly/target`
//! 的增量缓存每轮被打穿（全量重编）。同名跳过规则分两级：任意深度（构建
//! 产物/依赖树/git 元数据）与仅仓库根（bin/ 产物区、assembly 自身防递归）。
//!
//! 同步是三态镜像（BUG-20261112-123）：新增/更新之外还要**删除**——仓库侧
//! 已不存在的条目从复制体摘除。include_dir! 嵌入面（`nemesisbot/workspace/`
//! 模板与技能、`crates/nemesis-web/static/` 资产）对「删了但复制体还留着」
//! 完全失明：增量装配不摘除，陈旧幽灵文件烤进商业交付物。跳过名单（构建
//! 产物/依赖树/git 元数据）不参与删除——从未同步的条目不归同步管。

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// 任意深度跳过的目录名：git 元数据（含 submodule 挂载点里的 `.git` 指针
/// 文件——复制体不做任何 git 操作，悬空指针无害跳过）、构建产物、前端依赖
/// 树、本地工具配置。
pub const SKIP_ANY_DEPTH: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".claude",
    ".vscode",
    ".idea",
    ".nemesisbot",
];

/// 仅仓库根跳过的目录名：根 `bin/` 是平台产物区（build-windows.bat 输出）；
/// `installer/core/src/bin`、`test-tools/dummy-targets/bin` 等源码 bin 目录
/// 不受影响。`assembly` 跳过防递归（复制体自身不在复制源里）。
pub const SKIP_ROOT_ONLY: &[&str] = &["bin", "assembly"];

#[derive(Debug, Default)]
pub struct SyncStats {
    pub copied: usize,
    pub skipped_unchanged: usize,
    pub dirs_created: usize,
    /// 镜像删除：仓库侧已删、复制体侧摘除的文件数（BUG-20261112-123）。
    pub pruned_files: usize,
    /// 镜像删除：仓库侧已删、复制体侧整目录摘除的目录数（BUG-20261112-123）。
    pub pruned_dirs: usize,
    pub warnings: Vec<String>,
}

pub fn sync_repo(src_root: &Path, dst_root: &Path) -> Result<SyncStats> {
    let mut stats = SyncStats::default();
    sync_dir(src_root, dst_root, true, &mut stats)?;
    Ok(stats)
}

fn sync_dir(src: &Path, dst: &Path, is_root: bool, stats: &mut SyncStats) -> Result<()> {
    let rd = fs::read_dir(src).with_context(|| format!("枚举目录 {}", src.display()))?;
    if !dst.exists() {
        fs::create_dir_all(dst).with_context(|| format!("创建目录 {}", dst.display()))?;
        stats.dirs_created += 1;
    }
    // 镜像删除的 src 名集合：**跳过名单条目也计入**（它们在场、只是不同步
    // ——dst 对应条目必须存活，删除面只针对 src 真缺席的名字）。
    let mut src_names: Vec<String> = Vec::new();
    for entry in rd {
        let entry = entry.with_context(|| format!("读目录项 {}", src.display()))?;
        let ft = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        src_names.push(name.clone());
        if ft.is_symlink() {
            stats
                .warnings
                .push(format!("跳过符号链接 {}", entry.path().display()));
            continue;
        }
        if ft.is_dir() {
            if SKIP_ANY_DEPTH.contains(&name.as_str())
                || (is_root && SKIP_ROOT_ONLY.contains(&name.as_str()))
            {
                continue;
            }
            sync_dir(&entry.path(), &dst.join(&name), false, stats)?;
        } else if name == ".git" {
            // submodule 挂载点的 .git 指针**文件**——按名跳过（复制体不做
            // git 操作，悬空指针无意义）
            continue;
        } else {
            let src_path = entry.path();
            let dst_path = dst.join(&name);
            if file_unchanged(&src_path, &dst_path) {
                stats.skipped_unchanged += 1;
            } else {
                fs::copy(&src_path, &dst_path).with_context(|| {
                    format!("复制 {} → {}", src_path.display(), dst_path.display())
                })?;
                preserve_mtime(&src_path, &dst_path);
                stats.copied += 1;
            }
        }
    }
    prune_extraneous(dst, &src_names, is_root, stats)?;
    Ok(())
}

/// 镜像删除（同步三态的第三态，BUG-20261112-123）：复制体侧 src 缺席的
/// 条目摘除——文件摘单文件、目录整子树摘除。跳过名单内的名字不摘（与
/// 同步面同一豁免口径：构建产物/依赖树/git 元数据从未同步，不归同步管，
/// 保守不越权）。失败即 Err（同步阶段目标文件未被构建占用；韧性退避由
/// `remove_with_retry` 承担 Windows AV/索引器瞬时句柄）。
fn prune_extraneous(
    dst: &Path,
    src_names: &[String],
    is_root: bool,
    stats: &mut SyncStats,
) -> Result<()> {
    let rd = fs::read_dir(dst).with_context(|| format!("枚举目标目录 {}", dst.display()))?;
    for entry in rd {
        let entry = entry.with_context(|| format!("读目录项 {}", dst.display()))?;
        let ft = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_ANY_DEPTH.contains(&name.as_str())
            || (is_root && SKIP_ROOT_ONLY.contains(&name.as_str()))
            || src_names.contains(&name)
        {
            continue;
        }
        let path = dst.join(&name);
        if ft.is_dir() {
            remove_with_retry(&path, true)
                .with_context(|| format!("镜像删除多余目录 {}", path.display()))?;
            stats.pruned_dirs += 1;
        } else {
            // 符号链接也落此臂（is_dir=false）——remove_file 摘链接本身。
            remove_with_retry(&path, false)
                .with_context(|| format!("镜像删除多余文件 {}", path.display()))?;
            stats.pruned_files += 1;
        }
    }
    Ok(())
}

/// 韧性删除（Windows AV/索引器瞬时句柄是常态）：3 次重试 + 退避，与
/// assemble::remove_dir_all_resilient 同口径。
fn remove_with_retry(path: &Path, is_dir: bool) -> Result<()> {
    let mut last_err = None;
    for attempt in 0..3 {
        let r = if is_dir {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match r {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(300 * (attempt + 1)));
            }
        }
    }
    Err(anyhow::Error::from(last_err.expect("有错才走到这")))
}

/// size+mtime 双等 = 视为未变（与 cargo fingerprint 同款口径）。
fn file_unchanged(src: &Path, dst: &Path) -> bool {
    let (Ok(s), Ok(d)) = (fs::metadata(src), fs::metadata(dst)) else {
        return false;
    };
    s.len() == d.len() && s.modified().ok() == d.modified().ok()
}

/// 复制后把源 mtime 写到目标（std file_set_times）。失败静默——最坏代价是
/// 下轮多复制一次该文件，不影响正确性。
fn preserve_mtime(src: &Path, dst: &Path) {
    if let Ok(mtime) = fs::metadata(src).and_then(|m| m.modified())
        && let Ok(f) = fs::OpenOptions::new().write(true).open(dst)
    {
        let _ = f.set_modified(mtime);
    }
}

#[cfg(test)]
mod tests;
