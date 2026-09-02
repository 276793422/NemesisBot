//! S9 覆盖率批次（request_logger.rs:771 resolve_log_path 双分支）。
//! 私有函数，经由子模块直接驱动（绝对路径原样 / 相对路径 join workspace）。

use super::*;

#[test]
fn resolve_log_path_absolute_passthrough() {
    let abs = if cfg!(windows) {
        "C:/nemesis/s9/abs/logs"
    } else {
        "/nemesis/s9/abs/logs"
    };
    let out = resolve_log_path(abs, std::path::Path::new("/some/workspace"));
    assert_eq!(out, std::path::PathBuf::from(abs));
}

#[test]
fn resolve_log_path_relative_joins_workspace() {
    let out = resolve_log_path(
        "logs/llm",
        std::path::Path::new(
            "C:/ws/s9", /* 平台无关：PathBuf join 语义一致 */
        ),
    );
    assert_eq!(out, std::path::Path::new("C:/ws/s9").join("logs/llm"));
}
