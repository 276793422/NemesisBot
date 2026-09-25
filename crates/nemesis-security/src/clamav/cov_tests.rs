// clamav/ 模块覆盖率补充测试（config 生成器 43/134 / daemon 启动守卫
// 52 与就绪等待失败路径 101/134-135 / updater 自更新启停 148 / ownership
// 无监听臂 28 / 伪 clamd 服务驱动的 scanner 206、hook 75、client 259）。
//
// 豁免（环境/平台依赖，仅记录不硬凑）：
// - config.rs 171/172/174/180：cfg! 非 Windows 编译期恒假分支（macos/
//   linux 候选目录与 unix 可执行名）。
// - config.rs 186（detect_clamav_path 命中臂）：需要真实 ClamAV 安装在
//   标准路径。
// - manager.rs 87/168/212-214/241：manager.start 需要 clamd 真实就绪
//   （G3 门还要求真实病毒库 main.cvd），stop 的 daemon 分支同因不可达。
// - updater.rs 171：自动更新的 5 分钟超时臂需要一次挂起的下载。
// - ownership.rs 103：需要「占着端口的受保护进程」使
//   QueryFullProcessImageNameW 失败。
// - mod.rs 22：find_executable 的非 Windows 可执行名分支（cfg! 恒假）。

use super::client::Client;
use super::config::{DaemonConfig, generate_clamd_config, generate_freshclam_config};
use super::daemon::Daemon;
use super::hook::ScanHook;
use super::scanner::{Scanner, ScannerConfig};
use super::updater::{Updater, UpdaterConfig};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn temp_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-cl-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 伪 clamd：回 PONG；SCAN（单文件）/CONTSCAN（目录）按脚本回感染/干净。
async fn spawn_fake_clamd(infected: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                continue;
            }
            let line = String::from_utf8_lossy(&buf[..n]);
            if line.contains("PING") {
                let _ = sock.write_all(b"PONG\n").await;
            } else if let Some(rest) = line.split("CONTSCAN ").nth(1) {
                let path = rest.trim();
                let reply = if infected {
                    format!("{path}: Eicar-Test-Signature FOUND\n")
                } else {
                    format!("{path}: OK\n")
                };
                let _ = sock.write_all(reply.as_bytes()).await;
            } else if let Some(rest) = line.split("SCAN ").nth(1) {
                // 单文件扫描（client scan_file 用 SCAN 而非 CONTSCAN）。
                let path = rest.trim();
                let reply = if infected {
                    format!("{path}: Eicar-Test-Signature FOUND\n")
                } else {
                    format!("{path}: OK\n")
                };
                let _ = sock.write_all(reply.as_bytes()).await;
            } else {
                let _ = sock.write_all(b"UNKNOWN\n").await;
            }
        }
    });
    addr
}

fn scanner_config(address: &str) -> ScannerConfig {
    ScannerConfig {
        enabled: true,
        address: address.to_string(),
        scan_on_write: true,
        scan_on_download: true,
        scan_on_exec: true,
        max_file_size: 50 * 1024 * 1024,
        timeout: std::time::Duration::from_secs(5),
    }
}

// ---------------------------------------------------------------------------
// config 生成器（跨平台）
// ---------------------------------------------------------------------------

/// clamd/freshclam 配置生成：带父目录的路径（43/134 的 create_dir_all 臂），
/// 产物落盘含核心指令。
#[test]
fn generate_clamd_and_freshclam_configs_write_files() {
    let dir = temp_dir("cfg");
    let db_dir = dir.join("database");
    let conf = dir.join("config").join("clamd.conf");
    let daemon_cfg = DaemonConfig {
        clamav_path: dir.to_string_lossy().to_string(),
        config_file: conf.to_string_lossy().to_string(),
        database_dir: db_dir.to_string_lossy().to_string(),
        listen_addr: "127.0.0.1:3310".to_string(),
        temp_dir: dir.join("temp").to_string_lossy().to_string(),
        log_file: dir
            .join("logs")
            .join("clamd.log")
            .to_string_lossy()
            .to_string(),
        startup_timeout_secs: 1,
    };
    generate_clamd_config(&daemon_cfg).unwrap();
    let body = std::fs::read_to_string(&conf).unwrap();
    assert!(body.contains("TCPSocket"), "{body}");

    let fconf = dir.join("config").join("freshclam.conf");
    generate_freshclam_config(
        &db_dir.to_string_lossy(),
        &fconf.to_string_lossy(),
        &dir.join("logs").join("freshclam.log").to_string_lossy(),
    )
    .unwrap();
    let fbody = std::fs::read_to_string(&fconf).unwrap();
    assert!(fbody.contains("DatabaseDirectory"), "{fbody}");
    assert!(db_dir.exists(), "db 目录必须创建");

    let _ = std::fs::remove_dir_all(&dir);
}

/// wait_for_ready：不可达端口轮询到 deadline（134-135 的 sleep 臂）。
#[tokio::test]
async fn wait_for_ready_times_out() {
    let daemon = Daemon::new(DaemonConfig {
        listen_addr: "127.0.0.1:1".to_string(),
        ..Default::default()
    });
    let err = daemon
        .wait_for_ready(std::time::Duration::from_millis(700))
        .await
        .unwrap_err();
    assert!(err.contains("timed out"), "{err}");
}

// ---------------------------------------------------------------------------
// updater（跨平台：不触真实 freshclam，stop 在首次更新前生效）
// ---------------------------------------------------------------------------

/// start_auto_update 记录启动信息（148），stop 后任务退出。
#[tokio::test]
async fn auto_update_starts_and_stops() {
    let dir = temp_dir("upd");
    let updater = Arc::new(Updater::new(UpdaterConfig {
        clamav_path: String::new(),
        database_dir: dir.to_string_lossy().to_string(),
        config_file: String::new(),
        update_interval: std::time::Duration::from_millis(400),
        mirror_urls: vec![],
    }));
    let task = tokio::spawn({
        let updater = Arc::clone(&updater);
        async move { updater.start_auto_update().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    updater.stop();
    // 任务在下一个间隔检查到 stop 后退出。
    tokio::time::timeout(std::time::Duration::from_secs(3), task)
        .await
        .expect("stop 后任务必须退出")
        .unwrap();

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 伪 clamd 驱动的扫描链路（跨平台：纯 TCP）
// ---------------------------------------------------------------------------

/// 感染流：scanner.scan_file → hook.scan_downloaded_file（75 warn +
/// 删除感染文件）→ 目录扫描感染警告（206）。
#[tokio::test]
async fn infected_scan_flow_through_scanner_and_hook() {
    let addr = spawn_fake_clamd(true).await;
    let dir = temp_dir("infected");
    let target = dir.join("sample.exe");
    std::fs::write(&target, b"fake malware").unwrap();

    let scanner = Arc::new(Scanner::new_with_client(
        Client::with_timeout(&addr, std::time::Duration::from_secs(5)),
        scanner_config(&addr),
    ));
    let r = scanner.scan_file(&target).await.unwrap();
    assert!(r.infected, "{:?}", r.raw);

    let hook = ScanHook::new(scanner.clone());
    let (allowed, result) = hook.scan_downloaded_file(&target).await.unwrap();
    assert!(!allowed, "感染文件必须拒绝");
    let result = result.expect("感染必须带结果");
    assert!(result.infected);
    assert!(!target.exists(), "感染文件必须被移除");

    // 目录扫描：感染结果带 dir 警告（206）。
    let results = scanner.scan_directory(&dir).await.unwrap();
    assert!(results.iter().any(|r| r.infected));

    let _ = std::fs::remove_dir_all(&dir);
}

/// 干净流：OK 响应 → clean 判定（client 259 的 OK 收尾）。
#[tokio::test]
async fn clean_scan_flow_returns_ok() {
    let addr = spawn_fake_clamd(false).await;
    let dir = temp_dir("clean");
    let target = dir.join("fine.exe");
    std::fs::write(&target, b"fine content").unwrap();

    let scanner = Arc::new(Scanner::new_with_client(
        Client::with_timeout(&addr, std::time::Duration::from_secs(5)),
        scanner_config(&addr),
    ));
    let r = scanner.scan_file(&target).await.unwrap();
    assert!(!r.infected, "{:?}", r.raw);

    let hook = ScanHook::new(scanner);
    let (allowed, result) = hook.scan_downloaded_file(&target).await.unwrap();
    assert!(allowed, "干净文件必须放行");
    // hook 对存在的文件总是带出扫描结果（干净时 Some + infected=false）。
    let result = result.expect("干净扫描也必须带结果");
    assert!(!result.infected, "{:?}", result.raw);
    assert!(target.exists(), "干净文件不得删除");

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Windows 专属：daemon 启动守卫（用 cmd.exe 副本冒充 clamd.exe）
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_daemon {
    use super::super::config::{DaemonConfig, generate_clamd_config};
    use super::super::daemon::Daemon;
    use super::temp_dir;
    use std::path::{Path, PathBuf};

    /// 用 cmd.exe 副本冒充 clamd.exe：能 spawn、会自行退出，满足
    /// 「可执行存在」前置检查而不产生真实副作用。
    fn fake_clamd_exe(dir: &Path) -> PathBuf {
        let exe = dir.join("clamd.exe");
        if !exe.exists() {
            let comspec = std::env::var("COMSPEC")
                .unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string());
            std::fs::copy(comspec, &exe).unwrap();
        }
        exe
    }

    /// clamd.exe 存在但 config_file 为空 → 守卫臂（52）。
    #[tokio::test]
    async fn daemon_start_requires_config_file() {
        let dir = temp_dir("daemon-nocfg");
        fake_clamd_exe(&dir);
        let daemon = Daemon::new(DaemonConfig {
            clamav_path: dir.to_string_lossy().to_string(),
            config_file: String::new(),
            startup_timeout_secs: 1,
            ..Default::default()
        });
        let err = daemon.start().await.unwrap_err();
        assert!(err.contains("config file path is required"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// clamd.exe + 配置齐备但端口无真实 clamd → 等待超时（101 循环臂 +
    /// stop 清理）→ Err 收尾。
    #[tokio::test]
    async fn daemon_start_times_out_without_real_clamd() {
        let dir = temp_dir("daemon-timeout");
        fake_clamd_exe(&dir);
        let conf = dir.join("clamd.conf");
        let daemon_cfg = DaemonConfig {
            clamav_path: dir.to_string_lossy().to_string(),
            config_file: conf.to_string_lossy().to_string(),
            database_dir: dir.join("database").to_string_lossy().to_string(),
            listen_addr: "127.0.0.1:1".to_string(),
            temp_dir: dir.join("temp").to_string_lossy().to_string(),
            log_file: String::new(),
            startup_timeout_secs: 1,
        };
        generate_clamd_config(&daemon_cfg).unwrap();

        let daemon = Daemon::new(daemon_cfg);
        let err = daemon.start().await.unwrap_err();
        assert!(err.contains("failed to become ready"), "{err}");
        assert!(!daemon.is_running(), "失败后必须已停止并清理");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 端口无监听 → 判非我方 clamd（28）。
    #[test]
    fn ownership_false_when_nothing_listens() {
        assert!(!super::super::ownership::clamd_is_ours(
            "127.0.0.1:1",
            Path::new("C:\\definitely\\not\\clamd.exe")
        ));
    }
}
