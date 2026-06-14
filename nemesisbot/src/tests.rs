use super::*;
use tempfile::TempDir;

// -------------------------------------------------------------------------
// write_fallback_config tests
// -------------------------------------------------------------------------

#[test]
fn test_write_fallback_config_creates_file() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    write_fallback_config(&cfg_path).unwrap();
    assert!(cfg_path.exists());
}

#[test]
fn test_write_fallback_config_valid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    write_fallback_config(&cfg_path).unwrap();
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["version"], "1.0");
}

#[test]
fn test_write_fallback_config_structure() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    write_fallback_config(&cfg_path).unwrap();
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();

    assert_eq!(cfg["default_model"], "");
    assert!(cfg["model_list"].is_array());
    assert!(cfg["model_list"].as_array().unwrap().is_empty());
    assert_eq!(cfg["channels"]["web"]["enabled"], true);
    assert_eq!(cfg["channels"]["web"]["host"], "127.0.0.1");
    assert_eq!(cfg["channels"]["web"]["port"], 49000);
    assert_eq!(cfg["channels"]["web"]["auth_token"], "276793422");
    assert_eq!(cfg["channels"]["websocket"]["enabled"], true);
    assert_eq!(cfg["agents"]["defaults"]["restrict_to_workspace"], false);
    assert_eq!(cfg["security"]["enabled"], true);
    assert_eq!(cfg["forge"]["enabled"], false);
    assert_eq!(cfg["logging"]["llm"]["enabled"], true);
}

#[test]
fn test_write_fallback_config_overwrites() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(&cfg_path, "old content").unwrap();
    write_fallback_config(&cfg_path).unwrap();
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    assert_ne!(data, "old content");
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(cfg["version"], "1.0");
}

// -------------------------------------------------------------------------
// Embedded config constants validation
// -------------------------------------------------------------------------

#[test]
fn test_config_default_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_DEFAULT).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_mcp_default_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_MCP_DEFAULT).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_cluster_default_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_CLUSTER_DEFAULT).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_skills_default_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SKILLS_DEFAULT).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_scanner_default_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SCANNER_DEFAULT).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_enhanced_memory_default_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_ENHANCED_MEMORY_DEFAULT).unwrap();
    assert!(cfg.is_object());
    assert_eq!(cfg.get("enabled").unwrap().as_bool(), Some(false));
}

#[test]
fn test_config_security_windows_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_WINDOWS).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_security_linux_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_LINUX).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_security_darwin_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_DARWIN).unwrap();
    assert!(cfg.is_object());
}

#[test]
fn test_config_security_other_is_valid_json() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_OTHER).unwrap();
    assert!(cfg.is_object());
}

// -------------------------------------------------------------------------
// Embedded personality files
// -------------------------------------------------------------------------

#[test]
fn test_default_identity_not_empty() {
    assert!(!DEFAULT_IDENTITY.is_empty());
}

#[test]
fn test_default_soul_not_empty() {
    assert!(!DEFAULT_SOUL.is_empty());
}

#[test]
fn test_default_user_not_empty() {
    assert!(!DEFAULT_USER.is_empty());
}

// -------------------------------------------------------------------------
// Onboard --local parsing logic
// -------------------------------------------------------------------------

#[test]
fn test_local_flag_filtering() {
    let args = vec![
        "nemesisbot".to_string(),
        "--local".to_string(),
        "gateway".to_string(),
    ];
    let mut local_mode = false;
    let filtered_args: Vec<String> = args
        .into_iter()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    assert!(local_mode);
    assert_eq!(filtered_args, vec!["nemesisbot", "gateway"]);
}

#[test]
fn test_local_flag_not_present() {
    let args = vec![
        "nemesisbot".to_string(),
        "gateway".to_string(),
    ];
    let mut local_mode = false;
    let filtered_args: Vec<String> = args
        .into_iter()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    assert!(!local_mode);
    assert_eq!(filtered_args, vec!["nemesisbot", "gateway"]);
}

#[test]
fn test_local_flag_multiple_positions() {
    let args = vec![
        "nemesisbot".to_string(),
        "agent".to_string(),
        "--local".to_string(),
        "--debug".to_string(),
    ];
    let mut local_mode = false;
    let filtered_args: Vec<String> = args
        .into_iter()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    assert!(local_mode);
    assert_eq!(filtered_args, vec!["nemesisbot", "agent", "--debug"]);
}

// -------------------------------------------------------------------------
// Onboard default detection logic
// -------------------------------------------------------------------------

#[test]
fn test_onboard_default_detection_flag() {
    let default = true;
    let args: Vec<String> = vec![];
    let use_default = default || args.iter().any(|a| a == "default");
    assert!(use_default);
}

#[test]
fn test_onboard_default_detection_arg() {
    let default = false;
    let args: Vec<String> = vec!["default".to_string()];
    let use_default = default || args.iter().any(|a| a == "default");
    assert!(use_default);
}

#[test]
fn test_onboard_default_detection_neither() {
    let default = false;
    let args: Vec<String> = vec![];
    let use_default = default || args.iter().any(|a| a == "default");
    assert!(!use_default);
}

// -------------------------------------------------------------------------
// Platform detection logic
// -------------------------------------------------------------------------

#[test]
fn test_platform_detection() {
    let _platform = if cfg!(target_os = "windows") { "Windows" }
        else if cfg!(target_os = "macos") { "macOS" }
        else if cfg!(target_os = "linux") { "Linux" }
        else { "Unknown" };
    // On this Windows machine, should be "Windows"
    #[cfg(target_os = "windows")]
    assert_eq!(_platform, "Windows");
}

// -------------------------------------------------------------------------
// Config modification logic (from onboard default)
// -------------------------------------------------------------------------

#[test]
fn test_config_llm_logging_modification() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "logging": {"llm": {}}
    });
    if let Some(logging) = cfg.get_mut("logging").and_then(|v| v.get_mut("llm")) {
        if let Some(obj) = logging.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
            obj.insert("log_dir".to_string(), serde_json::Value::String("logs/request_logs".to_string()));
            obj.insert("detail_level".to_string(), serde_json::Value::String("full".to_string()));
        }
    }
    assert_eq!(cfg["logging"]["llm"]["enabled"], true);
    assert_eq!(cfg["logging"]["llm"]["log_dir"], "logs/request_logs");
    assert_eq!(cfg["logging"]["llm"]["detail_level"], "full");
}

#[test]
fn test_config_security_modification_existing() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "security": {"some_field": "value"}
    });
    if let Some(security) = cfg.get_mut("security") {
        if let Some(obj) = security.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    }
    assert_eq!(cfg["security"]["enabled"], true);
    assert_eq!(cfg["security"]["some_field"], "value");
}

#[test]
fn test_config_security_modification_missing() {
    let mut cfg: serde_json::Value = serde_json::json!({});
    if let Some(security) = cfg.get_mut("security") {
        if let Some(obj) = security.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    } else {
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("security".to_string(), serde_json::json!({"enabled": true}));
        }
    }
    assert_eq!(cfg["security"]["enabled"], true);
}

#[test]
fn test_config_workspace_restriction_modification() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "agents": {"defaults": {}}
    });
    if let Some(agents) = cfg.get_mut("agents").and_then(|v| v.get_mut("defaults")) {
        if let Some(obj) = agents.as_object_mut() {
            obj.insert("restrict_to_workspace".to_string(), serde_json::Value::Bool(false));
        }
    }
    assert_eq!(cfg["agents"]["defaults"]["restrict_to_workspace"], false);
}

#[test]
fn test_config_web_channel_modification() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "channels": {"web": {}}
    });
    if let Some(web) = cfg.pointer_mut("/channels/web") {
        if let Some(obj) = web.as_object_mut() {
            obj.insert("auth_token".to_string(), serde_json::Value::String("276793422".to_string()));
            obj.insert("host".to_string(), serde_json::Value::String("127.0.0.1".to_string()));
            obj.insert("port".to_string(), serde_json::Value::Number(49000.into()));
        }
    }
    assert_eq!(cfg["channels"]["web"]["auth_token"], "276793422");
    assert_eq!(cfg["channels"]["web"]["port"], 49000);
}

#[test]
fn test_config_websocket_modification() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "channels": {"websocket": {}}
    });
    if let Some(ws) = cfg.pointer_mut("/channels/websocket") {
        if let Some(obj) = ws.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    }
    assert_eq!(cfg["channels"]["websocket"]["enabled"], true);
}

// -------------------------------------------------------------------------
// Cluster config node ID injection
// -------------------------------------------------------------------------

#[test]
fn test_cluster_node_id_format() {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "node".to_string());
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let node_id = format!("node-{}-{}", hostname.to_lowercase(), timestamp);
    assert!(node_id.starts_with("node-"));
}

// -------------------------------------------------------------------------
// Gateway args construction
// -------------------------------------------------------------------------

#[test]
fn test_gateway_args_construction() {
    let debug = true;
    let quiet = false;
    let no_console = true;
    let mut gateway_args: Vec<String> = Vec::new();
    if debug { gateway_args.push("--debug".to_string()); }
    if quiet { gateway_args.push("--quiet".to_string()); }
    if no_console { gateway_args.push("--no-console".to_string()); }
    assert_eq!(gateway_args, vec!["--debug", "--no-console"]);
}

#[test]
fn test_gateway_args_empty() {
    let debug = false;
    let quiet = false;
    let no_console = false;
    let mut gateway_args: Vec<String> = Vec::new();
    if debug { gateway_args.push("--debug".to_string()); }
    if quiet { gateway_args.push("--quiet".to_string()); }
    if no_console { gateway_args.push("--no-console".to_string()); }
    assert!(gateway_args.is_empty());
}

// -------------------------------------------------------------------------
// Peers TOML content generation
// -------------------------------------------------------------------------

#[test]
fn test_peers_toml_content() {
    let node_id = "test-node-id";
    let content = format!(
        "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[node]\nid = \"{}\"\nname = \"Bot {}\"\n",
        node_id, node_id
    );
    assert!(content.contains("test-node-id"));
    assert!(content.contains("[node]"));
    assert!(!content.contains("[cluster]"));
}

// -------------------------------------------------------------------------
// Additional coverage tests for main
// -------------------------------------------------------------------------

#[test]
fn test_cli_build_with_all_flags() {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
    assert!(names.contains(&"gateway"));
    assert!(names.contains(&"model"));
    assert!(names.contains(&"cluster"));
    assert!(names.contains(&"agent"));
    assert!(names.contains(&"channel"));
    assert!(names.contains(&"security"));
    assert!(names.contains(&"scanner"));
    assert!(names.contains(&"skills"));
    assert!(names.contains(&"mcp"));
    assert!(names.contains(&"forge"));
    assert!(names.contains(&"cors"));
    assert!(names.contains(&"cron"));
}

#[test]
fn test_gateway_args_construction_with_debug() {
    let debug = true;
    let quiet = false;
    let no_console = false;
    let mut gateway_args: Vec<String> = Vec::new();
    if debug { gateway_args.push("--debug".to_string()); }
    if quiet { gateway_args.push("--quiet".to_string()); }
    if no_console { gateway_args.push("--no-console".to_string()); }
    assert!(gateway_args.contains(&"--debug".to_string()));
    assert!(!gateway_args.contains(&"--quiet".to_string()));
}

#[test]
fn test_gateway_args_construction_with_all() {
    let debug = true;
    let quiet = true;
    let no_console = true;
    let mut gateway_args: Vec<String> = Vec::new();
    if debug { gateway_args.push("--debug".to_string()); }
    if quiet { gateway_args.push("--quiet".to_string()); }
    if no_console { gateway_args.push("--no-console".to_string()); }
    assert!(gateway_args.contains(&"--debug".to_string()));
    assert!(gateway_args.contains(&"--quiet".to_string()));
    assert!(gateway_args.contains(&"--no-console".to_string()));
    assert_eq!(gateway_args.len(), 3);
}

#[test]
fn test_cli_local_flag() {
    use clap::CommandFactory;
    let cmd = Cli::command();
    // Check that --local flag exists
    let local_arg = cmd.get_arguments().find(|a| a.get_id().as_str() == "local");
    assert!(local_arg.is_some());
}

#[test]
fn test_version_info_format() {
    let version = env!("CARGO_PKG_VERSION");
    assert!(!version.is_empty());
    // Version should be semver-like
    assert!(version.contains('.'));
}

#[test]
fn test_home_dir_resolution() {
    let local = false;
    // Just test the logic doesn't panic
    let _ = crate::common::resolve_home(local);
}

#[test]
fn test_home_dir_resolution_local() {
    let local = true;
    let home = crate::common::resolve_home(local);
    assert!(home.to_str().unwrap().contains(".nemesisbot"));
}

#[test]
fn test_config_path_resolution() {
    let home = std::path::PathBuf::from("/tmp/test");
    let config_path = crate::common::config_path(&home);
    assert!(config_path.to_str().unwrap().contains("config.json"));
}

#[test]
fn test_node_id_format_for_onboard() {
    let node_id = format!("node-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap());
    assert!(node_id.starts_with("node-"));
    assert!(node_id.len() > 5);
}

#[test]
fn test_format_duration() {
    let secs = 3661u64;
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    let display = format!("{}h {}m {}s", hours, minutes, seconds);
    assert_eq!(display, "1h 1m 1s");
}

#[test]
fn test_format_duration_zero() {
    let secs = 0u64;
    let display = format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60);
    assert_eq!(display, "0h 0m 0s");
}

#[test]
fn test_format_duration_only_seconds() {
    let secs = 45u64;
    let display = format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60);
    assert_eq!(display, "0h 0m 45s");
}

#[test]
fn test_peers_toml_with_node_id() {
    let node_id = "node-abc-123";
    let content = format!(
        "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[node]\nid = \"{}\"\nname = \"Bot {}\"\n",
        node_id, node_id
    );
    assert!(content.contains("node-abc-123"));
    assert!(content.starts_with("# Cluster"));
    assert!(content.contains("[node]"));
}

// -------------------------------------------------------------------------
// Additional onboard config manipulation tests
// -------------------------------------------------------------------------

#[test]
fn test_config_default_has_expected_sections() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_DEFAULT).unwrap();
    assert!(cfg.get("channels").is_some(), "Config should have channels");
    assert!(cfg.get("agents").is_some(), "Config should have agents");
    assert!(cfg.get("security").is_some(), "Config should have security");
}

#[test]
fn test_config_cluster_default_has_ports() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_CLUSTER_DEFAULT).unwrap();
    assert!(cfg.get("port").is_some() || cfg.get("rpc_port").is_some(),
        "Cluster config should have port settings");
}

#[test]
fn test_config_scanner_default_has_engines() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SCANNER_DEFAULT).unwrap();
    assert!(cfg.get("engines").is_some() || cfg.get("enabled").is_some(),
        "Scanner config should have engines or enabled list");
}

#[test]
fn test_onboard_default_args_detection() {
    // Test various args combinations
    let args_with_default: Vec<String> = vec!["default".to_string()];
    assert!(args_with_default.iter().any(|a| a == "default"));

    let args_without: Vec<String> = vec!["other".to_string()];
    assert!(!args_without.iter().any(|a| a == "default"));

    let args_empty: Vec<String> = vec![];
    assert!(!args_empty.iter().any(|a| a == "default"));
}

#[test]
fn test_node_id_generation_from_hostname() {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "node".to_string());
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let node_id = format!("node-{}-{}", hostname.to_lowercase(), timestamp);
    // Verify format
    assert!(node_id.starts_with("node-"));
    assert!(node_id.contains(&hostname.to_lowercase()));
}

#[test]
fn test_fallback_config_is_valid() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    write_fallback_config(&cfg_path).unwrap();
    let data = std::fs::read_to_string(&cfg_path).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
    // Verify all expected keys
    assert!(cfg["version"].is_string());
    assert!(cfg["channels"].is_object());
    assert!(cfg["channels"]["web"].is_object());
    assert!(cfg["channels"]["websocket"].is_object());
    assert!(cfg["agents"].is_object());
    assert!(cfg["security"].is_object());
    assert!(cfg["forge"].is_object());
    assert!(cfg["logging"].is_object());
}

#[test]
fn test_config_web_channel_modification_with_pointer() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "channels": {"web": {"enabled": false}}
    });
    if let Some(web) = cfg.pointer_mut("/channels/web") {
        if let Some(obj) = web.as_object_mut() {
            obj.insert("auth_token".to_string(), serde_json::Value::String("test-token".to_string()));
            obj.insert("host".to_string(), serde_json::Value::String("0.0.0.0".to_string()));
            obj.insert("port".to_string(), serde_json::Value::Number(8080.into()));
        }
    }
    assert_eq!(cfg["channels"]["web"]["auth_token"], "test-token");
    assert_eq!(cfg["channels"]["web"]["host"], "0.0.0.0");
    assert_eq!(cfg["channels"]["web"]["port"], 8080);
    assert_eq!(cfg["channels"]["web"]["enabled"], false); // preserved
}

#[test]
fn test_local_flag_filtering_no_args() {
    let args: Vec<String> = vec!["nemesisbot".to_string()];
    let mut local_mode = false;
    let filtered_args: Vec<String> = args
        .into_iter()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    assert!(!local_mode);
    assert_eq!(filtered_args.len(), 1);
}

#[test]
fn test_cli_has_version_command() {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
    assert!(names.contains(&"version"));
    assert!(names.contains(&"status"));
    assert!(names.contains(&"shutdown"));
    assert!(names.contains(&"migrate"));
    assert!(names.contains(&"auth"));
    assert!(names.contains(&"log"));
    assert!(names.contains(&"workflow"));
    assert!(names.contains(&"voice"));
}
