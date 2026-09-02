//! schema 初始化 / 迁移测试。

use super::*;

#[test]
fn test_init_db_creates_file_and_tables() {
    let dir = unique_dir("init-tables");
    let path = dir.join("board.db");
    let conn = init_db(&path).expect("init_db should succeed");
    assert!(path.exists());

    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for expected in [
        "board_meta",
        "issue",
        "comment",
        "activity_log",
        "issue_subscriber",
        "project",
        "attachment",
        "notification",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "missing table {expected}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_init_db_is_idempotent() {
    let dir = unique_dir("idempotent");
    let path = dir.join("board.db");
    let _ = init_db(&path).unwrap();
    // 二次 open 不应报错也不应清数据。
    let conn = init_db(&path).unwrap();
    conn.execute("INSERT INTO board_meta(key, value) VALUES('k', 'v')", [])
        .unwrap();
    drop(conn);
    let conn2 = init_db(&path).unwrap();
    let v: String = conn2
        .query_row("SELECT value FROM board_meta WHERE key='k'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(v, "v");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_init_db_creates_parent_dirs_and_sets_version() {
    let dir = unique_dir("nested/deep/path");
    let path = dir.join("nested2/board.db");
    let conn = init_db(&path).unwrap();
    assert!(path.exists());
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    let _ = std::fs::remove_dir_all(&dir);
}

/// v1 旧库 → init_db 自动迁移到最新版（issue_dispatch + notification 表补建，
/// 旧数据保留）。
#[test]
fn test_migration_v1_to_latest_adds_dispatch_and_notification() {
    let dir = unique_dir("migrate-v1-latest");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    // 手工造一个 v1 库（只跑 SCHEMA_V1，版本钉在 1）。
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO board_meta(key, value) VALUES('number_prefix', 'NB')",
            [],
        )
        .unwrap();
    }
    // init_db：一路迁移到最新。
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue_dispatch", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM notification", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    // v1 期数据保留。
    let prefix: String = conn
        .query_row(
            "SELECT value FROM board_meta WHERE key='number_prefix'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(prefix, "NB");
    let _ = std::fs::remove_dir_all(&dir);
}

/// v2 库 → init_db 迁移 v3（notification 表补建；v2 期数据保留）。
#[test]
fn test_migration_v2_to_v3_adds_notification_table() {
    let dir = unique_dir("migrate-v2-v3");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        // v2 期已有数据：一条 issue + 一条派发记录（满足 FK）。
        conn.execute(
            "INSERT INTO issue (number, title, status, priority, creator_type, creator_id,
                position, created_at, updated_at)
             VALUES('NB-1', '种子', 'backlog', 1, 'admin', 'admin', 1, 1700000000, 1700000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO issue_dispatch(task_id, issue_id, worker_id, state, dispatched_at)
             VALUES('t-1', 1, 'node-b', 'dispatched', 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    // notification 表存在且为空；v2 期派发数据保留。
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM notification", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue_dispatch", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// v3 库 → init_db 迁移 v4（autopilot 表补建；v3 期数据保留）。
#[test]
fn test_migration_v3_to_v4_adds_autopilot_table() {
    let dir = unique_dir("migrate-v3-v4");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        // v3 期已有数据：一条通知（保留断言用）。
        conn.execute(
            "INSERT INTO notification(recipient_type, recipient_id, kind, title, content, read, created_at)
             VALUES('admin', 'admin', 'commented', 't', 'c', 0, 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    // autopilot 表存在且为空；v3 期通知数据保留。
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM autopilot", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM notification", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

fn unique_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-dbtest-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
