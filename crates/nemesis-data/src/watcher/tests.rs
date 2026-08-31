//! watcher 原语测试。

use super::*;
use std::path::PathBuf;

fn tmp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nb-data-watcher-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("nemesisbot_data.db")
}

/// data_version 对**其他连接**的写敏感：watcher 连接先读一个基线，另一条
/// 连接建表+插行后，watcher 再读应看到更大的值。
#[test]
fn data_version_sees_other_connection_writes() {
    let db = tmp_db("other-writes");
    // 先由 DataStore 的正常路径初始化 schema（watcher 不建表）。
    crate::db::init_db(&db).unwrap();

    let watch_conn = open_conn(&db).unwrap();
    let baseline = data_version(&watch_conn).unwrap();

    // 另一条连接写一次（模拟 AgentLoop 落明细行）。
    {
        let writer = Connection::open(&db).unwrap();
        writer
            .execute(
                "INSERT INTO request_logs (trace_id, model, created_at) VALUES ('t','m',1)",
                [],
            )
            .unwrap();
    }

    let after = data_version(&watch_conn).unwrap();
    assert!(after > baseline, "other-connection write must bump data_version");
}

/// watcher 连接保持零写入：open_conn 不跑迁移（user_version 不被碰），
/// 且对全新（不存在）路径也能打开（库文件由调用方先建；这里验证的是
/// open_conn 自身不因缺目录失败）。
#[test]
fn open_conn_is_zero_write_and_creates_missing_dirs() {
    let db = tmp_db("zero-write");
    crate::db::init_db(&db).unwrap();
    let before: i32 = Connection::open(&db)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();

    let watch_conn = open_conn(&db).unwrap();
    let _ = data_version(&watch_conn).unwrap(); // 只读

    let after: i32 = Connection::open(&db)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(before, after, "watcher must not touch user_version");

    // 缺失父目录：create_dir_all 补齐，不报错。
    let nested = std::env::temp_dir()
        .join(format!("nb-data-watcher-missing-{}", std::process::id()))
        .join("a")
        .join("b")
        .join("nemesisbot_data.db");
    let _ = std::fs::remove_dir_all(nested.parent().unwrap().parent().unwrap());
    assert!(open_conn(&nested).is_ok());
}
