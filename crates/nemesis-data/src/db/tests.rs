//! db::init_db 的单元测试。
//!
//! 覆盖点：
//! - 正常路径：自动创建多级父目录、建 v1 全部表、写 user_version=1、二次 init 幂等；
//! - 行 61 错误分支：db 父路径的祖先是已存在的**文件** → create_dir_all 失败 →
//!   返回 "Failed to create data directory" 错误（未打开数据库即返回）。

use super::*;

/// 建一个干净的临时基准目录（按进程 + tag 隔离，进入前先清残留）。
fn temp_base(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("nemesis_data_db_test_{}_{tag}", std::process::id()));
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

// -------------------------------------------------- B10（2026-10-18 存储韧性）

#[test]
fn init_db_rejects_foreign_file_loudly() {
    // 手法①：非 SQLite 内容（被覆盖/截断）→ loud Err 带 hex 摘要，
    // 不再是"打开成功、第一条语句才炸"的模糊失败面。
    let base = temp_base("b10_foreign");
    let db_path = base.join("usage.db");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(&db_path, b"this is not a sqlite db!").unwrap();

    let err = init_db(&db_path).unwrap_err();
    assert!(
        err.contains("不是 SQLite 格式") && err.contains("hex:"),
        "Foreign 文件应带 hex 摘要 loud 拒绝: {err}"
    );
    // 拒绝发生在打开前——文件内容未被触碰。
    assert_eq!(
        std::fs::read(&db_path).unwrap(),
        b"this is not a sqlite db!",
        "拒绝路径不改写原文件"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn init_db_rejects_newer_schema_version() {
    // 手法②：user_version 高于本程序支持值（来自更新版本的程序）→
    // 拒绝降级打开；旧行为会把版本静默降级写回、迁移链跳过。
    let base = temp_base("b10_drift");
    let db_path = base.join("usage.db");
    {
        let conn = init_db(&db_path).unwrap();
        assert_eq!(SCHEMA_VERSION, 3);
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let err = init_db(&db_path).unwrap_err();
    assert!(
        err.contains("高于本程序支持的 3"),
        "高版本库应 loud 拒绝: {err}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn init_db_migration_from_v1_keeps_backup() {
    // 手法③：模拟旧版库（fresh v1 + user_version=1）→ init_db 走全迁移
    // 链升到当前版，且迁移前 .bak.1 在场（头合法 + 版本 = 旧版 1）。
    let base = temp_base("b10_migrate_bak");
    let db_path = base.join("usage.db");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
    }
    let conn = init_db(&db_path).unwrap();
    let ver: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(ver, SCHEMA_VERSION, "迁移链走满");

    let bak1 = base.join("usage.db.bak.1");
    assert!(bak1.exists(), "迁移前备份在场");
    let bak_ver: i32 = rusqlite::Connection::open(&bak1)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(bak_ver, 1, "备份持迁移前快照");

    let _ = std::fs::remove_dir_all(&base);
}
