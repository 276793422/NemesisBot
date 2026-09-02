use super::*;

fn test_config() -> DaemonConfig {
    DaemonConfig {
        clamav_path: "/usr/bin".to_string(),
        config_file: "/tmp/clamd.conf".to_string(),
        database_dir: "/tmp/db".to_string(),
        listen_addr: "127.0.0.1:3310".to_string(),
        temp_dir: "/tmp".to_string(),
        startup_timeout_secs: 120,
        ..Default::default()
    }
}

#[test]
fn test_daemon_config_defaults() {
    let cfg = DaemonConfig::default();
    assert!(cfg.clamav_path.is_empty());
    assert!(cfg.config_file.is_empty());
    assert!(cfg.database_dir.is_empty());
    assert_eq!(cfg.listen_addr, "127.0.0.1:3310");
    assert!(cfg.temp_dir.is_empty());
    assert_eq!(cfg.startup_timeout_secs, 120);
}

#[test]
fn test_daemon_new() {
    let daemon = Daemon::new(test_config());
    assert!(!daemon.is_running());
}

#[test]
fn test_daemon_new_empty_listen_addr() {
    let mut cfg = test_config();
    cfg.listen_addr = String::new();
    let daemon = Daemon::new(cfg);
    assert!(!daemon.is_running());
    // Verify the client was configured with default address
    assert_eq!(daemon.client().address(), "127.0.0.1:3310");
}

#[test]
fn test_daemon_is_running_initially_false() {
    let daemon = Daemon::new(test_config());
    assert!(!daemon.is_running());
}

#[test]
fn test_daemon_is_ready_not_running() {
    let daemon = Daemon::new(test_config());
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert!(!rt.block_on(async { daemon.is_ready().await }));
}

#[test]
fn test_daemon_client() {
    let daemon = Daemon::new(test_config());
    assert_eq!(daemon.client().address(), "127.0.0.1:3310");
}

#[tokio::test]
async fn test_daemon_stop_when_not_running() {
    let daemon = Daemon::new(test_config());
    let result = daemon.stop().await;
    assert!(result.is_ok());
}

#[test]
fn test_find_executable() {
    let exe = super::super::find_executable("/usr/bin", "clamd");
    if cfg!(target_os = "windows") {
        assert!(exe.ends_with("clamd.exe"));
    } else {
        assert!(exe.ends_with("clamd"));
    }
}

#[tokio::test]
async fn test_daemon_start_already_running() {
    let daemon = Daemon::new(test_config());
    daemon.running.store(true, Ordering::SeqCst);
    let result = daemon.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already running"));
}

#[tokio::test]
async fn test_daemon_start_exe_not_found() {
    let daemon = Daemon::new(DaemonConfig {
        clamav_path: "/nonexistent/path".to_string(),
        config_file: "/tmp/clamd.conf".to_string(),
        ..Default::default()
    });
    let result = daemon.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_daemon_start_empty_config_file() {
    let daemon = Daemon::new(DaemonConfig {
        clamav_path: "/usr/bin".to_string(),
        config_file: String::new(),
        ..Default::default()
    });
    // If clamd exists at /usr/bin, this will fail due to empty config file
    // If clamd doesn't exist, it will fail due to missing exe
    let result = daemon.start().await;
    // Either way it should fail
    assert!(result.is_err());
}

#[test]
fn test_daemon_config_debug() {
    let cfg = test_config();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("/usr/bin"));
    assert!(debug.contains("3310"));
}

#[test]
fn test_daemon_is_ready_when_running_but_no_daemon() {
    // Closed port: with `running` forced true, ping must still fail, so
    // is_ready() is false. Deterministic regardless of a real clamd.
    let mut cfg = test_config();
    cfg.listen_addr = "127.0.0.1:1".to_string();
    let daemon = Daemon::new(cfg);
    daemon.running.store(true, Ordering::SeqCst);
    let rt = tokio::runtime::Runtime::new().unwrap();
    assert!(!rt.block_on(async { daemon.is_ready().await }));
    daemon.running.store(false, Ordering::SeqCst);
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_daemon_config_default_values() {
    let cfg = DaemonConfig::default();
    assert_eq!(cfg.startup_timeout_secs, 120);
    assert_eq!(cfg.listen_addr, "127.0.0.1:3310");
}

#[test]
fn test_daemon_new_with_custom_address() {
    let mut cfg = test_config();
    cfg.listen_addr = "192.168.1.1:9999".to_string();
    let daemon = Daemon::new(cfg);
    assert_eq!(daemon.client().address(), "192.168.1.1:9999");
    assert!(!daemon.is_running());
}

#[test]
fn test_daemon_is_running_flag_toggle() {
    let daemon = Daemon::new(test_config());
    assert!(!daemon.is_running());
    daemon.running.store(true, Ordering::SeqCst);
    assert!(daemon.is_running());
    daemon.running.store(false, Ordering::SeqCst);
    assert!(!daemon.is_running());
}

#[tokio::test]
async fn test_daemon_stop_idempotent() {
    let daemon = Daemon::new(test_config());
    // Stop when not running should succeed
    assert!(daemon.stop().await.is_ok());
    // Stop again should still succeed
    assert!(daemon.stop().await.is_ok());
}

#[test]
fn test_daemon_client_default_address_on_empty() {
    let mut cfg = test_config();
    cfg.listen_addr = String::new();
    let daemon = Daemon::new(cfg);
    assert_eq!(daemon.client().address(), "127.0.0.1:3310");
}

#[tokio::test]
async fn test_daemon_start_already_running_different_state() {
    let daemon = Daemon::new(test_config());
    // Set running to true, then try to start
    daemon.running.store(true, Ordering::SeqCst);
    let result = daemon.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already running"));
    // Reset state
    daemon.running.store(false, Ordering::SeqCst);
}

#[tokio::test]
async fn test_daemon_start_empty_clamav_path() {
    let daemon = Daemon::new(DaemonConfig {
        clamav_path: String::new(),
        config_file: "/tmp/clamd.conf".to_string(),
        ..Default::default()
    });
    let result = daemon.start().await;
    assert!(result.is_err());
    // Should fail because clamd not found
}

#[test]
fn test_daemon_config_clone() {
    let cfg = test_config();
    let cloned = cfg.clone();
    assert_eq!(cfg.clamav_path, cloned.clamav_path);
    assert_eq!(cfg.config_file, cloned.config_file);
    assert_eq!(cfg.listen_addr, cloned.listen_addr);
    assert_eq!(cfg.startup_timeout_secs, cloned.startup_timeout_secs);
}

#[tokio::test]
async fn test_daemon_is_ready_returns_false_when_not_running() {
    let daemon = Daemon::new(test_config());
    assert!(!daemon.is_running());
    // is_ready checks is_running first, so should return false
    assert!(!daemon.is_ready().await);
}

#[tokio::test]
async fn test_daemon_process_initially_none() {
    let daemon = Daemon::new(test_config());
    // The internal process should be None
    let proc = daemon.process.lock().await;
    assert!(proc.is_none());
}

// ============================================================
// Fake clamd / fake executable tests (2026-08-25 coverage push)
// ============================================================
// 真 clamd 启停算结构性；这里用两个替身把 spawn+readiness+stop 全链路拉通：
// - 假 clamd.exe = C:\Windows\System32\where.exe 的副本（合法 PE、立即退出，
//   start() 不检查 child 状态，只要 spawn 成功）
// - 假 clamd 服务 = 进程内 tokio TcpListener 应答 PONG（readiness 的 ping
//   打到它就 Ok）
// 全部 #[cfg(windows)]（where.exe / taskkill 依赖）。

#[cfg(windows)]
fn place_fake_exe(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".to_string());
    let src = std::path::Path::new(&windir)
        .join("System32")
        .join("where.exe");
    assert!(src.exists(), "where.exe not found at {}", src.display());
    let dst = dir.join(format!("{}.exe", name));
    std::fs::copy(&src, &dst).unwrap();
    dst
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
fn fake_daemon_config(clamav_dir: &std::path::Path, listen_addr: String) -> DaemonConfig {
    let conf = clamav_dir.join("clamd.conf");
    std::fs::write(&conf, "# fake clamd config for daemon test\n").unwrap();
    DaemonConfig {
        clamav_path: clamav_dir.to_string_lossy().to_string(),
        config_file: conf.to_string_lossy().to_string(),
        database_dir: clamav_dir.join("database").to_string_lossy().to_string(),
        listen_addr,
        temp_dir: clamav_dir.join("temp").to_string_lossy().to_string(),
        startup_timeout_secs: 2,
        ..Default::default()
    }
}

#[cfg(windows)]
#[tokio::test]
async fn fake_daemon_start_ready_success() {
    // 假 clamd.exe spawn 成功 + 进程内 PONG 服务 → readiness 首轮 ping 即 Ok。
    let dir = tempfile::tempdir().unwrap();
    place_fake_exe(dir.path(), "clamd");
    let addr = serve_pong().await;
    let daemon = Daemon::new(fake_daemon_config(dir.path(), addr));
    daemon.start().await.unwrap();
    assert!(daemon.is_running());
    assert!(daemon.is_ready().await);
    // stop 清理（child 可能已自行退出；kill/wait 结果被忽略）
    daemon.stop().await.unwrap();
    assert!(!daemon.is_running());
}

#[cfg(windows)]
#[tokio::test]
async fn fake_daemon_start_readiness_timeout_cleans_up() {
    // 假 clamd.exe 存在但监听端口无人应答 → readiness 轮询到
    // startup_timeout_secs → 内部 stop() + Err。
    let dir = tempfile::tempdir().unwrap();
    place_fake_exe(dir.path(), "clamd");
    let daemon = Daemon::new(fake_daemon_config(dir.path(), "127.0.0.1:1".to_string()));
    let err = daemon.start().await.unwrap_err();
    assert!(err.contains("failed to become ready"), "{err}");
    assert!(!daemon.is_running(), "timeout must stop the daemon");
    let proc = daemon.process.lock().await;
    assert!(proc.is_none(), "timeout must clear the child handle");
}

#[cfg(windows)]
#[tokio::test]
async fn fake_daemon_stop_with_live_child() {
    // stop() 的真分支：塞一个活着的子进程（ping -n 30）+ running=true →
    // kill+wait+清理，Ok。
    let daemon = Daemon::new(test_config());
    let child = tokio::process::Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    *daemon.process.lock().await = Some(child);
    daemon.running.store(true, Ordering::SeqCst);
    daemon.stop().await.unwrap();
    assert!(!daemon.is_running());
    assert!(daemon.process.lock().await.is_none());
}

#[cfg(windows)]
#[tokio::test]
async fn fake_daemon_wait_for_ready_success() {
    // wait_for_ready 不看 running 标志，ping 通即 Ok。
    let addr = serve_pong().await;
    let mut cfg = test_config();
    cfg.listen_addr = addr;
    let daemon = Daemon::new(cfg);
    daemon.wait_for_ready(Duration::from_secs(3)).await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn fake_daemon_wait_for_ready_timeout() {
    // 关闭端口 → 轮询到 deadline → Err("wait_for_ready timed out")。
    let mut cfg = test_config();
    cfg.listen_addr = "127.0.0.1:1".to_string();
    let daemon = Daemon::new(cfg);
    let err = daemon
        .wait_for_ready(Duration::from_millis(200))
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

#[cfg(windows)]
#[tokio::test]
async fn fake_daemon_start_ping_succeeds_after_initial_delay() {
    // 慢就绪变体：PONG 服务延迟 1.2s 才应答（内核 backlog 先完成握手，
    // 首轮 ping 会阻塞到服务开始应答后返回 Ok）——验证 start() 能等过
    // 慢启动窗口。（sleep-then-retry 分支由上面的 timeout 测试覆盖。）
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1200)).await;
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
    let dir = tempfile::tempdir().unwrap();
    place_fake_exe(dir.path(), "clamd");
    let mut cfg = fake_daemon_config(dir.path(), addr.to_string());
    cfg.startup_timeout_secs = 10;
    let daemon = Daemon::new(cfg);
    daemon.start().await.unwrap();
    assert!(daemon.is_ready().await);
    daemon.stop().await.unwrap();
}

use std::time::Duration;

#[tokio::test]
async fn daemon_start_missing_clamd_exe_errors() {
    let cfg = DaemonConfig {
        clamav_path: tempfile::tempdir()
            .unwrap()
            .path()
            .to_string_lossy()
            .to_string(),
        config_file: "clamd.conf".to_string(),
        ..Default::default()
    };
    let daemon = Daemon::new(cfg);
    let e = daemon.start().await.unwrap_err();
    assert!(e.contains("clamd executable not found"), "{e}");
}

async fn pong_server() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 64];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(b"PONG\n").await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr.to_string()
}

#[tokio::test]
async fn daemon_is_ready_and_wait_for_ready_with_mock_server() {
    let addr = pong_server().await;
    let cfg = DaemonConfig {
        listen_addr: addr,
        ..Default::default()
    };
    let daemon = Daemon::new(cfg);
    // Simulate a started daemon; readiness then depends on ping() only.
    daemon
        .running
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(daemon.is_ready().await);
    daemon.wait_for_ready(Duration::from_secs(2)).await.unwrap();
}

#[tokio::test]
async fn daemon_wait_for_ready_zero_deadline_times_out_immediately() {
    // Port 1 on loopback: connection refused instantly, nothing listens there.
    let cfg = DaemonConfig {
        listen_addr: "127.0.0.1:1".to_string(),
        ..Default::default()
    };
    let daemon = Daemon::new(cfg);
    daemon
        .running
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let e = daemon.wait_for_ready(Duration::ZERO).await.unwrap_err();
    assert!(e.contains("timed out"), "{e}");
}
