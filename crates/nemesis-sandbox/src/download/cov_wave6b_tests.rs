//! download.rs 覆盖率收尾（Wave6B）：`download_and_verify` 的 dest 无父
//! 目录臂（盘根 → 跳过 create_dir_all）。
//!
//! 纪律：URL 用空串——reqwest 在构造期即报错（relative URL without a
//! base），await 立即返回 Err，**不触网**；dest 盘根也不落任何文件。

use super::*;

/// dest 为盘根（parent() == None）→ 跳过 create_dir_all（48 收口臂）→
/// 空 URL 在 reqwest 构造期失败 → Err 带 "fetch" context。
#[tokio::test]
async fn download_and_verify_skips_mkdir_when_dest_has_no_parent() {
    let err = download_and_verify("", None, std::path::Path::new(r"C:\"))
        .await
        .expect_err("空 URL 必须失败");
    let msg = format!("{err:#}");
    assert!(msg.contains("fetch"), "必须带 fetch context: {msg}");
}
