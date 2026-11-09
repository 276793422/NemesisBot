//! 韧性文件操作（环境敏感纪律：Windows AV 扫描 / 索引器瞬时句柄是常态
//! 不是异常——裸 remove 在 CI/杀软机上假红率高，统一走有限重试）。
//!
//! 与主仓 `nemesis-cluster::fsutil` 同纪律；本 crate 独立 workspace 不
//! path dep nemesis-cluster（依赖单向：只允许 installer→外部最小面，
//! nemesis-cluster 拖 tokio 全家，不值）。

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow};

/// 重试参数：3 次 × 150ms（AV 扫描窗口通常 < 500ms，超出 = 真问题）。
const ATTEMPTS: u32 = 3;
const BACKOFF: Duration = Duration::from_millis(150);

/// 删单个文件（幂等：不存在 = Ok）。
pub fn delete_file(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(BACKOFF);
            }
        }
    }
    Err(anyhow!(
        "删除失败（重试 {ATTEMPTS} 次）: {}: {}",
        path.display(),
        last.unwrap()
    ))
}

/// 递归删目录（幂等；韧性重试逐层）。
pub fn remove_dir_all(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let mut last = None;
    for _ in 0..ATTEMPTS {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(BACKOFF);
            }
        }
    }
    Err(anyhow!(
        "删除目录失败（重试 {ATTEMPTS} 次）: {}: {}",
        path.display(),
        last.unwrap()
    ))
}

/// 自 `start` 向上剪除空目录，到 `stop_at` 为止（不含）。best-effort：
/// 目录非空 / 删除失败静默停（剪空目录是卫生动作，不是正确性动作）。
pub fn prune_empty_dirs(start: &Path, stop_at: &Path) {
    let mut cur = start.to_path_buf();
    loop {
        if cur == stop_at || !cur.starts_with(stop_at) {
            return;
        }
        let empty = std::fs::read_dir(&cur).is_ok_and(|mut it| it.next().is_none());
        if !empty {
            return;
        }
        if std::fs::remove_dir(&cur).is_err() {
            return;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests;
