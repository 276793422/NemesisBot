//! 文件系统韧性小工具（crate 内共享）。

/// 带退避重试的目录树删除，返回是否删除成功。
///
/// Windows 上刚写完的文件常被 Defender 实时扫描 / 索引服务短暂持有句柄，
/// `std::fs::remove_dir_all` 会瞬时失败（os error 5 Access denied /
/// 32 Sharing violation / 145 Directory not empty）——单发必胜的假设在
/// CI runner 上实证不成立（cluster-uat T-MRG-1：变更集/工作副本清扫
/// 失败被 `let _` 静默吞掉，残留目录直接打红验收断言）。本包装在
/// ~4.4s 窗口内退避重试让句柄释放；窗口内的中间失败是常态，只落
/// `debug!`，彻底失败才 `warn!` 留痕（带最后错误）。清理类调用点维持
/// 「尽力而为」语义：不向上传播错误，残留由启动清扫兜底。
pub(crate) fn remove_dir_all_resilient(dir: &std::path::Path) -> bool {
    let mut last_err = None;
    for attempt in 0..10u32 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
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
        std::thread::sleep(std::time::Duration::from_millis(80 * (attempt as u64 + 1)));
    }
    tracing::warn!(
        dir = %dir.display(),
        last_error = ?last_err,
        "[FsUtil] 目录树删除重试耗尽（残留交启动清扫兜底）"
    );
    false
}
