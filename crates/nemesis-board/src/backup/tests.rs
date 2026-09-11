//! 备份 + 恢复演练测试（Swarm M2 出口判据「备份恢复演练通过」）。

use super::*;
use std::path::PathBuf;

static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn unique_dir(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-backuptest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 造一个带数据的 board.db（一条 issue，内容可回读断言）。
fn seed_db(dir: &Path, number: &str) -> PathBuf {
    let db_path = dir.join("board").join("board.db");
    let store = crate::BoardStore::open(&db_path, "NB").expect("open store");
    store
        .create_issue(crate::NewIssue {
            title: format!("备份演练 {number}"),
            ..Default::default()
        })
        .unwrap();
    drop(store);
    db_path
}

fn count_issues(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("SELECT COUNT(*) FROM issue", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn test_backup_creates_snapshot_and_recovers() {
    let dir = unique_dir("recover");
    let db_path = seed_db(&dir, "一号");
    let backups_dir = dir.join("backups");

    let target = backup_database(&db_path, &backups_dir, 3)
        .unwrap()
        .expect("keep>0 应产生备份文件");
    assert!(target.exists());
    assert!(
        target
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("board-")
    );

    // 恢复演练：备份是完整可打开的库，数据在场。
    assert_eq!(count_issues(&target), 1);

    // 备份后主库继续写入 → 备份不随后变（快照语义）。
    {
        let store = crate::BoardStore::open(&db_path, "NB").unwrap();
        store
            .create_issue(crate::NewIssue {
                title: "备份之后的新单".to_string(),
                ..Default::default()
            })
            .unwrap();
    }
    assert_eq!(count_issues(&target), 1, "备份快照不应随后续写入变化");
    assert_eq!(count_issues(&db_path), 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backup_same_day_rerun_is_idempotent() {
    let dir = unique_dir("same-day");
    let db_path = seed_db(&dir, "同日");
    let backups_dir = dir.join("backups");

    let first = backup_database(&db_path, &backups_dir, 3).unwrap().unwrap();
    let second = backup_database(&db_path, &backups_dir, 3).unwrap().unwrap();
    assert_eq!(first, second, "同日重跑覆盖同一文件");
    assert_eq!(count_issues(&second), 1);
    assert_eq!(
        std::fs::read_dir(&backups_dir).unwrap().count(),
        1,
        "不应堆出多份同日备份"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backup_prunes_beyond_keep() {
    let dir = unique_dir("prune");
    let db_path = seed_db(&dir, "裁剪");
    let backups_dir = dir.join("backups");
    std::fs::create_dir_all(&backups_dir).unwrap();
    // 手工堆 4 份「历史」备份（不同日期名，字典序=时间序）。
    for day in ["20260101", "20260102", "20260103", "20260104"] {
        std::fs::copy(&db_path, backups_dir.join(format!("board-{day}.db"))).unwrap();
    }

    backup_database(&db_path, &backups_dir, 2).unwrap().unwrap();

    let mut names: Vec<String> = std::fs::read_dir(&backups_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names.len(), 2, "keep=2 应裁剪到只剩 2 份，实际: {names:?}");
    // 剩的是最新的「今日」备份（升序在末尾）+ 次新的 20260104。
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    assert_eq!(names[0], "board-20260104.db");
    assert_eq!(names[1], format!("{BACKUP_PREFIX}{today}.db"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_backup_disabled_when_keep_zero_and_missing_db_errors() {
    let dir = unique_dir("disabled");
    let db_path = seed_db(&dir, "关闭");
    let backups_dir = dir.join("backups");

    // keep=0 = 备份关闭，不做任何 IO。
    assert!(
        backup_database(&db_path, &backups_dir, 0)
            .unwrap()
            .is_none()
    );
    assert!(!backups_dir.exists(), "关闭时不应创建备份目录");

    // 主库缺失 = 诚实报错（不静默产出空备份）。
    let err = backup_database(&dir.join("nope").join("board.db"), &backups_dir, 3).unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
    let _ = std::fs::remove_dir_all(&dir);
}
