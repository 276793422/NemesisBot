//! 文件系统韧性小工具（crate 内共享）。

/// 带退避重试的目录树删除，返回是否删除成功。
///
/// Windows 上刚写完的文件常被 Defender 实时扫描 / 索引服务短暂持有句柄，
/// `std::fs::remove_dir_all` 会瞬时失败（os error 5 Access denied /
/// 32 Sharing violation / 145 Directory not empty）——单发必胜的假设在
/// CI runner 上实证不成立（cluster-uat T-MRG-1：变更集/工作副本清扫
/// 失败被 `let _` 静默吞掉，残留目录直接打红验收断言）。本包装在
/// ~2s 短窗内退避重试让句柄释放；彻底失败时 `warn!` 留痕。清理类
/// 调用点维持「尽力而为」语义：不向上传播错误，残留由启动清扫兜底。
pub(crate) fn remove_dir_all_resilient(dir: &std::path::Path) -> bool {
    for attempt in 0..8u32 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(e) => {
                // 末次失败才留痕（重试窗口内的中间失败是常态，不刷日志）。
                if attempt == 7 {
                    tracing::warn!(
                        dir = %dir.display(),
                        error = %e,
                        "[FsUtil] 目录树删除重试耗尽（残留交启动清扫兜底）"
                    );
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
    }
    false
}
