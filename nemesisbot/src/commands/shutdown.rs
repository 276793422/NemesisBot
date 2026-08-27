//! Shutdown command - graceful shutdown of a running gateway.
//!
//! Uses PID file to locate the running gateway process and sends
//! a shutdown signal. The gateway writes its PID to
//! `{home}/gateway.pid` on startup.

use crate::common;
use anyhow::Result;

/// Name of the PID file written by the gateway on startup.
const PID_FILE: &str = "gateway.pid";

pub fn run(local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let pid_path = home.join(PID_FILE);

    println!("Sending shutdown signal...");

    // Method 1: Try PID file
    if pid_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = data.trim().parse::<u32>() {
                println!("  Found gateway PID: {}", pid);

                // Send SIGTERM on Unix, or use taskkill on Windows
                #[cfg(target_os = "windows")]
                {
                    // On Windows, send CTRL_BREAK_EVENT or use taskkill
                    let result = std::process::Command::new("taskkill")
                        .args(["/PID", &pid.to_string()])
                        .output();
                    match result {
                        Ok(output) if output.status.success() => {
                            println!("  Shutdown signal sent to PID {}.", pid);
                            // Clean up PID file
                            let _ = std::fs::remove_file(&pid_path);
                        }
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            if stderr.contains("not found") {
                                println!("  Process {} is not running.", pid);
                                let _ = std::fs::remove_file(&pid_path);
                            } else {
                                println!("  Failed to signal process: {}", stderr.trim());
                            }
                        }
                        Err(e) => println!("  Failed to send signal: {}", e),
                    }
                }

                #[cfg(not(target_os = "windows"))]
                {
                    // On Unix, send SIGTERM
                    unsafe {
                        if libc::kill(pid as i32, libc::SIGTERM) == 0 {
                            println!("  SIGTERM sent to PID {}.", pid);
                            let _ = std::fs::remove_file(&pid_path);
                        } else {
                            println!("  Failed to signal process {} (may not be running).", pid);
                            let _ = std::fs::remove_file(&pid_path);
                        }
                    }
                }

                return Ok(());
            }
        }
    }

    // (BUG #31, quality-hardening goal 冲刺 S11e) legacy 无消费方：全仓库没有
    // 任何代码读取 shutdown.signal 文件（PID 臂与 HTTP 臂是仅有的两条活路径）。
    // 仅为外部脚本兼容保留写入；成功走 HTTP 后仍会清理。
    let signal_path = home.join("shutdown.signal");
    std::fs::write(&signal_path, chrono::Local::now().to_rfc3339())?;
    println!("  Shutdown signal file written: {}", signal_path.display());

    // (BUG #31, quality-hardening goal 冲刺 S11e) Method 3 重写：旧实现打
    // `/api/shutdown` —— 该路由在 nemesis-web 从未注册，必 404，是一条死臂。
    // 现在打既有内部控制端点 `POST /api/internal` body {"cmd":"shutdown"}
    // （gateway 经 InternalCommand mpsc 走 Ctrl+C 同源的优雅停机），鉴权用
    // config.json 的 channels.web.auth_token（未配置则不发 header——服务端
    // 空 token 可过的既有约定不变）。
    if let Ok(data) = std::fs::read_to_string(common::config_path(&home)) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            let port = cfg
                .get("channels")
                .and_then(|c| c.get("web"))
                .and_then(|w| w.get("port"))
                .and_then(|v| v.as_u64())
                .unwrap_or(8080);
            let token = cfg
                .pointer("/channels/web/auth_token")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            match post_internal_shutdown(port as u16, &token) {
                Ok(200) => {
                    println!("  Shutdown requested via HTTP API (/api/internal).");
                    let _ = std::fs::remove_file(&signal_path);
                    return Ok(());
                }
                Ok(401) => {
                    // Token 配置不一致：gateway 的 web.auth_token 与本地 config
                    // 不匹配。如实提示，与原实现一致地继续走收尾提示。
                    println!("  Gateway rejected shutdown (HTTP 401): auth_token mismatch.");
                }
                Ok(status) => {
                    println!("  Gateway responded with status: {}", status);
                }
                Err(err) => {
                    println!("  Gateway HTTP API not reachable at port {}: {}", port, err);
                }
            }
        }
    }

    println!();
    println!("  Could not reach a running gateway.");
    println!("  The gateway will complete in-progress operations before stopping.");
    println!("  Make sure the gateway is running: nemesisbot gateway");
    Ok(())
}

/// Fire-and-forget POST `{ "cmd": "shutdown" }` to `/api/internal`.
///
/// Returns `Ok(http_status)` on a completed round-trip (the CLI inspects
/// success/non-success), `Err(message)` for transport failure.
///
/// (BUG #31 / BUG #27 同类横向, quality-hardening goal 冲刺 S11e；机制勘误
/// 见 BUG #50, 2026-08-28) 为什么是「独立线程 + 私有 runtime + 【异步】client」
/// 而不是 `reqwest::blocking`：本命令以同步签名被 main.rs 的 async
/// `run_command` 直接调用（本批次不允许改 main.rs 把它 await 化），而
/// `run_command` 运行在 multi-thread runtime 的 block_on 上下文里。
/// `reqwest::blocking::Client` 在该上下文的问题分两层：
///   1. debug 构建：new() 内部 wait::enter() 的嵌套-runtime 守卫（reqwest
///      0.12.28 src/blocking/wait.rs，**带 `#[cfg(debug_assertions)]`**）会让
///      守卫 shell runtime 在 async 上下文内建+弃 → tokio panic「Cannot drop
///      a runtime in a context where blocking is not allowed」（#27 实证）。
///   2. release 构建：守卫被编译掉、不 panic——但 new()/drop 会 park/join
///      一个 worker 线程，满载下可把 multi-thread runtime 饿死（同样不可用，
///      只是死法不同）。
/// 这里把整段 HTTP 调用搬到一条全新 OS 线程上执行：新线程没有任何 ambient
/// runtime context，私有 runtime 的创建与销毁都合法；且全程使用异步 client，
/// 两种 profile 下都安全。
/// 探针测试见 shutdown/tests.rs（红对照按 profile 断言，见 BUG #50）。
fn post_internal_shutdown(port: u16, token: &str) -> Result<u16, String> {
    let url = format!("http://127.0.0.1:{}/api/internal", port);
    let token = token.to_string();

    // 手工设 Content-Type/body，不依赖 reqwest 的 .json() 序列化路径，
    // 保持显式可控的请求形状。
    let body = serde_json::to_vec(&serde_json::json!({ "cmd": "shutdown" }))
        .map_err(|e| format!("serialize body: {e}"))?;

    let handle = std::thread::Builder::new()
        .name("shutdown-http".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .map_err(|e| format!("runtime: {e}"))?;
            rt.block_on(async move {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .map_err(|e| format!("http client: {e}"))?;
                let mut req = client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .body(body);
                // 未配置 token（空）→ 不发 header；服务端对空 auth_token 放行。
                if !token.is_empty() {
                    req = req.header("X-Auth-Token", token);
                }
                let resp = req.send().await.map_err(|e| format!("send: {e}"))?;
                Ok::<u16, String>(resp.status().as_u16())
            })
        })
        .map_err(|e| format!("spawn thread: {e}"))?;

    match handle.join() {
        Ok(result) => result,
        Err(_) => Err("http worker panicked".to_string()),
    }
}

#[cfg(test)]
mod tests;
