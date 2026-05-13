//! Shutdown command - graceful shutdown of a running gateway.
//!
//! Uses PID file to locate the running gateway process and sends
//! a shutdown signal. The gateway writes its PID to
//! `{home}/gateway.pid` on startup.

use anyhow::Result;
use crate::common;

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
                            if stderr.contains("not found") || stderr.contains("not found") {
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

    // Method 2: Try named pipe / signal file
    let signal_path = home.join("shutdown.signal");
    std::fs::write(&signal_path, chrono::Utc::now().to_rfc3339())?;
    println!("  Shutdown signal file written: {}", signal_path.display());

    // Method 3: Try HTTP endpoint
    if let Ok(data) = std::fs::read_to_string(common::config_path(&home)) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            let port = cfg.get("channels")
                .and_then(|c| c.get("web"))
                .and_then(|w| w.get("port"))
                .and_then(|v| v.as_u64())
                .unwrap_or(8080);
            let url = format!("http://127.0.0.1:{}/api/shutdown", port);
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build();
            if let Ok(client) = client {
                match client.post(&url).send() {
                    Ok(resp) if resp.status().is_success() => {
                        println!("  Shutdown signal sent via HTTP API.");
                        let _ = std::fs::remove_file(&signal_path);
                        return Ok(());
                    }
                    Ok(resp) => {
                        println!("  Gateway responded with status: {}", resp.status());
                    }
                    Err(_) => {
                        println!("  Gateway HTTP API not reachable at port {}.", port);
                    }
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
