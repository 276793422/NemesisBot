//! 备份 + 恢复演练测试（Swarm 出口判据「备份恢复演练通过」）。

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

// --- BUG-20261126-121 回归锁：删除路径瞬时句柄韧性 -------------------------
// Windows 形态（share_mode(0) 独占句柄模拟 AV/用户读句柄）；Unix 上打开中
// 的 fd 不阻止 unlink，该失败形态不存在。

/// 陈旧快照摘除（VACUUM INTO 前置必要条件）：句柄在退避窗内释放 → 重试
/// 恢复删除、备份照常成功（修复前裸单发立刻假失败「remove stale backup」）。
#[cfg(windows)]
#[test]
fn test_backup_stale_target_retries_through_transient_handle() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = unique_dir("stale-busy");
    let db_path = seed_db(&dir, "占用");
    let backups_dir = dir.join("backups");
    let first = backup_database(&db_path, &backups_dir, 3).unwrap().unwrap();

    // 独占句柄（无 FILE_SHARE_DELETE）持住今日已存在的快照，200ms 后释放
    //（重试窗 80ms×线性递增×10 ≈ 4.4s，4 倍余量）。
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&first)
        .unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        drop(holder);
    });

    let target = backup_database(&db_path, &backups_dir, 3)
        .expect("瞬时句柄释放后备份必须成功（裸单发修复前在此报 remove stale backup 错）")
        .expect("keep>0 应产生备份");
    assert_eq!(target, first, "同日重跑覆盖同一文件");
    assert!(releaser.join().is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

/// 保留裁剪容错臂：单个超龄旧备份被句柄持住 → 裁剪 resilient WARN 继续，
/// 已写成功的备份**不得谎报 Err**（修复前 `?` 硬上浮让 backup_database
/// 返回 prune 错，尽管 VACUUM INTO 已完成）。未被持住的旧备份照常裁掉。
#[cfg(windows)]
#[test]
fn test_backup_prune_failure_does_not_abort_backup() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = unique_dir("prune-busy");
    let db_path = seed_db(&dir, "裁剪占用");
    let backups_dir = dir.join("backups");
    std::fs::create_dir_all(&backups_dir).unwrap();
    // 两份超龄假历史备份（keep=1：今日新备份在场后都该被裁）。
    let stale1 = backups_dir.join("board-20200101.db");
    let stale2 = backups_dir.join("board-20200102.db");
    std::fs::copy(&db_path, &stale1).unwrap();
    std::fs::copy(&db_path, &stale2).unwrap();

    // 整个调用期间持住 stale1（独占无共享）→ 对该件的裁剪必然失败。
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&stale1)
        .unwrap();

    let target = backup_database(&db_path, &backups_dir, 1)
        .expect("裁剪失败不得让已成功的备份报错（裸 ? 修复前在此报 prune 错）")
        .expect("keep=1 应产生今日备份");
    assert!(target.exists(), "今日备份必须真实在场");
    drop(holder);
    assert!(!stale2.exists(), "未被占用的超龄备份应照常裁掉");
    assert!(stale1.exists(), "被持住的裁剪失败件残留（下轮再裁）");
    let _ = std::fs::remove_dir_all(&dir);
}
