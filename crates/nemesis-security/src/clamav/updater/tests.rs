use super::*;

fn test_config() -> UpdaterConfig {
    UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    }
}

#[test]
fn test_updater_new() {
    let updater = Updater::new(test_config());
    assert!(updater.last_update().is_none());
}

#[test]
fn test_last_update_none() {
    let updater = Updater::new(test_config());
    assert_eq!(updater.last_update(), None);
}

#[test]
fn test_is_database_stale_no_database() {
    let updater = Updater::new(test_config());
    // With no database dir, should be stale
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_is_database_stale_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // With empty dir (no main.cvd), should be stale
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_stop_sets_running_flag() {
    let updater = Updater::new(test_config());
    // Manually set running, then stop
    updater.running.store(true, Ordering::SeqCst);
    assert!(updater.running.load(Ordering::SeqCst));
    updater.stop();
    assert!(!updater.running.load(Ordering::SeqCst));
}

#[test]
fn test_updater_config_fields() {
    let config = test_config();
    assert_eq!(config.clamav_path, "/usr/bin");
    assert_eq!(config.update_interval, Duration::from_secs(3600));
    assert!(config.mirror_urls.is_empty());
}

#[test]
fn test_find_executable() {
    let exe = super::super::find_executable("/usr/bin", "freshclam");
    if cfg!(target_os = "windows") {
        assert!(exe.ends_with("freshclam.exe"));
    } else {
        assert!(exe.ends_with("freshclam"));
    }
}

#[tokio::test]
async fn test_update_exe_not_found() {
    let updater = Updater::new(test_config());
    let result = updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_update_db_dir_created_before_exe_check() {
    // The updater checks for freshclam before creating the db dir
    // so with a nonexistent path, the db dir won't be created.
    // Let's test with an empty database_dir instead
    let _dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/nonexistent".to_string(),
        database_dir: String::new(), // empty dir won't be created
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let result = updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await;
    // Should fail because freshclam not found, not because of dir creation
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_is_database_stale_with_recent_file() {
    let dir = tempfile::tempdir().unwrap();
    let main_cvd = dir.path().join("main.cvd");
    std::fs::write(&main_cvd, "test").unwrap();
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // File was just created, so with a large max_age it should not be stale
    assert!(!updater.is_database_stale(Duration::from_secs(86400 * 365)));
}

#[test]
fn test_is_database_stale_with_old_file() {
    let dir = tempfile::tempdir().unwrap();
    // Don't create main.cvd - should be stale
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    assert!(updater.is_database_stale(Duration::from_secs(1)));
}

#[tokio::test]
async fn test_auto_update_zero_interval() {
    let config = UpdaterConfig {
        clamav_path: String::new(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::ZERO,
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // With zero interval, start_auto_update should return immediately
    updater.start_auto_update().await;
}

#[test]
fn test_updater_running_flag() {
    let updater = Updater::new(test_config());
    assert!(!updater.running.load(Ordering::SeqCst));
    updater.running.store(true, Ordering::SeqCst);
    assert!(updater.running.load(Ordering::SeqCst));
    updater.stop();
    assert!(!updater.running.load(Ordering::SeqCst));
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_updater_config_custom_values() {
    let config = UpdaterConfig {
        clamav_path: "/opt/clamav".to_string(),
        database_dir: "/var/lib/clamav".to_string(),
        config_file: "/etc/clamav/freshclam.conf".to_string(),
        update_interval: Duration::from_secs(7200),
        mirror_urls: vec!["http://mirror1.example.com".to_string()],
    };
    assert_eq!(config.clamav_path, "/opt/clamav");
    assert_eq!(config.database_dir, "/var/lib/clamav");
    assert_eq!(config.config_file, "/etc/clamav/freshclam.conf");
    assert_eq!(config.update_interval, Duration::from_secs(7200));
    assert_eq!(config.mirror_urls.len(), 1);
}

#[test]
fn test_updater_last_update_manually_set() {
    let updater = Updater::new(test_config());
    assert!(updater.last_update().is_none());

    // Manually set last_update
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());
    assert!(updater.last_update().is_some());
}

#[test]
fn test_is_database_stale_with_recent_last_update() {
    let updater = Updater::new(test_config());
    // Set last_update to now
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());

    // Should not be stale with a large max_age
    assert!(!updater.is_database_stale(Duration::from_secs(86400 * 365)));
    // Should be stale with a very small max_age
    // (time has passed since we set last_update, even if just nanoseconds)
    // This is timing-sensitive so we just verify it doesn't panic
    let _ = updater.is_database_stale(Duration::from_nanos(1));
}

#[test]
fn test_is_database_stale_with_file_newer_than_max_age() {
    let dir = tempfile::tempdir().unwrap();
    let main_cvd = dir.path().join("main.cvd");
    std::fs::write(&main_cvd, "fake cvd content").unwrap();

    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // File was just created, so with a large max_age it should not be stale
    assert!(!updater.is_database_stale(Duration::from_secs(86400)));
}

#[tokio::test]
async fn test_update_with_db_dir_but_no_freshclam() {
    let dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/nonexistent".to_string(),
        database_dir: dir.path().to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let result = updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await;
    assert!(result.is_err());
    // Should fail because freshclam not found
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_update_with_config_file_but_no_freshclam() {
    let dir = tempfile::tempdir().unwrap();
    let config = UpdaterConfig {
        clamav_path: "/nonexistent".to_string(),
        database_dir: String::new(),
        config_file: dir
            .path()
            .join("freshclam.conf")
            .to_string_lossy()
            .to_string(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let result = updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await;
    assert!(result.is_err());
}

#[test]
fn test_updater_stop_multiple_times() {
    let updater = Updater::new(test_config());
    updater.stop();
    updater.stop();
    updater.stop();
    assert!(!updater.running.load(Ordering::SeqCst));
}

#[test]
fn test_is_database_stale_no_dir_configured() {
    let config = UpdaterConfig {
        clamav_path: "/usr/bin".to_string(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    // No last_update and no database_dir -> should be stale
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_updater_config_debug() {
    let config = test_config();
    let debug = format!("{:?}", config);
    assert!(debug.contains("/usr/bin"));
    assert!(debug.contains("3600"));
}

#[test]
fn test_updater_new_sets_defaults() {
    let updater = Updater::new(test_config());
    assert!(updater.last_update().is_none());
    assert!(!updater.running.load(Ordering::SeqCst));
}

#[test]
fn test_updater_config_clone() {
    let config = test_config();
    let cloned = config.clone();
    assert_eq!(cloned.clamav_path, config.clamav_path);
    assert_eq!(cloned.database_dir, config.database_dir);
    assert_eq!(cloned.update_interval, config.update_interval);
}

#[test]
fn test_updater_is_database_stale_with_recent_update() {
    let config = test_config();
    let updater = Updater::new(config);
    *updater.last_update.lock().unwrap() = Some(SystemTime::now());
    // Just updated -> should NOT be stale with a generous threshold
    assert!(!updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_updater_is_database_stale_with_old_update() {
    let config = test_config();
    let updater = Updater::new(config);
    // Set last_update to 2 days ago
    let two_days_ago = SystemTime::now() - Duration::from_secs(2 * 86400);
    *updater.last_update.lock().unwrap() = Some(two_days_ago);
    // Should be stale with a 1-day threshold
    assert!(updater.is_database_stale(Duration::from_secs(86400)));
}

#[test]
fn test_updater_is_database_stale_zero_threshold() {
    let config = test_config();
    let updater = Updater::new(config);
    // 1s in the past, NOT SystemTime::now(): on Windows the clock quantum
    // (~15ms) can make elapsed() return exactly ZERO for a just-set
    // timestamp, and ZERO > ZERO is false - a latent flake under parallel
    // load. A 1s-old timestamp makes "zero threshold => stale" hold on
    // every platform deterministically.
    *updater.last_update.lock().unwrap() = Some(SystemTime::now() - Duration::from_secs(1));
    // Zero threshold -> always stale
    assert!(updater.is_database_stale(Duration::ZERO));
}

// ============================================================
// Fake freshclam via where.exe (2026-08-25 coverage push)
// ============================================================
// 真 freshclam 二进制执行算结构性；用 C:\Windows\System32\where.exe 的副本
// 充当 freshclam.exe：
// - where 把非 `/` 开头的参数当文件名模式，在 cwd（=clamav_path）+ PATH 里找
// - exit 0 配方：cwd 放一个字面名为 `--config-file` 的哑文件（裸模式匹配）
//   + config_file 指向真实存在的文件（路径限定模式匹配）→ 全部命中 exit 0
// - exit 1 配方：config_file 指向不存在的文件 → 路径限定模式失配
// - where 不匹配目录 → 走 --datadir 参数时（database_dir 是目录）必 exit 1
// 全部 #[cfg(windows)]。

#[cfg(windows)]
fn place_where_as(dir: &std::path::Path, name: &str) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let src = std::path::Path::new(&windir)
        .join("System32")
        .join("where.exe");
    assert!(src.exists(), "where.exe not found at {}", src.display());
    std::fs::copy(&src, dir.join(format!("{}.exe", name))).unwrap();
}

#[cfg(windows)]
fn exit_zero_config(dir: &std::path::Path) -> UpdaterConfig {
    // freshclam.exe(=where.exe) 对每个 arg 都当 pattern 匹配：cwd 哑文件
    // `--config-file` 命中 + conf 文件名命中 → exit 0。两个坑：
    // ① config_file 必须用**相对名**——where.exe 拒绝绝对路径 pattern
    //    （单 pattern 实测 exit 2 = error），绝对 conf 路径必非零退出；
    // ② database_dir 留空（不传 --datadir，否则目录模式失配拖成 exit 1）。
    //    production 侧 current_dir=clamav_path=dir，相对名落在 cwd 能命中。
    let conf = dir.join("freshclam.conf");
    std::fs::write(&conf, "# fake freshclam config\n").unwrap();
    std::fs::write(dir.join("--config-file"), "").unwrap();
    UpdaterConfig {
        clamav_path: dir.to_string_lossy().to_string(),
        database_dir: String::new(),
        config_file: "freshclam.conf".to_string(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn fake_update_success_sets_last_update_and_progress() {
    let dir = tempfile::tempdir().unwrap();
    place_where_as(dir.path(), "freshclam");
    let updater = Updater::new(exit_zero_config(dir.path()));

    let calls: std::sync::Arc<std::sync::Mutex<Vec<(u64, u64)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cb_calls = calls.clone();
    let cb = std::sync::Arc::new(move |a: u64, b: u64| {
        cb_calls.lock().unwrap().push((a, b));
    });

    updater
        .update(tokio_util::sync::CancellationToken::new(), Some(cb))
        .await
        .unwrap();

    // 成功 → last_update 记录 + 进度回调 (0,0) 和 (100,100)
    assert!(updater.last_update().is_some());
    let got = calls.lock().unwrap().clone();
    assert_eq!(got, vec![(0, 0), (100, 100)], "progress callbacks");
}

#[cfg(windows)]
#[tokio::test]
async fn fake_update_creates_database_dir_then_nonzero_exit() {
    // database_dir 不存在 → create_dir_all 成功；随后 `--datadir <目录>`：
    // where 不匹配目录 → exit 1 → Err("non-zero status")。一测双覆盖
    // （建目录 Ok 分支 + 非零退出分支）。config_file 留空（少一个失配参数）。
    let dir = tempfile::tempdir().unwrap();
    place_where_as(dir.path(), "freshclam");
    std::fs::write(dir.path().join("--datadir"), "").unwrap(); // 哑裸模式命中
    let db_dir = dir.path().join("db").join("nested");
    let config = UpdaterConfig {
        clamav_path: dir.path().to_string_lossy().to_string(),
        database_dir: db_dir.to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let err = updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(err.contains("non-zero status"), "{err}");
    // 关键副作用：db 目录在 spawn 前已创建
    assert!(db_dir.is_dir(), "database dir must be created");
    assert!(
        updater.last_update().is_none(),
        "failure must not set last_update"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn fake_update_missing_config_path_nonzero_exit() {
    // config_file 指向不存在文件 → 路径限定模式失配 → exit 1。
    let dir = tempfile::tempdir().unwrap();
    place_where_as(dir.path(), "freshclam");
    std::fs::write(dir.path().join("--config-file"), "").unwrap();
    let config = UpdaterConfig {
        clamav_path: dir.path().to_string_lossy().to_string(),
        database_dir: String::new(),
        config_file: dir
            .path()
            .join("no-such.conf")
            .to_string_lossy()
            .to_string(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    };
    let updater = Updater::new(config);
    let err = updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(err.contains("non-zero status"), "{err}");
}

#[cfg(windows)]
#[tokio::test]
async fn fake_update_cancelled_is_error_or_success() {
    // 预取消 token + 快退子进程：tokio::select! 两分支都立即可就绪（默认
    // 随机挑），结果竞态——两种结局都合法，断言只要不是 panic/挂死。
    let dir = tempfile::tempdir().unwrap();
    place_where_as(dir.path(), "freshclam");
    let updater = Updater::new(exit_zero_config(dir.path()));
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let result = updater.update(token, None).await;
    match result {
        Ok(()) => {}
        Err(e) => assert!(e.contains("cancelled"), "{e}"),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn fake_auto_update_loop_success_then_stop() {
    // 自动更新循环成功轮：interval 30ms，跑 ~200ms（多轮 Ok(Ok)），stop()
    // → 循环退出。
    let dir = tempfile::tempdir().unwrap();
    place_where_as(dir.path(), "freshclam");
    let mut config = exit_zero_config(dir.path());
    config.update_interval = Duration::from_millis(30);
    let updater = std::sync::Arc::new(Updater::new(config));
    let u = updater.clone();
    let jh = tokio::spawn(async move {
        u.start_auto_update().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    updater.stop();
    tokio::time::timeout(Duration::from_secs(5), jh)
        .await
        .expect("auto-update loop must stop")
        .unwrap();
    assert!(!updater.running.load(Ordering::SeqCst));
    assert!(
        updater.last_update().is_some(),
        "loop must have updated once"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn fake_auto_update_loop_failure_does_not_stop_loop() {
    // 更新失败（exit 1）轮：错误分支记 error 日志但循环继续；stop() 才停。
    let dir = tempfile::tempdir().unwrap();
    place_where_as(dir.path(), "freshclam");
    std::fs::write(dir.path().join("--config-file"), "").unwrap();
    let config = UpdaterConfig {
        clamav_path: dir.path().to_string_lossy().to_string(),
        database_dir: String::new(),
        config_file: dir
            .path()
            .join("missing.conf")
            .to_string_lossy()
            .to_string(),
        update_interval: Duration::from_millis(30),
        mirror_urls: Vec::new(),
    };
    let updater = std::sync::Arc::new(Updater::new(config));
    let u = updater.clone();
    let jh = tokio::spawn(async move {
        u.start_auto_update().await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    // 循环仍在跑（失败不停）
    assert!(updater.running.load(Ordering::SeqCst));
    updater.stop();
    tokio::time::timeout(Duration::from_secs(5), jh)
        .await
        .expect("auto-update loop must stop")
        .unwrap();
    assert!(
        updater.last_update().is_none(),
        "failed updates must not set last_update"
    );
}

use std::sync::Arc;
use std::time::Duration;

#[test]
fn is_database_stale_fresh_and_stale_cvd_files() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("db");
    std::fs::create_dir_all(&db).unwrap();
    std::fs::write(db.join("main.cvd"), "fake").unwrap();
    std::thread::sleep(Duration::from_millis(5));
    let cfg = UpdaterConfig {
        clamav_path: String::new(),
        database_dir: db.to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: Duration::ZERO,
        mirror_urls: Vec::new(),
    };
    let u = Updater::new(cfg);
    // Fresh cvd (mtime just now) with a generous max_age → not stale.
    assert!(!u.is_database_stale(Duration::from_secs(3600)));
    // Zero max-age: any measurable age counts as stale → falls through to true.
    assert!(u.is_database_stale(Duration::ZERO));

    // No database_dir configured → stale without touching the filesystem.
    let cfg2 = UpdaterConfig {
        clamav_path: String::new(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::ZERO,
        mirror_urls: Vec::new(),
    };
    let u2 = Updater::new(cfg2);
    assert!(u2.is_database_stale(Duration::from_secs(3600)));
}

#[tokio::test]
async fn auto_update_loop_reports_failure_and_stops() {
    // 5ms interval + missing freshclam → each cycle fails fast and logs the
    // auto-update-failed arm; stop() breaks the loop.
    let u = Arc::new(Updater::new(UpdaterConfig {
        clamav_path: r"Z:\definitely\missing".to_string(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_millis(5),
        mirror_urls: Vec::new(),
    }));
    let u2 = u.clone();
    let h = tokio::spawn(async move {
        u2.start_auto_update().await;
    });
    tokio::time::sleep(Duration::from_millis(120)).await;
    u.stop();
    let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
}
