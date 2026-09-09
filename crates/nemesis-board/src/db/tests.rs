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
        "channel",
        "channel_member",
        "channel_message",
        "asset",
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

/// v5 库 → init_db 迁移 v6（channel 三表 + asset 补建；v5 期数据保留）。
#[test]
fn test_migration_v5_to_v6_adds_channel_and_asset_tables() {
    let dir = unique_dir("migrate-v5-v6");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute_batch(SCHEMA_V4).unwrap();
        conn.execute_batch(SCHEMA_V5).unwrap();
        conn.pragma_update(None, "user_version", 5).unwrap();
        // v5 期已有数据：一条 issue（保留断言用）。
        conn.execute(
            "INSERT INTO issue (number, title, status, priority, creator_type, creator_id,
                position, created_at, updated_at)
             VALUES('NB-1', '种子', 'backlog', 1, 'admin', 'admin', 1, 1700000000, 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    for table in ["channel", "channel_member", "channel_message", "asset"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "table {table} should exist and be empty");
    }
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM issue", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "v5 issue data must survive migration");
    let _ = std::fs::remove_dir_all(&dir);
}

/// v6 库 → init_db 迁移 v7（msg_dedup + seq_ledger 补建；v6 期数据保留）。
#[test]
fn test_migration_v6_to_v7_adds_dedup_and_ledger() {
    let dir = unique_dir("migrate-v6-v7");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        for schema in [SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6] {
            conn.execute_batch(schema).unwrap();
        }
        conn.pragma_update(None, "user_version", 6).unwrap();
        // v6 期已有数据：一条频道消息（保留断言用）。
        conn.execute(
            "INSERT INTO channel (name, topic, created_at) VALUES('#dev', '', 1700000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO channel_message (channel_id, sender_type, sender_id, content,
                mtype, created_at)
             VALUES(1, 'agent', 'node-b', '旧消息', 'text', 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    for table in ["msg_dedup", "seq_ledger"] {
        let n: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "table {table} should exist and be empty");
    }
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM channel_message", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "v6 message data must survive migration");
    // seq_ledger 发号可用（AUTOINCREMENT 从 1 开始）。
    conn.execute(
        "INSERT INTO seq_ledger (thread_kind, thread_id, message_id, sender_id, created_at)
         VALUES('channel', 1, 1, 'node-b', 1700000001)",
        [],
    )
    .unwrap();
    let seq: i64 = conn
        .query_row("SELECT seq FROM seq_ledger", [], |r| r.get(0))
        .unwrap();
    assert_eq!(seq, 1);
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
