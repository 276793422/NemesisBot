use super::*;
use tempfile::TempDir;

// -------------------------------------------------------------------------
// PID_FILE constant
// -------------------------------------------------------------------------

#[test]
fn test_pid_file_constant() {
    assert_eq!(PID_FILE, "gateway.pid");
}

// -------------------------------------------------------------------------
// PID file parsing logic
// -------------------------------------------------------------------------

#[test]
fn test_pid_file_parsing_valid() {
    let tmp = TempDir::new().unwrap();
    let pid_path = tmp.path().join(PID_FILE);
    std::fs::write(&pid_path, "12345\n").unwrap();

    let data = std::fs::read_to_string(&pid_path).unwrap();
    let pid = data.trim().parse::<u32>();
    assert!(pid.is_ok());
    assert_eq!(pid.unwrap(), 12345);
}

#[test]
fn test_pid_file_parsing_no_newline() {
    let tmp = TempDir::new().unwrap();
    let pid_path = tmp.path().join(PID_FILE);
    std::fs::write(&pid_path, "99999").unwrap();

    let data = std::fs::read_to_string(&pid_path).unwrap();
    let pid = data.trim().parse::<u32>();
    assert!(pid.is_ok());
    assert_eq!(pid.unwrap(), 99999);
}

#[test]
fn test_pid_file_parsing_invalid() {
    let tmp = TempDir::new().unwrap();
    let pid_path = tmp.path().join(PID_FILE);
    std::fs::write(&pid_path, "not-a-number").unwrap();

    let data = std::fs::read_to_string(&pid_path).unwrap();
    let pid = data.trim().parse::<u32>();
    assert!(pid.is_err());
}

#[test]
fn test_pid_file_parsing_empty() {
    let tmp = TempDir::new().unwrap();
    let pid_path = tmp.path().join(PID_FILE);
    std::fs::write(&pid_path, "").unwrap();

    let data = std::fs::read_to_string(&pid_path).unwrap();
    let pid = data.trim().parse::<u32>();
    assert!(pid.is_err());
}

// -------------------------------------------------------------------------
// Shutdown signal file logic
// -------------------------------------------------------------------------

#[test]
fn test_shutdown_signal_file_creation() {
    let tmp = TempDir::new().unwrap();
    let signal_path = tmp.path().join("shutdown.signal");
    let timestamp = chrono::Local::now().to_rfc3339();
    std::fs::write(&signal_path, &timestamp).unwrap();

    assert!(signal_path.exists());
    let content = std::fs::read_to_string(&signal_path).unwrap();
    assert!(!content.is_empty());
}

#[test]
fn test_shutdown_signal_cleanup() {
    let tmp = TempDir::new().unwrap();
    let signal_path = tmp.path().join("shutdown.signal");
    std::fs::write(&signal_path, "test").unwrap();
    assert!(signal_path.exists());

    let _ = std::fs::remove_file(&signal_path);
    assert!(!signal_path.exists());
}

// -------------------------------------------------------------------------
// PID file cleanup logic
// -------------------------------------------------------------------------

#[test]
fn test_pid_file_cleanup() {
    let tmp = TempDir::new().unwrap();
    let pid_path = tmp.path().join(PID_FILE);
    std::fs::write(&pid_path, "12345").unwrap();
    assert!(pid_path.exists());

    let _ = std::fs::remove_file(&pid_path);
    assert!(!pid_path.exists());
}

// -------------------------------------------------------------------------
// Port extraction from config (shutdown HTTP fallback)
// -------------------------------------------------------------------------

#[test]
fn test_port_extraction_from_config() {
    let cfg = serde_json::json!({
        "channels": {
            "web": {
                "port": 49000
            }
        }
    });
    let port = cfg
        .get("channels")
        .and_then(|c| c.get("web"))
        .and_then(|w| w.get("port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(8080);
    assert_eq!(port, 49000);
}

#[test]
fn test_port_extraction_default() {
    let cfg = serde_json::json!({});
    let port = cfg
        .get("channels")
        .and_then(|c| c.get("web"))
        .and_then(|w| w.get("port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(8080);
    assert_eq!(port, 8080);
}

#[test]
fn test_shutdown_url_construction() {
    // (BUG #31, quality-hardening goal 冲刺 S11e) Method 3 改打既有内部控制
    // 端点 /api/internal（旧 /api/shutdown 从未注册，是死臂）。
    let port: u64 = 49000;
    let url = format!("http://127.0.0.1:{}/api/internal", port);
    assert_eq!(url, "http://127.0.0.1:49000/api/internal");
}

// -------------------------------------------------------------------------
// Config path resolution
// -------------------------------------------------------------------------

#[test]
fn test_config_path_for_shutdown() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    assert!(!cfg_path.exists());

    // Verify we can read when it doesn't exist
    let result = std::fs::read_to_string(&cfg_path);
    assert!(result.is_err());
}

#[test]
fn test_config_path_with_valid_config() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({"channels": {"web": {"port": 12345}}});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
    let port = parsed
        .get("channels")
        .and_then(|c| c.get("web"))
        .and_then(|w| w.get("port"))
        .and_then(|v| v.as_u64())
        .unwrap_or(8080);
    assert_eq!(port, 12345);
}

// -------------------------------------------------------------------------
// HTTP timeout configuration
// -------------------------------------------------------------------------

#[test]
fn test_shutdown_http_timeout() {
    let timeout = std::time::Duration::from_secs(5);
    assert_eq!(timeout.as_secs(), 5);
}

// ===========================================================================
// run() 全臂（S11c 建立基础；S11e 随 BUG #31 重写更新到 /api/internal 端点）
// —— 既有 10 个测试只钉常量/解析片段，run() 本体从没跑过。run() 是同步 fn，
// 全部走 env home 隔离 + 本地 mock（127.0.0.1:0）。
//
// S11e 变更：
// - Method 3 打 POST /api/internal {"cmd":"shutdown"}（原 /api/shutdown 死臂）
//   并断言 body / X-Auth-Token header；
// - 补 token 缺失分支（config 无 auth_token → 不发 header）；
// - 两个 panic 探针：#27 同款拓扑红对照 + 生产新写法绿证明（见 probe 段）。
// ===========================================================================

mod run_arm {
    use super::*;

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    /// 记录型本地 HTTP mock：应答固定响应，同时把收到的请求要素
    /// （请求行 / 是否带 X-Auth-Token / token 值 / body）经 std::mpsc 回传，
    /// 供测试断言。大小写不敏感地找 header（hyper 发 lowercase 头名）。
    /// 用 std::net 单线程 + 子线程 serve（生产 helper 自带独立线程，无冲突）。
    struct MockRecord {
        request_line: String,
        auth_token: Option<String>,
        body: String,
    }

    fn start_mock(status: &'static str, body: &'static str) -> (u16, std::sync::mpsc::Receiver<MockRecord>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel::<MockRecord>();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for _ in 0..4 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                // 读头部
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let head_end = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .unwrap_or(buf.len());
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let request_line = lines.next().unwrap_or("").to_string();
                let mut auth_token = None;
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("x-auth-token") {
                            auth_token = Some(v.trim().to_string());
                        }
                    }
                }
                // 按 Content-Length 读 body（带超时兜底）
                let content_length = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.trim()
                            .eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                stream
                    .set_read_timeout(Some(std::time::Duration::from_millis(500)))
                    .ok();
                let mut body_bytes = buf[head_end.min(buf.len())..].to_vec();
                while body_bytes.len() < content_length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => body_bytes.extend_from_slice(&chunk[..n]),
                    }
                }
                let resp = format!(
                    "HTTP/1.1 {s}\r\nContent-Type: application/json\r\nContent-Length: {l}\r\n\r\n{b}",
                    s = status,
                    l = body.len(),
                    b = body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = tx.send(MockRecord {
                    request_line,
                    auth_token,
                    body: String::from_utf8_lossy(&body_bytes).to_string(),
                });
            }
        });
        (port, rx)
    }

    /// 拿一个确定空闲的端口号（bind 后立刻 drop）。
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn run_without_pid_and_config_writes_signal_and_reports_unreachable() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            run(false).expect("无 PID 文件无 config → 信号文件 + Could not reach + Ok");
            assert!(home.join("shutdown.signal").exists(), "信号文件必须落盘");
        });
    }

    #[test]
    fn run_with_unparsable_pid_falls_through_to_signal_file() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("gateway.pid"), "not-a-number\n").unwrap();
            run(false).expect("PID 解析失败 → 落到信号文件路径");
            assert!(home.join("shutdown.signal").exists());
            // PID 文件保持原样（该分支不清理）。
            assert!(home.join("gateway.pid").exists());
        });
    }

    #[test]
    fn run_with_out_of_range_pid_is_safe_noop() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            // 0xFFFFFFFF 超出 Windows 句柄表 PID 空间（≤0xFFFFFC），
            // taskkill 必然打不到任何真进程——确定性安全。
            std::fs::write(home.join("gateway.pid"), "4294967295\n").unwrap();
            // 无论 taskkill 走 "not found" 清理分支还是 Failed to signal
            // 分支（系统语言相关），run 都必须 Ok 返回。
            run(false).expect("taskkill 失败路径 → Ok");
        });
    }

    #[test]
    fn run_with_live_pid_uses_taskkill_graceful_path() {
        // 真 spawn 一个本机 sleep 进程（继承当前控制台，不开新窗口），
        // 再用生产同款 taskkill /PID 收掉——覆盖 Ok+success 分支。
        // spawn 失败（如 CI 无 ping）则跳过，不制造环境性红。
        let sleeper = std::process::Command::new("ping")
            .args(["127.0.0.1", "-n", "30"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        let Ok(child) = sleeper else {
            return; // 环境无 ping：跳过（非断言失败）
        };
        let pid = child.id();

        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("gateway.pid"), format!("{pid}\n")).unwrap();
            // taskkill 无 /F：先礼后兵。对控制台进程可能回"只能强制终止"
            // （非 success 分支），两种结果 run 都 Ok——只钉"不炸 + 不杀错"。
            run(false).expect("taskkill graceful 路径 → Ok");
        });
        // 收尾兜底：确保 sleeper 不滞留 30s。
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
    }

    #[test]
    fn run_http_shutdown_success_posts_internal_cmd_and_removes_signal_file() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            let (port, rx) = start_mock("200 OK", r#"{"status":"ok"}"#);
            std::fs::write(
                home.join("config.json"),
                serde_json::json!({
                    "channels": {"web": {"port": port, "auth_token": "tok-abc"}}
                })
                .to_string(),
            )
            .unwrap();
            run(false).expect("HTTP 200 → Shutdown requested via HTTP API + Ok");
            assert!(
                !home.join("shutdown.signal").exists(),
                "成功后信号文件应被清理"
            );
            // 断言打到的是【新端点 + 正确形状】而非旧死臂。
            let rec = rx.recv_timeout(std::time::Duration::from_secs(2))
                .expect("mock 必须收到请求");
            assert!(
                rec.request_line.starts_with("POST /api/internal "),
                "必须 POST /api/internal，实际: {}",
                rec.request_line
            );
            assert_eq!(
                rec.auth_token.as_deref(),
                Some("tok-abc"),
                "config 配了 auth_token 就必须带上 X-Auth-Token"
            );
            assert!(
                rec.body.contains("\"cmd\":\"shutdown\""),
                "body 必须是 {{\"cmd\":\"shutdown\"}}，实际: {}",
                rec.body
            );
        });
    }

    #[test]
    fn run_http_shutdown_missing_token_omits_header_still_succeeds() {
        // token 缺失分支：config 未配 auth_token → 不发 X-Auth-Token 头
        // （服务端空 token 可过的既有约定），命令仍成功并清理信号文件。
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            let (port, rx) = start_mock("200 OK", r#"{"status":"ok"}"#);
            std::fs::write(
                home.join("config.json"),
                serde_json::json!({"channels": {"web": {"port": port}}}).to_string(),
            )
            .unwrap();
            run(false).expect("无 token 配置 → 照常成功");
            assert!(!home.join("shutdown.signal").exists());
            let rec = rx.recv_timeout(std::time::Duration::from_secs(2))
                .expect("mock 必须收到请求");
            assert!(
                rec.request_line.starts_with("POST /api/internal "),
                "实际: {}",
                rec.request_line
            );
            assert!(
                rec.auth_token.is_none(),
                "未配置 token 时绝不能发 X-Auth-Token，实际发了: {:?}",
                rec.auth_token
            );
        });
    }

    #[test]
    fn run_http_shutdown_non_2xx_keeps_signal_file() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            let (port, rx) = start_mock("500 Internal Server Error", r#"{"err":"no"}"#);
            std::fs::write(
                home.join("config.json"),
                serde_json::json!({"channels": {"web": {"port": port}}}).to_string(),
            )
            .unwrap();
            run(false).expect("HTTP 500 → 打印状态 + Could not reach + Ok");
            assert!(
                home.join("shutdown.signal").exists(),
                "非 2xx 不清理信号文件"
            );
            // 收到过请求（端点确实被打了，是网关侧拒绝/错误）。
            let rec = rx.recv_timeout(std::time::Duration::from_secs(2)).expect("mock 必须收到请求");
            assert!(rec.request_line.starts_with("POST /api/internal "));
        });
    }

    #[test]
    fn run_http_shutdown_unreachable_port_reports_not_reachable() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            let port = free_port();
            std::fs::write(
                home.join("config.json"),
                serde_json::json!({"channels": {"web": {"port": port}}}).to_string(),
            )
            .unwrap();
            run(false).expect("连接拒绝 → not reachable + Could not reach + Ok");
            assert!(home.join("shutdown.signal").exists());
        });
    }
}

// ===========================================================================
// panic 探针（BUG #27 同类横向 / BUG #31, quality-hardening goal 冲刺 S11e）
//
// 生产修复模式（本批首次引入）：「独立 OS 线程 + 私有 multi-thread runtime +
// 异步 client」——新线程没有 ambient runtime context，runtime 创建/销毁合法，
// 且全程不存在嵌套 runtime 的 blocking client。
// 每个【首次使用】的模式必须有 catch_unwind 探针实证（#27 的探针拓扑纪律）：
//   红①：multi_thread rt + block_on 内创建并 drop reqwest::blocking::Client
//         → 必 panic（文档化为什么不能用旧写法）。
//   绿②：同样的 async 上下文里调用 post_internal_shutdown（内部走独立线程）
//         → 绝不 panic 且往返成功。
// ===========================================================================

mod probes {
    use super::*;

    /// 红①对照探针：在 multi_thread runtime 的 block_on 未来体里创建并 drop
    /// reqwest::blocking::Client 必 panic（忠实复刻 main.rs 非 gateway 命令的
    /// 拓扑：rt.block_on(run_command) 里同步代码创建/丢弃 blocking client）。
    /// 实证 #27 结论对本文件的旧 Method 3 同样成立——这就是重写的理由。
    #[test]
    fn probe_blocking_client_drop_inside_block_on_panics_red_documentation() {
        std::thread::scope(|s| {
            let handle = s.spawn(|| {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                // catch_unwind 包住 block_on 整体（未来体里的 panic 会传播到
                // block_on 调用方）。未来的 poll 期间线程持有 runtime enter
                // guard——这正是会炸掉 blocking client drop 的上下文。
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    rt.block_on(async {
                        let client = reqwest::blocking::Client::new();
                        drop(client); // ← 嵌套 runtime 在此 drop → 应当 panic
                    })
                }));
                drop(rt); // block_on 已返回，无 enter guard，drop 合法
                outcome
            });
            let inner = handle.join().expect("探针宿主线程自身不能炸");
            assert!(
                inner.is_err(),
                "blocking client 在 block_on 上下文 drop 应当 panic（若不再 panic 说明 tokio 行为变了，生产注释与本测试都要更新）"
            );
            // 消息文本不强校验：不同 tokio 版本措辞可能微调；钉死的契约是「必 panic」行为。
        });
    }

    /// 绿②探针：生产模式——同样在 multi_thread rt + block_on 拓扑里调用
    /// post_internal_shutdown（内部经独立 OS 线程 + 私有 runtime + 异步
    /// client），必须不 panic、正常拿到 200 往返。若这里 panic，join 的
    /// expect 会如实把测试打红。
    #[test]
    fn probe_post_internal_shutdown_from_async_context_is_panic_free() {
        // 本地一次性 mock：回 200。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for _ in 0..3 {
                let Ok((mut stream, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp_body = r#"{"status":"ok"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {l}\r\n\r\n{b}",
                    l = resp_body.len(),
                    b = resp_body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        std::thread::scope(|s| {
            let handle = s.spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let result =
                    rt.block_on(async move { post_internal_shutdown(port, "probe-token") });
                drop(rt);
                result
            });
            let status = handle
                .join()
                .expect("生产模式绝不允许 panic 外溢（这是本模式的全部意义）")
                .expect("HTTP 往返应当成功");
            assert_eq!(status, 200, "探针 mock 固定回 200");
        });
    }
}
