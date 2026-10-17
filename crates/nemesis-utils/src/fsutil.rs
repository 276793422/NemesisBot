//! 文件系统韧性小工具（全仓共享；原 nemesis-cluster::fsutil 提升，见该 crate 的 re-export）。
//!
//! Windows 上刚写完的文件常被 Defender 实时扫描 / 索引服务短暂持有句柄，
//! `std::fs::remove_dir_all` 会瞬时失败（os error 5 Access denied /
//! 32 Sharing violation / 145 Directory not empty）——单发必胜的假设在
//! CI runner 上实证不成立（cluster-uat T-MRG-1：变更集/工作副本清扫
//! 失败被 `let _` 静默吞掉，残留目录直接打红验收断言）。

use std::path::Path;
use std::time::Duration;

const MAX_ATTEMPTS: u32 = 10;
const BACKOFF_MS: u64 = 80;

/// 带退避重试的目录树删除；耗尽后返回最后一次错误，`NotFound` 视为成功。
///
/// ~4.4s 窗口内退避重试让瞬时句柄释放。操作面变体：调用方把错误上抛
/// 报给用户/对端（rmdir、skill 删除、导出清空等「失败必须说话」的路径）。
pub fn remove_dir_all_retry(dir: impl AsRef<Path>) -> std::io::Result<()> {
    let dir = dir.as_ref();
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                tracing::debug!(
                    dir = %dir.display(),
                    attempt,
                    error = %e,
                    "[FsUtil] 目录树删除失败，退避重试"
                );
                last_err = Some(e);
            }
        }
        std::thread::sleep(Duration::from_millis(BACKOFF_MS * (attempt as u64 + 1)));
    }
    match last_err {
        Some(e) => Err(e),
        None => Err(std::io::Error::other("remove_dir_all retry exhausted")),
    }
}

/// 带退避重试的目录树删除，返回是否删除成功。
///
/// 清理类变体：窗口内的中间失败是常态，只落 `debug!`，彻底失败才
/// `warn!` 留痕（带最后错误）。清理类调用点维持「尽力而为」语义：
/// 不向上传播错误，残留由启动清扫兜底。
pub fn remove_dir_all_resilient(dir: impl AsRef<Path>) -> bool {
    let dir = dir.as_ref();
    match remove_dir_all_retry(dir) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                dir = %dir.display(),
                last_error = %e,
                "[FsUtil] 目录树删除重试耗尽（残留交启动清扫兜底）"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests;
