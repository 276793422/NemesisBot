//! watcher 连接原语覆盖率补充测试（父目录创建 + 非法路径错误分支）。

use std::path::PathBuf;

use crate::watcher::{data_version, open_conn};

fn temp_path(tag: &str, suffix: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nemesis_watcher_cov_{}_{}_{}{}",
        tag,
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        suffix
    ));
    path
}

/// 正常路径：缺失父目录自动创建，连接打开 + busy_timeout 生效，
/// data_version 初读为非负。
#[test]
fn open_conn_creates_parent_and_reads_data_version() {
    let db_path = temp_path("ok", "/data/nemesisbot_data.db");
    let conn = open_conn(&db_path).expect("open with missing parents");
    let v = data_version(&conn).expect("read data_version");
    assert!(v >= 0);
    let _ = std::fs::remove_dir_all(db_path.parent().unwrap());
}

/// 数据库路径指向一个目录 → Connection::open 失败（错误分支）。
#[test]
fn open_conn_on_directory_fails() {
    let dir = temp_path("dir", "");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let err = open_conn(&dir).expect_err("directory is not a database file");
    assert!(err.contains("watcher open"), "got: {err}");
    let _ = std::fs::remove_dir(&dir);
}
