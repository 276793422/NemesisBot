// media.rs 覆盖率补充测试（下载 send 失败 → 空串诚实返回 202-204）。
//
// 豁免：182-184（reqwest client 构建失败臂——带 timeout 的 builder 实践
// 上不失败，死臂）。

use super::*;

/// 不可达 URL → send 失败 → 空串（202-204），不落盘、不 panic。
#[tokio::test]
async fn download_dead_url_returns_empty_string() {
    let out = download_file_with_opts(
        "http://127.0.0.1:1/nmb-cov-gone.bin",
        "nmb-cov-download.bin",
        DownloadOptions::default(),
    )
    .await;
    assert_eq!(out, "");
}
