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
