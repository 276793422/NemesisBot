//! session 辅助函数单测（纯本机、无浏览器依赖）。

use super::{kill_process, process_alive, tail_file};

#[test]
fn tail_file_reads_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("log.txt");
    std::fs::write(&p, "l1\nl2\nl3\nl4\nl5\n").unwrap();
    assert_eq!(tail_file(&p, 2), "l4\nl5");
    assert_eq!(tail_file(&p, 10), "l1\nl2\nl3\nl4\nl5");
}

#[test]
fn tail_file_missing_is_empty() {
    assert_eq!(tail_file(std::path::Path::new("Z:/no/such.log"), 5), "");
}

#[test]
fn tail_file_non_utf8_lossy_not_empty() {
    // F30 回归锁（2026-11-15 五审）：中文 Windows 上浏览器 stderr 是本地
    // 代码页（GBK）——旧 read_to_string 严格 UTF-8 读取 Err → 尾部静默变
    // 空串，诊断现场整段丢失。字节读 + lossy 后 ASCII 骨架（路径/端口/
    // 错误码）保真。GBK 字节 =「中文错误 port 9222」。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("stderr.log");
    let gbk: &[u8] = &[
        0xD6, 0xD0, 0xCE, 0xC4, 0xB4, 0xED, 0xCE, 0xF3, b' ', b'p', b'o', b'r', b't', b' ', b'9',
        b'2', b'2', b'2',
    ];
    std::fs::write(&p, gbk).unwrap();
    let tail = tail_file(&p, 10);
    assert!(tail.contains("port 9222"), "{tail:?}");
}

#[test]
fn summary_projects_alive_dead_state() {
    // F27 回归锁（2026-11-15 五审）：summary 的 alive 字段随 exited 旗标
    // 翻转——用户手关窗口后 reaper ms 级置位，前端「已退出」渲染数据源。
    use crate::driver::DriverKind;
    let s = super::BrowserSession::test_with("bT", DriverKind::Edge, false, 1);
    assert_eq!(s.summary()["alive"], serde_json::Value::Bool(true));
    s.exited.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(s.summary()["alive"], serde_json::Value::Bool(false));
}

#[test]
fn process_alive_current_pid() {
    // 本测试进程自己必然活着。
    assert!(process_alive(std::process::id()));
}

#[test]
fn process_alive_bogus_pid() {
    // PID u32::MAX 不可能存在（Windows 32 位 pid 上限远低于此）。
    assert!(!process_alive(u32::MAX));
}

#[test]
fn kill_process_bogus_pid_is_noop() {
    // 对不存在的 pid 强杀必须无害（不 panic、不误伤）。
    kill_process(u32::MAX);
}

// --- pid=0 守卫（F36，2026-11-15 六审）---

#[test]
fn process_alive_zero_pid_is_false() {
    // 0 号是保留值：Unix kill(0,0) 语义=「探测本进程组」恒真（必假活）；
    // spawn 竞态 unwrap_or(0) 与收养会话都不许被误判。
    assert!(!process_alive(0));
}

#[test]
fn kill_process_zero_pid_is_noop() {
    // Unix `kill(0, SIGKILL)` = 信号发给整个进程组 = 网关自己被杀。守卫
    // 必须 no-op（不 panic；Windows taskkill /PID 0 无害但 Linux 是组语义）。
    kill_process(0);
}

// --- 孤儿窗口收养（F37，2026-11-15 六审）---

/// 假 CDP HTTP 端点（收养探针打 GET /json/version；先读请求再写响应——
/// 带未读数据 close 时 Windows 发 RST 冲掉响应，见 F28 坑位实录）。
fn spawn_canned_version_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let resp: &[u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut drain = [0u8; 2048];
            let _ = std::io::Read::read(&mut s, &mut drain);
            let _ = std::io::Write::write_all(&mut s, resp);
        }
    });
    port
}

#[tokio::test]
async fn spawn_adopts_orphan_browser_via_live_port_file() {
    // F37 回归锁：端口文件在 + 端点活 = 不 spawn 直接收养（pid=0、端口
    // 来自文件、预清理不删文件）。executable 是不存在的路径——收养臂不
    // 触碰它，spawn 臂才会「启动浏览器」失败。
    use crate::cdp::DEVTOOLS_PORT_FILE;
    use crate::driver::DriverKind;

    let port = spawn_canned_version_server();
    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join(DEVTOOLS_PORT_FILE),
        format!("{port}\n/devtools/browser/x\n"),
    )
    .unwrap();

    let s = super::BrowserSession::spawn(
        "bA".to_string(),
        1,
        DriverKind::Edge,
        std::path::PathBuf::from("Z:/no/such/browser.exe"),
        &profile_root,
    )
    .await
    .expect("活端口文件应触发收养而非 spawn");
    assert_eq!(s.port, port, "收养会话端口应来自端口文件");
    assert_eq!(
        s.pid, 0,
        "收养会话 pid 未知 = 0（强杀兜底被 F36 守卫 no-op）"
    );
    assert!(s.is_alive().await, "端点活 = 会话活");
    // 端口文件必须仍在（收养臂不得触发预清理）。
    assert!(profile_dir.join(DEVTOOLS_PORT_FILE).exists());
}

#[tokio::test]
async fn spawn_with_dead_port_file_falls_through_to_launch() {
    // 死端点（端口 1 无监听）= 不收养，回落正常 spawn 路径——本例
    // executable 无效，报「启动浏览器」错证明走的是 spawn 臂。
    use crate::cdp::DEVTOOLS_PORT_FILE;
    use crate::driver::DriverKind;

    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join(DEVTOOLS_PORT_FILE),
        "1\n/devtools/browser/x\n",
    )
    .unwrap();

    let Err(err) = super::BrowserSession::spawn(
        "bB".to_string(),
        1,
        DriverKind::Edge,
        std::path::PathBuf::from("Z:/no/such/browser.exe"),
        &profile_root,
    )
    .await
    else {
        panic!("死端口文件不应收养（必须回落 spawn 臂）");
    };
    assert!(err.contains("启动浏览器"), "{err}");
}
