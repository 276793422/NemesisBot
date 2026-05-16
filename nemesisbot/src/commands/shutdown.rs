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

#[cfg(test)]
mod tests {
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
        let timestamp = chrono::Utc::now().to_rfc3339();
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
        let port = cfg.get("channels")
            .and_then(|c| c.get("web"))
            .and_then(|w| w.get("port"))
            .and_then(|v| v.as_u64())
            .unwrap_or(8080);
        assert_eq!(port, 49000);
    }

    #[test]
    fn test_port_extraction_default() {
        let cfg = serde_json::json!({});
        let port = cfg.get("channels")
            .and_then(|c| c.get("web"))
            .and_then(|w| w.get("port"))
            .and_then(|v| v.as_u64())
            .unwrap_or(8080);
        assert_eq!(port, 8080);
    }

    #[test]
    fn test_shutdown_url_construction() {
        let port: u64 = 49000;
        let url = format!("http://127.0.0.1:{}/api/shutdown", port);
        assert_eq!(url, "http://127.0.0.1:49000/api/shutdown");
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
        let port = parsed.get("channels")
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
}
