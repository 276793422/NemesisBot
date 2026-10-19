//! sqlite_preflight 单元测试。
//!
//! 覆盖点：
//! - ① 能力准入：不存在/空文件 = New；魔数命中 = Valid；垃圾/截断 = Foreign
//!   （head_hex 摘要在场）；
//! - ② drift 对账：高版本 loud Err、等值/低版本放行；
//! - ③ 备份轮换：真库两代轮换（bak.1/bak.2 各持旧代快照，不双丢）；
//! - ④ WAL 降级：只读连接确定性失败 → warn 不 panic；可写连接 → wal 生效。

use super::*;
use std::path::{Path, PathBuf};

/// 在已持有的 tempdir 内给 db 文件起名（guard 由测试持有，结束时统一清）。
fn temp_db(dir: &Path, tag: &str) -> PathBuf {
    dir.join(format!("{tag}.db"))
}

// ---------------------------------------------------------------- ① 准入

#[test]
fn header_kind_missing_and_empty_are_new() {
    let dir = tempfile::tempdir().unwrap();
    let p = temp_db(dir.path(), "hk_missing");
    assert_eq!(header_kind(&p).unwrap(), HeaderKind::New);
    std::fs::write(&p, b"").unwrap();
    assert_eq!(header_kind(&p).unwrap(), HeaderKind::New);
}

#[test]
fn header_kind_valid_magic() {
    let dir = tempfile::tempdir().unwrap();
    let p = temp_db(dir.path(), "hk_valid");
    let mut blob = SQLITE_HEADER.to_vec();
    blob.extend_from_slice(b"trailing junk is fine");
    std::fs::write(&p, blob).unwrap();
    assert_eq!(header_kind(&p).unwrap(), HeaderKind::Valid);
}

#[test]
fn header_kind_foreign_full_and_truncated() {
    let dir = tempfile::tempdir().unwrap();
    let p = temp_db(dir.path(), "hk_foreign_full");
    std::fs::write(&p, b"this is not a sqlite db!").unwrap();
    match header_kind(&p).unwrap() {
        HeaderKind::Foreign { head_hex } => {
            // "this is not a sq" 的 hex（16 字节 → 32 hex 字符）。
            let expect: String = b"this is not a sq"
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert_eq!(head_hex, expect);
        }
        other => panic!("期望 Foreign，实际 {other:?}"),
    }

    // 截断（<16 字节）：读多少记多少，仍判 Foreign。
    let p2 = temp_db(dir.path(), "hk_foreign_short");
    std::fs::write(&p2, b"SQLi").unwrap();
    match header_kind(&p2).unwrap() {
        HeaderKind::Foreign { head_hex } => assert_eq!(head_hex, "53514c69"),
        other => panic!("期望 Foreign（截断），实际 {other:?}"),
    }
}

// ---------------------------------------------------------------- ② drift

#[test]
fn drift_higher_is_loud_error() {
    let err = check_schema_drift(99, 19).unwrap_err();
    assert!(err.contains("99"), "报错带实际版本: {err}");
    assert!(err.contains("19"), "报错带支持版本: {err}");
}

#[test]
fn drift_equal_and_lower_pass() {
    assert!(check_schema_drift(19, 19).is_ok());
    assert!(check_schema_drift(1, 19).is_ok());
    assert!(check_schema_drift(0, 19).is_ok(), "0 = 全新库放行");
}

// ---------------------------------------------------------------- ③ 备份轮换

#[test]
fn backup_rotation_keeps_two_generations() {
    let dir = tempfile::tempdir().unwrap();
    let p = temp_db(dir.path(), "bak_rotate");
    let conn = rusqlite::Connection::open(&p).unwrap();
    conn.execute_batch("CREATE TABLE t(x TEXT); INSERT INTO t VALUES ('one');")
        .unwrap();

    backup_before_migrate(&conn, &p, "test-rotate");
    let bak1 = backup_path(&p, 1);
    assert!(bak1.exists(), "第一代备份在场");
    let snap1: String = rusqlite::Connection::open(&bak1)
        .unwrap()
        .query_row("SELECT x FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(snap1, "one", "bak.1 持第一代快照");

    // 推进主库再备份：bak.1 = 新快照（2 行），bak.2 = 旧快照（1 行）。
    conn.execute_batch("INSERT INTO t VALUES ('two');").unwrap();
    backup_before_migrate(&conn, &p, "test-rotate");
    let bak2 = backup_path(&p, 2);
    assert!(bak2.exists(), "第二代备份在场（轮换不双丢）");
    let snap1b: i64 = rusqlite::Connection::open(&bak1)
        .unwrap()
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    let snap2b: i64 = rusqlite::Connection::open(&bak2)
        .unwrap()
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .unwrap();
    assert_eq!(snap1b, 2, "bak.1 持最新快照");
    assert_eq!(snap2b, 1, "bak.2 持上一代快照");
}

// ---------------------------------------------------------------- ④ WAL

#[test]
fn wal_lenient_rw_conn_enables_wal() {
    let dir = tempfile::tempdir().unwrap();
    let p = temp_db(dir.path(), "wal_rw");
    let conn = rusqlite::Connection::open(&p).unwrap();
    set_wal_lenient(&conn, "test-wal");
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal", "可写连接 WAL 生效");
}

#[test]
fn wal_lenient_read_only_conn_no_panic() {
    // 只读连接上写 journal_mode 确定性失败 → warn 不崩、模式非 wal。
    let dir = tempfile::tempdir().unwrap();
    let p = temp_db(dir.path(), "wal_ro");
    {
        let conn = rusqlite::Connection::open(&p).unwrap();
        conn.execute_batch("CREATE TABLE t(x);").unwrap();
    }
    let ro = rusqlite::Connection::open_with_flags(&p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    set_wal_lenient(&ro, "test-wal-readonly");
    let mode: String = ro
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_ne!(mode, "wal", "只读连接不应进入 WAL");
}
