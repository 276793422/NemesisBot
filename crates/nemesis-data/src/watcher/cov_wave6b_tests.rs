//! watcher.rs 覆盖率收尾（Wave6B）：`open_conn` 的无父目录臂（26 行：
//! db_path 无 parent → 跳过 create_dir_all）。
//!
//! 空路径 = SQLite 匿名临时库（关闭即弃），零磁盘残留。

use super::open_conn;

/// db_path 无父目录 → 跳过 create_dir_all → Connection::open("") 打开
/// 匿名临时库 → Ok。
#[test]
fn open_conn_without_parent_skips_mkdir() {
    let conn = open_conn(std::path::Path::new("")).expect("匿名临时库必须能打开");
    // 连接可用性对照：能跑 PRAGMA 即活连接。
    let ver: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("live connection");
    assert_eq!(ver, 0);
}
