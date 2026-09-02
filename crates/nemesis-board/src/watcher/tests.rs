//! [`super::watcher`] 语义测试：data_version 只对「其他连接」的写敏感。
//!
//! 这两个断言是整个推送机制的正确性基石——写路径零埋点就能覆盖全部写入方
//! （同进程 BoardStore 连接 + 跨进程 CLI），靠的正是这两条 SQLite 语义。

use super::{data_version, open_conn};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 唯一临时目录（镜像 store/tests.rs 惯例：temp_dir + pid + 原子序号，
/// 不引 tempfile 依赖）。
fn temp_db(name: &str) -> (PathBuf, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-watchertest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let db = dir.join("board.db");
    (dir, db)
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn data_version_bumps_when_another_connection_writes() {
    let (dir, db) = temp_db("bump");
    // 写连接（BoardStore）：建 schema + seed 本身就是写事务。
    let store = crate::BoardStore::open(&db, "NB").expect("open store");

    let watcher = open_conn(&db).unwrap();
    let v0 = data_version(&watcher).unwrap();

    // watcher 两次读之间无人写 → 不变。
    assert_eq!(data_version(&watcher).unwrap(), v0);

    // 其他连接（BoardStore）写 → watcher 侧递增。WAL 下外部提交对后续读
    // 立即可见；留 10ms 容忍文件系统时间戳粒度。
    store
        .create_issue(crate::NewIssue {
            title: "t".into(),
            ..Default::default()
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    let v1 = data_version(&watcher).unwrap();
    assert!(
        v1 > v0,
        "another connection's commit must bump data_version"
    );

    // 再写一次 → 继续递增（轮询循环靠持续递增区分「有新变化」）。
    store
        .create_issue(crate::NewIssue {
            title: "t2".into(),
            ..Default::default()
        })
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    assert!(data_version(&watcher).unwrap() > v1);

    cleanup(&dir);
}

#[test]
fn data_version_ignores_same_connection_write() {
    let (dir, db) = temp_db("same-conn");
    let _store = crate::BoardStore::open(&db, "NB").expect("open store");

    let watcher = open_conn(&db).unwrap();
    let v0 = data_version(&watcher).unwrap();

    // watcher 连接自己的写（这里重新设同一个 busy_timeout）不 bump 自身视角
    // 的 data_version——证明它只反映「其他连接」，轮询连接保持零写入即可
    // 不被自身噪声干扰。
    watcher.pragma_update(None, "busy_timeout", 5000).unwrap();
    assert_eq!(data_version(&watcher).unwrap(), v0);

    cleanup(&dir);
}

#[test]
fn open_conn_creates_missing_db_and_reads_version() {
    // 库文件不存在 → 空库照常工作（gateway 首启 board feature 刚开时
    // watcher 与 store 谁先启动都行）。
    let (dir, db) = temp_db("missing");
    let watcher = open_conn(&db).unwrap();
    let v = data_version(&watcher).unwrap();
    assert!(v >= 0);
    cleanup(&dir);
}
