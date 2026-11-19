use super::*;
use tempfile::TempDir;

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
    // (, quality-hardening goal 冲刺) Method 3 改打既有内部控制
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
// run() 全臂（建立基础； 随 重写更新到 /api/internal 端点）
// —— 既有 10 个测试只钉常量/解析片段，run() 本体从没跑过。run() 是同步 fn，
// 全部走 env home 隔离 + 本地 mock（127.0.0.1:0）。
//
// 变更：
// - Method 3 打 POST /api/internal {"cmd":"shutdown"}（原 /api/shutdown 死臂）
//   并断言 body / X-Auth-Token header；
// - 补 token 缺失分支（config 无 auth_token → 不发 header）；
// - 两个 panic 探针：#27 同款拓扑红对照 + 生产新写法绿证明（见 probe 段）。
// - (BUG-20261114-025) PID 臂整臂移除：原 PID 文件解析/taskkill/SIGTERM
//   测试组随之删除；改造出「残留 gateway.pid 被忽略」回归锁（死臂不再读它，
//   也不清理它——现场保留）。
// ===========================================================================

// 整 mod Windows 形态（测试 + 专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
mod run_arm {
    use super::*;

    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

    fn start_mock(
        status: &'static str,
        body: &'static str,
    ) -> (u16, std::sync::mpsc::Receiver<MockRecord>) {
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
                    if let Some((k, v)) = line.split_once(':')
                        && k.trim().eq_ignore_ascii_case("x-auth-token")
                    {
                        auth_token = Some(v.trim().to_string());
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

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
    #[test]
    fn run_without_pid_and_config_writes_signal_and_reports_unreachable() {
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            run(false).expect("无 config → 信号文件 + Could not reach + Ok");
            assert!(home.join("shutdown.signal").exists(), "信号文件必须落盘");
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
    #[test]
    fn run_ignores_stale_pid_file_and_still_goes_http() {
        // (BUG-20261114-025 回归锁) 残留 gateway.pid（任何内容形态）必须被
        // 完全忽略：既不触发任何进程信号假成功，也不清理现场——命令照走
        // HTTP /api/internal 优雅停机臂。修复前该文件会把命令带进 taskkill
        // 假成功分支（报「sent」进程不停 + 删 PID 文件湮灭现场）。
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("gateway.pid"), "not-a-number\n").unwrap();
            let (port, rx) = start_mock("200 OK", r#"{"status":"ok"}"#);
            std::fs::write(
                home.join("config.json"),
                serde_json::json!({"channels": {"web": {"port": port}}}).to_string(),
            )
            .unwrap();
            run(false).expect("PID 文件在场 → 忽略，走 HTTP 臂 + Ok");
            assert!(
                home.join("gateway.pid").exists(),
                "死臂已移除：残留 PID 文件不得被读取方清理（现场保留）"
            );
            assert!(!home.join("shutdown.signal").exists());
            let rec = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("mock 必须收到请求");
            assert!(
                rec.request_line.starts_with("POST /api/internal "),
                "PID 文件在场也必须走 HTTP 优雅停机臂，实际: {}",
                rec.request_line
            );
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
    #[test]
    fn run_pid_file_as_directory_is_ignored_no_side_effect() {
        // (BUG-20261114-025 回归锁) gateway.pid 是【目录】的极端形态：
        // 同样必须被忽略（无读取、无清理、无进程信号），照常落 signal 文件。
        with_env_home(|home| {
            std::fs::create_dir_all(home.join("gateway.pid")).unwrap();
            run(false).expect("pid 目录形态 → 忽略 + Ok");
            assert!(home.join("shutdown.signal").exists());
            assert!(
                home.join("gateway.pid").is_dir(),
                "死臂已移除：目录形态现场保留"
            );
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
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
            let rec = rx
                .recv_timeout(std::time::Duration::from_secs(2))
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

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
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
            let rec = rx
                .recv_timeout(std::time::Duration::from_secs(2))
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

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
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
            let rec = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("mock 必须收到请求");
            assert!(rec.request_line.starts_with("POST /api/internal "));
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
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

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
    #[test]
    fn run_http_401_reports_auth_mismatch_and_keeps_signal_file() {
        // （coverage-95 goal）Ok(401) 臂：token 配置不一致时如实提示，
        // 且不走清理路径（signal 文件保留，供人工排查）。
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            let (port, rx) = start_mock("401 Unauthorized", r#"{"error":"unauthorized"}"#);
            std::fs::write(
                home.join("config.json"),
                serde_json::json!({
                    "channels": {"web": {"port": port, "auth_token": "tok-mismatch"}}
                })
                .to_string(),
            )
            .unwrap();
            run(false).expect("HTTP 401 → 打印 mismatch 提示 + Ok");
            assert!(
                home.join("shutdown.signal").exists(),
                "401 不算送达 → signal 文件保留"
            );
            let rec = rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("mock 必须收到请求");
            assert_eq!(rec.auth_token.as_deref(), Some("tok-mismatch"));
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, sweep)
    #[test]
    fn run_corrupt_config_json_skips_http_block_and_keeps_signal() {
        // config.json 存在但 JSON 损坏：serde 解析失败 → 跳过整个 HTTP 块
        //（隐式 else 边），落到收尾提示；信号文件必须已落盘。
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("config.json"), "{ broken json").unwrap();
            run(false).expect("损坏 config → 跳过 HTTP 块 + Ok");
            assert!(home.join("shutdown.signal").exists());
        });
    }
}

// ===========================================================================
// panic 探针（同类横向 /, quality-hardening goal 冲刺）
//
// 生产修复模式（本批首次引入）：「独立 OS 线程 + 私有 multi-thread runtime +
// 异步 client」——新线程没有 ambient runtime context，runtime 创建/销毁合法，
// 且全程不存在嵌套 runtime 的 blocking client。
// 每个【首次使用】的模式必须有 catch_unwind 探针实证（#27 的探针拓扑纪律）：
//   红①：multi_thread rt + block_on 内创建并 drop reqwest::blocking:Client
//         → 按 profile 断言（debug 必 panic / release 守卫编译掉不 panic，
//           勘误：reqwest enter() 守卫带 cfg(debug_assertions)）。
//   绿②：同样的 async 上下文里调用 post_internal_shutdown（内部走独立线程）
//         → 绝不 panic 且往返成功。
// ===========================================================================

mod probes {
    use super::*;

    /// 红①对照探针：在 multi_thread runtime 的 block_on 未来体里创建并 drop
    /// reqwest::blocking:Client（忠实复刻 main.rs 非 gateway 命令的拓扑：
    /// rt.block_on(run_command) 里同步代码创建/丢弃 blocking client）。
    /// 契约是 **profile 依赖**的（勘误）——reqwest 0.12.28
    /// src/blocking/wait.rs 的 `enter()` 嵌套-runtime 守卫带
    /// `#[cfg(debug_assertions)]`（workspace 无 package 覆盖，两侧同 profile）：
    ///   - debug：守卫在编译 → new() 内 shell runtime 在 async 上下文建+弃 →
    ///     tokio panic「Cannot drop a runtime in a context where blocking is
    ///     not allowed」→ block_on 必须 Err（#27 当年实证的就是这条）。
    ///   - release：守卫被编译掉 → new/drop 都不 panic（drop 只是 join 后台
    ///     线程，后台线程上的 runtime 弃置合法）——但会 park/join 一个 worker
    ///     线程（满载饿死隐患，生产注释第 2 层理由）。此前 release 全量跑的
    ///     偶发"通过"= 负载下 new() 资源失败的另一种 panic（假红），非本守卫。
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
                // guard——这正是 debug 守卫要拦的上下文。
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    rt.block_on(async {
                        let client = reqwest::blocking::Client::new();
                        drop(client);
                    })
                }));
                drop(rt); // block_on 已返回，无 enter guard，drop 合法
                outcome
            });
            let inner = handle.join().expect("探针宿主线程自身不能炸");
            if cfg!(debug_assertions) {
                assert!(
                    inner.is_err(),
                    "debug 构建：blocking client 在 block_on 上下文创建/drop 必 panic（reqwest enter() 守卫）；若不再 panic 说明 reqwest/tokio 行为变了，生产注释与本测试都要更新"
                );
            } else {
                assert!(
                    inner.is_ok(),
                    "release 构建：enter() 守卫被 cfg(debug_assertions) 编译掉，new/drop 都不应 panic（worker park 是隐患不是 panic 源）；若 panic 了说明 reqwest/tokio 行为变了（或环境资源失败），生产注释与本测试都要更新"
                );
            }
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
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
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
