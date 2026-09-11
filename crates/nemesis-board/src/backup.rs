//! board.db 每日备份（Swarm M2；impl-plan §4.3）。
//!
//! master 单点低成本兜底：`VACUUM INTO` 产生一致性快照到
//! `workspace/backups/board-YYYYMMDD.db`（WAL 模式下直接 copy 会撕裂，
//! VACUUM INTO 由 SQLite 保证原子）。保留最近 N 份，超出按日期名删除。
//! master 永久丢失场景：人工换 config 到新节点 + 拷回备份；HA 自动接管
//! 留给选举阶段（D1 裁决）。

use std::path::{Path, PathBuf};

/// 备份文件名前缀（ prune 只清这个模式的文件，不碰用户放的其他东西）。
const BACKUP_PREFIX: &str = "board-";

/// 生成今日备份：`VACUUM INTO <backups_dir>/board-YYYYMMDD.db`，然后裁剪
/// 只保留最近 `keep` 份。`keep == 0` = 备份关闭，直接返回 Ok(None)。
/// 同日重跑 = 覆盖当日文件（幂等）。返回备份文件路径（关闭时 None）。
pub fn backup_database(
    db_path: &Path,
    backups_dir: &Path,
    keep: usize,
) -> Result<Option<PathBuf>, String> {
    if keep == 0 {
        return Ok(None);
    }
    if !db_path.exists() {
        return Err(format!("board database not found at {}", db_path.display()));
    }
    std::fs::create_dir_all(backups_dir).map_err(|e| format!("create backups dir: {e}"))?;

    let today = chrono::Local::now().format("%Y%m%d");
    let target = backups_dir.join(format!("{BACKUP_PREFIX}{today}.db"));
    // VACUUM INTO 要求目标不存在；同日重跑先删旧快照（当日内容必然更新）。
    if target.exists() {
        std::fs::remove_file(&target).map_err(|e| format!("remove stale backup: {e}"))?;
    }

    let conn =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("open board db for backup: {e}"))?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")
        .map_err(|e| format!("set busy_timeout: {e}"))?;
    let target_sql = target
        .to_str()
        .ok_or_else(|| "backup path is not valid UTF-8".to_string())?
        .replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{target_sql}';"))
        .map_err(|e| format!("VACUUM INTO backup: {e}"))?;

    prune_old_backups(backups_dir, keep)?;
    Ok(Some(target))
}

/// 按文件名降序保留最近 `keep` 份 `board-*.db`，其余删除（忽略不合法名）。
fn prune_old_backups(backups_dir: &Path, keep: usize) -> Result<(), String> {
    let mut backups: Vec<PathBuf> = std::fs::read_dir(backups_dir)
        .map_err(|e| format!("read backups dir: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(BACKUP_PREFIX) && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();
    // 文件名含 YYYYMMDD，字典序即时间序。
    backups.sort();
    backups.reverse();
    for stale in backups.into_iter().skip(keep) {
        std::fs::remove_file(&stale).map_err(|e| format!("prune {}: {e}", stale.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
