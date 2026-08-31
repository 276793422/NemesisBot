//! db::init_db 的单元测试。
//!
//! 覆盖点：
//! - 正常路径：自动创建多级父目录、建 v1 全部表、写 user_version=1、二次 init 幂等；
//! - 行 61 错误分支：db 父路径的祖先是已存在的**文件** → create_dir_all 失败 →
//!   返回 "Failed to create data directory" 错误（未打开数据库即返回）。

use super::*;

/// 建一个干净的临时基准目录（按进程 + tag 隔离，进入前先清残留）。
fn temp_base(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_data_db_test_{}_{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn init_db_creates_parent_dirs_schema_and_is_idempotent() {
    let base = temp_base("ok");
    // 嵌套两级不存在的父目录 → init_db 应自动创建
    let db_path = base.join("nested").join("deeper").join("data.db");

    let conn = init_db(&db_path).expect("init_db should create parent dirs and schema");

    // v1 的三张表都已建好且为空
    for table in ["request_logs", "daily_rollups", "model_pricing"] {
        let n: i64 = conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table}"),
                rusqlite::params![],
                |r| r.get(0),
            )
            .unwrap_or_else(|e| panic!("table {table} should exist after init: {e}"));
        assert_eq!(n, 0, "table {table} should be empty right after init");
    }
    drop(conn);

    // 幂等：第二次 init（user_version 已 = SCHEMA_VERSION，跳过迁移）不报错，
    // 版本保持当前值（A3 起为 2）
    let conn2 = init_db(&db_path).expect("re-init on existing db should be idempotent");
    let ver: i32 = conn2
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(ver, SCHEMA_VERSION);

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn init_db_fails_when_parent_is_blocked_by_file() {
    // 覆盖行 61：db 路径的祖先 "blocker" 是一个已存在的【文件】，
    // create_dir_all 无法在其下建目录 → 错误必须以 "Failed to create data directory" 开头
    let base = temp_base("blocked");
    let blocker = base.join("blocker");
    std::fs::write(&blocker, b"i am a file, not a dir").unwrap();
    let db_path = blocker.join("sub").join("data.db");

    let err = init_db(&db_path).unwrap_err();
    assert!(
        err.starts_with("Failed to create data directory"),
        "unexpected error: {err}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
#[cfg(windows)]
fn init_db_root_drive_path_parent_none() {
    // 覆盖行 61：根路径 "c:\" 的 parent() == None → 跳过建目录分支（else 臂），
    // 随后 Connection::open 打不开根目录 → "Failed to open database"。
    let err = init_db(std::path::Path::new("c:\\")).unwrap_err();
    assert!(
        err.starts_with("Failed to open database"),
        "unexpected error: {err}"
    );
}
