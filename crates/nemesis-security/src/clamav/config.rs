//! ClamAV configuration file generation (clamd.conf and freshclam.conf).

use std::fs;
use std::path::Path;

/// Daemon configuration.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub clamav_path: String,
    pub config_file: String,
    pub database_dir: String,
    pub listen_addr: String,
    pub temp_dir: String,
    pub startup_timeout_secs: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            clamav_path: String::new(),
            config_file: String::new(),
            database_dir: String::new(),
            listen_addr: "127.0.0.1:3310".to_string(),
            temp_dir: String::new(),
            startup_timeout_secs: 120,
        }
    }
}

/// Generate a minimal clamd.conf for TCP mode.
pub fn generate_clamd_config(cfg: &DaemonConfig) -> Result<(), String> {
    if cfg.config_file.is_empty() {
        return Err("config file path is required".to_string());
    }

    if let Some(parent) = Path::new(&cfg.config_file).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create config dir: {}", e))?;
    }

    let mut lines = vec![
        "# Auto-generated clamd.conf for NemesisBot".to_string(),
    ];

    // Parse listen address
    let (host, port) = if cfg.listen_addr.is_empty() {
        ("127.0.0.1".to_string(), "3310".to_string())
    } else {
        let parts: Vec<&str> = cfg.listen_addr.splitn(2, ':').collect();
        if parts.len() == 2 {
            (parts[0].to_string(), parts[1].to_string())
        } else {
            (cfg.listen_addr.clone(), "3310".to_string())
        }
    };

    lines.push(format!("TCPSocket {}", port));
    lines.push(format!("TCPAddr {}", host));
    lines.push(String::new());

    if !cfg.database_dir.is_empty() {
        lines.push(format!("DatabaseDirectory {}", cfg.database_dir.replace('\\', "/")));
    }

    if !cfg.temp_dir.is_empty() {
        lines.push(format!("TemporaryDirectory {}", cfg.temp_dir.replace('\\', "/")));
    }

    lines.extend([
        String::new(),
        "# Logging".to_string(),
        "LogTime yes".to_string(),
        "LogRotate yes".to_string(),
        "LogFileMaxSize 10M".to_string(),
        String::new(),
        "# Scan options".to_string(),
        "ScanPE yes".to_string(),
        "ScanELF yes".to_string(),
        "ScanOLE2 yes".to_string(),
        "ScanPDF yes".to_string(),
        "ScanSWF yes".to_string(),
        "ScanXMLDOCS yes".to_string(),
        "ScanHWP3 yes".to_string(),
        "ScanMail yes".to_string(),
        "ScanArchive yes".to_string(),
        "MaxScanSize 100M".to_string(),
        "MaxFileSize 50M".to_string(),
    ]);

    if cfg!(target_os = "windows") {
        lines.extend([
            String::new(),
            "# Windows-specific".to_string(),
            "FollowDirectorySymlinks no".to_string(),
            "FollowFileSymlinks no".to_string(),
        ]);
    }

    let content = lines.join("\n") + "\n";
    fs::write(&cfg.config_file, content).map_err(|e| format!("write clamd.conf: {}", e))?;

    Ok(())
}

/// Generate a minimal freshclam.conf.
pub fn generate_freshclam_config(db_dir: &str, config_file: &str) -> Result<(), String> {
    if config_file.is_empty() {
        return Err("config file path is required".to_string());
    }

    if let Some(parent) = Path::new(config_file).parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create config dir: {}", e))?;
    }

    let mut lines = vec![
        "# Auto-generated freshclam.conf for NemesisBot".to_string(),
    ];

    if !db_dir.is_empty() {
        lines.push(format!("DatabaseDirectory {}", db_dir.replace('\\', "/")));
        fs::create_dir_all(db_dir).map_err(|e| format!("create db dir: {}", e))?;
    }

    lines.extend([
        String::new(),
        "# Database mirror (ClamAV official)".to_string(),
        "DatabaseMirror database.clamav.net".to_string(),
        String::new(),
        "# Update settings".to_string(),
        "Checks 24".to_string(),
        "LogTime yes".to_string(),
        "LogRotate yes".to_string(),
    ]);

    let content = lines.join("\n") + "\n";
    fs::write(config_file, content).map_err(|e| format!("write freshclam.conf: {}", e))?;

    Ok(())
}

/// Detect ClamAV installation path.
pub fn detect_clamav_path() -> Option<String> {
    let candidates: Vec<&str> = if cfg!(target_os = "windows") {
        vec!["C:\\Program Files\\ClamAV", "C:\\ClamAV"]
    } else if cfg!(target_os = "macos") {
        vec!["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin"]
    } else {
        vec!["/usr/bin", "/usr/local/bin", "/usr/sbin"]
    };

    let exe_name = if cfg!(target_os = "windows") {
        "clamd.exe"
    } else {
        "clamd"
    };

    for dir in &candidates {
        let exe_path = Path::new(dir).join(exe_name);
        if exe_path.exists() {
            return Some(dir.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_clamd_config() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            database_dir: dir.path().join("db").to_string_lossy().to_string(),
            listen_addr: "127.0.0.1:3310".to_string(),
            temp_dir: dir.path().join("tmp").to_string_lossy().to_string(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        assert!(content.contains("TCPSocket 3310"));
        assert!(content.contains("ScanPE yes"));
    }

    #[test]
    fn test_generate_freshclam_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("freshclam.conf").to_string_lossy().to_string();
        let db_dir = dir.path().join("db").to_string_lossy().to_string();
        generate_freshclam_config(&db_dir, &config_file).unwrap();
        let content = fs::read_to_string(&config_file).unwrap();
        assert!(content.contains("DatabaseMirror database.clamav.net"));
    }

    #[test]
    fn test_empty_config_path_fails() {
        let cfg = DaemonConfig::default();
        assert!(generate_clamd_config(&cfg).is_err());
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
    fn test_generate_clamd_config_custom_port() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            listen_addr: "0.0.0.0:9999".to_string(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        assert!(content.contains("TCPSocket 9999"));
        assert!(content.contains("TCPAddr 0.0.0.0"));
    }

    #[test]
    fn test_generate_clamd_config_includes_scan_options() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            listen_addr: "127.0.0.1:3310".to_string(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        assert!(content.contains("ScanPE yes"));
        assert!(content.contains("ScanELF yes"));
        assert!(content.contains("ScanArchive yes"));
        assert!(content.contains("MaxScanSize 100M"));
        assert!(content.contains("MaxFileSize 50M"));
    }

    #[test]
    fn test_generate_freshclam_config_creates_db_dir() {
        let dir = tempfile::tempdir().unwrap();
        let db_dir = dir.path().join("database");
        let config_file = dir.path().join("freshclam.conf").to_string_lossy().to_string();
        generate_freshclam_config(&db_dir.to_string_lossy(), &config_file).unwrap();
        assert!(db_dir.exists());
        let content = fs::read_to_string(&config_file).unwrap();
        assert!(content.contains("DatabaseMirror database.clamav.net"));
        assert!(content.contains("Checks 24"));
    }

    #[test]
    fn test_generate_freshclam_config_empty_path_fails() {
        let result = generate_freshclam_config("/tmp", "");
        assert!(result.is_err());
    }

    #[test]
    fn test_detect_clamav_path_returns_none_or_some() {
        // Just verify it doesn't panic; actual result depends on environment
        let _ = detect_clamav_path();
    }

    #[test]
    fn test_generate_clamd_config_with_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            database_dir: dir.path().join("db").to_string_lossy().to_string(),
            temp_dir: dir.path().join("tmp").to_string_lossy().to_string(),
            listen_addr: "127.0.0.1:3310".to_string(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        assert!(content.contains("DatabaseDirectory"));
        assert!(content.contains("TemporaryDirectory"));
    }

    #[test]
    fn test_generate_clamd_config_empty_listen_addr() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            listen_addr: String::new(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        // Should use default 127.0.0.1:3310
        assert!(content.contains("TCPSocket 3310"));
        assert!(content.contains("TCPAddr 127.0.0.1"));
    }

    #[test]
    fn test_generate_clamd_config_no_colon_in_addr() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            listen_addr: "onlyhost".to_string(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        assert!(content.contains("TCPAddr onlyhost"));
        assert!(content.contains("TCPSocket 3310")); // default port
    }

    #[test]
    fn test_generate_clamd_config_empty_db_and_temp() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DaemonConfig {
            config_file: dir.path().join("clamd.conf").to_string_lossy().to_string(),
            database_dir: String::new(),
            temp_dir: String::new(),
            listen_addr: "127.0.0.1:3310".to_string(),
            ..Default::default()
        };
        generate_clamd_config(&cfg).unwrap();
        let content = fs::read_to_string(&cfg.config_file).unwrap();
        assert!(!content.contains("DatabaseDirectory"));
        assert!(!content.contains("TemporaryDirectory"));
    }

    #[test]
    fn test_generate_freshclam_config_empty_db_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config_file = dir.path().join("freshclam.conf").to_string_lossy().to_string();
        generate_freshclam_config("", &config_file).unwrap();
        let content = fs::read_to_string(&config_file).unwrap();
        assert!(content.contains("DatabaseMirror"));
        // No DatabaseDirectory line
        assert!(!content.contains("DatabaseDirectory"));
    }

    #[test]
    fn test_generate_clamd_config_invalid_path() {
        let cfg = DaemonConfig {
            config_file: "/nonexistent/deep/path/config.conf".to_string(),
            listen_addr: "127.0.0.1:3310".to_string(),
            ..Default::default()
        };
        // On Windows this might succeed if the path is valid
        let result = generate_clamd_config(&cfg);
        // Result depends on filesystem permissions
        let _ = result;
    }
}
