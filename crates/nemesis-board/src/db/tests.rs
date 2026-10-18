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

/// v19（G4 交付证据契约，2026-10-18 批六）：旧库迁移后 `requires_evidence`
/// 列在场且存量行默认 0（新列 NOT NULL DEFAULT 0，旧行不误判为要求证据）。
#[test]
fn test_migration_v1_to_latest_adds_requires_evidence_default_zero() {
    let dir = unique_dir("migrate-v19-requires-evidence");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO board_meta(key, value) VALUES('number_prefix', 'NB')",
            [],
        )
        .unwrap();
        // v1 期已有数据：存量单（迁移后默认 0 断言对象）。
        conn.execute(
            "INSERT INTO issue (number, title, status, priority, creator_type, creator_id,
                position, created_at, updated_at)
             VALUES('NB-1', '存量单', 'backlog', 1, 'admin', 'admin', 1, 1700000000, 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    let flag: i64 = conn
        .query_row(
            "SELECT requires_evidence FROM issue WHERE number = 'NB-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(flag, 0, "存量行默认不要求证据（新列 DEFAULT 0）");
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
        for schema in [
            SCHEMA_V1, SCHEMA_V2, SCHEMA_V3, SCHEMA_V4, SCHEMA_V5, SCHEMA_V6,
        ] {
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

/// v11 库 → init_db 迁移 v12（project.directory 列补建；存量项目 directory
/// 保持 NULL = 不参与档案管线，goal §一 Out 不回填）。
#[test]
fn test_migration_v11_to_v12_adds_project_directory() {
    let dir = unique_dir("migrate-v11-v12");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        for v in 1..=11 {
            let schema = match v {
                1 => SCHEMA_V1,
                2 => SCHEMA_V2,
                3 => SCHEMA_V3,
                4 => SCHEMA_V4,
                5 => SCHEMA_V5,
                6 => SCHEMA_V6,
                7 => SCHEMA_V7,
                8 => SCHEMA_V8,
                9 => SCHEMA_V9,
                10 => SCHEMA_V10,
                11 => SCHEMA_V11,
                _ => unreachable!(),
            };
            conn.execute_batch(schema).unwrap();
            conn.pragma_update(None, "user_version", v).unwrap();
        }
        // v11 期已有项目（无 directory 列概念）。
        conn.execute(
            "INSERT INTO project (name, description, status, priority, icon, acceptance_criteria, created_at)
             VALUES('老项目', '', 'active', 1, '', '', 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    // 存量项目 directory = NULL（不参与档案管线）。
    let directory: Option<String> = conn
        .query_row(
            "SELECT directory FROM project WHERE name='老项目'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        directory, None,
        "存量项目 directory 必须保持 NULL（不回填）"
    );
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

/// v17（职能框架 M2）：v16 旧库迁移补 `issue.required_profession` 列；
/// 存量行 NULL（= 不限职能），新列可写可读（迁移 round-trip）。
#[test]
fn test_migration_v16_to_v17_adds_required_profession() {
    let dir = unique_dir("migrate-v16-v17");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        // 手工把库钉在 v16（跑全部中间迁移再回写版本，模拟真实存量库）。
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute_batch(SCHEMA_V4).unwrap();
        conn.execute_batch(SCHEMA_V5).unwrap();
        conn.execute_batch(SCHEMA_V6).unwrap();
        conn.execute_batch(SCHEMA_V7).unwrap();
        conn.execute_batch(SCHEMA_V8).unwrap();
        conn.execute_batch(SCHEMA_V9).unwrap();
        conn.execute_batch(SCHEMA_V10).unwrap();
        conn.execute_batch(SCHEMA_V11).unwrap();
        conn.execute_batch(SCHEMA_V12).unwrap();
        conn.execute_batch(SCHEMA_V13).unwrap();
        conn.execute_batch(SCHEMA_V14).unwrap();
        conn.execute_batch(SCHEMA_V15).unwrap();
        conn.execute_batch(SCHEMA_V16).unwrap();
        conn.pragma_update(None, "user_version", 16).unwrap();
        conn.execute(
            "INSERT INTO board_meta(key, value) VALUES('number_prefix', 'NB')",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION, "迁移到 v17");
    // 存量行：新列读出 NULL。
    conn.execute(
        "INSERT INTO issue (number, title, creator_type, creator_id, position, created_at, updated_at)
         VALUES ('NB-1', 'legacy', 'admin', 'admin', 1, 1, 1)",
        [],
    )
    .unwrap();
    let legacy: Option<String> = conn
        .query_row(
            "SELECT required_profession FROM issue WHERE number='NB-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy, None, "存量行新列 = NULL（不限职能）");
    // 新列可写可读。
    conn.execute(
        "UPDATE issue SET required_profession='dev:cpp' WHERE number='NB-1'",
        [],
    )
    .unwrap();
    let set: String = conn
        .query_row(
            "SELECT required_profession FROM issue WHERE number='NB-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(set, "dev:cpp");
    let _ = std::fs::remove_dir_all(&dir);
}

/// v18（NB-122 批 2026-10-16）：v17 旧库迁移补 `issue.required_capabilities`
/// 列；存量行 NULL（= 无能力需求），新列可写可读（迁移 round-trip）。
#[test]
fn test_migration_v17_to_v18_adds_required_capabilities() {
    let dir = unique_dir("migrate-v17-v18");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        // 手工把库钉在 v17（跑全部中间迁移再回写版本，模拟真实存量库）。
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(SCHEMA_V2).unwrap();
        conn.execute_batch(SCHEMA_V3).unwrap();
        conn.execute_batch(SCHEMA_V4).unwrap();
        conn.execute_batch(SCHEMA_V5).unwrap();
        conn.execute_batch(SCHEMA_V6).unwrap();
        conn.execute_batch(SCHEMA_V7).unwrap();
        conn.execute_batch(SCHEMA_V8).unwrap();
        conn.execute_batch(SCHEMA_V9).unwrap();
        conn.execute_batch(SCHEMA_V10).unwrap();
        conn.execute_batch(SCHEMA_V11).unwrap();
        conn.execute_batch(SCHEMA_V12).unwrap();
        conn.execute_batch(SCHEMA_V13).unwrap();
        conn.execute_batch(SCHEMA_V14).unwrap();
        conn.execute_batch(SCHEMA_V15).unwrap();
        conn.execute_batch(SCHEMA_V16).unwrap();
        conn.execute_batch(SCHEMA_V17).unwrap();
        conn.pragma_update(None, "user_version", 17).unwrap();
        conn.execute(
            "INSERT INTO board_meta(key, value) VALUES('number_prefix', 'NB')",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION, "迁移到 v18");
    // 存量行：新列读出 NULL。
    conn.execute(
        "INSERT INTO issue (number, title, creator_type, creator_id, position, created_at, updated_at)
         VALUES ('NB-1', 'legacy', 'admin', 'admin', 1, 1, 1)",
        [],
    )
    .unwrap();
    let legacy: Option<String> = conn
        .query_row(
            "SELECT required_capabilities FROM issue WHERE number='NB-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy, None, "存量行新列 = NULL（无能力需求）");
    // 新列可写可读（JSON 数组形态，store 层契约）。
    conn.execute(
        "UPDATE issue SET required_capabilities='[\"chrome\",\"docker\"]' WHERE number='NB-1'",
        [],
    )
    .unwrap();
    let set: String = conn
        .query_row(
            "SELECT required_capabilities FROM issue WHERE number='NB-1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(set, r#"["chrome","docker"]"#);
    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------- B10（2026-10-18 存储韧性）

#[test]
fn b10_init_db_rejects_foreign_file_loudly() {
    // 手法①：非 SQLite 内容 → loud Err 带 hex 摘要，拒绝发生在打开前
    // （原文件内容不被触碰）。
    let dir = unique_dir("b10-foreign");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, b"this is not a sqlite db!").unwrap();

    let err = init_db(&path).unwrap_err();
    assert!(
        err.contains("不是 SQLite 格式") && err.contains("hex:"),
        "Foreign 文件应带 hex 摘要 loud 拒绝: {err}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"this is not a sqlite db!",
        "拒绝路径不改写原文件"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn b10_init_db_rejects_newer_schema_version() {
    // 手法②：user_version 高于本程序支持值 → 拒绝降级打开（旧行为会把
    // 版本静默降级写回、迁移链跳过、运行期 "no such column"）。
    let dir = unique_dir("b10-drift");
    let path = dir.join("board.db");
    {
        let conn = init_db(&path).unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 80)
            .unwrap();
    }
    let err = init_db(&path).unwrap_err();
    assert!(
        err.contains(&format!("高于本程序支持的 {SCHEMA_VERSION}")),
        "高版本库应 loud 拒绝: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn b10_migration_from_v1_keeps_backup() {
    // 手法③：模拟旧版库（fresh v1 + 存量数据 + user_version=1）→ init_db
    // 走全迁移链升到当前版，迁移前 .bak.1 在场且持旧版快照（存量单可查）。
    let dir = unique_dir("b10-migrate-bak");
    let path = dir.join("board.db");
    std::fs::create_dir_all(&dir).unwrap();
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO issue (number, title, status, priority, creator_type, creator_id,
                position, created_at, updated_at)
             VALUES('NB-1', '存量单', 'backlog', 1, 'admin', 'admin', 1, 1700000000, 1700000000)",
            [],
        )
        .unwrap();
    }
    let conn = init_db(&path).unwrap();
    let v: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(v, SCHEMA_VERSION, "迁移链走满");

    let bak1 = dir.join("board.db.bak.1");
    assert!(bak1.exists(), "迁移前备份在场");
    let bak_conn = Connection::open(&bak1).unwrap();
    let bak_v: i32 = bak_conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(bak_v, 1, "备份持迁移前版本");
    let bak_title: String = bak_conn
        .query_row("SELECT title FROM issue WHERE number='NB-1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(bak_title, "存量单", "备份含迁移前存量数据");
    let _ = std::fs::remove_dir_all(&dir);
}
