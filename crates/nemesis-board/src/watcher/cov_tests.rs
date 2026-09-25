// watcher.rs 覆盖率补充测试（wave6：open_conn 父目录缺失时现场补建 25-26）。

use super::*;
use std::path::PathBuf;

fn w6_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nmb-watcher-w6-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// open_conn：库文件父目录不存在 → create_dir_all 补建后正常打开（26）；
/// data_version 原语可读。
#[test]
fn w6_open_conn_creates_missing_parent_dir() {
    let dir = w6_dir("fresh");
    let db_path = dir.join("fresh").join("nested").join("board.db");
    let conn = open_conn(&db_path).expect("watcher 连接必须补建父目录");
    assert!(db_path.is_file());
    let v0 = data_version(&conn).unwrap();

    // 另一连接写入 → watcher 视角 data_version 前进（本连接自身零写入）。
    let writer = rusqlite::Connection::open(&db_path).unwrap();
    writer
        .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
        .unwrap();
    let v1 = data_version(&conn).unwrap();
    assert!(v1 > v0, "其他连接的写必须被看见：{v0} -> {v1}");
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}
