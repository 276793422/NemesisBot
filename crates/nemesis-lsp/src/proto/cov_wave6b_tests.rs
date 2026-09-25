//! proto.rs 覆盖率收尾（Wave6B）：`path_to_uri` 的相对路径回退臂——
//! Url::from_file_path 对相对路径失败 → 手拼 file:// 补前导斜杠（202）。
//!
//! 既有测试全走绝对路径（Url 直通臂），本用例补相对路径形态。

use super::*;

/// 相对路径 → Url 失败 → 回退手拼（197-203 全链，含 202 的补斜杠）。
#[test]
fn path_to_uri_relative_path_falls_back_to_manual_encoding() {
    let uri = path_to_uri(std::path::Path::new("relative_covw6b/dir/file.rs"));
    assert!(uri.starts_with("file:///"), "相对路径必须补前导斜杠: {uri}");
    assert!(uri.ends_with("/relative_covw6b/dir/file.rs"), "{uri}");
}
