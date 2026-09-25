//! pricing_store.rs 覆盖率收尾（Wave6B）：`lookup` 空串早退臂（143 行）。
//!
//! 分层查表的正向/降级路径由既有 cov_tests 覆盖，这里只补 trim 后为空
//! 的入口卫兵。tempfile 不在本 crate dev-deps，沿本目录惯例用
//! `std::env::temp_dir` + 进程 id 命名，测后清理。

use crate::pricing_store::PricingStore;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "nemesis_data_pricing_store_covw6b_{}_{}",
        std::process::id(),
        tag
    ));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// 空串 / 纯空白 → None（不进分层查表）。
#[test]
fn lookup_empty_or_blank_returns_none() {
    let dir = temp_dir("empty_lookup");
    let store = PricingStore::open(&dir).expect("空目录 open 必须成功");
    assert!(store.lookup("").is_none());
    assert!(store.lookup("   ").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
