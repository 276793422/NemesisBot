//! File utilities.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Atomically write data to a file: unique same-dir temp file → write →
/// flush → `sync_all` → (unix) rename over target → (unix) parent-dir sync.
///
/// REL-002（2026-09-23）关键配置/状态写入的唯一权威 helper——所有配置修改
/// 入口都调这里，不在各模块重复自制 tmp+rename。
///
/// - **唯一临时名** `.tmp-{pid}-{nanos}`：并发写同一目标不互踩。
/// - **`sync_all` 先于 rename**：断电/崩溃后目标要么是完整新内容要么是旧
///   文件，绝不出现半截 JSON/TOML。
/// - **unix 权限在临时文件创建时即挂**（`mode(perm)`）：不存在先宽后收的
///   窗口；Windows 为 no-op（ACL 不处理，诚实边界）。
/// - **失败清理**：任一步失败删除临时文件、原文件不动，错误带路径上下文。
/// - Windows `fs::rename` 即 `MOVEFILE_REPLACE_EXISTING`（覆盖已存在目标）；
///   目标被外部占用（AV 扫描/打开句柄）时 rename 失败属预期，由调用方决定
///   重试或上报。
/// - **不做跨进程锁**：配置写入低频，同配置多进程同写场景当前不存在
///   （诚实边界；唯一临时名已消除进程内并发互踩）。
pub fn write_file_atomic(path: &str, data: &[u8], perm: u32) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = perm; // Windows：POSIX 权限不存在，ACL 不处理（诚实边界）
    let p = Path::new(path);
    let parent = p.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| format!("atomic write {}: mkdir: {e}", p.display()))?;

    let tmp_path = parent.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));

    let mut created = false;
    let result = (|| -> Result<(), String> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(perm);
        }
        let mut tmp = opts
            .open(&tmp_path)
            .map_err(|e| format!("atomic write {}: create temp: {e}", p.display()))?;
        created = true;
        tmp.write_all(data)
            .map_err(|e| format!("atomic write {}: write temp: {e}", p.display()))?;
        tmp.flush()
            .map_err(|e| format!("atomic write {}: flush: {e}", p.display()))?;
        tmp.sync_all()
            .map_err(|e| format!("atomic write {}: sync: {e}", p.display()))?;
        drop(tmp);
        // 覆盖语义：unix rename 天然替换；Windows std rename 即
        // MOVEFILE_REPLACE_EXISTING。
        fs::rename(&tmp_path, p)
            .map_err(|e| format!("atomic write {}: rename: {e}", p.display()))?;
        // rename 本身的持久化：尽力 sync 父目录（失败不影响结果正确性）。
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    if result.is_err() && created {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Ensure a directory exists.
pub fn ensure_dir(path: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("create dir: {}", e))
}

/// Read file to string.
pub fn read_file_string(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("read: {}", e))
}

#[cfg(test)]
mod tests;
