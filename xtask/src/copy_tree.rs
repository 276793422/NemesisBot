//! 仓库 → 装配复制体的递增同步复制（`assembly/`，git 忽略）。
//!
//! mtime 保持：每个复制后的文件 `set_modified(源 mtime)`——下游 cargo 以
//! mtime+size 做 fingerprint，若按复制时刻写时间戳，共享 `assembly/target`
//! 的增量缓存每轮被打穿（全量重编）。同名跳过规则分两级：任意深度（构建
//! 产物/依赖树/git 元数据）与仅仓库根（bin/ 产物区、assembly 自身防递归）。

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
    for entry in rd {
        let entry = entry.with_context(|| format!("读目录项 {}", src.display()))?;
        let ft = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().to_string();
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
    Ok(())
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
