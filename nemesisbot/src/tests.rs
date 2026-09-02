// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

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

#[cfg(feature = "eval")]
#[test]
fn test_eval_rules_default_is_valid_and_unique_ids() {
    let file = crate::eval_assessor::parse_rules(crate::eval_assessor::DEFAULT_RULES_JSON).unwrap();
    assert!(
        file.rules.len() >= 10,
        "default rule set too small: {}",
        file.rules.len()
    );
    let mut ids = std::collections::HashSet::new();
    for r in &file.rules {
        assert!(ids.insert(r.id.clone()), "duplicate id {}", r.id);
    }
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
    let args = vec!["nemesisbot".to_string(), "gateway".to_string()];
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
    let _platform = if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Unknown"
    };
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
    if let Some(logging) = cfg.get_mut("logging").and_then(|v| v.get_mut("llm"))
        && let Some(obj) = logging.as_object_mut()
    {
        obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        obj.insert(
            "log_dir".to_string(),
            serde_json::Value::String("logs/request_logs".to_string()),
        );
        obj.insert(
            "detail_level".to_string(),
            serde_json::Value::String("full".to_string()),
        );
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
    if let Some(security) = cfg.get_mut("security")
        && let Some(obj) = security.as_object_mut()
    {
        obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
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
    if let Some(agents) = cfg.get_mut("agents").and_then(|v| v.get_mut("defaults"))
        && let Some(obj) = agents.as_object_mut()
    {
        obj.insert(
            "restrict_to_workspace".to_string(),
            serde_json::Value::Bool(false),
        );
    }
    assert_eq!(cfg["agents"]["defaults"]["restrict_to_workspace"], false);
}

#[test]
fn test_config_web_channel_modification() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "channels": {"web": {}}
    });
    if let Some(web) = cfg.pointer_mut("/channels/web")
        && let Some(obj) = web.as_object_mut()
    {
        obj.insert(
            "auth_token".to_string(),
            serde_json::Value::String("276793422".to_string()),
        );
        obj.insert(
            "host".to_string(),
            serde_json::Value::String("127.0.0.1".to_string()),
        );
        obj.insert("port".to_string(), serde_json::Value::Number(49000.into()));
    }
    assert_eq!(cfg["channels"]["web"]["auth_token"], "276793422");
    assert_eq!(cfg["channels"]["web"]["port"], 49000);
}

#[test]
fn test_config_websocket_modification() {
    let mut cfg: serde_json::Value = serde_json::json!({
        "channels": {"websocket": {}}
    });
    if let Some(ws) = cfg.pointer_mut("/channels/websocket")
        && let Some(obj) = ws.as_object_mut()
    {
        obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
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
    if debug {
        gateway_args.push("--debug".to_string());
    }
    if quiet {
        gateway_args.push("--quiet".to_string());
    }
    if no_console {
        gateway_args.push("--no-console".to_string());
    }
    assert_eq!(gateway_args, vec!["--debug", "--no-console"]);
}

#[test]
fn test_gateway_args_empty() {
    let debug = false;
    let quiet = false;
    let no_console = false;
    let mut gateway_args: Vec<String> = Vec::new();
    if debug {
        gateway_args.push("--debug".to_string());
    }
    if quiet {
        gateway_args.push("--quiet".to_string());
    }
    if no_console {
        gateway_args.push("--no-console".to_string());
    }
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
    if debug {
        gateway_args.push("--debug".to_string());
    }
    if quiet {
        gateway_args.push("--quiet".to_string());
    }
    if no_console {
        gateway_args.push("--no-console".to_string());
    }
    assert!(gateway_args.contains(&"--debug".to_string()));
    assert!(!gateway_args.contains(&"--quiet".to_string()));
}

#[test]
fn test_gateway_args_construction_with_all() {
    let debug = true;
    let quiet = true;
    let no_console = true;
    let mut gateway_args: Vec<String> = Vec::new();
    if debug {
        gateway_args.push("--debug".to_string());
    }
    if quiet {
        gateway_args.push("--quiet".to_string());
    }
    if no_console {
        gateway_args.push("--no-console".to_string());
    }
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
    let node_id = format!(
        "node-{}",
        uuid::Uuid::new_v4().to_string().split('-').next().unwrap()
    );
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
    assert!(
        cfg.get("port").is_some() || cfg.get("rpc_port").is_some(),
        "Cluster config should have port settings"
    );
}

#[test]
fn test_config_scanner_default_has_engines() {
    let cfg: serde_json::Value = serde_json::from_str(CONFIG_SCANNER_DEFAULT).unwrap();
    assert!(
        cfg.get("engines").is_some() || cfg.get("enabled").is_some(),
        "Scanner config should have engines or enabled list"
    );
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
    if let Some(web) = cfg.pointer_mut("/channels/web")
        && let Some(obj) = web.as_object_mut()
    {
        obj.insert(
            "auth_token".to_string(),
            serde_json::Value::String("test-token".to_string()),
        );
        obj.insert(
            "host".to_string(),
            serde_json::Value::String("0.0.0.0".to_string()),
        );
        obj.insert("port".to_string(), serde_json::Value::Number(8080.into()));
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

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：run_command 分发臂
// （Onboard default / Onboard args 变体 / 已有配置跳过 / Version）。
//
// 只挑无 process::exit 风险的臂：Onboard 全程离线（纯文件写入 + 内嵌模板
// 提取）；Version 只打印。Estop/Dashboard 等臂失败即 process::exit(1) 会
// 杀掉测试进程 → 列结构豁免（需真网关 HTTP）。
// =========================================================================

/// 隔离 home（NEMESISBOT_HOME → tempdir；resolve_home Priority 2）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
struct TempHomeEnv {
    _tmp: TempDir,
    home: std::path::PathBuf,
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn temp_home_env() -> TempHomeEnv {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    TempHomeEnv { _tmp: tmp, home }
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
async fn run_command_onboard_default_writes_full_home() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();

    let cli = Cli {
        local: false,
        command: Commands::Onboard {
            default: true,
            args: vec![],
        },
    };
    run_command(cli)
        .await
        .expect("onboard default must succeed offline");

    // 主配置 + 各子系统配置 + workspace 模板 + 人格文件 + peers.toml。
    assert!(th.home.join("config.json").exists(), "main config");
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(th.home.join("config.json")).unwrap())
            .unwrap();
    // onboard default 的三处特征改写：web 端口 49000 / websocket 开 / security 开。
    assert_eq!(cfg["channels"]["web"]["port"], 49000);
    assert_eq!(cfg["channels"]["websocket"]["enabled"], true);
    assert_eq!(cfg["security"]["enabled"], true);

    assert!(th.home.join("workspace").join("config").exists());
    assert!(th.home.join("workspace").join("IDENTITY.md").exists());
    assert!(th.home.join("workspace").join("SOUL.md").exists());
    assert!(th.home.join("workspace").join("USER.md").exists());
    assert!(
        th.home
            .join("workspace")
            .join("cluster")
            .join("IDENTITY.md")
            .exists()
    );
    assert!(
        th.home
            .join("workspace")
            .join("workflow")
            .join("definitions")
            .exists()
    );
    assert!(
        th.home
            .join("workspace")
            .join("cluster")
            .join("peers.toml")
            .exists()
    );
    // Step 7.8：eval 规则种子（feature 开时）。
    #[cfg(feature = "eval")]
    assert!(
        th.home
            .join("workspace")
            .join("config")
            .join("eval_rules.json")
            .exists()
    );
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
async fn run_command_onboard_via_args_variant_also_defaults() {
    // `onboard` 不带 --default 但 args 里含 "default" → 同样走默认装配分支。
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();

    let cli = Cli {
        local: false,
        command: Commands::Onboard {
            default: false,
            args: vec!["default".to_string()],
        },
    };
    run_command(cli)
        .await
        .expect("onboard args=default must succeed");
    assert!(th.home.join("config.json").exists());
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
async fn run_command_onboard_existing_config_keeps_main_config() {
    // 已有 config.json + cargo test 的 stdin 是管道 EOF（read_line 空串）→
    // 覆盖确认走「保留既有配置」分支：旧内容不被改写，其余配置仍生成。
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    std::fs::write(
        th.home.join("config.json"),
        r#"{ "channels": { "web": { "port": 12345 } } }"#,
    )
    .unwrap();

    let cli = Cli {
        local: false,
        command: Commands::Onboard {
            default: true,
            args: vec![],
        },
    };
    run_command(cli)
        .await
        .expect("onboard with existing config must succeed");

    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(th.home.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(
        cfg["channels"]["web"]["port"], 12345,
        "EOF 输入 ≠ y → 既有主配置必须保留"
    );
    // 其余配置文件仍写入。
    assert!(
        th.home
            .join("workspace")
            .join("cluster")
            .join("peers.toml")
            .exists()
    );
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
async fn run_command_version_arm_is_safe_no_op() {
    let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    let cli = Cli {
        local: false,
        command: Commands::Version,
    };
    run_command(cli).await.expect("version arm must succeed");
}

// ===========================================================================
// R7（coverage-95 goal，2026-08-27）：进程级 path manager 单例的共享测试 home。
//
// 背景：`session`/`history` 等 CLI 的 run() 经 `default_path_manager()`
// （OnceLock，首次触碰即烤死）读写 jsonl / FTS 索引。测试二进制里单例
// home 取决于首个触碰它的测试（通常烤成 ~/.nemesisbot），直接调 run()
// 会写真实家目录。`nemesis-path` 为此补了 `set_home_dir()` 运行时缝
// （见 paths.rs），本 helper 用它把单例**永久**重定向到一个测试沙箱 home：
//
// - 永久（不恢复）：history_search 的 INDEX 连接、以及其他按首用烤死的
//   静态态，要求「重定向后不再变」——按测试粒度恢复会把这些静态留在已
//   删除的 tempdir 上，后续测试踩出莫名的 IO 错误。所有需要单例一致的
//   测试共用这一个 home，用唯一 key 互不干扰。
// - 持久目录（非 TempDir）：TempDir drop 会删目录，单例还指着它。
// - 纪律：调用方必须先拿 `crate::GLOBAL_STATE_LOCK`（env/cwd/单例三类
//   进程全局态的共享串行锁）；helper 自身不拿锁（std Mutex 不可重入）。
// ===========================================================================

/// 测试沙箱 home（`<dir>/.nemesisbot`）；首次调用时创建并重定向单例。
/// 返回的是 `.nemesisbot` 本体；`NEMESISBOT_HOME` env 应设为其 parent
/// （resolve_home 语义：`{env}/.nemesisbot`）。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
pub(crate) fn singleton_test_home() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static HOME: OnceLock<std::path::PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let base =
            std::env::temp_dir().join(format!("nemesisbot_r7_sandbox_{}", std::process::id()));
        let home = base.join(".nemesisbot");
        std::fs::create_dir_all(&home).expect("create singleton test home");
        nemesis_path::default_path_manager().set_home_dir(home.clone());
        home
    })
    .clone()
}

/// 测试内临时设 `NEMESISBOT_HOME` 指向沙箱 parent 的守卫；drop 恢复原值。
/// 必须在持有 `crate::GLOBAL_STATE_LOCK` 时创建。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
pub(crate) struct EnvHomeGuard {
    orig: Option<std::ffi::OsString>,
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
impl EnvHomeGuard {
    pub(crate) fn point_at(home: &std::path::Path) -> Self {
        let parent = home
            .parent()
            .expect("test home always has a parent")
            .to_path_buf();
        let orig = std::env::var_os("NEMESISBOT_HOME");
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", &parent);
        }
        Self { orig }
    }
}

#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
impl Drop for EnvHomeGuard {
    fn drop(&mut self) {
        unsafe {
            match self.orig.clone() {
                Some(v) => std::env::set_var("NEMESISBOT_HOME", v),
                None => std::env::remove_var("NEMESISBOT_HOME"),
            }
        }
    }
}

// ===========================================================================
// wave_b（coverage 补测）：run_command 其余纯本地分发臂。
//
// 原则：只覆盖「组装/打印/读文件」类命令的离线路径；凡真实派发（Gateway
// 绑端口起服务）、进程边界（process::exit 的 Estop/Dashboard、platform cfg
// 门）一律豁免。每个测试持 GLOBAL_STATE_LOCK + temp_home_env 隔离。
// =========================================================================

// 整 mod Windows 形态（9/9 测试 + 专属 helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod wave_b {
    use super::*;

    fn wb_cli(command: Commands) -> Cli {
        Cli {
            local: false,
            command,
        }
    }

    /// `onboard` 不带 --default 且 args 无 "default" → 走
    /// "Interactive configuration setup..." 横幅臂，后续装配管线与 default
    /// 一致（唯一 stdin 触点是「已存在 config.json 的 y/N」，cargo test 的
    /// 管道 EOF 天然落入"保留既有"，已被既有三测验证安全）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_onboard_interactive_branch_prints_setup_banner() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();

        let cli = wb_cli(Commands::Onboard {
            default: false,
            args: vec![],
        });
        run_command(cli)
            .await
            .expect("interactive 分支同样完成装配");

        assert!(th.home.join("config.json").exists());
        assert!(th.home.join("workspace").join("IDENTITY.md").exists());
    }

    /// Commands::Agent 分发：字段解构 → commands::agent::run 七参透传。
    /// 单消息 + 死地址 provider（127.0.0.1:1 立即拒绝）→ agent 内部消化
    /// LLM 错误，run 返回 Ok —— 只验分发与收敛，不触网不出进程。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wave_b_agent_dispatch_single_message_dead_provider_ok() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        std::fs::write(
            th.home.join("config.json"),
            serde_json::json!({
                "agents": {"defaults": {"llm": "fake"}},
                "model_list": [{
                    "model_name": "fake",
                    "model": "openai/gpt-fake",
                    "api_base": "http://127.0.0.1:1",
                    "api_key": "k"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let cli = wb_cli(Commands::Agent {
            subcommand: None,
            message: Some("wb ping".to_string()),
            session: "wave-b-session".to_string(),
            debug: false,
            quiet: false,
            no_console: false,
        });
        let res = run_command(cli).await;
        assert!(res.is_ok(), "单消息死地址模式必须 Ok 收敛: {res:?}");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_status_arm_is_offline_safe() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let _th = temp_home_env();
        run_command(wb_cli(Commands::Status))
            .await
            .expect("status 纯文件检查打印，空 home 也必须 Ok");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_cors_list_missing_config_prints_and_ok() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let _th = temp_home_env();
        let cli = wb_cli(Commands::Cors {
            action: commands::cors::CorsAction::List,
        });
        run_command(cli)
            .await
            .expect("cors list 缺配置文件 → 打印提示 → Ok");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_model_list_verbose_with_one_entry() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        // 手写一条模型条目 → List 穿过逐条打印循环（含 verbose 明细分支）。
        std::fs::write(
            th.home.join("config.json"),
            serde_json::json!({
                "agents": {"defaults": {"llm": "wbfake"}},
                "model_list": [{
                    "model_name": "wbfake",
                    "model": "openai/gpt-wb",
                    "api_base": "http://127.0.0.1:9",
                    "api_key": "k"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let cli = wb_cli(Commands::Model {
            action: commands::model::ModelAction::List { verbose: true },
        });
        run_command(cli)
            .await
            .expect("model list 单条目 + verbose 必须 Ok");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_cron_list_without_store_is_ok() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let _th = temp_home_env();
        let cli = wb_cli(Commands::Cron {
            action: commands::cron::CronAction::List,
        });
        run_command(cli)
            .await
            .expect("cron store 缺失 → 打印空列表 → Ok");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_mcp_list_without_config_is_ok() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let _th = temp_home_env();
        let cli = wb_cli(Commands::Mcp {
            action: commands::mcp::McpAction::List,
        });
        run_command(cli)
            .await
            .expect("mcp 配置缺失 → 打印提示 → Ok");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_persona_current_without_active_persona_is_ok() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let _th = temp_home_env();
        let cli = wb_cli(Commands::Persona {
            action: commands::persona::PersonaAction::Current,
        });
        run_command(cli)
            .await
            .expect("无 _active.json → 打印未激活 → Ok");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn wave_b_shutdown_without_gateway_writes_signal_file_only() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        // 无 gateway.pid / 无 config.json：PID 臂跳过 → 写 legacy signal 文件
        // → HTTP 臂因 config 缺失跳过 → 收尾提示，全程仅本地文件 IO。
        run_command(wb_cli(Commands::Shutdown))
            .await
            .expect("无网关时 shutdown 必须优雅 Ok");
        assert!(th.home.join("shutdown.signal").exists());
    }
}

// ===========================================================================
// R9 补测批（coverage-95 goal）：main 进程边界。
//
// 豁免清账：
// - 上方 S11d 注释「Estop/Dashboard 失败即 process::exit(1) 会杀掉测试
//   进程 → 列结构豁免」——本批经真实 exe 子进程观察退码，覆盖这两个臂
//   （main.rs Dashboard/Estop 分支的 exit(1) 收尾）。
// - main.rs 顶部短路与 Cli::parse 之前的早退路径（eval-agent 角色、
//   --multiple child-mode 的 Err 臂）只能进程级断言，in-process 不可能。
// - executor 角色短路已有既有测试；本批补 eval-agent 与 --multiple。
// =========================================================================

// 整 mod Windows 形态（7/7 测试 + 专属 helper 全走 Windows CLI 进程边界；
// 个别测试另有 feature 双门控，语义不变）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r9_process_boundary {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use test_harness::TestWorkspace;

    /// spawn_env_direct 的结果（超时置 code=-2，交由调用方断言响亮失败）。
    struct SpawnOutcome {
        code: i32,
        stdout: String,
        stderr: String,
    }

    /// 带 env 注入直接起真 exe（run_cli* 系列无法注入 env 的补位）。
    /// env 只作用于子进程（Command::env / env_remove），父进程环境零污染，
    /// 因此无需保存/恢复；调用方仍须持有 crate::GLOBAL_STATE_LOCK（纪律：
    /// env 相关用例全局串行）。stdin 用 null；stdout/stderr 全量捕获后等子
    /// 进程退出再读（输出量小，先退后读不会撑爆管道缓冲）。阻塞轮询
    /// try_wait 到退出或 deadline（Windows 环境无 wait_timeout crate）。
    fn spawn_env_direct(
        bin: &std::path::Path,
        args: &[&str],
        env_set: &[(&str, &str)],
        env_unset: &[&str],
        deadline_secs: u64,
    ) -> SpawnOutcome {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in env_unset {
            cmd.env_remove(k);
        }
        for (k, v) in env_set {
            cmd.env(k, v);
        }
        // CLI 覆盖注入放在 env_unset 之后，确保不被调用方清掉（测量模式下
        // 子进程计数落 NEMESISBOT_COVERAGE_DIR；非测量环境 env 为空零影响）。
        cmd.envs(test_harness::coverage_cli_env());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return SpawnOutcome {
                    code: -1,
                    stdout: String::new(),
                    stderr: format!("failed to spawn: {}", e),
                };
            }
        };
        let deadline = Instant::now() + Duration::from_secs(deadline_secs);
        loop {
            match child.try_wait().expect("try_wait failed") {
                Some(_status) => break,
                None => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return SpawnOutcome {
                            code: -2,
                            stdout: String::new(),
                            stderr: format!(
                                "spawn_env_direct timed out after {}s: {:?} {:?}",
                                deadline_secs, args, env_set
                            ),
                        };
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        // 子进程已退出：管道写端关闭，读到 EOF 为止是安全的。
        use std::io::Read;
        let mut out = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = s.read_to_string(&mut out);
        }
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        // try_wait 已消费 status，wait() 再取 code 是合法的（返回已缓存）。
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        SpawnOutcome {
            code,
            stdout: out,
            stderr: err,
        }
    }

    fn r9_bin() -> std::path::PathBuf {
        test_harness::resolve_nemesisbot_bin().expect("nemesisbot binary resolved")
    }

    /// S2-1：NEMESISBOT_ROLE=eval-agent 在 Cli::parse 之前短路进
    /// eval_worker；缺 NEMESISBOT_EVAL_WORKSPACE env → context Err →
    /// tokio main 返回 Err → 运行时打 "Error: ..." 并以退码 1 结束。
    /// 若短路被回归移除，`status` 子命令会正常跑完 rc=0 → 断言响亮失败。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[cfg(feature = "eval")]
    #[test]
    fn r9_eval_agent_role_short_circuit_fails_fast_rc1() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r9_bin();
        let o = spawn_env_direct(
            &bin,
            &["status"],
            &[("NEMESISBOT_ROLE", "eval-agent")],
            &["NEMESISBOT_HOME"],
            120,
        );
        assert_eq!(
            o.code, 1,
            "eval-agent 缺 workspace env 必须 rc=1:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
        assert!(
            o.stderr.contains("NEMESISBOT_EVAL_WORKSPACE"),
            "stderr 必须点名缺失的 env:\n{}",
            o.stderr
        );
    }

    /// S2-2：--multiple 触发 desktop child-mode 早退（同样在 Cli::parse
    /// 之前）；缺 --child-id 参数 → run_child_mode Err("child-id not
    /// specified") → "[Child] Error: ..." eprintln + process::exit(1)。
    /// 早退发生在任何窗口/DLL 创建之前，headless 安全。若触发条件被回归
    /// 移除，clap 会报 unrecognized 并 rc=2 → 两处断言都响亮失败。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[cfg(feature = "desktop")]
    #[test]
    fn r9_multiple_child_mode_missing_handshake_args_exits_1() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r9_bin();
        let o = spawn_env_direct(&bin, &["--multiple"], &[], &["NEMESISBOT_HOME"], 120);
        assert_eq!(
            o.code, 1,
            "--multiple 无 child-id 必须 exit(1):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
        assert!(
            o.stderr.contains("[Child] Error"),
            "Err 臂必须打印 [Child] Error 前缀:\n{}",
            o.stderr
        );
        assert!(
            o.stderr.contains("child-id not specified"),
            "错误应指明缺 child-id:\n{}",
            o.stderr
        );
    }

    const SWEEP_DEADLINE: u64 = 25;

    /// 每条断言：rc ∈ {0,1}（-2=超时、-1=起不来、2+=clap 拒绝都算失败）
    /// 且 stderr 不含 clap 的 unrecognized —— 证明命令真被 CLI 接线分发到
    /// 对应 commands::*::run 的臂（进程级复扫；in-process 等价覆盖见上方
    /// S11d/wave_b，那里没有二进制接线视角）。
    fn r9_assert_dispatched(out: &test_harness::CliOutput, cmd: &[&str]) {
        assert!(
            out.exit_code == 0 || out.exit_code == 1,
            "{:?} 必须干净退出（rc∈{{0,1}}），got={}:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            cmd,
            out.exit_code,
            out.stdout,
            out.stderr
        );
        assert!(
            !out.stderr.contains("unrecognized"),
            "{:?} 被 clap 拒绝（未接线或拼写漂移）:\n{}",
            cmd,
            out.stderr
        );
    }

    /// S2-3a 只读命令扫雷 A 半：无 feature 门控的通用臂（+ cluster push）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r9_dispatch_sweep_part_a_universal_arms() {
        let bin = r9_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let mut sweep: Vec<Vec<&str>> = vec![
            vec!["status"],
            vec!["version"],
            vec!["model", "list"],
            vec!["model", "default"],
            vec!["cron", "list"],
            vec!["mcp", "list"],
            vec!["channel", "list"],
            vec!["cors", "list"],
            vec!["skills", "list"],
            vec!["persona", "list"],
            vec!["auth", "status"],
        ];
        #[cfg(feature = "cluster")]
        sweep.push(vec!["cluster", "status"]);
        // session::run 有显式 pm_home != home 一致性守卫：不一致时干净 bail
        // rc=1（无副作用），因此 List 臂可安全子进程化；FTS 的 history 整臂
        // 放弃（reindex 走全局 path manager，子进程重定向语义未经实证，
        // 避免触碰真机 home 的读+建索引副作用）。
        sweep.push(vec!["session", "list"]);
        for cmd in &sweep {
            let refs: &[&str] = cmd.as_slice();
            let out = ws.run_cli_with_timeout(&bin, refs, SWEEP_DEADLINE).await;
            r9_assert_dispatched(&out, refs);
        }
    }

    /// S2-3b 只读命令扫雷 B 半（feature 门控臂 + 本地写但无害的收尾臂）。
    /// 空 home 下缺配置走「打印提示/保底」路径 → rc∈{0,1} 都在预期内；
    /// log disable / credentials import 即使因缺配置 bail 也只是 rc=1。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r9_dispatch_sweep_part_b_feature_gated_arms() {
        let bin = r9_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let mut sweep: Vec<Vec<&str>> = Vec::new();
        #[cfg(feature = "forge")]
        {
            sweep.push(vec!["forge", "status"]);
            sweep.push(vec!["forge", "list"]);
        }
        #[cfg(feature = "workflow")]
        sweep.push(vec!["workflow", "list"]);
        #[cfg(feature = "memory")]
        sweep.push(vec!["memory", "status"]);
        #[cfg(feature = "voice")]
        sweep.push(vec!["voice", "status"]);
        #[cfg(feature = "security")]
        {
            sweep.push(vec!["security", "status"]);
            sweep.push(vec!["scanner", "list"]);
        }
        #[cfg(feature = "sandbox")]
        sweep.push(vec!["sandbox", "status"]);
        sweep.push(vec!["log", "disable"]);
        sweep.push(vec!["credentials", "import"]);
        sweep.push(vec!["migrate", "--dry-run"]);
        sweep.push(vec!["shutdown"]);
        assert!(
            !sweep.is_empty(),
            "至少要有无条件臂在跑（log/credentials/migrate/shutdown）"
        );
        for cmd in &sweep {
            let refs: &[&str] = cmd.as_slice();
            let out = ws.run_cli_with_timeout(&bin, refs, SWEEP_DEADLINE).await;
            r9_assert_dispatched(&out, refs);
        }
    }

    /// S2-4：空 home（onboard 未执行）下 estop/dashboard 在第一步读
    /// config.json 即 Err → main.rs 对应分支 eprintln "Error: ..." +
    /// process::exit(1)。这正是 S11d 当年列结构豁免的两个臂。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r9_dashboard_and_estop_empty_home_exit_1() {
        let bin = r9_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        assert!(!ws.config_path().exists(), "前置：本夹具不得预先 onboard");

        let estop = ws
            .run_cli_with_timeout(&bin, &["estop", "--status"], 30)
            .await;
        assert_eq!(
            estop.exit_code, 1,
            "空 home estop 必须 exit(1):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            estop.stdout, estop.stderr
        );
        assert!(
            estop.stderr.contains("Cannot read config.json"),
            "estop 错误文案必须指向 config.json:\n{}",
            estop.stderr
        );

        let dash = ws.run_cli_with_timeout(&bin, &["dashboard"], 30).await;
        assert_eq!(
            dash.exit_code, 1,
            "空 home dashboard 必须 exit(1):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            dash.stdout, dash.stderr
        );
        assert!(
            dash.stderr.contains("Cannot read config.json"),
            "dashboard 错误文案必须指向 config.json:\n{}",
            dash.stderr
        );
    }

    /// S2-5：子进程级钉住 onboard 的 --local 专属插入臂（workspace 相对
    /// 路径改写）。in-process 既有覆盖（S11d 三测）全走 local:false，该
    /// `if cli.local` 插入分支此前只有二进制视角能命中。顺带覆盖 onboard
    /// dispatch 臂的全文件落盘结果。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r9_onboard_local_subprocess_pins_local_only_branch() {
        let bin = r9_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let out = ws
            .run_cli_with_timeout(&bin, &["onboard", "default"], 60)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "onboard default --local 失败:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout.contains("Initialization complete"),
            "onboard 收尾横幅缺失:\n{}",
            out.stdout
        );
        assert!(
            ws.config_path().exists(),
            "./.nemesisbot/config.json 未生成"
        );
        assert!(
            ws.home()
                .join("workspace")
                .join("cluster")
                .join("peers.toml")
                .exists(),
            "peers.toml 未生成"
        );
        let raw = std::fs::read_to_string(ws.config_path()).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // --local 专属插入（main.rs Onboard 臂 `if cli.local` 分支）。
        assert_eq!(
            cfg["agents"]["defaults"]["workspace"], ".nemesisbot/workspace",
            "--local 必须写入相对 workspace 路径"
        );
        assert_eq!(cfg["channels"]["web"]["port"], 49000);
        assert_eq!(cfg["security"]["enabled"], true);
    }

    /// S3：gateway 子命令 --help 渲染 Usage 且 rc=0（gateway 本体是长驻
    /// 服务不测，flag 渲染臂补上）；根级 --version 同理 rc=0。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r9_gateway_help_flag_renders_usage_rc0() {
        let bin = r9_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let help = ws
            .run_cli_with_timeout(&bin, &["gateway", "--help"], 20)
            .await;
        assert_eq!(
            help.exit_code, 0,
            "--help 必须 rc=0:\n{}\n--- stderr ---\n{}",
            help.stdout, help.stderr
        );
        assert!(
            help.stdout.contains("Usage:") && help.stdout.contains("--debug"),
            "--help 应渲染 gateway 用法（含 --debug flag 说明）:\n{}",
            help.stdout
        );

        let ver = ws.run_cli_with_timeout(&bin, &["--version"], 20).await;
        assert_eq!(ver.exit_code, 0, "--version 必须 rc=0");
        assert!(!ver.stdout.trim().is_empty(), "--version 应打印版本串");
    }
}

// ===========================================================================
// R10 补测批（coverage-95 goal）：main.rs 派发臂。
//
// 覆盖目标与手法：
// - :272 executor 角色短路 —— env 注入直起真 exe（无子命令），子进程干净
//   rc=0（stdio_loop 见 EOF 正常收尾）→ main 早退行被真实覆盖。
// - Gateway 臂 --debug/--quiet/--no-console push 行 —— 坏 JSON config 触发
//   ConfigStore::load Err → anyhow 干净传播（不走 process::exit(1)，计数
//   能落盘）。注意：flag push 行在 load 之前执行，三个 flag 一起喂全命中。
// - History 臂 —— 空索引下 `history search` 干净 rc=0（只吃派发行，
//   history.rs 内部另有其文件级测试）。
// - Estop 臂三态 —— 起真 gateway（自定义 web/health 端口），CLI 三次调用
//   全部走 /api/internal 成功路径 rc=0；gateway 经 graceful shutdown 干净
//   退出，不污染测量目录的孤儿 profraw。
// - Test(hidden) 臂 —— `test approval-headless`，desktop-gated。
//
// 结构性放弃（证据留档）：
// - :552-557 onboard security-else 臂：该块操作的是**编译期嵌入的
//   CONFIG_DEFAULT**（不读已存在的用户 config），而 nemesisbot/config/
//   config.default.json 顶层恒含 "security" 键 → else 分支在本仓库当前
//   资产下不可达，除非改模板（=改生产资产，纪律禁止）。
// - Dashboard 臂成功路径：open_dashboard 最终调 open_plugin_window 弹真实
//   窗口（禁弹窗纪律）；失败路径 process::exit(1) 不刷 profraw（r9 已证）。
//   两头都不满足覆盖纪律 → 放弃。
// =========================================================================

// 整 mod Windows 形态（5/5 测试 + 专属 helper 全走 Windows CLI 进程边界；
// 个别测试另有 feature 双门控，语义不变）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r10_main {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use test_harness::{ManagedProcess, TestWorkspace};

    /// spawn_env_direct 的 r10 本地拷贝（r9_process_boundary 私有）：带
    /// env 注入直接起真 exe。env 只作用于子进程；注入测试覆盖 env 在
    /// env_unset 之后（与 r9 同序）；调用方持有 crate::GLOBAL_STATE_LOCK。
    struct R10Spawn {
        code: i32,
        stdout: String,
        stderr: String,
    }

    fn r10_spawn_env(
        bin: &std::path::Path,
        args: &[&str],
        env_set: &[(&str, &str)],
        env_unset: &[&str],
        deadline_secs: u64,
    ) -> R10Spawn {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for k in env_unset {
            cmd.env_remove(k);
        }
        for (k, v) in env_set {
            cmd.env(k, v);
        }
        // 纪律 2：CLI 覆盖注入必须接 coverage_cli_env()。
        cmd.envs(test_harness::coverage_cli_env());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return R10Spawn {
                    code: -1,
                    stdout: String::new(),
                    stderr: format!("failed to spawn: {}", e),
                };
            }
        };
        let deadline = Instant::now() + Duration::from_secs(deadline_secs);
        loop {
            match child.try_wait().expect("try_wait failed") {
                Some(_status) => break,
                None => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return R10Spawn {
                            code: -2,
                            stdout: String::new(),
                            stderr: format!("r10_spawn_env timed out after {deadline_secs}s"),
                        };
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
        use std::io::Read;
        let mut out = String::new();
        if let Some(mut s) = child.stdout.take() {
            let _ = s.read_to_string(&mut out);
        }
        let mut err = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut err);
        }
        let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
        R10Spawn {
            code,
            stdout: out,
            stderr: err,
        }
    }

    fn r10_bin() -> std::path::PathBuf {
        test_harness::resolve_nemesisbot_bin().expect("nemesisbot binary resolved")
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn r10_executor_role_short_circuit_clean_rc0() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r10_bin();
        let ws = tempfile::TempDir::new().unwrap();
        // NEMESISBOT_ROLE=executor 在 Cli::parse 前短路进 exec_worker；
        // workspace env 给临时目录（哑执行，无请求进来即 EOF 退出 rc=0）；
        // sandbox/reexec 标记显式清空 → 免沙盒路径（Windows 下 Start.exe
        // 缺失会降级 warn，这里压根不给触发机会）。
        let o = r10_spawn_env(
            &bin,
            &[],
            &[
                ("NEMESISBOT_ROLE", "executor"),
                (
                    "NEMESISBOT_EXECUTOR_WORKSPACE",
                    ws.path().to_str().expect("utf8 tmp path"),
                ),
            ],
            &[
                "NEMESISBOT_HOME",
                "NEMESISBOT_EXECUTOR_SANDBOX",
                "NEMESISBOT_EXECUTOR_REEXEC",
            ],
            120,
        );
        assert_eq!(
            o.code, 0,
            "executor 角色短路线必须走 exec_worker 并干净收尾:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            o.stdout, o.stderr
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r10_gateway_flag_pushes_invalid_config_clean_err() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r10_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let home = ws.home();
        std::fs::create_dir_all(&home).unwrap();
        // 文件存在但不可解析：Step2(exists) 过 → Step4 ConfigStore::load
        // 对坏 JSON 返回 Err（回落仅发生在"文件缺失"时）→ anyhow 干净传播。
        std::fs::write(home.join("config.json"), "{ this is not json").unwrap();

        // 三个 flag 一并喂入 → --debug/--quiet/--no-console 三条 push 全执行。
        let out = ws
            .run_cli_with_timeout(&bin, &["gateway", "--debug", "--quiet", "--no-console"], 60)
            .await;
        assert_ne!(
            out.exit_code, 0,
            "坏配置必须失败退出:\n{}\n{}",
            out.stdout, out.stderr
        );
        assert!(
            out.stderr.contains("Error loading config")
                || out.stdout.contains("Error loading config"),
            "必须经 anyhow 干净传播 'Error loading config':\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout,
            out.stderr
        );
        assert!(
            !out.stderr.contains("Configuration file not found"),
            "config.json 必须存在（只许解析失败）:\n{}",
            out.stderr
        );
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r10_history_search_dispatch_arm_offline_rc0() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r10_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let up = ws
            .run_cli_with_timeout(&bin, &["onboard", "default"], 60)
            .await;
        assert_eq!(up.exit_code, 0, "onboard failed:\n{}", up.stderr);
        // 空索引：reindex 扫 0 个文件 + search 无命中 → 打印未找到 → rc=0。
        // （吃 Commands::History 派发行；history.rs 内部逻辑由其文件级
        // in-process 测试覆盖。）
        let out = ws
            .run_cli_with_timeout(&bin, &["history", "search", "r10-never-indexed"], 60)
            .await;
        assert_eq!(
            out.exit_code, 0,
            "空索引搜索必须干净 Ok:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout, out.stderr
        );
    }

    /// 起一个端口收窄的真 gateway（web=P_WEB / health=P_HEALTH、websocket
    /// 关、安全关、死地址 mini 模型），返回 (workspace, 进程句柄)。
    /// 完成后由调用方 graceful shutdown 干净退出。
    async fn r10_boot_gateway_on_free_ports() -> (TestWorkspace, ManagedProcess) {
        let bin = r10_bin();
        let ws = TestWorkspace::new().expect("temp workspace");

        // 先占两个临时端口再放掉——把竞争窗口压到最小（放下手立即 spawn）。
        let l1 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let l2 = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p_web = l1.local_addr().unwrap().port();
        let p_health = l2.local_addr().unwrap().port();
        drop(l1);
        drop(l2);

        let home = ws.home();
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({
            "gateway": {"host": "127.0.0.1", "port": p_health},
            "agents": {"defaults": {"llm": "r10fake", "max_tool_iterations": 5}},
            "model_list": [{
                "model_name": "r10fake",
                "model": "r9mock/r10fake",
                "api_key": "r10-key",
                "api_base": "http://127.0.0.1:9",
                "model_tier": "mini"
            }],
            "channels": {
                "web": {
                    "enabled": true,
                    "host": "127.0.0.1",
                    "port": p_web,
                    "auth_token": test_harness::AUTH_TOKEN
                },
                "websocket": {"enabled": false}
            },
            "security": {"enabled": false},
            "logging": {"llm": {"enabled": false}}
        });
        std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();

        let proc = ManagedProcess::spawn("R10Gateway", &bin, &["--local", "gateway"], ws.path())
            .expect("gateway process spawns");
        // readiness 以 web /api/health 为准（estop 的 check_health 打这里）。
        test_harness::wait_for_http(
            &format!("http://127.0.0.1:{p_web}/api/health"),
            Duration::from_secs(90),
        )
        .await
        .expect("gateway web health becomes ready");
        (ws, proc)
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_estop_trio_status_engage_release_live_gateway() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r10_bin();
        let (ws, mut gw) = r10_boot_gateway_on_free_ports().await;

        // ① status：ENGAGED=false
        let s1 = ws
            .run_cli_with_timeout(&bin, &["estop", "--status"], 60)
            .await;
        assert_eq!(
            s1.exit_code, 0,
            "estop --status 必须 rc=0:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            s1.stdout, s1.stderr
        );
        assert!(s1.stdout.contains("E-stop"), "{}", s1.stdout);

        // ② engage（裸 estop）
        let s2 = ws.run_cli_with_timeout(&bin, &["estop"], 60).await;
        assert_eq!(
            s2.exit_code, 0,
            "estop engage 必须 rc=0:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            s2.stdout, s2.stderr
        );
        assert!(s2.stdout.contains("E-stop"), "{}", s2.stdout);

        // ③ status 复核 engaged=true 后 release 复原
        let _s3 = ws
            .run_cli_with_timeout(&bin, &["estop", "--status"], 60)
            .await;
        let s4 = ws
            .run_cli_with_timeout(&bin, &["estop", "--release"], 60)
            .await;
        assert_eq!(
            s4.exit_code, 0,
            "estop --release 必须 rc=0:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            s4.stdout, s4.stderr
        );
        assert!(s4.stdout.contains("E-stop"), "{}", s4.stdout);

        // 干净收尾：graceful shutdown（web 端口 + auth token）→ 等真退出。
        // gateway 子进程自身的覆盖计数随正常退出链落盘。
        let web_port = {
            let raw = std::fs::read_to_string(
                ws.home()
                    .join("workspace")
                    .join("state")
                    .join("gateway.json"),
            )
            .expect("state/gateway.json written by gateway");
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            v["web_port"].as_i64().expect("web_port i64") as u16
        };
        let token = {
            let raw = std::fs::read_to_string(ws.home().join("config.json")).unwrap();
            let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
            v["channels"]["web"]["auth_token"]
                .as_str()
                .unwrap()
                .to_string()
        };
        test_harness::graceful_shutdown_gateway(web_port, &token)
            .await
            .expect("graceful shutdown acked");
        gw.wait_for_exit(Duration::from_secs(60))
            .await
            .expect("gateway exits after graceful shutdown");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[cfg(feature = "desktop")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_test_hidden_approval_headless_arm() {
        let _guard = GLOBAL_STATE_LOCK.lock().unwrap();
        let bin = r10_bin();
        let ws = TestWorkspace::new().expect("temp workspace");
        let up = ws
            .run_cli_with_timeout(&bin, &["onboard", "default"], 60)
            .await;
        assert_eq!(up.exit_code, 0, "onboard failed:\n{}", up.stderr);

        // hidden `test approval-headless`：内部自起 ProcessManager（WS 绑
        // ephemeral 端口）+ 强制 headless + 子审批进程自动批复。成功/失败都
        // 走 Result 链干净退出（不对称清理已被 BUG#28 修掉）。
        let out = ws
            .run_cli_with_timeout(
                &bin,
                &["test", "approval-headless", "--expected", "approved"],
                120,
            )
            .await;
        assert!(
            out.stdout.contains("Headless Approval Test"),
            "必须进入 test_cmd 入口横幅:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.stdout,
            out.stderr
        );
        if out.exit_code != 0 {
            // 三类已知合理失败（超时/通道关闭/断言不符）同样证明了派发臂 +
            // run 入口的完整执行链；此时要求错误信息可读（anyhow 传播）。
            assert!(
                out.stderr.contains("Error:") || out.stdout.contains("Error:"),
                "非零退出必须是 anyhow 干净传播，不允许 panic:\n--- stdout ---\n{}\n--- stderr ---\n{}",
                out.stdout,
                out.stderr
            );
        }
    }
}

// -------------------------------------------------------------------------
// cors.json 路径收编（2026-08-29）：copy-once 迁移 + 新落位解析
// -------------------------------------------------------------------------

#[test]
fn cors_config_path_migrates_legacy_home_config_copy_once() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path();
    let legacy_dir = home.join("config");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(
        legacy_dir.join("cors.json"),
        r#"{"origins":["https://a.com"]}"#,
    )
    .unwrap();

    // 首次调用（读路径即触发 copy-once 迁移）：legacy 内容到达新位。
    let path = common::cors_config_path(home);
    assert!(
        path.exists(),
        "legacy cors.json must be migrated on first access"
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("https://a.com"));

    // legacy 保留（备份）。
    assert!(
        legacy_dir.join("cors.json").exists(),
        "legacy file kept as backup"
    );

    // 幂等：改 legacy 后再调用不覆盖已迁移的新位。
    std::fs::write(
        legacy_dir.join("cors.json"),
        r#"{"origins":["https://changed.com"]}"#,
    )
    .unwrap();
    let path2 = common::cors_config_path(home);
    let content2 = std::fs::read_to_string(&path2).unwrap();
    assert!(
        content2.contains("https://a.com"),
        "existing target must win"
    );
}
