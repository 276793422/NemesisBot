use super::*;
use std::time::Duration;

#[test]
fn test_parse_duration_24h() {
    assert_eq!(
        parse_duration_string("24h"),
        Some(Duration::from_secs(24 * 3600))
    );
}

#[test]
fn test_parse_duration_1h30m() {
    assert_eq!(
        parse_duration_string("1h30m"),
        Some(Duration::from_secs(90 * 60))
    );
}

#[test]
fn test_parse_duration_30m() {
    assert_eq!(
        parse_duration_string("30m"),
        Some(Duration::from_secs(30 * 60))
    );
}

#[test]
fn test_parse_duration_1d() {
    assert_eq!(
        parse_duration_string("1d"),
        Some(Duration::from_secs(86400))
    );
}

#[test]
fn test_parse_duration_seconds() {
    assert_eq!(parse_duration_string("45s"), Some(Duration::from_secs(45)));
}

#[test]
fn test_parse_duration_composite() {
    assert_eq!(
        parse_duration_string("1d2h30m15s"),
        Some(Duration::from_secs(86400 + 7200 + 1800 + 15))
    );
}

#[test]
fn test_parse_duration_empty() {
    // 空串 = Some(ZERO)：文档化的「禁用」哨兵（向后兼容）。
    assert_eq!(parse_duration_string(""), Some(Duration::ZERO));
}

#[test]
fn test_parse_duration_invalid() {
    // 非法值 -> None（fail-loud 契约：调用方 WARN + 回落默认，不再静默禁用）。
    assert_eq!(parse_duration_string("abc"), None);
}

#[test]
fn test_parse_duration_invalid_mixed() {
    assert_eq!(parse_duration_string("1x"), None);
}

// --- BUG-20261116-026 新契约（修复后绿证形态） ---

#[test]
fn test_parse_duration_plain_digits_are_seconds() {
    // 纯数字 = 秒（对齐 nemesis-workflow parse_duration 语义）；旧实现
    // 尾部数字永不消费 → ZERO = 静默禁用病毒库自动更新（红证修复前实测）。
    assert_eq!(parse_duration_string("123"), Some(Duration::from_secs(123)));
}

#[test]
fn test_parse_duration_trailing_digits_consumed() {
    // 尾部数字按秒消费（"24h30" -> 24h + 30s）；旧实现静默丢弃尾部 "30"。
    assert_eq!(
        parse_duration_string("24h30"),
        Some(Duration::from_secs(24 * 3600 + 30))
    );
}

#[test]
fn test_parse_duration_word_value_is_none() {
    // "daily"（ScannerView 测试 mock 里的真实历史形态值）旧实现解析为
    // ZERO = 自动更新被静默禁用；新契约 None -> 调用方 WARN + 回落默认。
    assert_eq!(parse_duration_string("daily"), None);
}

#[test]
fn test_parse_duration_whitespace_trimmed() {
    assert_eq!(
        parse_duration_string("  12h  "),
        Some(Duration::from_secs(12 * 3600))
    );
}

#[test]
fn test_parse_duration_unit_without_number_is_none() {
    // 单位前无数字（"h30"）= 畸形 -> None，不静默按 30s 收。
    assert_eq!(parse_duration_string("h30"), None);
}

#[test]
fn test_parse_duration_overflow_saturates() {
    // 巨值饱和到「事实不限时」（与 nemesis-workflow parse_duration
    // BUG-20261114-057 同族语义），debug 不 panic。
    let parsed = parse_duration_string("99999999999999999999999h");
    assert!(parsed.is_some());
    assert!(parsed.unwrap() > Duration::from_secs(u64::MAX / 2));
}

#[test]
fn test_resolve_update_interval_passthrough() {
    assert_eq!(
        resolve_update_interval("1h30m"),
        Duration::from_secs(90 * 60)
    );
}

#[test]
fn test_resolve_update_interval_malformed_falls_back_to_24h() {
    // 畸形配置值（"daily"/空格形态）不再静默禁用自动更新，回落 24h 默认。
    assert_eq!(
        resolve_update_interval("daily"),
        Duration::from_secs(24 * 3600)
    );
    assert_eq!(
        resolve_update_interval("1 day"),
        Duration::from_secs(24 * 3600)
    );
}

#[test]
fn test_resolve_update_interval_empty_stays_disabled() {
    // 空串哨兵语义保持：禁用自动更新（向后兼容）。
    assert_eq!(resolve_update_interval(""), Duration::ZERO);
}

#[test]
fn test_manager_new() {
    let config = ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    assert!(!manager.is_running());
    assert!(manager.hook().is_none());
    assert!(manager.scanner().is_none());
}

#[tokio::test]
async fn test_manager_get_stats_not_started() {
    let config = ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    let stats = manager.get_stats().await;
    assert_eq!(stats["enabled"], false);
    assert_eq!(stats["started"], false);
    assert!(stats.get("scanner").is_none());
}

#[tokio::test]
async fn test_manager_stop_when_not_started() {
    let config = ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    // Should succeed without error even when not started
    let result = manager.stop().await;
    assert!(result.is_ok());
}

#[test]
fn test_manager_config_debug() {
    let config = ManagerConfig {
        enabled: true,
        clamav_path: "/usr/bin".to_string(),
        data_dir: "/tmp/clamav".to_string(),
        address: "127.0.0.1:3310".to_string(),
        scanner: None,
        update_interval: "24h".to_string(),
    };
    let debug = format!("{:?}", config);
    assert!(debug.contains("enabled"));
    assert!(debug.contains("/usr/bin"));
}

#[tokio::test]
async fn test_manager_start_disabled() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    let result = manager.start().await;
    assert!(result.is_ok());
    assert!(!manager.is_running());
}

#[tokio::test]
async fn test_manager_start_already_started() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    manager.started.store(true, Ordering::SeqCst);
    let result = manager.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already started"));
}

#[tokio::test]
async fn test_manager_start_missing_clamav() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: true,
        clamav_path: "/nonexistent/path".to_string(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    // This will fail because the path doesn't exist
    let result = manager.start().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_manager_get_stats_with_updater() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    // Manually inject an updater with a recent last_update
    let updater = Arc::new(Updater::new(UpdaterConfig {
        clamav_path: String::new(),
        database_dir: String::new(),
        config_file: String::new(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    }));
    manager.updater = Some(updater);
    let stats = manager.get_stats().await;
    assert_eq!(stats["enabled"], false);
    // last_update_secs_ago should not be present since last_update is None
}

#[tokio::test]
async fn test_manager_hook_and_scanner_none_before_start() {
    let manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    assert!(manager.hook().is_none());
    assert!(manager.scanner().is_none());
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_parse_duration_2h() {
    assert_eq!(parse_duration_string("2h"), Some(Duration::from_secs(7200)));
}

#[test]
fn test_parse_duration_15m() {
    assert_eq!(parse_duration_string("15m"), Some(Duration::from_secs(900)));
}

#[test]
fn test_parse_duration_90s() {
    assert_eq!(parse_duration_string("90s"), Some(Duration::from_secs(90)));
}

#[test]
fn test_parse_duration_7d() {
    assert_eq!(
        parse_duration_string("7d"),
        Some(Duration::from_secs(7 * 86400))
    );
}

#[test]
fn test_parse_duration_1d12h() {
    assert_eq!(
        parse_duration_string("1d12h"),
        Some(Duration::from_secs(86400 + 43200))
    );
}

#[test]
fn test_parse_duration_zero() {
    assert_eq!(parse_duration_string("0s"), Some(Duration::from_secs(0)));
}

#[test]
fn test_parse_duration_only_digits() {
    // 纯数字 = 秒（BUG-20261116-026 修复：旧实现尾部数字永不消费返回
    // ZERO，此测试彼时钉住的是缺陷形态本身）。
    assert_eq!(parse_duration_string("123"), Some(Duration::from_secs(123)));
}

#[test]
fn test_manager_config_custom_scanner() {
    let scanner_cfg = ScannerConfig {
        enabled: true,
        address: "127.0.0.1:3310".to_string(),
        scan_on_write: false,
        scan_on_download: true,
        scan_on_exec: true,
        max_file_size: 100 * 1024 * 1024,
        timeout: Duration::from_secs(120),
    };
    let config = ManagerConfig {
        enabled: true,
        clamav_path: "/opt/clamav".to_string(),
        data_dir: "/tmp/clamav-data".to_string(),
        address: "127.0.0.1:3310".to_string(),
        scanner: Some(scanner_cfg),
        update_interval: "12h".to_string(),
    };
    let manager = Manager::new(config);
    assert!(!manager.is_running());
    assert!(manager.hook().is_none());
    assert!(manager.scanner().is_none());
}

#[tokio::test]
async fn test_manager_stop_when_disabled() {
    let manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    // Stop should succeed when disabled and not started
    let result = manager.stop().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_manager_get_stats_structure() {
    let config = ManagerConfig {
        enabled: true,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    };
    let manager = Manager::new(config);
    let stats = manager.get_stats().await;

    // Verify JSON structure
    assert!(stats.is_object());
    assert_eq!(stats["enabled"], true);
    assert_eq!(stats["started"], false);
    // No scanner -> no scanner stats
    assert!(stats.get("scanner").is_none());
}

#[test]
fn test_manager_new_with_enabled() {
    let config = ManagerConfig {
        enabled: true,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: "127.0.0.1:9999".to_string(),
        scanner: None,
        update_interval: "6h".to_string(),
    };
    let manager = Manager::new(config);
    assert!(!manager.is_running());
}

#[tokio::test]
async fn test_manager_start_disabled_returns_ok() {
    let mut manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    let result = manager.start().await;
    assert!(result.is_ok());
    // Even though start succeeded, is_running is still false because disabled
    assert!(!manager.is_running());
}

#[test]
fn test_parse_duration_complex_composite() {
    // Test 2d8h45m30s
    let expected = 2 * 86400 + 8 * 3600 + 45 * 60 + 30;
    assert_eq!(
        parse_duration_string("2d8h45m30s"),
        Some(Duration::from_secs(expected))
    );
}

#[test]
fn test_parse_duration_invalid_char_in_middle() {
    assert_eq!(parse_duration_string("10x5m"), None);
}

#[test]
fn test_parse_duration_only_days() {
    assert_eq!(
        parse_duration_string("3d"),
        Some(Duration::from_secs(3 * 86400))
    );
}

#[test]
fn test_parse_duration_only_hours() {
    assert_eq!(
        parse_duration_string("12h"),
        Some(Duration::from_secs(12 * 3600))
    );
}

#[test]
fn test_parse_duration_only_minutes() {
    assert_eq!(
        parse_duration_string("45m"),
        Some(Duration::from_secs(45 * 60))
    );
}

#[test]
fn test_parse_duration_only_seconds() {
    assert_eq!(parse_duration_string("30s"), Some(Duration::from_secs(30)));
}

#[test]
fn test_parse_duration_hours_and_minutes() {
    let expected = 2 * 3600 + 30 * 60;
    assert_eq!(
        parse_duration_string("2h30m"),
        Some(Duration::from_secs(expected))
    );
}

// ============================================================
// Fake clamd / where.exe full-start tests (coverage push)
// ============================================================
// 把 manager 的完整启动序列拉通：
// - 假 clamd.exe = where.exe 副本（spawn 成功即退，start() 只看 ping）
// - 假 clamd 服务 = 进程内 PONG server（readiness + scanner.ping 都打它）
// - 预置 <data_dir>/database/main.cvd（mtime 新鲜 → is_database_stale=false
//   → 跳过 freshclam；同时过 检查）
// - update_interval "1h" → 覆盖 step 8 的 spawn 分支（1h 内不会真跑 freshclam）

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
async fn serve_pong() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        loop {
            let (mut s, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let mut buf = [0u8; 64];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"PONG\n").await;
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

#[cfg(windows)]
fn fake_manager_config(
    clamav_dir: &std::path::Path,
    data_dir: &std::path::Path,
    addr: String,
) -> ManagerConfig {
    ManagerConfig {
        enabled: true,
        clamav_path: clamav_dir.to_string_lossy().to_string(),
        data_dir: data_dir.to_string_lossy().to_string(),
        address: addr,
        scanner: None,
        update_interval: "1h".to_string(),
    }
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_full_start_and_stop() {
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    // 预置新鲜 main.cvd：跳过 freshclam 下载 + 过
    std::fs::create_dir_all(data_dir.path().join("database")).unwrap();
    std::fs::write(
        data_dir.path().join("database").join("main.cvd"),
        "fake cvd",
    )
    .unwrap();

    let addr = serve_pong().await;
    let mut manager = Manager::new(fake_manager_config(
        clamav_dir.path(),
        data_dir.path(),
        addr,
    ));
    manager.start().await.unwrap();
    assert!(manager.is_running());

    // hook / scanner 就绪（ping 到进程内 PONG 服务）
    let hook = manager.hook().expect("hook after start");
    hook.health_check().await.unwrap();
    let scanner = manager.scanner().expect("scanner after start");
    scanner.ping().await.unwrap();

    // 配置文件已生成在 data_dir/config/
    assert!(data_dir.path().join("config").join("clamd.conf").exists());
    assert!(
        data_dir
            .path()
            .join("config")
            .join("freshclam.conf")
            .exists()
    );

    // get_stats：started=true + scanner 统计块存在
    let stats = manager.get_stats().await;
    assert_eq!(stats["started"], serde_json::json!(true));
    assert!(stats.get("scanner").is_some(), "stats: {stats}");

    // 二次 start → already started
    let err = manager.start().await.unwrap_err();
    assert!(err.contains("already started"), "{err}");

    // stop → 幂等
    manager.stop().await.unwrap();
    assert!(!manager.is_running());
    manager.stop().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_start_missing_database_refuses() {
    // 无 main.cvd：stale → update（freshclam 缺失 → warn）→ 拒绝启动。
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    let addr = serve_pong().await;
    let mut manager = Manager::new(fake_manager_config(
        clamav_dir.path(),
        data_dir.path(),
        addr,
    ));
    let err = manager.start().await.unwrap_err();
    assert!(err.contains("virus database missing"), "{err}");
    assert!(!manager.is_running());
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_start_with_malformed_update_interval_still_starts() {
    // BUG-20261116-026 端到端臂：畸形 update_interval（"daily"）经
    // resolve_update_interval 回落 24h（WARN 留痕）→ start() 照常拉起、
    // 自动更新照常 spawn——不再被 ZERO 闸静默禁用。
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_dir.path().join("database")).unwrap();
    std::fs::write(
        data_dir.path().join("database").join("main.cvd"),
        "fake cvd",
    )
    .unwrap();
    let addr = serve_pong().await;
    let mut cfg = fake_manager_config(clamav_dir.path(), data_dir.path(), addr);
    cfg.update_interval = "daily".to_string();
    let mut manager = Manager::new(cfg);
    manager.start().await.unwrap();
    assert!(manager.is_running());
    manager.stop().await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_restart_cycle() {
    // 全量 start 成功后 restart：stop 掉死 where.exe 子进程 + 重新 spawn。
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_dir.path().join("database")).unwrap();
    std::fs::write(
        data_dir.path().join("database").join("main.cvd"),
        "fake cvd",
    )
    .unwrap();

    let addr = serve_pong().await;
    let mut manager = Manager::new(fake_manager_config(
        clamav_dir.path(),
        data_dir.path(),
        addr,
    ));
    manager.start().await.unwrap();
    manager.restart().await.unwrap();
    manager.restart().await.unwrap();
    assert!(manager.is_running());
    assert!(manager.hook().unwrap().health_check().await.is_ok());
    manager.stop().await.unwrap();
}

#[tokio::test]
async fn manager_restart_without_daemon_fails() {
    let manager = Manager::new(ManagerConfig {
        enabled: false,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    let err = manager.restart().await.unwrap_err();
    assert!(err.contains("no daemon to restart"), "{err}");
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_get_stats_with_real_last_update() {
    // 注入一个真跑成功过 update 的 updater（where.exe exit-0 技巧）→
    // get_stats 出现 last_update_secs_ago。
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "freshclam");
    let conf = clamav_dir.path().join("freshclam.conf");
    std::fs::write(&conf, "# fake\n").unwrap();
    std::fs::write(clamav_dir.path().join("--config-file"), "").unwrap();
    let updater = Arc::new(Updater::new(UpdaterConfig {
        clamav_path: clamav_dir.path().to_string_lossy().to_string(),
        database_dir: String::new(),
        // 相对名：where.exe 拒绝绝对路径 pattern（exit 2），见 updater/tests.rs
        // exit_zero_config 注释。cwd=clamav_path，相对 conf 名可命中。
        config_file: "freshclam.conf".to_string(),
        update_interval: Duration::from_secs(3600),
        mirror_urls: Vec::new(),
    }));
    updater
        .update(tokio_util::sync::CancellationToken::new(), None)
        .await
        .unwrap();

    let mut manager = Manager::new(ManagerConfig {
        enabled: true,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    manager.updater = Some(updater);
    let stats = manager.get_stats().await;
    assert!(
        stats.get("last_update_secs_ago").is_some(),
        "stats: {stats}"
    );
}

#[tokio::test]
async fn manager_start_auto_detect_without_clamav_fails() {
    // clamav_path 空 → detect_clamav_path()：测试机无 ClamAV → None → Err；
    // 即使机器装了 ClamAV（Program Files 不可写）也会在 create_dir_all 失败，
    // 两种环境都 Err。
    let mut manager = Manager::new(ManagerConfig {
        enabled: true,
        clamav_path: String::new(),
        data_dir: String::new(),
        address: String::new(),
        scanner: None,
        update_interval: String::new(),
    });
    assert!(manager.start().await.is_err());
    assert!(!manager.is_running());
}

#[cfg(windows)]
fn place_hostname_as(dir: &std::path::Path, name: &str) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let src = std::path::Path::new(&windir)
        .join("System32")
        .join("hostname.exe");
    assert!(src.exists(), "hostname.exe not found at {}", src.display());
    std::fs::copy(&src, dir.join(format!("{}.exe", name))).unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_freshclam_conf_write_failure_errors() {
    // freshclam.conf path occupied by a directory → generate_freshclam_config's
    // fs::write fails → start() propagates through the `?` at the call site.
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    let data_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(data_dir.path().join("config").join("freshclam.conf")).unwrap();
    let addr = serve_pong().await;
    let mut manager = Manager::new(fake_manager_config(
        clamav_dir.path(),
        data_dir.path(),
        addr,
    ));
    let err = manager.start().await.unwrap_err();
    assert!(err.contains("freshclam.conf"), "{err}");
}

#[cfg(windows)]
#[tokio::test]
async fn fake_manager_initial_database_download_success_arm() {
    // hostname.exe ignores all arguments and exits 0 → the fake freshclam
    // "succeeds" → the "Virus database downloaded successfully" arm runs.
    // No main.cvd afterwards → refuses to start clamd (still fine here).
    let clamav_dir = tempfile::tempdir().unwrap();
    place_where_as(clamav_dir.path(), "clamd");
    place_hostname_as(clamav_dir.path(), "freshclam");
    let data_dir = tempfile::tempdir().unwrap();
    let addr = serve_pong().await;
    let mut manager = Manager::new(fake_manager_config(
        clamav_dir.path(),
        data_dir.path(),
        addr,
    ));
    let err = manager.start().await.unwrap_err();
    assert!(err.contains("virus database missing"), "{err}");
}
