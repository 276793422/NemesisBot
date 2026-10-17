//! 文件系统韧性小工具（crate 内共享）。
//!
//! 实现已提升到 `nemesis_utils::fsutil`（全仓共享单一真相源——board/sandbox/
//! skills 等不依赖 cluster 的 crate 同样需要韧性删除）；本模块保留 re-export，
//! crate 内既有调用点零改动。

/// 远端可控 task_id → 安全文件名成分（crate 内单一包装，T7/T8/T9/T10 同族
/// 修复的统一入口，2026-10-17 三轮审计）。
///
/// 委托仓内单一真相源 [`nemesis_utils::sanitize::sanitize_path_segment`]
/// 白名单消毒（兜 `..` / 路径分隔符 / 超长）；消毒改变原值时 WARN 留痕
/// （可观测性：正常 UUID/单号 task_id 映射恒等，见 WARN 即有对端发来
/// 异形 id）。读侧消费者（nemesis-board dossier / nemesisbot
/// board_archive_ingest）直接调 `sanitize_path_segment`——映射同源恒等，
/// 保证「写入消毒名、读取同名」不错位。
pub(crate) fn safe_task_file_name(task_id: &str) -> String {
    let safe = nemesis_utils::sanitize::sanitize_path_segment(task_id);
    if safe != task_id {
        tracing::warn!("[FsUtil] task_id 含不安全字符，已消毒落盘: {task_id:?} -> {safe:?}");
    }
    safe
}

/// 带退避重试的**单文件**删除，返回是否删除成功（LOW-5，2026-10-17 四轮
/// 审计）：Windows 瞬时句柄（AV/索引器）对单文件同样成立——裸
/// `std::fs::remove_file` 单发必胜假设与目录树版同理不成立，影响 = 残留
/// 文件 + WARN（清理类调用点维持「尽力而为」语义：NotFound 视为成功，
/// 不向上传播错误）。目录树版已在 `nemesis_utils::fsutil`（全仓共享，
/// 见上方 re-export）；单文件版消费面目前全在本 crate，留此待共享面
/// 出现再提升。
pub(crate) fn remove_file_resilient(path: &std::path::Path) -> bool {
    let mut last_err = None;
    for attempt in 0..10u32 {
        match std::fs::remove_file(path) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(e) => {
                tracing::debug!(
                    path = %path.display(),
                    attempt,
                    error = %e,
                    "[FsUtil] 文件删除失败，退避重试"
                );
                last_err = Some(e);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(80 * (attempt as u64 + 1)));
    }
    tracing::warn!(
        path = %path.display(),
        last_error = ?last_err,
        "[FsUtil] 文件删除重试耗尽（残留文件留存）"
    );
    false
}

pub(crate) use nemesis_utils::fsutil::remove_dir_all_resilient;
