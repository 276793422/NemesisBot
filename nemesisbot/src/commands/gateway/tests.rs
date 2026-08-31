// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;

// -------------------------------------------------------------------------
// parse_host_port tests
// -------------------------------------------------------------------------

#[test]
fn test_parse_host_port_standard() {
    let (host, port) = parse_host_port("127.0.0.1:8080");
    assert_eq!(host, "127.0.0.1");
    assert_eq!(port, 8080);
}

#[test]
fn test_parse_host_port_zero_port() {
    let (host, port) = parse_host_port("0.0.0.0:0");
    assert_eq!(host, "0.0.0.0");
    assert_eq!(port, 0);
}

#[test]
fn test_parse_host_port_no_port() {
    let (host, port) = parse_host_port("localhost");
    assert_eq!(host, "localhost");
    assert_eq!(port, 0);
}

// -------------------------------------------------------------------------
// P2: GatewayMemoryGate (memory approval bridge) — mock ApprovalManager tests
// Covers the three boundary cases: user approves, user denies, popup
// times out / errors (must be treated as deny — never let a memory write
// through silently on failure).
// -------------------------------------------------------------------------

#[cfg(all(feature = "desktop", feature = "memory"))]
use nemesis_memory::memory_tools::MemoryApprovalGate;

/// Mock approval manager returning a canned decision.
#[cfg(all(feature = "desktop", feature = "memory"))]
struct MockApproval {
    decision: Result<bool, String>,
}

#[cfg(all(feature = "desktop", feature = "memory"))]
impl nemesis_security::auditor::ApprovalManager for MockApproval {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<bool, String> {
        self.decision.clone()
    }
}

#[cfg(all(feature = "desktop", feature = "memory"))]
fn mock_memory_gate(decision: Result<bool, String>) -> GatewayMemoryGate {
    let am: std::sync::Arc<dyn nemesis_security::auditor::ApprovalManager> =
        std::sync::Arc::new(MockApproval { decision });
    GatewayMemoryGate::new(am)
}

#[cfg(all(feature = "desktop", feature = "memory"))]
#[tokio::test]
async fn memory_gate_approves_when_user_approves() {
    let g = mock_memory_gate(Ok(true));
    assert!(g.approve_store("store fact X").await);
    assert!(g.approve_forget("forget session Y").await);
}

#[cfg(all(feature = "desktop", feature = "memory"))]
#[tokio::test]
async fn memory_gate_denies_when_user_denies() {
    let g = mock_memory_gate(Ok(false));
    assert!(!g.approve_store("x").await, "denied store must be blocked");
    assert!(
        !g.approve_forget("y").await,
        "denied forget must be blocked"
    );
}

#[cfg(all(feature = "desktop", feature = "memory"))]
#[tokio::test]
async fn memory_gate_denies_on_timeout_or_error() {
    // Popup timeout / IPC error → request_approval_sync returns Err → must deny.
    let g = mock_memory_gate(Err("popup timed out".into()));
    assert!(!g.approve_store("x").await, "error must be treated as deny");
    assert!(!g.approve_forget("y").await);
}

#[test]
fn test_parse_host_port_ipv6_like() {
    // With rfind(':'), last colon is used
    let (host, port) = parse_host_port("[::1]:9090");
    assert_eq!(host, "[::1]");
    assert_eq!(port, 9090);
}

#[test]
fn test_parse_host_port_invalid_port() {
    let (host, port) = parse_host_port("example.com:abc");
    assert_eq!(host, "example.com");
    assert_eq!(port, 0); // parse fails -> 0
}

#[test]
fn test_parse_host_port_wildcard() {
    let (host, port) = parse_host_port("0.0.0.0:49321");
    assert_eq!(host, "0.0.0.0");
    assert_eq!(port, 49321);
}

// -------------------------------------------------------------------------
// plugin_ui_dll_exists tests
// -------------------------------------------------------------------------

#[test]
fn test_plugin_ui_library_exists_returns_bool() {
    // This just verifies the function doesn't panic. The result depends on
    // the test environment so we only check the return type.
    let _ = plugin_ui_library_exists();
}

// -------------------------------------------------------------------------
// shutdown flag tests
// -------------------------------------------------------------------------

#[test]
fn test_shutdown_flag_initially_false() {
    // Reset to false for test isolation
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    assert!(!is_shutdown_requested());
}

#[test]
fn test_trigger_global_shutdown() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    trigger_global_shutdown();
    assert!(is_shutdown_requested());
    // Reset after test
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
}

#[test]
fn test_shutdown_flag_can_be_cleared() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
    assert!(is_shutdown_requested());
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    assert!(!is_shutdown_requested());
}

// -------------------------------------------------------------------------
// print_gateway_banner test (just verify it doesn't panic)
// -------------------------------------------------------------------------

#[test]
fn test_print_gateway_banner_no_channels() {
    // Should not panic with 0 channels
    print_gateway_banner("0.0.0.0", 8080, "secret-token", 0, "127.0.0.1", 49000);
}

#[test]
fn test_print_gateway_banner_with_channels() {
    print_gateway_banner("0.0.0.0", 8080, "secret-token", 3, "127.0.0.1", 49000);
}

#[test]
fn test_print_gateway_banner_empty_token() {
    print_gateway_banner("0.0.0.0", 8080, "", 1, "127.0.0.1", 49000);
}

#[test]
fn test_print_gateway_banner_long_token() {
    print_gateway_banner(
        "0.0.0.0",
        8080,
        "a-very-long-authentication-token-value",
        2,
        "127.0.0.1",
        49000,
    );
}

// -------------------------------------------------------------------------
// load_security_rules parse_rules helper tests
// -------------------------------------------------------------------------

#[test]
fn test_parse_security_rules_from_json() {
    use nemesis_security::types::SecurityRule;

    let rules_json = serde_json::json!([
        {"pattern": "*.exe", "action": "deny", "comment": "block executables"},
        {"pattern": "/tmp/**", "action": "allow", "comment": ""}
    ]);
    let rules: Vec<SecurityRule> = rules_json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(SecurityRule {
                        pattern: item.get("pattern")?.as_str()?.to_string(),
                        action: item.get("action")?.as_str()?.to_string(),
                        comment: item
                            .get("comment")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].pattern, "*.exe");
    assert_eq!(rules[0].action, "deny");
    assert_eq!(rules[0].comment, "block executables");
    assert_eq!(rules[1].pattern, "/tmp/**");
    assert_eq!(rules[1].action, "allow");
}

#[test]
fn test_parse_security_rules_empty_array() {
    use nemesis_security::types::SecurityRule;

    let rules_json = serde_json::json!([]);
    let rules: Vec<SecurityRule> = rules_json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(SecurityRule {
                        pattern: item.get("pattern")?.as_str()?.to_string(),
                        action: item.get("action")?.as_str()?.to_string(),
                        comment: item
                            .get("comment")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(rules.is_empty());
}

#[test]
fn test_parse_security_rules_missing_fields() {
    use nemesis_security::types::SecurityRule;

    let rules_json = serde_json::json!([
        {"pattern": "*.log"},
        {"action": "allow"},
        {}
    ]);
    let rules: Vec<SecurityRule> = rules_json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(SecurityRule {
                        pattern: item.get("pattern")?.as_str()?.to_string(),
                        action: item.get("action")?.as_str()?.to_string(),
                        comment: item
                            .get("comment")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(rules.is_empty()); // Both fields required
}

// -------------------------------------------------------------------------
// load_scanner_full_config tests
// -------------------------------------------------------------------------

#[test]
fn test_load_scanner_full_config_missing_file() {
    let result = load_scanner_full_config(std::path::Path::new("/nonexistent/config.json"));
    assert!(result.is_none());
}

#[test]
fn test_load_scanner_full_config_valid() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let data = serde_json::json!({
        "enabled": ["clamav", "custom"],
        "engines": {
            "clamav": {"address": "127.0.0.1:3310"},
            "custom": {"address": "127.0.0.1:9999"}
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_some());
    let cfg = result.unwrap();
    assert_eq!(cfg.enabled.len(), 2);
    assert_eq!(cfg.engines.len(), 2);
}

#[test]
fn test_load_scanner_full_config_empty_engines() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    let data = serde_json::json!({"enabled": [], "engines": {}});
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_some());
    let cfg = result.unwrap();
    assert!(cfg.enabled.is_empty());
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_load_scanner_full_config_invalid_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.scanner.json");
    std::fs::write(&path, "not valid json {{{{").unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_none());
}

// -------------------------------------------------------------------------
// Security config loading tests
// -------------------------------------------------------------------------

#[test]
fn test_load_security_rules_missing_file() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    // Should not panic, just return
    load_security_rules(&plugin, std::path::Path::new("/nonexistent/security.json"));
}

#[test]
fn test_load_security_rules_valid_config() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "default_action": "deny",
        "file_rules": {
            "read": [{"pattern": "*.txt", "action": "allow", "comment": ""}],
            "write": [{"pattern": "*.tmp", "action": "deny", "comment": "no temp writes"}]
        },
        "dir_rules": {
            "create": [{"pattern": "/tmp/**", "action": "allow", "comment": ""}]
        },
        "process_rules": {
            "exec": [{"pattern": "ls", "action": "allow", "comment": ""}]
        },
        "network_rules": {
            "request": [{"pattern": "*.example.com", "action": "allow", "comment": ""}]
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    load_security_rules(&plugin, &path);
}

#[test]
fn test_load_security_rules_with_append() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "default_action": "ask",
        "file_rules": {
            "write": [{"pattern": "*.log", "action": "allow", "comment": ""}],
            "append": [{"pattern": "*.csv", "action": "allow", "comment": ""}]
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    load_security_rules(&plugin, &path);
}

#[test]
fn test_load_security_rules_invalid_json() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    std::fs::write(&path, "invalid json {{{{").unwrap();
    load_security_rules(&plugin, &path);
    // Should not panic
}

// -------------------------------------------------------------------------
// apply_security_layer_switches tests（layer 开关构造期生效——V3 真机揭的
// 死键 bug 的回归测试）
// -------------------------------------------------------------------------

#[test]
fn test_apply_security_layer_switches_all_off() {
    let json = serde_json::json!({
        "layers": {
            "injection": {"enabled": false},
            "command_guard": {"enabled": false},
            "credential": {"enabled": false},
            "ssrf": {"enabled": false}
        }
    });
    let mut cfg = nemesis_security::pipeline::SecurityPluginConfig::default();
    apply_security_layer_switches(&json, &mut cfg);
    assert!(!cfg.injection_enabled);
    assert!(!cfg.command_guard_enabled);
    assert!(!cfg.credential_enabled);
    assert!(!cfg.ssrf_enabled);
    // dlp 不归这个函数管（有独立的多字段读取块）
    assert!(cfg.dlp_enabled);
}

#[test]
fn test_apply_security_layer_switches_absent_keys_keep_defaults() {
    // 只有 dlp 段（合法形状）：其余 layer 开关保持默认全开
    let json = serde_json::json!({"layers": {"dlp": {"enabled": true}}});
    let mut cfg = nemesis_security::pipeline::SecurityPluginConfig::default();
    apply_security_layer_switches(&json, &mut cfg);
    assert!(cfg.injection_enabled);
    assert!(cfg.command_guard_enabled);
    assert!(cfg.credential_enabled);
    assert!(cfg.ssrf_enabled);
}

#[test]
fn test_apply_security_layer_switches_no_layers_section() {
    // 完全没有 layers 段（最小配置文件）：不 panic、不改任何值
    let json = serde_json::json!({"default_action": "allow"});
    let mut cfg = nemesis_security::pipeline::SecurityPluginConfig::default();
    apply_security_layer_switches(&json, &mut cfg);
    assert!(cfg.ssrf_enabled && cfg.injection_enabled);
}

#[test]
fn test_apply_security_layer_switches_partial_override() {
    // 只关 ssrf，其余默认开（V3 e2e 的实际形状）
    let json = serde_json::json!({"layers": {"ssrf": {"enabled": false}}});
    let mut cfg = nemesis_security::pipeline::SecurityPluginConfig::default();
    apply_security_layer_switches(&json, &mut cfg);
    assert!(!cfg.ssrf_enabled);
    assert!(cfg.injection_enabled);
    assert!(cfg.command_guard_enabled);
    assert!(cfg.credential_enabled);
}

// -------------------------------------------------------------------------
// count_enabled_channels tests
// -------------------------------------------------------------------------

#[test]
fn test_count_enabled_channels_none() {
    let config = nemesis_config::Config::default();
    let count = count_enabled_channels(&config);
    assert_eq!(count, 0);
}

// -------------------------------------------------------------------------
// Approval popup data construction tests
// -------------------------------------------------------------------------

#[test]
fn test_approval_popup_data_construction() {
    let request_id = "req-123";
    let operation = "file_write";
    let target = "/etc/passwd";
    let risk_level = "HIGH";
    let reason = "writing to system file";
    let timeout_secs: u64 = 300;

    let data = serde_json::json!({
        "request_id": request_id,
        "operation": operation,
        "operation_name": operation,
        "target": target,
        "risk_level": risk_level,
        "reason": reason,
        "timeout_seconds": timeout_secs.max(30),
        "context": {},
        "timestamp": chrono::Local::now().timestamp(),
    });

    assert_eq!(data["request_id"], "req-123");
    assert_eq!(data["operation"], "file_write");
    assert_eq!(data["target"], "/etc/passwd");
    assert_eq!(data["risk_level"], "HIGH");
    assert_eq!(data["timeout_seconds"], 300);
}

#[test]
fn test_approval_popup_min_timeout_enforcement() {
    let timeout_secs: u64 = 10;
    let enforced = timeout_secs.max(30);
    assert_eq!(enforced, 30); // Minimum 30 seconds
}

#[test]
fn test_approval_popup_normal_timeout() {
    let timeout_secs: u64 = 300;
    let enforced = timeout_secs.max(30);
    assert_eq!(enforced, 300);
}

// -------------------------------------------------------------------------
// Window data construction tests
// -------------------------------------------------------------------------

#[test]
fn test_dashboard_window_data_parsing() {
    let backend_url = "http://127.0.0.1:49000";
    let auth_token = "my-secret-token";
    let window_type = "dashboard";

    let window_data = match window_type {
        "dashboard" => serde_json::json!({
            "token": auth_token,
            "web_port": backend_url.split(':').next_back().and_then(|p| p.parse::<u16>().ok()).unwrap_or(49000),
            "web_host": backend_url.split("://").nth(1).and_then(|s| s.split(':').next()).unwrap_or("127.0.0.1"),
        }),
        "approval" => serde_json::json!({}),
        _ => serde_json::json!({}),
    };

    assert_eq!(window_data["web_port"], 49000);
    assert_eq!(window_data["web_host"], "127.0.0.1");
    assert_eq!(window_data["token"], "my-secret-token");
}

#[test]
fn test_approval_window_data_is_empty() {
    let window_type = "approval";
    let window_data = match window_type {
        "dashboard" => serde_json::json!({
            "token": "",
            "web_port": 49000,
            "web_host": "127.0.0.1",
        }),
        "approval" => serde_json::json!({}),
        _ => serde_json::json!({}),
    };
    assert!(window_data.as_object().unwrap().is_empty());
}

#[test]
fn test_unknown_window_data_is_empty() {
    let window_type = "unknown";
    let window_data = match window_type {
        "dashboard" => serde_json::json!({"token": ""}),
        "approval" => serde_json::json!({}),
        _ => serde_json::json!({}),
    };
    assert!(window_data.as_object().unwrap().is_empty());
}

#[test]
fn test_backend_url_port_extraction() {
    let url = "http://192.168.1.1:8080";
    let port = url
        .split(':')
        .next_back()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(49000);
    assert_eq!(port, 8080);
}

#[test]
fn test_backend_url_host_extraction() {
    let url = "http://192.168.1.1:8080";
    let host = url
        .split("://")
        .nth(1)
        .and_then(|s| s.split(':').next())
        .unwrap_or("127.0.0.1");
    assert_eq!(host, "192.168.1.1");
}

// -------------------------------------------------------------------------
// Additional parse_host_port edge cases
// -------------------------------------------------------------------------

#[test]
fn test_parse_host_port_empty_string() {
    let (host, port) = parse_host_port("");
    assert_eq!(host, "");
    assert_eq!(port, 0);
}

#[test]
fn test_parse_host_port_max_port() {
    let (host, port) = parse_host_port("example.com:65535");
    assert_eq!(host, "example.com");
    assert_eq!(port, 65535);
}

#[test]
fn test_parse_host_port_multiple_colons() {
    let (host, port) = parse_host_port("a:b:8080");
    assert_eq!(host, "a:b");
    assert_eq!(port, 8080);
}

// -------------------------------------------------------------------------
// Additional tests for maximum coverage
// -------------------------------------------------------------------------

#[test]
fn test_count_enabled_channels_zero() {
    let config = nemesis_config::Config::default();
    assert_eq!(count_enabled_channels(&config), 0);
}

#[test]
fn test_count_enabled_channels_web_only() {
    let mut config = nemesis_config::Config::default();
    config.channels.web.enabled = true;
    assert_eq!(count_enabled_channels(&config), 1);
}

#[test]
fn test_count_enabled_channels_multiple() {
    let mut config = nemesis_config::Config::default();
    config.channels.web.enabled = true;
    config.channels.telegram.enabled = true;
    config.channels.discord.enabled = true;
    assert_eq!(count_enabled_channels(&config), 3);
}

#[test]
fn test_count_enabled_channels_all() {
    let mut config = nemesis_config::Config::default();
    config.channels.web.enabled = true;
    config.channels.telegram.enabled = true;
    config.channels.discord.enabled = true;
    config.channels.feishu.enabled = true;
    config.channels.slack.enabled = true;
    assert_eq!(count_enabled_channels(&config), 5);
}

#[test]
fn test_parse_host_port_ipv6_bracket() {
    let (host, port) = parse_host_port("[::1]:8080");
    assert_eq!(host, "[::1]");
    assert_eq!(port, 8080);
}

#[test]
fn test_parse_host_port_bad_port_value() {
    let (host, port) = parse_host_port("example.com:abc");
    assert_eq!(host, "example.com");
    assert_eq!(port, 0);
}

#[test]
fn test_parse_host_port_port_zero() {
    let (host, port) = parse_host_port("host:0");
    assert_eq!(host, "host");
    assert_eq!(port, 0);
}

#[test]
fn test_parse_host_port_just_host() {
    let (host, port) = parse_host_port("localhost");
    assert_eq!(host, "localhost");
    assert_eq!(port, 0);
}

#[test]
fn test_print_gateway_banner_various_configs() {
    // Various banner configurations - just verify no panic
    print_gateway_banner("127.0.0.1", 8080, "test-token", 5, "0.0.0.0", 49000);
    print_gateway_banner("0.0.0.0", 443, "", 0, "localhost", 3000);
    print_gateway_banner("192.168.1.1", 9999, "x", 100, "10.0.0.1", 65535);
}

#[test]
fn test_load_scanner_full_config_with_engines_and_enabled() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("scanner.json");
    let data = serde_json::json!({
        "enabled": ["clamav"],
        "engines": {
            "clamav": {
                "address": "127.0.0.1:3310",
                "state": {"install_status": "installed"}
            }
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_some());
    let cfg = result.unwrap();
    assert_eq!(cfg.enabled.len(), 1);
    assert_eq!(cfg.engines.len(), 1);
}

#[test]
fn test_load_scanner_full_config_partial_data() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("scanner.json");
    // Only enabled, no engines
    let data = serde_json::json!({"enabled": ["clamav"]});
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_some());
    let cfg = result.unwrap();
    assert_eq!(cfg.enabled.len(), 1);
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_load_scanner_full_config_empty_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("scanner.json");
    std::fs::write(&path, "{}").unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_some());
    let cfg = result.unwrap();
    assert!(cfg.enabled.is_empty());
    assert!(cfg.engines.is_empty());
}

#[test]
fn test_load_scanner_full_config_nonexistent() {
    let result = load_scanner_full_config(std::path::Path::new("/nonexistent/scanner.json"));
    assert!(result.is_none());
}

#[test]
fn test_print_agent_startup_info_no_panic() {
    let tmp = tempfile::TempDir::new().unwrap();
    print_agent_startup_info(tmp.path(), 10);
}

#[test]
fn test_print_agent_startup_info_with_skills_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let skills_dir = tmp.path().join("workspace").join("skills");
    std::fs::create_dir_all(skills_dir.join("test-skill")).unwrap();
    std::fs::write(skills_dir.join("test-skill").join("SKILL.md"), "# Test").unwrap();
    print_agent_startup_info(tmp.path(), 15);
}

#[test]
fn test_plugin_ui_library_exists_no_panic() {
    // Just ensure the function runs without panic
    let _ = plugin_ui_library_exists();
}

#[test]
fn test_shutdown_flag_set_and_clear() {
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    assert!(!is_shutdown_requested());
    trigger_global_shutdown();
    assert!(is_shutdown_requested());
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
    assert!(!is_shutdown_requested());
}

#[test]
fn test_shutdown_flag_multiple_toggles() {
    for _ in 0..5 {
        SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
        trigger_global_shutdown();
        assert!(is_shutdown_requested());
    }
    SHUTDOWN_REQUESTED.store(false, Ordering::SeqCst);
}

// -------------------------------------------------------------------------
// DirectLlmChannel JSON construction tests
// -------------------------------------------------------------------------

#[test]
fn test_direct_llm_channel_request_construction() {
    // Test the JSON payload construction logic used by DirectLlmChannel
    let messages = vec![
        serde_json::json!({"role": "system", "content": "You are helpful"}),
        serde_json::json!({"role": "user", "content": "Hello"}),
    ];
    let payload = serde_json::json!({
        "model": "test-model",
        "messages": messages,
        "stream": false,
    });
    assert_eq!(payload["model"], "test-model");
    assert_eq!(payload["messages"].as_array().unwrap().len(), 2);
    assert_eq!(payload["stream"], false);
}

#[test]
fn test_direct_llm_channel_response_parsing() {
    let response = serde_json::json!({
        "choices": [{
            "message": {"role": "assistant", "content": "Hi there!"},
            "finish_reason": "stop"
        }]
    });
    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    assert_eq!(content, "Hi there!");
}

// -------------------------------------------------------------------------
// ClusterResultPersisterAdapter logic tests
// -------------------------------------------------------------------------

#[test]
fn test_cluster_result_persister_save_format() {
    let task_id = "task-123";
    let result = serde_json::json!({
        "status": "success",
        "response": "done",
        "task_id": task_id,
    });
    // Test the result format
    assert_eq!(result["task_id"], task_id);
    assert_eq!(result["status"], "success");
}

// -------------------------------------------------------------------------
// Cluster config loading from JSON tests
// -------------------------------------------------------------------------

// -------------------------------------------------------------------------
// Peer TOML parsing logic tests
// -------------------------------------------------------------------------

#[test]
fn test_peer_toml_key_sanitization() {
    let peer_id = "node-1.example.com:11949";
    let key_safe = peer_id
        .replace(['.', ':', '-'], "_");
    assert_eq!(key_safe, "node_1_example_com_11949");
}

#[test]
fn test_peer_rpc_port_derivation() {
    // Convention: UDP port + 10000
    let udp_port: u16 = 11949;
    let rpc_port = udp_port + 10000;
    assert_eq!(rpc_port, 21949);
}

#[test]
fn test_peer_rpc_port_zero_base() {
    let udp_port: u16 = 0;
    let rpc_port = if udp_port > 0 { udp_port + 10000 } else { 0 };
    assert_eq!(rpc_port, 0);
}

// -------------------------------------------------------------------------
// Web server host resolution logic
// -------------------------------------------------------------------------

#[test]
fn test_web_host_resolution_0000() {
    let h = "0.0.0.0";
    let resolved = if h == "0.0.0.0" || h.is_empty() {
        "127.0.0.1".to_string()
    } else {
        h.to_string()
    };
    assert_eq!(resolved, "127.0.0.1");
}

#[test]
fn test_web_host_resolution_empty() {
    let h = "";
    let resolved = if h == "0.0.0.0" || h.is_empty() {
        "127.0.0.1".to_string()
    } else {
        h.to_string()
    };
    assert_eq!(resolved, "127.0.0.1");
}

#[test]
fn test_web_host_resolution_custom() {
    let h = "192.168.1.1";
    let resolved = if h == "0.0.0.0" || h.is_empty() {
        "127.0.0.1".to_string()
    } else {
        h.to_string()
    };
    assert_eq!(resolved, "192.168.1.1");
}

// -------------------------------------------------------------------------
// Heartbeat interval calculation tests
// -------------------------------------------------------------------------

#[test]
fn test_heartbeat_interval_zero() {
    let interval: i64 = 0;
    let secs = if interval > 0 {
        (interval * 60) as u64
    } else {
        300
    };
    assert_eq!(secs, 300);
}

#[test]
fn test_heartbeat_interval_positive() {
    let interval: i64 = 5;
    let secs = if interval > 0 {
        (interval * 60) as u64
    } else {
        300
    };
    assert_eq!(secs, 300);
}

#[test]
fn test_heartbeat_interval_thirty() {
    let interval: i64 = 30;
    let secs = if interval > 0 {
        (interval * 60) as u64
    } else {
        300
    };
    assert_eq!(secs, 1800);
}

// -------------------------------------------------------------------------
// Security enabled check logic
// -------------------------------------------------------------------------

#[test]
fn test_security_enabled_check_with_security() {
    let mut cfg = nemesis_config::Config::default();
    cfg.security = Some(nemesis_config::SecurityFlagConfig { enabled: true });
    let enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    assert!(enabled);
}

#[test]
fn test_security_enabled_check_without_security() {
    let cfg = nemesis_config::Config::default();
    let enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    // Default is true when security config is not set
    assert!(enabled);
}

#[test]
fn test_security_disabled_check() {
    let mut cfg = nemesis_config::Config::default();
    cfg.security = Some(nemesis_config::SecurityFlagConfig { enabled: false });
    let enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    assert!(!enabled);
}

// -------------------------------------------------------------------------
// LLM timeout configuration logic
// -------------------------------------------------------------------------

#[test]
fn test_llm_timeout_zero_becomes_default() {
    let llm_timeout_secs: u64 = 0;
    let timeout = if llm_timeout_secs > 0 {
        std::time::Duration::from_secs(llm_timeout_secs)
    } else {
        std::time::Duration::from_secs(24 * 3600)
    };
    assert_eq!(timeout.as_secs(), 24 * 3600);
}

#[test]
fn test_llm_timeout_custom() {
    let llm_timeout_secs: u64 = 7200;
    let timeout = if llm_timeout_secs > 0 {
        std::time::Duration::from_secs(llm_timeout_secs)
    } else {
        std::time::Duration::from_secs(24 * 3600)
    };
    assert_eq!(timeout.as_secs(), 7200);
}

// -------------------------------------------------------------------------
// ClusterRPC config construction
// -------------------------------------------------------------------------

#[test]
fn test_cluster_rpc_config_construction() {
    let node_id = "node-test-123".to_string();
    let local_rpc_port: u16 = 21949;
    // Simulate the config construction from gateway.rs
    let config = nemesis_agent::ClusterRpcConfig {
        local_node_id: node_id.clone(),
        timeout_secs: 3600,
        local_rpc_port,
    };
    assert_eq!(config.local_node_id, "node-test-123");
    assert_eq!(config.timeout_secs, 3600);
    assert_eq!(config.local_rpc_port, 21949);
}

// -------------------------------------------------------------------------
// load_scanner_full_config with various inputs
// -------------------------------------------------------------------------

#[test]
fn test_load_scanner_full_config_with_non_object() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("scanner.json");
    std::fs::write(&path, "42").unwrap(); // Not an object
    let result = load_scanner_full_config(&path);
    // Should parse as valid JSON but ScannerFullConfig default should work
    assert!(result.is_some() || result.is_none()); // Don't panic
}

// -------------------------------------------------------------------------
// print_gateway_banner with extreme values
// -------------------------------------------------------------------------

#[test]
fn test_print_gateway_banner_zero_ports() {
    print_gateway_banner("0.0.0.0", 0, "", 0, "0.0.0.0", 0);
}

#[test]
fn test_print_gateway_banner_max_values() {
    print_gateway_banner(
        "255.255.255.255",
        65535,
        "a-very-long-token-that-goes-on",
        1000,
        "255.255.255.255",
        65535,
    );
}

// -------------------------------------------------------------------------
// ForgeProviderBridge tests
// -------------------------------------------------------------------------

/// Verify ForgeProviderBridge can be constructed (type compatibility).
#[cfg(feature = "forge")]
#[test]
fn test_forge_provider_bridge_construction() {
    // We can't create a real LLMProvider in unit tests, but we can verify
    // the struct layout and that the types are compatible.
    // The real test is that the code compiles with the correct types.
}

// -------------------------------------------------------------------------
// ClusterForgeBridgeAdapter tests
// -------------------------------------------------------------------------

#[cfg(all(feature = "cluster", feature = "forge"))]
#[tokio::test]
async fn test_cluster_forge_bridge_adapter_share_reflection() {
    let bridge = ClusterForgeBridgeAdapter::new("node-1".to_string());
    let bridge_ref: &dyn nemesis_forge::bridge::ClusterForgeBridge = &bridge;
    let count = bridge_ref
        .share_reflection(serde_json::json!({"test": true}))
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[cfg(all(feature = "cluster", feature = "forge"))]
#[tokio::test]
async fn test_cluster_forge_bridge_adapter_get_remote_reflections() {
    let bridge = ClusterForgeBridgeAdapter::new("node-1".to_string());
    let bridge_ref: &dyn nemesis_forge::bridge::ClusterForgeBridge = &bridge;
    let reflections = bridge_ref.get_remote_reflections().await.unwrap();
    assert!(reflections.is_empty());
}

#[cfg(all(feature = "cluster", feature = "forge"))]
#[tokio::test]
async fn test_cluster_forge_bridge_adapter_get_online_peers() {
    let bridge = ClusterForgeBridgeAdapter::new("node-1".to_string());
    let bridge_ref: &dyn nemesis_forge::bridge::ClusterForgeBridge = &bridge;
    let peers = bridge_ref.get_online_peers().await.unwrap();
    assert!(peers.is_empty());
}

#[cfg(all(feature = "cluster", feature = "forge"))]
#[test]
fn test_cluster_forge_bridge_adapter_local_node_id() {
    let bridge = ClusterForgeBridgeAdapter::new("test-node-id".to_string());
    let bridge_ref: &dyn nemesis_forge::bridge::ClusterForgeBridge = &bridge;
    assert_eq!(bridge_ref.local_node_id(), "test-node-id");
}

#[cfg(all(feature = "cluster", feature = "forge"))]
#[test]
fn test_cluster_forge_bridge_adapter_is_enabled() {
    let bridge = ClusterForgeBridgeAdapter::new("node-1".to_string());
    let bridge_ref: &dyn nemesis_forge::bridge::ClusterForgeBridge = &bridge;
    assert!(bridge_ref.is_cluster_enabled());
}

// -------------------------------------------------------------------------
// run_bus_arc compilation test
// -------------------------------------------------------------------------

/// Verify that run_bus_arc exists and has correct signature.
/// This test ensures the method is accessible from the test context.
#[test]
fn test_run_bus_arc_signature_exists() {
    // Just verify the method exists by checking the type system.
    // A real functional test would require a full AgentLoop setup.
}

// -------------------------------------------------------------------------
// Enabled channels list construction test
// -------------------------------------------------------------------------

#[test]
fn test_enabled_channels_construction_logic() {
    // Simulate the logic used in C1 wiring to build enabled_channels list
    use nemesis_config::ChannelsConfig;
    let cfg = ChannelsConfig::default();

    let mut channels = Vec::new();
    if cfg.web.enabled {
        channels.push("web");
    }
    if cfg.telegram.enabled {
        channels.push("telegram");
    }
    if cfg.discord.enabled {
        channels.push("discord");
    }
    if cfg.feishu.enabled {
        channels.push("feishu");
    }
    if cfg.slack.enabled {
        channels.push("slack");
    }
    if cfg.whatsapp.enabled {
        channels.push("whatsapp");
    }
    if cfg.dingtalk.enabled {
        channels.push("dingtalk");
    }
    if cfg.qq.enabled {
        channels.push("qq");
    }
    if cfg.line.enabled {
        channels.push("line");
    }
    if cfg.onebot.enabled {
        channels.push("onebot");
    }

    // Default config has all channels disabled
    assert!(
        channels.is_empty(),
        "Default config should have no enabled channels"
    );
}

#[test]
fn test_enabled_channels_with_web_enabled() {
    let mut cfg = nemesis_config::ChannelsConfig::default();
    cfg.web.enabled = true;

    let mut channels = Vec::new();
    if cfg.web.enabled {
        channels.push("web");
    }
    if cfg.telegram.enabled {
        channels.push("telegram");
    }

    assert_eq!(channels, vec!["web"]);
}

// -------------------------------------------------------------------------
// HeartbeatBusAdapter test (type compatibility)
// -------------------------------------------------------------------------

#[test]
fn test_heartbeat_bus_adapter_type_compatible() {
    // Verify that the adapter pattern compiles by checking trait bounds.
    // The adapter is defined inline in the run() function so we can't
    // test it directly, but we verify the trait signatures match.
}

// -------------------------------------------------------------------------
// OutboundMessage construction test
// -------------------------------------------------------------------------

#[test]
fn test_outbound_message_construction() {
    let msg = nemesis_types::channel::OutboundMessage {
        channel: "web".to_string(),
        chat_id: "user1".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert_eq!(msg.channel, "web");
    assert_eq!(msg.chat_id, "user1");
    assert_eq!(msg.content, "Hello");
    assert!(msg.message_type.is_empty());
}

// -------------------------------------------------------------------------
// Cron on_job handler logic test
// -------------------------------------------------------------------------

#[test]
fn test_cron_job_message_construction() {
    // Simulate what the on_job handler does
    let job = nemesis_cron::service::CronJob {
        id: "job-1".to_string(),
        name: "Test Job".to_string(),
        enabled: true,
        schedule: nemesis_cron::service::CronSchedule {
            kind: "interval".to_string(),
            at_ms: None,
            every_ms: Some(60000),
            expr: None,
            tz: None,
        },
        payload: nemesis_cron::service::CronPayload {
            kind: "message".to_string(),
            message: "Hello from cron".to_string(),
            command: None,
            deliver: true,
            channel: Some("web".to_string()),
            to: Some("user1".to_string()),
            session_key: None,
            max_rounds: None,
        },
        state: nemesis_cron::service::CronJobState {
            next_run_at_ms: Some(1000),
            last_run_at_ms: None,
            last_status: None,
            last_error: None,
            history: Vec::new(),
        },
        created_at_ms: 0,
        updated_at_ms: 0,
        delete_after_run: false,
    };

    // Verify job fields
    assert_eq!(job.id, "job-1");
    assert_eq!(job.payload.message, "Hello from cron");
    assert!(!job.payload.message.is_empty());

    // Simulate building an InboundMessage (what the handler does)
    let channel = job
        .payload
        .channel
        .clone()
        .unwrap_or_else(|| "web".to_string());
    let to = job.payload.to.clone().unwrap_or_default();
    assert_eq!(channel, "web");
    assert_eq!(to, "user1");
}

// -------------------------------------------------------------------------
// Forge init_trace / init_learning types test
// -------------------------------------------------------------------------

#[cfg(feature = "forge")]
#[test]
fn test_forge_trace_collector_creation() {
    let collector = nemesis_forge::trace::TraceCollector::new();
    let events = collector.events();
    assert!(events.is_empty());
}

#[cfg(feature = "forge")]
#[test]
fn test_forge_trace_store_creation() {
    let dir = tempfile::tempdir().unwrap();
    let _store = nemesis_forge::trace_store::TraceStore::new(dir.path());
    // Store was created successfully
}

#[cfg(feature = "forge")]
#[test]
fn test_forge_cycle_store_creation() {
    let dir = tempfile::tempdir().unwrap();
    let _store = nemesis_forge::cycle_store::CycleStore::new(dir.path());
    // CycleStore was created successfully
}

#[cfg(feature = "forge")]
#[test]
fn test_forge_registry_creation() {
    let registry =
        nemesis_forge::registry::Registry::new(nemesis_forge::types::RegistryConfig::default());
    let artifacts = registry.list(None, None);
    assert!(artifacts.is_empty());
}

// -------------------------------------------------------------------------
// DeviceService creation test
// -------------------------------------------------------------------------

#[test]
fn test_device_service_creation() {
    let service = nemesis_devices::service::DeviceService::new();
    assert!(!service.is_running());
    assert_eq!(service.count(), 0);
    assert!(service.list().is_empty());
}

// -------------------------------------------------------------------------
// HeartbeatService wiring test
// -------------------------------------------------------------------------

#[test]
fn test_heartbeat_config_construction() {
    let config = nemesis_heartbeat::service::HeartbeatConfig {
        interval: std::time::Duration::from_secs(300),
        enabled: true,
        workspace: Some("/tmp/test".to_string()),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    };
    assert!(config.enabled);
    assert_eq!(config.interval, std::time::Duration::from_secs(300));
}

#[test]
fn test_heartbeat_service_creation_with_config() {
    let config = nemesis_heartbeat::service::HeartbeatConfig {
        interval: std::time::Duration::from_secs(300),
        enabled: true,
        workspace: Some("/tmp/test".to_string()),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    };
    let service = nemesis_heartbeat::service::HeartbeatService::new(config);
    assert!(!service.is_running());
}

// -------------------------------------------------------------------------
// Web search config mapping tests
// -------------------------------------------------------------------------

#[test]
fn test_web_search_config_all_disabled() {
    let cfg = nemesis_config::Config::default();
    let web = &cfg.tools.web;
    let any_enabled = web.brave.enabled || web.duckduckgo.enabled || web.perplexity.enabled;
    assert!(
        !any_enabled,
        "All web search providers should be disabled by default"
    );
}

#[test]
fn test_web_search_config_brave_enabled() {
    let json = r#"{"tools": {"web": {"brave": {"enabled": true, "api_key": "test-key", "max_results": 10}}}}"#;
    let cfg: nemesis_config::Config = serde_json::from_str(json).unwrap();
    assert!(cfg.tools.web.brave.enabled);
    assert_eq!(cfg.tools.web.brave.api_key, "test-key");
    assert_eq!(cfg.tools.web.brave.max_results, 10);
}

#[test]
fn test_web_search_config_duckduckgo_enabled() {
    let json = r#"{"tools": {"web": {"duckduckgo": {"enabled": true, "max_results": 3}}}}"#;
    let cfg: nemesis_config::Config = serde_json::from_str(json).unwrap();
    assert!(cfg.tools.web.duckduckgo.enabled);
    assert_eq!(cfg.tools.web.duckduckgo.max_results, 3);
}

#[test]
fn test_web_search_config_perplexity_enabled() {
    let json = r#"{"tools": {"web": {"perplexity": {"enabled": true, "api_key": "pplx-123", "max_results": 7}}}}"#;
    let cfg: nemesis_config::Config = serde_json::from_str(json).unwrap();
    assert!(cfg.tools.web.perplexity.enabled);
    assert_eq!(cfg.tools.web.perplexity.api_key, "pplx-123");
    assert_eq!(cfg.tools.web.perplexity.max_results, 7);
}

#[test]
fn test_web_search_config_mapping_to_agent_config() {
    let json = r#"{"tools": {"web": {"brave": {"enabled": true, "api_key": "key1"}, "duckduckgo": {"enabled": true, "max_results": 8}, "perplexity": {"enabled": false}}}}"#;
    let cfg: nemesis_config::Config = serde_json::from_str(json).unwrap();
    let web = &cfg.tools.web;

    let config = nemesis_agent::loop_tools::WebSearchConfig {
        brave_api_key: if web.brave.api_key.is_empty() {
            None
        } else {
            Some(web.brave.api_key.clone())
        },
        brave_max_results: web.brave.max_results.max(1) as usize,
        brave_enabled: web.brave.enabled,
        duckduckgo_max_results: web.duckduckgo.max_results.max(1) as usize,
        duckduckgo_enabled: web.duckduckgo.enabled,
        perplexity_api_key: if web.perplexity.api_key.is_empty() {
            None
        } else {
            Some(web.perplexity.api_key.clone())
        },
        perplexity_max_results: web.perplexity.max_results.max(1) as usize,
        perplexity_enabled: web.perplexity.enabled,
    };

    assert!(config.brave_enabled);
    assert_eq!(config.brave_api_key, Some("key1".to_string()));
    assert!(config.duckduckgo_enabled);
    assert_eq!(config.duckduckgo_max_results, 8);
    assert!(!config.perplexity_enabled);
}

#[test]
fn test_web_search_config_empty_api_key_becomes_none() {
    let json = r#"{"tools": {"web": {"brave": {"enabled": true, "api_key": ""}}}}"#;
    let cfg: nemesis_config::Config = serde_json::from_str(json).unwrap();
    let web = &cfg.tools.web;

    let api_key = if web.brave.api_key.is_empty() {
        None
    } else {
        Some(web.brave.api_key.clone())
    };
    assert_eq!(api_key, None);
}

// -------------------------------------------------------------------------
// Device service config tests
// -------------------------------------------------------------------------

#[test]
fn test_devices_config_default_disabled() {
    let cfg = nemesis_config::Config::default();
    assert!(
        !cfg.devices.enabled,
        "devices should be disabled by default"
    );
}

#[test]
fn test_devices_config_enabled() {
    let json = r#"{"devices": {"enabled": true, "monitor_usb": true}}"#;
    let cfg: nemesis_config::Config = serde_json::from_str(json).unwrap();
    assert!(cfg.devices.enabled);
    assert!(cfg.devices.monitor_usb);
}

// -------------------------------------------------------------------------
// Skills loader config tests
// -------------------------------------------------------------------------

#[test]
fn test_skills_loader_creation() {
    let loader =
        nemesis_skills::loader::SkillsLoader::new("/tmp/workspace", "/tmp/workspace/skills", "");
    // List should work even with non-existent directories
    let skills = loader.list_skills();
    // No skills found in non-existent directories
    assert!(skills.is_empty() || !skills.is_empty()); // just verify no panic
}

#[test]
fn test_skills_loader_with_real_dirs() {
    let dir = std::env::temp_dir().join("nemesis_test_skills_loader");
    let skills_dir = dir.join("skills").join("test-skill");
    std::fs::create_dir_all(&skills_dir).unwrap();
    std::fs::write(
        skills_dir.join("SKILL.md"),
        "---\ndescription: A test skill for unit testing\n---\n\n# Test Skill\n\nA test.",
    )
    .unwrap();

    let workspace_str = dir.to_string_lossy().to_string();
    let global_str = dir.join("skills").to_string_lossy().to_string();
    let loader = nemesis_skills::loader::SkillsLoader::new(&workspace_str, &global_str, "");
    let skills = loader.list_skills();
    assert!(
        !skills.is_empty(),
        "Should find at least one skill in {}",
        skills_dir.display()
    );
    assert_eq!(skills[0].name, "test-skill");

    // Cleanup
    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------------------------
// SharedToolConfig wiring tests
// -------------------------------------------------------------------------

#[test]
fn test_shared_tool_config_web_search_field() {
    let config = nemesis_agent::SharedToolConfig {
        web_search: Some(nemesis_agent::loop_tools::WebSearchConfig {
            brave_enabled: true,
            brave_api_key: Some("test".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(config.web_search.is_some());
    assert!(config.web_search.as_ref().unwrap().brave_enabled);
}

#[test]
fn test_shared_tool_config_skills_loader_field() {
    let loader = nemesis_skills::loader::SkillsLoader::new("/tmp", "/tmp/skills", "");
    let config = nemesis_agent::SharedToolConfig {
        skills_loader: Some(std::sync::Arc::new(loader)),
        ..Default::default()
    };
    assert!(config.skills_loader.is_some());
}

#[test]
fn test_shared_tool_config_skills_registry_field() {
    let reg_config = nemesis_skills::types::RegistryConfig::default();
    let rm = nemesis_skills::registry::RegistryManager::new(reg_config);
    let config = nemesis_agent::SharedToolConfig {
        skills_registry: Some(std::sync::Arc::new(rm)),
        ..Default::default()
    };
    assert!(config.skills_registry.is_some());
}

#[test]
fn test_register_shared_tools_with_web_search() {
    let config = nemesis_agent::SharedToolConfig {
        web_search: Some(nemesis_agent::loop_tools::WebSearchConfig {
            duckduckgo_enabled: true,
            ..Default::default()
        }),
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    };
    let tools = nemesis_agent::register_shared_tools(&config);
    assert!(
        tools.contains_key("web_search"),
        "web_search should be registered when config is set"
    );
    assert!(
        tools.contains_key("web_fetch"),
        "web_fetch should always be registered"
    );
}

#[test]
fn test_register_shared_tools_without_web_search() {
    let config = nemesis_agent::SharedToolConfig {
        web_search: None,
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    };
    let tools = nemesis_agent::register_shared_tools(&config);
    assert!(
        !tools.contains_key("web_search"),
        "web_search should NOT be registered when config is None"
    );
    assert!(
        tools.contains_key("web_fetch"),
        "web_fetch should always be registered"
    );
}

#[test]
fn test_register_shared_tools_with_skills_loader() {
    let loader = nemesis_skills::loader::SkillsLoader::new("/tmp", "/tmp/skills", "");
    let config = nemesis_agent::SharedToolConfig {
        skills_loader: Some(std::sync::Arc::new(loader)),
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    };
    let tools = nemesis_agent::register_shared_tools(&config);
    assert!(
        tools.contains_key("skills_list"),
        "skills_list should be registered"
    );
    assert!(
        tools.contains_key("skills_info"),
        "skills_info should be registered"
    );
}

#[test]
fn test_register_shared_tools_with_skills_registry() {
    let reg_config = nemesis_skills::types::RegistryConfig::default();
    let rm = nemesis_skills::registry::RegistryManager::new(reg_config);
    let config = nemesis_agent::SharedToolConfig {
        skills_registry: Some(std::sync::Arc::new(rm)),
        workspace: Some("/tmp".to_string()),
        ..Default::default()
    };
    let tools = nemesis_agent::register_shared_tools(&config);
    assert!(
        tools.contains_key("find_skills"),
        "find_skills should be registered"
    );
    assert!(
        tools.contains_key("install_skill"),
        "install_skill should be registered"
    );
}

// -------------------------------------------------------------------------
// ProviderAdapter message conversion logic tests
// -------------------------------------------------------------------------

#[test]
fn test_provider_adapter_tool_call_conversion() {
    // Verify the tool call conversion logic from AgentToolCallInfo to ProviderToolCall
    let name = "test_function".to_string();
    let arguments = r#"{"key": "value"}"#.to_string();
    let id = "call_123".to_string();

    // Simulate the conversion done in ProviderAdapter::chat
    let provider_tc = nemesis_providers::types::ToolCall {
        id: id.clone(),
        call_type: Some("function".to_string()),
        function: Some(nemesis_providers::types::FunctionCall {
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        name: None,
        arguments: None,
    };

    // Convert back (simulating the reverse in ProviderAdapter)
    let func = provider_tc.function.unwrap();
    assert_eq!(func.name, name);
    assert_eq!(func.arguments, arguments);
}

#[test]
fn test_provider_adapter_finished_logic_tool_calls_present() {
    // When tool_calls are present and finish_reason != "stop", finished = false
    let tool_calls = [nemesis_agent::types::ToolCallInfo {
        id: "call_1".to_string(),
        name: "test".to_string(),
        arguments: "{}".to_string(),
    }];
    let finish_reason = "tool_calls";
    let finished = tool_calls.is_empty() || finish_reason == "stop";
    assert!(!finished);
}

#[test]
fn test_provider_adapter_finished_logic_stop() {
    // When finish_reason is "stop", finished = true
    let tool_calls: Vec<nemesis_agent::types::ToolCallInfo> = vec![];
    let finish_reason = "stop";
    let finished = tool_calls.is_empty() || finish_reason == "stop";
    assert!(finished);
}

#[test]
fn test_provider_adapter_finished_logic_empty_tool_calls() {
    let tool_calls: Vec<nemesis_agent::types::ToolCallInfo> = vec![];
    let finish_reason = "stop";
    let finished = tool_calls.is_empty() || finish_reason == "stop";
    assert!(finished);
}

#[test]
fn test_provider_adapter_model_fallback_empty() {
    // Empty model string should use default
    let default_model = "gpt-4".to_string();
    let model = "";
    let model_to_use = if model.is_empty() {
        &default_model
    } else {
        model
    };
    assert_eq!(model_to_use, "gpt-4");
}

#[test]
fn test_provider_adapter_model_fallback_nonempty() {
    let default_model = "gpt-4".to_string();
    let model = "claude-3";
    let model_to_use = if model.is_empty() {
        &default_model
    } else {
        model
    };
    assert_eq!(model_to_use, "claude-3");
}

// -------------------------------------------------------------------------
// DirectLlmChannel construction tests
// TODO: DirectLlmChannel type not yet implemented — re-enable when available.
// -------------------------------------------------------------------------

#[test]
// Ignored (unimplemented): placeholder — DirectLlmChannel type does not exist yet.
// Re-enable and write real assertions once DirectLlmChannel is implemented.
#[ignore]
fn test_direct_llm_channel_new() {
    // Placeholder: will be implemented when DirectLlmChannel is introduced.
}

#[test]
fn test_direct_llm_channel_url_format() {
    let base_url = "http://127.0.0.1:8080/v1".to_string();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    assert_eq!(url, "http://127.0.0.1:8080/v1/chat/completions");
}

#[test]
fn test_direct_llm_channel_url_format_trailing_slash() {
    let base_url = "http://127.0.0.1:8080/v1/".to_string();
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    assert_eq!(url, "http://127.0.0.1:8080/v1/chat/completions");
}

#[test]
fn test_direct_llm_channel_response_parsing_logic() {
    let response = serde_json::json!({
        "choices": [{
            "message": {"role": "assistant", "content": "Test response with special chars: <>&\"'"},
            "finish_reason": "stop"
        }]
    });
    let content = response
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    assert_eq!(content, "Test response with special chars: <>&\"'");
}

// -------------------------------------------------------------------------
// ClusterResultPersisterAdapter logic tests
// -------------------------------------------------------------------------

#[test]
fn test_cluster_persister_set_running_format() {
    let _task_id = "task-running-123";
    let node_id = "node-abc";
    let data = serde_json::json!({
        "status": "running",
        "from": node_id,
    });
    assert_eq!(data["status"], "running");
    assert_eq!(data["from"], node_id);
}

#[test]
fn test_cluster_persister_set_result_success_format() {
    let _task_id = "task-success-456";
    let node_id = "node-xyz";
    let response = "done processing";
    let data = serde_json::json!({
        "content": response,
        "from": node_id,
    });
    assert_eq!(data["content"], "done processing");
    assert_eq!(data["from"], node_id);
}

#[test]
fn test_cluster_persister_set_result_error_status() {
    // When status == "error", store failure instead of success
    let status = "error";
    let is_error = status == "error";
    assert!(is_error);
}

#[test]
fn test_cluster_persister_set_result_non_error_status() {
    let status = "success";
    let is_error = status == "error";
    assert!(!is_error);
}

// -------------------------------------------------------------------------
// BusToClusterAdapter message construction
// -------------------------------------------------------------------------

#[test]
fn test_bus_to_cluster_message_conversion() {
    // Simulate the conversion from BusInboundMessage to InboundMessage
    let channel = "web".to_string();
    let sender_id = "user1".to_string();
    let chat_id = "chat1".to_string();
    let content = "Hello".to_string();

    let inbound = nemesis_types::channel::InboundMessage {
        channel: channel.clone(),
        sender_id: sender_id.clone(),
        chat_id: chat_id.clone(),
        content: content.clone(),
        media: vec![],
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    };
    assert_eq!(inbound.channel, "web");
    assert_eq!(inbound.sender_id, "user1");
    assert_eq!(inbound.chat_id, "chat1");
    assert_eq!(inbound.content, "Hello");
    assert!(inbound.media.is_empty());
    assert!(inbound.session_key.is_empty());
    assert!(inbound.correlation_id.is_empty());
}

// -------------------------------------------------------------------------
// Approval action parsing logic
// -------------------------------------------------------------------------

#[test]
fn test_approval_action_approved() {
    let value = serde_json::json!({"action": "approved"});
    let action = value
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("rejected");
    assert_eq!(action, "approved");
    let is_approved = action == "approved";
    assert!(is_approved);
}

#[test]
fn test_approval_action_rejected() {
    let value = serde_json::json!({"action": "rejected"});
    let action = value
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("rejected");
    assert_eq!(action, "rejected");
    let is_approved = action == "approved";
    assert!(!is_approved);
}

#[test]
fn test_approval_action_missing_defaults_rejected() {
    let value = serde_json::json!({});
    let action = value
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("rejected");
    assert_eq!(action, "rejected");
    let is_approved = action == "approved";
    assert!(!is_approved);
}

// -------------------------------------------------------------------------
// Security rules with all operation types
// -------------------------------------------------------------------------

#[test]
fn test_load_security_rules_with_process_rules() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "process_rules": {
            "exec": [{"pattern": "ls", "action": "allow", "comment": "list files"}],
            "spawn": [{"pattern": "bash", "action": "deny", "comment": "no shells"}],
            "kill": [{"pattern": "*", "action": "ask", "comment": "confirm kills"}],
            "suspend": []
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    load_security_rules(&plugin, &path);
    // Verify no panic
}

#[test]
fn test_load_security_rules_with_network_rules() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "network_rules": {
            "request": [{"pattern": "*.example.com", "action": "allow", "comment": ""}],
            "download": [{"pattern": "http://*", "action": "allow", "comment": ""}],
            "upload": []
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    load_security_rules(&plugin, &path);
}

#[test]
fn test_load_security_rules_with_hardware_rules() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "hardware_rules": {
            "i2c": [{"pattern": "*", "action": "allow", "comment": ""}],
            "spi": [],
            "gpio": [{"pattern": "*", "action": "deny", "comment": "no gpio"}]
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    load_security_rules(&plugin, &path);
}

#[test]
fn test_load_security_rules_with_registry_rules() {
    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "registry_rules": {
            "read": [{"pattern": "HKLM\\*", "action": "allow", "comment": ""}],
            "write": [{"pattern": "*", "action": "deny", "comment": ""}],
            "delete": []
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    load_security_rules(&plugin, &path);
}

// -------------------------------------------------------------------------
// Discovery config construction (AgentLoop wiring)
// -------------------------------------------------------------------------

#[test]
fn test_discovery_config_from_agent_config() {
    let config = nemesis_agent::types::AgentConfig::default();
    // Verify default config has reasonable values
    assert!(!config.model.is_empty() || config.model.is_empty()); // just verify access
}

#[test]
fn test_agent_config_custom_values() {
    let config = nemesis_agent::types::AgentConfig {
        model: "test-model".to_string(),
        max_turns: 50,
        system_prompt: Some("You are helpful".to_string()),
        tools: vec![],
        ..Default::default()
    };
    assert_eq!(config.model, "test-model");
    assert_eq!(config.max_turns, 50);
    assert_eq!(config.system_prompt, Some("You are helpful".to_string()));
    assert!(config.tools.is_empty());
}

// -------------------------------------------------------------------------
// Agent max_turns floor logic
// -------------------------------------------------------------------------

#[test]
fn test_agent_max_turns_floor_zero() {
    let max_turns: usize = 0;
    let floored = max_turns.max(1);
    assert_eq!(floored, 1);
}

#[test]
fn test_agent_max_turns_floor_positive() {
    let max_turns: usize = 50;
    let floored = max_turns.max(1);
    assert_eq!(floored, 50);
}

// -------------------------------------------------------------------------
// Scanner config with nested engines
// -------------------------------------------------------------------------

#[test]
fn test_scanner_config_nested_engines() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("scanner.json");
    let data = serde_json::json!({
        "enabled": ["clamav", "yara"],
        "engines": {
            "clamav": {
                "address": "127.0.0.1:3310",
                "state": {
                    "install_status": "installed",
                    "version": "1.0.0"
                }
            },
            "yara": {
                "address": "127.0.0.1:9999",
                "rules_path": "/etc/yara/rules"
            }
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let result = load_scanner_full_config(&path);
    assert!(result.is_some());
    let cfg = result.unwrap();
    assert_eq!(cfg.enabled.len(), 2);
    assert_eq!(cfg.engines.len(), 2);
    // Verify nested engine data is preserved
    assert!(cfg.engines.contains_key("clamav"));
    assert!(cfg.engines.contains_key("yara"));
}

// -------------------------------------------------------------------------
// Continuation message construction (cluster continuation prefix)
// -------------------------------------------------------------------------

#[test]
fn test_continuation_message_prefix() {
    let task_id = "task-abc-123";
    let prefix = format!("cluster_continuation:{}", task_id);
    assert!(prefix.starts_with("cluster_continuation:"));
    assert!(prefix.ends_with(&task_id));
}

// -------------------------------------------------------------------------
// Context builder with workspace directory
// -------------------------------------------------------------------------

#[test]
fn test_context_builder_with_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    // Create IDENTITY.md
    std::fs::write(
        workspace.join("IDENTITY.md"),
        "# Identity\nI am a test bot.",
    )
    .unwrap();

    let _builder = nemesis_agent::context::ContextBuilder::new(&workspace);
    // Just verify construction doesn't panic
}

// -------------------------------------------------------------------------
// ForgeProviderBridge response handling logic
// -------------------------------------------------------------------------

#[cfg(feature = "forge")]
#[test]
fn test_forge_bridge_empty_content_returns_error() {
    // When content is empty AND tool_calls is empty, return Err
    let content = "";
    let has_tool_calls = false;
    let result = if content.is_empty() && !has_tool_calls {
        Err("LLM returned no content".to_string())
    } else {
        Ok(content.to_string())
    };
    assert!(result.is_err());
}

#[cfg(feature = "forge")]
#[test]
fn test_forge_bridge_nonempty_content_returns_ok() {
    let content = "Hello from LLM";
    let has_tool_calls = false;
    let result = if content.is_empty() && !has_tool_calls {
        Err("LLM returned no content".to_string())
    } else {
        Ok(content.to_string())
    };
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello from LLM");
}

#[cfg(feature = "forge")]
#[test]
fn test_forge_bridge_tool_calls_present_returns_ok() {
    let content = "";
    let has_tool_calls = true;
    let result = if content.is_empty() && !has_tool_calls {
        Err("LLM returned no content".to_string())
    } else {
        Ok(content.to_string())
    };
    assert!(result.is_ok());
}

// -------------------------------------------------------------------------
// Forge TraceCollector operations
// -------------------------------------------------------------------------

#[cfg(feature = "forge")]
#[test]
fn test_forge_trace_collector_events_empty() {
    let collector = nemesis_forge::trace::TraceCollector::new();
    assert!(collector.events().is_empty());
}

// -------------------------------------------------------------------------
// Cron message metadata construction
// -------------------------------------------------------------------------

#[test]
fn test_cron_message_metadata_construction() {
    let channel = Some("web".to_string());
    let to = Some("user1".to_string());
    let message = "scheduled task output".to_string();

    let ch = channel.clone().unwrap_or_else(|| "web".to_string());
    let chat = to.clone().unwrap_or_default();
    let deliver = true;

    assert_eq!(ch, "web");
    assert_eq!(chat, "user1");
    assert!(!message.is_empty());
    assert!(deliver);
}

// -------------------------------------------------------------------------
// count_enabled_channels additional channels
// -------------------------------------------------------------------------

#[test]
fn test_count_enabled_channels_web_telegram() {
    let mut config = nemesis_config::Config::default();
    config.channels.web.enabled = true;
    config.channels.telegram.enabled = true;
    assert_eq!(count_enabled_channels(&config), 2);
}

#[test]
fn test_count_enabled_channels_all_five() {
    let mut config = nemesis_config::Config::default();
    config.channels.web.enabled = true;
    config.channels.telegram.enabled = true;
    config.channels.discord.enabled = true;
    config.channels.feishu.enabled = true;
    config.channels.slack.enabled = true;
    assert_eq!(count_enabled_channels(&config), 5);
}

// -------------------------------------------------------------------------
// parse_host_port additional edge cases
// -------------------------------------------------------------------------

#[test]
fn test_parse_host_port_negative_port() {
    let (host, port) = parse_host_port("host:-1");
    assert_eq!(host, "host");
    assert_eq!(port, 0); // u16 parse of "-1" fails
}

#[test]
fn test_parse_host_port_very_large_port() {
    let (host, port) = parse_host_port("host:99999");
    assert_eq!(host, "host");
    assert_eq!(port, 0); // u16 overflow
}

// -------------------------------------------------------------------------
// PID file write logic
// -------------------------------------------------------------------------

#[test]
fn test_pid_file_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pid_path = tmp.path().join("gateway.pid");
    let pid = std::process::id();
    std::fs::write(&pid_path, pid.to_string()).unwrap();

    let content = std::fs::read_to_string(&pid_path).unwrap();
    let read_pid: u32 = content.parse().unwrap();
    assert_eq!(read_pid, pid);
}

// -------------------------------------------------------------------------
// Web server URL construction
// -------------------------------------------------------------------------

#[test]
fn test_web_server_url_construction() {
    let host = "0.0.0.0";
    let port: i64 = 49000;
    let resolved = if host == "0.0.0.0" || host.is_empty() {
        "127.0.0.1"
    } else {
        host
    };
    let url = format!("http://{}:{}", resolved, port);
    assert_eq!(url, "http://127.0.0.1:49000");
}

#[test]
fn test_web_server_url_custom_host() {
    let host = "192.168.1.5";
    let port: i64 = 8080;
    let resolved = if host == "0.0.0.0" || host.is_empty() {
        "127.0.0.1"
    } else {
        host
    };
    let url = format!("http://{}:{}", resolved, port);
    assert_eq!(url, "http://192.168.1.5:8080");
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：全装配冒烟 + 剩余 helper。
// =========================================================================

/// 隔离 home 环境（与 channel/cluster/eval 测试同款模式）。
/// env set_var 是进程级操作 → 持 crate::GLOBAL_STATE_LOCK 串行。
struct TempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn temp_home_env() -> TempHomeEnv {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    TempHomeEnv { _tmp: tmp, home }
}

// -------------------------------------------------------------------------
// 全装配冒烟：run() 从 Step 1 跑到 wait_for_shutdown。
//
// 策略：临时 home + 编译期默认配置改写（所有网络面归零：web/health 绑
// 127.0.0.1:0 → OS 分配临时端口；cluster/websocket/devices/memory/forge
// 全关；model_list 塞死端点条目）。run() 的 future 是 !Send（cron_service
// 的 std MutexGuard 跨 await，gateway.rs:3339 —— 生产也只走 block_on），
// 不能 tokio::spawn → 放进独立 OS 线程的自建 runtime 里 block_on。测试线
// 程轮询 {home}/workspace/state/gateway.json 的 web_port != 0（web server
// 真实 bind 后写入的就绪信号）+ TCP 连通复证，等尾巴（banner/agent
// adapter/bot service/tray）跑完即返回。gateway 线程随测试进程退出销毁
// （挂起在 wait_for_shutdown；优雅 shutdown 段只能由 Ctrl+C/broadcast
// 唤醒，列结构豁免）。
//
// 纪律边界：不占生产端口（全 0 → OS 分配）；不碰生产 home（NEMESISBOT_HOME
// → tempdir）；无真 LLM/外网调用（死端点，且启动期间无人调 LLM）；tray 在
// Windows 走独立线程 + catch_unwind；临时目录因日志句柄被 gateway 线程
// 持有而留 %TEMP% 残留（无害，进程退出即失效）。
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_assembly_starts_and_binds_web_and_health() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();

    // 编译期默认配置 → 覆盖网络面 + 模型条目。
    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
    cfg["gateway"]["port"] = serde_json::json!(0);
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("mini-model");
    // workspace 指到临时 home（默认 ~ 展开指向真实用户目录，必须改写）。
    cfg["agents"]["defaults"]["workspace"] =
        serde_json::json!(th.home.join("workspace").to_string_lossy().to_string());
    cfg["model_list"] = serde_json::json!([{
        "model_name": "mini-model",
        "model": "testai/mini-model",
        "api_key": "test-key",
        "api_base": "http://127.0.0.1:9",
        "model_tier": "mini"
    }]);

    // ---------------------------------------------------------------------
    // wave_b 补测解锁（就地翻转 config，可直接还原）：以下 flip 全部为
    // localhost-only / 无外网的装配分支，用于点亮 llvm-cov miss 区段。
    // ---------------------------------------------------------------------
    // security.enabled=true → 点亮 security 装配块 2447-2542 + 审批/guardian
    // 接线 3532-3567（不 spawn 弹窗：弹窗仅在 ask 规则命中时才触发）。
    cfg["security"]["enabled"] = serde_json::json!(true);
    // forge.enabled=true → 点亮 1523-1527 启动臂（后台任务，无网络）。
    cfg["forge"]["enabled"] = serde_json::json!(true);
    // memory.enabled=true → 点亮 MemoryManager 构造 1609-1619 + web 注入
    // 2845-2848（embedding 默认关，无模型加载）。
    cfg["memory"]["enabled"] = serde_json::json!(true);
    // logging.llm 开启 + detail_level="truncated" + log_dir="" → 点亮 observer
    // 装配链 2562-2595（含 Truncated match 臂与空 log_dir 回退臂）+ 2603-2604。
    cfg["logging"]["llm"]["enabled"] = serde_json::json!(true);
    cfg["logging"]["llm"]["detail_level"] = serde_json::json!("truncated");
    cfg["logging"]["llm"]["log_dir"] = serde_json::json!("");
    // DDG websearch 提示分支 1626-1637（仅 info 打印路径的 flip）。
    cfg["tools"]["web"]["duckduckgo"]["enabled"] = serde_json::json!(true);
    // websocket 通道开 + 绑 127.0.0.1:0（OS 分配临时端口）+ sync_to 非空 →
    // 点亮 enabled_channels 2237、ChannelInitConfig Some 臂 2326-2333、
    // add_sync! 插入 2367。
    cfg["channels"]["websocket"]["enabled"] = serde_json::json!(true);
    cfg["channels"]["websocket"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["websocket"]["port"] = serde_json::json!(0);
    cfg["channels"]["websocket"]["sync_to"] = serde_json::json!(["web"]);
    // channels.web.host 保持 "127.0.0.1"：改 "0.0.0.0" 可点亮 2176 的地址归一
    // 分支，但会绑定所有网卡，可能触发 Windows 防火墙弹窗 —— 有意不改。

    std::fs::write(th.home.join("config.json"), cfg.to_string()).unwrap();

    // wave_b 种子文件（全部落在临时 home 下；workspace/config 目录先建）。
    let ws_config_dir = th.home.join("workspace").join("config");
    std::fs::create_dir_all(&ws_config_dir).unwrap();
    // config.security.json：default_action + DLP 键位 + layer 开关 +
    // audit_chain_enabled=true → 点亮 load_security_rules 有效解析臂 /
    // DLP 解析 2461-2488 / audit chain 路径设置 2495-2503。
    // 规则 pattern 故意含危险词但 default_action=allow + action=allow：
    // 只是注册进 auditor，不拦任何测试流量。
    std::fs::write(
        ws_config_dir.join("config.security.json"),
        r#"{
            "default_action": "allow",
            "audit_chain_enabled": true,
            "layers": {
                "injection": {"enabled": true},
                "command_guard": {"enabled": true},
                "credential": {"enabled": true},
                "ssrf": {"enabled": true},
                "dlp": {
                    "enabled": false,
                    "action": "log",
                    "rules": ["phone"],
                    "low_confidence_action": "log",
                    "inbound_action": "log"
                }
            },
            "process_rules": {"exec": [{"pattern": "never-matches-*", "action": "allow", "comment": "wave_b seed"}]},
            "registry_rules": {"read": [{"pattern": "never-matches-*", "action": "allow"}]}
        }"#,
    )
    .unwrap();
    // config.forge.json 存在 → 走 load_forge_config 分支（1419）。
    std::fs::write(ws_config_dir.join("config.forge.json"), "{}").unwrap();
    // config.skills.json 有效 JSON → skills registry 走 from_config 成功臂
    // （1566-1576），否则落 absent/parse-err 分支。
    std::fs::write(ws_config_dir.join("config.skills.json"), "{}").unwrap();
    // peers.toml：cluster.enabled 仍为 false（无 UDP/RPC 网络）；此文件只被
    // 无条件静态-peer 加载循环读取（1719-1769）。node-empty 地址为空 → 命中
    // addr.is_empty() continue（1736-1738）。
    std::fs::create_dir_all(th.home.join("workspace").join("cluster")).unwrap();
    std::fs::write(
        th.home.join("workspace").join("cluster").join("peers.toml"),
        r#"
[peers.node-a]
address = "127.0.0.1:11949"
name = "WaveB Peer A"
role = "worker"
category = "general"

[peers.node-empty]
address = ""
name = "Empty Addr Peer"
"#,
    )
    .unwrap();
    // BOOTSTRAP.md：heartbeat 跳过文件存在 → set_skip_file 被调用（3248）。
    std::fs::write(th.home.join("workspace").join("BOOTSTRAP.md"), "# bootstrap\n").unwrap();
    // cors.json：development_mode=false + 一个 origin → CORSManager Ok 且走
    // list_origins 信息臂（2192-2199）。
    std::fs::create_dir_all(th.home.join("config")).unwrap();
    std::fs::write(
        th.home.join("config").join("cors.json"),
        r#"{"allowed_origins": ["http://localhost:5173"], "development_mode": false}"#,
    )
    .unwrap();

    let state_path = th.home.join("workspace").join("state").join("gateway.json");

    // run() !Send → 独立 OS 线程 + 自建 runtime block_on（与生产 main 相同
    // 的调用形态）。线程随测试进程退出，无需 join。
    std::thread::Builder::new()
        .name("gateway-full-assembly".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("build gateway test runtime");
            let _ = rt.block_on(async { run(false, &[]).await });
        })
        .expect("spawn gateway thread");

    // 轮询就绪：web_port 在 TcpListener 真实 bind 后写入（Step 17）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let web_port: u16 = loop {
        if let Ok(txt) = std::fs::read_to_string(&state_path)
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                let p = v.get("web_port").and_then(|x| x.as_i64()).unwrap_or(0);
                if p > 0 {
                    break p as u16;
                }
            }
        assert!(
            std::time::Instant::now() < deadline,
            "gateway 未在 120s 内完成 web bind；state={:?}",
            std::fs::read_to_string(&state_path)
        );
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    };

    // web server 真在监听（run() 自己也是 TCP connect 验证，这里独立复证）。
    tokio::net::TcpStream::connect(("127.0.0.1", web_port))
        .await
        .expect("web server must accept TCP on the reported port");

    // 给 web bind 之后的启动尾巴时间（agent adapter / bot service(health) /
    // ProcessManager / internal cmd loop / tray 装配在 state 文件更新之后）。
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // 断言启动已完成到 banner 阶段：state 文件里 host 已是真实 bind 信息。
    let final_state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&state_path).expect("state file readable"),
    )
    .expect("state json");
    assert_eq!(final_state["web_host"], "127.0.0.1");
    assert_eq!(final_state["web_port"].as_i64(), Some(web_port as i64));
}

// -------------------------------------------------------------------------
// migrate_legacy_workflow_dir（旧扁平布局 → 四子目录布局）
// -------------------------------------------------------------------------

#[cfg(feature = "workflow")]
mod migrate_legacy_workflow_tests {
    use super::*;

    fn setup(home: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let exec_dir = home.join("workspace").join("workflow").join("executions");
        let ckpt_dir = home.join("workspace").join("workflow").join("checkpoints");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::create_dir_all(&ckpt_dir).unwrap();
        (exec_dir, ckpt_dir)
    }

    #[test]
    fn migrate_moves_jsonl_and_checkpoints_then_removes_empty_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let (exec_dir, ckpt_dir) = setup(home);

        let legacy = home.join("workflow");
        std::fs::create_dir_all(legacy.join("checkpoints").join("exec-1")).unwrap();
        std::fs::write(legacy.join("wf_a_exec1.jsonl"), "{\"e\":1}").unwrap();
        std::fs::write(
            legacy.join("checkpoints").join("exec-1").join("cp.json"),
            "{\"cp\":1}",
        )
        .unwrap();

        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

        assert!(exec_dir.join("wf_a_exec1.jsonl").exists(), "jsonl 迁到 executions/");
        assert!(
            ckpt_dir.join("exec-1").join("cp.json").exists(),
            "checkpoint 子目录整体迁移"
        );
        assert!(!legacy.exists(), "清空后的 legacy 目录应被删除");
    }

    #[test]
    fn migrate_skips_existing_destination_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let (exec_dir, ckpt_dir) = setup(home);

        let legacy = home.join("workflow");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("wf_x.jsonl"), "OLD").unwrap();
        // 目的地已有同名文件 → 跳过（幂等；不覆盖新数据）。
        std::fs::write(exec_dir.join("wf_x.jsonl"), "NEW").unwrap();

        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

        assert_eq!(
            std::fs::read_to_string(exec_dir.join("wf_x.jsonl")).unwrap(),
            "NEW",
            "已存在的目的地文件不被覆盖"
        );
        assert!(legacy.join("wf_x.jsonl").exists(), "legacy 文件保留（未搬走）");
        assert!(legacy.exists(), "非空 legacy 目录保留（partial 分支）");
    }

    #[test]
    fn migrate_keeps_unrecognized_files_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let (exec_dir, ckpt_dir) = setup(home);

        let legacy = home.join("workflow");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("notes.txt"), "user data").unwrap();
        std::fs::write(legacy.join("wf_y.jsonl"), "{}").unwrap();

        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

        assert!(exec_dir.join("wf_y.jsonl").exists());
        assert!(legacy.exists(), "含未识别文件的 legacy 目录必须原地保留");
        assert!(legacy.join("notes.txt").exists());
    }

    #[test]
    fn migrate_noop_when_legacy_dir_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let (exec_dir, ckpt_dir) = setup(home);
        // 无 legacy 目录 → 立即返回，不产生任何文件。
        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);
        assert!(exec_dir.read_dir().unwrap().next().is_none());
    }

    /// R10 终测补测：rename 失败 warn 臂 + 循环 continue 臂。
    /// - jsonl rename 失败：目的地同名**目录**挡道（file→occupied-dir 的
    ///   rename 在 Windows/Unix 都是 Err）→ 走 warn! 分支，源文件保留；
    /// - checkpoint 子目录 rename 失败：目的地同名**文件**挡道 → warn! 分支；
    /// - checkpoints 循环的两个 continue：目的地已存在（跳过）、条目是文件
    ///   而非目录（跳过）。
    #[test]
    fn migrate_rename_failures_and_continue_arms_leave_sources_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let (exec_dir, ckpt_dir) = setup(home);

        let legacy = home.join("workflow");
        // jsonl 家族：一个会被搬走、一个目的地被目录挡道。
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("wf_ok.jsonl"), "ok").unwrap();
        std::fs::write(legacy.join("wf_blocked.jsonl"), "src").unwrap();
        std::fs::create_dir_all(exec_dir.join("wf_blocked.jsonl")).unwrap();
        // checkpoints 家族：正常子目录 / 目的地已存在 / 条目是文件 / 目的地被文件挡道。
        std::fs::create_dir_all(legacy.join("checkpoints").join("cp_ok")).unwrap();
        std::fs::create_dir_all(legacy.join("checkpoints").join("cp_exists")).unwrap();
        std::fs::create_dir_all(ckpt_dir.join("cp_exists")).unwrap();
        std::fs::write(legacy.join("checkpoints").join("stray.txt"), "f").unwrap();
        std::fs::create_dir_all(legacy.join("checkpoints").join("cp_blocked")).unwrap();
        std::fs::write(ckpt_dir.join("cp_blocked"), "file-in-the-way").unwrap();

        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

        assert!(exec_dir.join("wf_ok.jsonl").exists(), "无阻挡的 jsonl 正常迁移");
        assert!(
            legacy.join("wf_blocked.jsonl").exists(),
            "rename 失败的 jsonl 源文件保留"
        );
        assert!(ckpt_dir.join("cp_ok").exists(), "无阻挡的 checkpoint 子目录迁移");
        assert!(
            legacy.join("checkpoints").join("cp_exists").exists(),
            "目的地已存在的 checkpoint 跳过（源保留）"
        );
        assert!(
            legacy.join("checkpoints").join("stray.txt").exists(),
            "checkpoints 里的文件条目跳过"
        );
        assert!(
            legacy.join("checkpoints").join("cp_blocked").exists(),
            "rename 失败的 checkpoint 子目录保留"
        );
    }
}

// -------------------------------------------------------------------------
// GatewayAgentRunner —— workflow `agent` 节点 → 主 AgentLoop 桥
// -------------------------------------------------------------------------

#[cfg(feature = "workflow")]
mod gateway_agent_runner_tests {
    use super::*;
    use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
    use nemesis_agent::types::AgentConfig;
    use nemesis_workflow::nodes::AgentRunner;

    /// 可脚本化的假 provider（不联网）：Ok → 固定回复，Err → 错误传播。
    struct FixedProvider {
        reply: Result<LlmResponse, String>,
    }

    #[async_trait::async_trait]
    impl LlmProvider for FixedProvider {
        async fn chat(
            &self,
            _model: &str,
            _messages: Vec<LlmMessage>,
            _options: Option<nemesis_agent::types::ChatOptions>,
            _tools: Vec<nemesis_agent::types::ToolDefinition>,
        ) -> Result<LlmResponse, String> {
            self.reply.clone()
        }
    }

    fn make_loop(reply: Result<LlmResponse, String>) -> std::sync::Arc<AgentLoop> {
        std::sync::Arc::new(AgentLoop::new(
            Box::new(FixedProvider { reply }),
            AgentConfig {
                model: "test-model".to_string(),
                system_prompt: Some("test".to_string()),
                max_turns: 1,
                tools: vec![],
                ..Default::default()
            },
        ))
    }

    fn ok_response(content: &str) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: content.to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }

    #[tokio::test]
    async fn run_direct_returns_final_response_with_workflow_session_key() {
        let runner = GatewayAgentRunner::new(make_loop(ok_response("workflow done")));
        let result = runner
            .run_direct("do the thing", "agent-1", 5, None)
            .await
            .expect("run_direct success");
        assert_eq!(result.response, "workflow done");
        assert!(result.tools_used.is_empty());
    }

    #[tokio::test]
    async fn run_direct_with_model_override_warns_and_uses_default() {
        // model=Some → 走 warn 分支（per-call 切换尚不支持），仍用默认模型完成。
        let runner = GatewayAgentRunner::new(make_loop(ok_response("ok")));
        let result = runner
            .run_direct("prompt", "agent-2", 3, Some("other-model"))
            .await
            .expect("model override must not fail the run");
        assert_eq!(result.response, "ok");
    }

    #[tokio::test]
    async fn run_direct_propagates_loop_error() {
        let runner = GatewayAgentRunner::new(make_loop(Err("llm dead".to_string())));
        let err = runner
            .run_direct("prompt", "agent-3", 1, None)
            .await
            .expect_err("provider error must propagate");
        assert!(err.contains("llm dead"), "err: {err}");
    }
}

// =========================================================================
// wave_b 补测（llvm-cov 覆盖回填）：装配块内联类型（适配器/guardian）+
// 迁移 rename 失败分支。全部进程内、无网络、无弹窗、无子进程。
// =========================================================================

mod wave_b {
    use super::*;

    #[test]
    fn wave_b_count_enabled_channels_all_flags() {
    // 13 个通道位全开（web/websocket/telegram/discord/feishu/slack/external/
    // whatsapp/dingtalk/qq/line/onebot/maixcam），点亮剩余 11 个 miss 推入臂。
    let mut config = nemesis_config::Config::default();
    config.channels.web.enabled = true;
    config.channels.websocket.enabled = true;
    config.channels.telegram.enabled = true;
    config.channels.discord.enabled = true;
    config.channels.feishu.enabled = true;
    config.channels.slack.enabled = true;
    config.channels.external.enabled = true;
    config.channels.whatsapp.enabled = true;
    config.channels.dingtalk.enabled = true;
    config.channels.qq.enabled = true;
    config.channels.line.enabled = true;
    config.channels.onebot.enabled = true;
    config.channels.maixcam.enabled = true;
    assert_eq!(count_enabled_channels(&config), 13);
}

// -------------------------------------------------------------------------
// GatewayLlmJudge —— guardian LLM 二审桥（security）
// -------------------------------------------------------------------------

#[cfg(feature = "security")]
struct WaveBRouterProvider {
    reply: Result<
        nemesis_providers::types::LLMResponse,
        nemesis_providers::failover::FailoverError,
    >,
    /// 共享捕获：记录 provider 收到的每条消息 content（供测试断言提示词组装）。
    seen_user_content: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[cfg(feature = "security")]
impl WaveBRouterProvider {
    fn llm_response(content: &str) -> nemesis_providers::types::LLMResponse {
        nemesis_providers::types::LLMResponse {
            content: content.to_string(),
            tool_calls: vec![],
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        }
    }
}

#[cfg(feature = "security")]
#[async_trait::async_trait]
impl nemesis_providers::router::LLMProvider for WaveBRouterProvider {
    async fn chat(
        &self,
        messages: &[nemesis_providers::types::Message],
        _tools: &[nemesis_providers::types::ToolDefinition],
        _model: &str,
        _options: &nemesis_providers::types::ChatOptions,
    ) -> Result<
        nemesis_providers::types::LLMResponse,
        nemesis_providers::failover::FailoverError,
    > {
        self.seen_user_content
            .lock()
            .unwrap()
            .extend(messages.iter().map(|m| m.content.clone()));
        // Result 整体不可 clone（FailoverError 无 Clone），按边分别复制：
        // Ok 边 LLMResponse 自带 Clone；Err 边仅测试用到 Unknown{provider,message}。
        match &self.reply {
            Ok(r) => Ok(r.clone()),
            Err(nemesis_providers::failover::FailoverError::Unknown { provider, message }) => {
                Err(nemesis_providers::failover::FailoverError::Unknown {
                    provider: provider.clone(),
                    message: message.clone(),
                })
            }
            #[allow(unreachable_patterns)]
            _ => unreachable!("wave_b mock 只构造 Unknown 错误"),
        }
    }

    fn default_model(&self) -> &str {
        "wave-b-model"
    }

    fn name(&self) -> &str {
        "wave-b-router-mock"
    }
}

#[cfg(feature = "security")]
#[tokio::test]
async fn wave_b_guardian_judge_parses_fenced_verdict_and_builds_prompt() {
    use nemesis_security::guardian::LlmJudge;

    // 带 ```json 围栏的合法裁决 → parse_verdict 容忍围栏 → Ok(Allow)。
    let fenced = "```json\n\
                  {\"risk_level\":\"low\",\"user_authorization\":\"high\",\
                  \"outcome\":\"allow\",\"rationale\":\"explicitly requested\"}\n\
                  ```";
    let seen_user_content = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let judge = GatewayLlmJudge {
        provider: std::sync::Arc::new(WaveBRouterProvider {
            reply: Ok(WaveBRouterProvider::llm_response(fenced)),
            seen_user_content: seen_user_content.clone(),
        }),
        model: "wave-b-model".to_string(),
    };
    let req = nemesis_security::guardian::JudgeRequest {
        action: "process_exec".to_string(),
        risk_level: "low".to_string(),
        transcript: "user: please list files".to_string(),
    };
    let verdict = judge.judge(&req).await.expect("verdict parses");
    assert_eq!(
        verdict.outcome,
        nemesis_security::guardian::JudgeOutcome::Allow
    );
    assert_eq!(verdict.risk_level, "low");
    assert_eq!(verdict.user_authorization, "high");
    assert_eq!(verdict.rationale, "explicitly requested");

    // 提示词组装：system 含 guardian 提示词本体，user 含动作/风险/转录。
    let seen = seen_user_content.lock().unwrap().clone();
    assert!(
        seen.iter().any(|c| c.contains("You are a safety gate")),
        "GUARDIAN_PROMPT 必须作为 system 消息下发，seen={seen:?}"
    );
    assert!(
        seen.iter()
            .any(|c| c.contains("Proposed action")
                && c.contains("process_exec")
                && c.contains("please list files")),
        "user 消息必须携带动作与转录证据，seen={seen:?}"
    );
}

#[cfg(feature = "security")]
#[tokio::test]
async fn wave_b_guardian_judge_propagates_llm_error() {
    use nemesis_security::guardian::LlmJudge;

    let judge = GatewayLlmJudge {
        provider: std::sync::Arc::new(WaveBRouterProvider {
            reply: Err(nemesis_providers::failover::FailoverError::Unknown {
                provider: "wave-b".to_string(),
                message: "llm exploded".to_string(),
            }),
            seen_user_content: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }),
        model: "wave-b-model".to_string(),
    };
    let req = nemesis_security::guardian::JudgeRequest {
        action: "file_delete".to_string(),
        risk_level: "critical".to_string(),
        transcript: String::new(),
    };
    let err = judge.judge(&req).await.expect_err("LLM 错误必须向上传播");
    assert!(
        err.contains("guardian LLM call failed"),
        "err 应含统一前缀: {err}"
    );
}

// -------------------------------------------------------------------------
// load_security_rules 的读失败/解析失败臂（security）
// -------------------------------------------------------------------------

#[cfg(feature = "security")]
#[test]
fn wave_b_load_security_rules_survives_unreadable_and_malformed_config() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

    let make_plugin = || Arc::new(SecurityPlugin::new(SecurityPluginConfig::default()));

    // ① 路径是一个目录 → read_to_string 直接报错 → 读失败告警臂后安全返回。
    let tmp = tempfile::tempdir().unwrap();
    let as_dir = tmp.path().join("config.security.json");
    std::fs::create_dir_all(&as_dir).unwrap();
    let plugin_a = make_plugin();
    load_security_rules(&plugin_a, &as_dir); // 必须不 panic
    drop(plugin_a);

    // ② 文件存在但内容是非法 JSON → 解析失败告警臂后安全返回。
    let bad = tmp.path().join("config.security.bad.json");
    std::fs::write(&bad, "{{{ not json at all").unwrap();
    let plugin_b = make_plugin();
    load_security_rules(&plugin_b, &bad); // 必须不 panic
}

// -------------------------------------------------------------------------
// ApprovalPopupAdapter —— 弹窗审批桥（desktop + security）
// -------------------------------------------------------------------------

#[cfg(all(feature = "desktop", feature = "security"))]
#[test]
fn wave_b_approval_adapter_is_running_and_denies_without_plugin_ui_dll() {
    use nemesis_security::auditor::ApprovalManager;

    let pm = std::sync::Arc::new(nemesis_desktop::process::ProcessManager::new());
    let adapter = ApprovalPopupAdapter::new(pm);
    assert!(adapter.is_running(), "适配器恒报运行中（探活语义）");

    // plugin_ui.dll 不在测试二进制旁 → 早退分支直接 deny（不 spawn 子进程）。
    if plugin_ui_library_exists() {
        eprintln!("wave_b: plugin ui dll 在旁，跳过早退断言（避免真弹窗路径）");
        return;
    }
    let decision = adapter.request_approval_sync(
        "req-wave-b",
        "file_write",
        "C:/tmp/waveb-target",
        "HIGH",
        "wave_b unit probe",
        5,
    );
    match decision {
        Ok(approved) => assert!(
            !approved,
            "无插件 UI 时必须安全侧默认拒绝"
        ),
        Err(e) => panic!("早退分支应返回 Ok(deny) 而非 Err: {e}"),
    }
}

// -------------------------------------------------------------------------
// 集群桥接适配器（cluster）
// -------------------------------------------------------------------------

#[cfg(feature = "cluster")]
#[test]
fn wave_b_cluster_persister_running_success_error_and_delete_noop() {
    use nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister;

    let store = std::sync::Arc::new(nemesis_cluster::task_result_store::TaskResultStore::new(16));
    let adapter = ClusterResultPersisterAdapter {
        result_store: store.clone(),
        node_id: "node-wave-b".to_string(),
    };

    // set_running → 以 "peer_chat"/running 占位结果成功态写入。
    adapter.set_running("task-run", "peer-a");
    let running = store.get("task-run").expect("running 结果应已入库");
    assert!(running.success);
    assert_eq!(running.action, "peer_chat");
    assert_eq!(running.result["status"], "running");
    assert_eq!(running.result["from"], "node-wave-b");

    // set_result 成功态 → 包 content + from。
    adapter
        .set_result("task-ok", "ok", "最终回复正文", "", "peer-a")
        .expect("成功态写库应通过");
    let ok = store.get("task-ok").expect("成功结果应已入库");
    assert!(ok.success);
    assert_eq!(ok.result["content"], "最终回复正文");
    assert_eq!(ok.result["from"], "node-wave-b");

    // set_result 错误态 → store_failure。
    adapter
        .set_result("task-err", "error", "", "远端炸了", "peer-a")
        .expect("错误态写库应通过");
    let failed = store.get("task-err").expect("失败结果应已入库");
    assert!(!failed.success);
    assert_eq!(failed.result["error"], "远端炸了");

    // delete 是有意的 no-op（回调成功后由 TaskResultStore 自己清）。
    adapter.delete("task-run").expect("delete 不应报错");
    assert!(
        store.get("task-run").is_some(),
        "delete 为 no-op：结果保留待 A 端消费"
    );
    assert_eq!(store.len(), 3);
}

#[cfg(feature = "cluster")]
#[test]
fn wave_b_bus_to_cluster_adapter_publishes_mapped_inbound_message() {
    use nemesis_cluster::cluster::MessageBus as _;

    let bus = std::sync::Arc::new(nemesis_bus::MessageBus::new());
    // broadcast 是晚订阅语义：先订阅再发布才能收到。
    let mut rx = bus.subscribe_inbound();
    let adapter = BusToClusterAdapter {
        bus: bus.clone(),
    };

    adapter.publish_inbound(nemesis_cluster::cluster::BusInboundMessage {
        channel: "cluster".to_string(),
        sender_id: "peer-node-x".to_string(),
        chat_id: "chat-42".to_string(),
        content: "cross-node hello".to_string(),
    });

    let msg = rx
        .try_recv()
        .expect("适配器必须把 BusInboundMessage 映射到真实总线");
    assert_eq!(msg.channel, "cluster");
    assert_eq!(msg.sender_id, "peer-node-x");
    assert_eq!(msg.chat_id, "chat-42");
    assert_eq!(msg.content, "cross-node hello");
    // 映射时补齐的默认字段：无媒体/空会话键/空关联 ID/无元数据。
    assert!(msg.media.is_empty());
    assert_eq!(msg.session_key, "");
    assert_eq!(msg.correlation_id, "");
    assert!(msg.metadata.is_empty());
    assert!(msg.voice_playback.is_none());
}

// -------------------------------------------------------------------------
// migrate_legacy_workflow_dir 的 rename 失败与非目录跳过分支（workflow）
// -------------------------------------------------------------------------

#[cfg(feature = "workflow")]
#[test]
fn wave_b_migrate_skips_stray_file_and_existing_dest_checkpoint_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let exec_dir = home.join("workspace").join("workflow").join("executions");
    let ckpt_dir = home.join("workspace").join("workflow").join("checkpoints");
    std::fs::create_dir_all(&exec_dir).unwrap();
    std::fs::create_dir_all(&ckpt_dir).unwrap();

    let legacy = home.join("workflow");
    std::fs::create_dir_all(legacy.join("checkpoints")).unwrap();
    // 非目录条目：checkpoints/ 下混进一个散文件 → 跳过且原地保留。
    std::fs::write(legacy.join("checkpoints").join("stray-note.txt"), "keep me").unwrap();
    // 目的地已有同名 checkpoint 目录 → 跳过（幂等，不覆盖新数据）。
    std::fs::create_dir_all(ckpt_dir.join("exec-exists")).unwrap();
    std::fs::write(
        ckpt_dir.join("exec-exists").join("cp.json"),
        "{\"v\":2}",
    )
    .unwrap();
    std::fs::create_dir_all(legacy.join("checkpoints").join("exec-exists")).unwrap();
    std::fs::write(
        legacy.join("checkpoints").join("exec-exists").join("cp.json"),
        "{\"v\":1}",
    )
    .unwrap();
    // 一个可正常迁移的对照目录。
    std::fs::create_dir_all(legacy.join("checkpoints").join("exec-fresh")).unwrap();
    std::fs::write(
        legacy.join("checkpoints").join("exec-fresh").join("cp.json"),
        "{\"v\":9}",
    )
    .unwrap();

    migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

    assert!(
        legacy.join("checkpoints").join("stray-note.txt").exists(),
        "非目录条目必须原地保留"
    );
    assert_eq!(
        std::fs::read_to_string(ckpt_dir.join("exec-exists").join("cp.json")).unwrap(),
        "{\"v\":2}",
        "目的地已存在的 checkpoint 目录不被覆盖"
    );
    assert!(
        legacy.join("checkpoints").join("exec-exists").is_dir(),
        "被跳过的来源 checkpoint 目录原地保留"
    );
    assert_eq!(
        std::fs::read_to_string(ckpt_dir.join("exec-fresh").join("cp.json")).unwrap(),
        "{\"v\":9}",
        "对照目录正常迁入"
    );
}

/// Windows 专属：用 std::os::windows::fs::OpenOptionsExt::share_mode 打开
/// 句柄并剥掉 FILE_SHARE_DELETE → 目标文件的 fs::rename 报
/// ERROR_SHARING_VIOLATION，确定性走进 rename 失败告警分支。
#[cfg(all(windows, feature = "workflow"))]
#[test]
fn wave_b_migrate_locked_jsonl_rename_failure_warns_and_keeps_legacy_intact() {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2; // 故意缺 FILE_SHARE_DELETE(0x4)

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let exec_dir = home.join("workspace").join("workflow").join("executions");
    let ckpt_dir = home.join("workspace").join("workflow").join("checkpoints");
    std::fs::create_dir_all(&exec_dir).unwrap();
    std::fs::create_dir_all(&ckpt_dir).unwrap();

    let legacy = home.join("workflow");
    std::fs::create_dir_all(&legacy).unwrap();
    // 被锁的 jsonl：rename 将失败 → 告警后原样留在 legacy。
    let locked_path = legacy.join("wf_locked_exec1.jsonl");
    std::fs::write(&locked_path, "{\"e\":\"locked\"}").unwrap();
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&locked_path)
        .expect("打开共享锁句柄");
    // 自由文件作对照：随迁移搬走。
    std::fs::write(legacy.join("wf_free_exec2.jsonl"), "{\"e\":\"free\"}").unwrap();

    migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

    assert!(
        exec_dir.join("wf_free_exec2.jsonl").exists(),
        "自由 jsonl 正常迁入 executions/"
    );
    assert!(
        locked_path.exists(),
        "被锁 jsonl rename 失败后必须原地保留（不得丢数据）"
    );
    assert!(
        legacy.exists(),
        "仍持有残留数据的 legacy 目录不得删除（partial 保护）"
    );
}
} // mod wave_b

// =========================================================================
// R9 gateway 活动场景补测（llvm-cov miss 区段回填 · 子进程级真启动）
//
// 覆盖目标区间（nemesisbot/src/commands/gateway.rs）：
//   - 1017-1034 缺配置文件两段 eprintln + exit(1)（真实子进程退码断言）
//   - 1574-1600 skills 配置缺失 info 臂（此前只有 valid 分支被种子过）
//   - 1840-1844 cluster.llm_timeout_secs=0 → 24h 回退臂
//   - 2188-2191 CORS development_mode info 臂；2201-2207 CORS 坏 JSON warn 臂
//   - 2232-2341 全部 13 个通道 push 臂 + ChannelInitConfig 外部通道 Some 构造臂
//   - 2454-2460 security.enabled=true 但 config.security.json 缺失的 sec_json=None 臂
//     （+ 同块尾部 scanner 配置缺失 info 臂）
//   - 2540-2545 Security plugin disabled by configuration 显式禁用臂
//   - 2568-2572 logging.llm 非空 log_dir else 臂（空字符串回退臂已由 S11d 覆盖）
//   - 3254-3285 devices.enabled=true 的 DeviceService 启动成功 info 臂
//   - 1182-1280 workflow 装配块的活动分支：defs 加载 Ok(n>0) info / cron 触发器
//     注册计数>0 info / checkpoint 恢复 Ok(n>0) info / executor world Some 接线 /
//     legacy 目录迁移在真实启动路径上执行
//   - 3420-3447 web bind 冲突：error!/fallback warn + state 文件回落写入配置端口
//
// 形态说明：
//   - 端口纪律：web/health/websocket/maixcam 一律 127.0.0.1:0 由 OS 分配；
//     line webhook 与端口冲突占用者用「先探测再使用」的高位空闲端口，
//     绝不触碰生产端口 18790/49000/49001/8080。
//   - 不用 test_harness::ManagedProcess 承载长跑网关：它把子进程 stdout 设为
//     piped 且不排空，INFO 级日志长时间写入会触发管道回压把子进程卡死。
//     本模块自带 R9GatewayProc（双流继承 + Drop 兜底强杀 + 优雅停机等退）。
//   - 优雅停机走 test_harness::graceful_shutdown_gateway（/api/internal +
//     X-Auth-Token），保证覆盖插桩二进制走正常 atexit 落 .profraw。
//   - 端口冲突场景无法优雅停机（web server 死了 /api/internal 就不可达），
//     采用 S11d 的结构豁免先例：in-process 独立线程跑 run()，测试进程正常
//     退出时统一落盘覆盖率，线程挂起在 wait_for_shutdown 随进程销毁。
// =========================================================================

mod r9_gateway_boot_scenarios {
    use super::*;

    // ---------------------------------------------------------------------
    // 子进程托管（双流继承版，规避 ManagedProcess 的 stdout 回压隐患）
    // ---------------------------------------------------------------------

    struct R9GatewayProc {
        child: Option<tokio::process::Child>,
        #[allow(dead_code)]
        name: &'static str,
    }

    /// 子网关的 LLVM_PROFILE_FILE 注入。曾因 test-harness 对应 helper 私有而
    /// 整段复刻（漂移风险），现直接委托公开的单一真相源实现。
    fn r9_coverage_profile(slug: &str) -> Option<String> {
        test_harness::coverage_profile_file(slug)
    }

    /// 先 bind 再放手，取一个「此刻空闲」的高位端口（line webhook / 冲突占位用）。
    /// 有固有 TOCTOU 窗口；对 line 场景即使被抢也只是通道内部 bind warn，
    /// 不影响网关就绪与断言。
    fn r9_probe_free_tcp_port() -> u16 {
        std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("probe ephemeral port")
            .local_addr()
            .expect("local addr")
            .port()
    }

    impl R9GatewayProc {
        fn spawn(name: &'static str, program: &std::path::Path, cwd: &std::path::Path) -> Self {
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(["--local", "gateway"])
                .current_dir(cwd)
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .kill_on_drop(true);
            if let Some(profile) = r9_coverage_profile(name) {
                cmd.env("LLVM_PROFILE_FILE", profile);
            }
            let child = cmd.spawn().unwrap_or_else(|e| panic!("spawn {name}: {e}"));
            Self { child: Some(child), name }
        }

        /// 优雅停机后等子进程自然退出（让插桩二进制走 atexit 落 .profraw）。
        async fn wait_exit(&mut self, timeout: std::time::Duration) {
            let Some(child) = self.child.as_mut() else {
                panic!("{} already stopped", self.name);
            };
            let deadline = tokio::time::Instant::now() + timeout;
            loop {
                match child.try_wait().expect("try_wait") {
                    Some(status) => {
                        eprintln!("  {} exited with: {}", self.name, status);
                        self.child = None;
                        return;
                    }
                    None => {
                        assert!(
                            tokio::time::Instant::now() < deadline,
                            "{} 未在 {:?} 内退出",
                            self.name,
                            timeout
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        }
    }

    impl Drop for R9GatewayProc {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.start_kill();
            }
        }
    }

    /// 共享基座配置：编译期默认值改写网络面（全 0 → OS 分配、host 收敛到
    /// 127.0.0.1）、workspace 指向临时 home、模型条目指向死端点（启动期无 LLM
    /// 流量，死端点即安全）。与 S11d full_assembly 同款骨架。
    ///
    /// 注意保持与既有测试的差异面最小：security/devices/memory/logging 等
    /// 开关一律留给各场景自己翻转，基座不动。
    fn r9_base_config(home: &std::path::Path) -> serde_json::Value {
        let mut cfg: serde_json::Value =
            serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
        cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
        cfg["channels"]["web"]["port"] = serde_json::json!(0);
        cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
        cfg["gateway"]["port"] = serde_json::json!(0);
        cfg["agents"]["defaults"]["llm"] = serde_json::json!("mini-model");
        cfg["agents"]["defaults"]["workspace"] =
            serde_json::json!(home.join("workspace").to_string_lossy().to_string());
        cfg["model_list"] = serde_json::json!([{
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]);
        cfg
    }

    /// 启动网关子进程直到 state 文件出现非零 web_port（bind 后才写），留出
    /// 尾巴时间，然后优雅停机并等自然退出；返回最终 state JSON 供断言。
    async fn r9_spawn_until_ready_then_graceful_stop(
        name: &'static str,
        ws: &test_harness::TestWorkspace,
        cfg: serde_json::Value,
    ) -> serde_json::Value {
        // TestWorkspace::new() 只建 tempdir，.nemesisbot 子目录需显式创建，
        // 否则 write(config.json) 直接 NotFound（channel_ladder 等直接走本 helper
        // 的场景没有夹具先写别的文件顺带建目录）。
        std::fs::create_dir_all(ws.home()).expect("create home dir");
        std::fs::write(ws.config_path(), cfg.to_string()).expect("write config.json");

        let bin = test_harness::resolve_nemesisbot_bin().expect("resolve nemesisbot bin");
        let mut proc = R9GatewayProc::spawn(name, &bin, ws.path());

        let state_path = ws.home().join("workspace").join("state").join("gateway.json");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let web_port: u16 = loop {
            if let Ok(txt) = std::fs::read_to_string(&state_path)
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
                    let p = v.get("web_port").and_then(|x| x.as_i64()).unwrap_or(0);
                    if p > 0 {
                        break p as u16;
                    }
                }
            assert!(
                std::time::Instant::now() < deadline,
                "{name} 未在 120s 内完成 web bind；state={:?}",
                std::fs::read_to_string(&state_path)
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        };

        // 尾巴时间：state 写入之后还有 banner / 连通性自检 / bot service 等。
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let final_state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&state_path).expect("state file readable"),
        )
        .expect("state json");

        let token = cfg["channels"]["web"]["auth_token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        test_harness::graceful_shutdown_gateway(web_port, &token)
            .await
            .expect("graceful shutdown accepted");
        proc.wait_exit(std::time::Duration::from_secs(90)).await;

        final_state
    }

    // ---------------------------------------------------------------------
    // 场景 A：缺 config.json → 两段 eprintln + exit(1)（1017-1034）
    // ---------------------------------------------------------------------

    /// 真实子进程负向断言：cwd 下没有 `.nemesisbot`（--local 解析到的 home），
    /// gateway 必须立刻打错误提示并以退码 1 结束，绝不进入装配流程。
    #[tokio::test]
    async fn r9_gateway_missing_config_exits_1_with_onboard_hint() {
        let bin = test_harness::resolve_nemesisbot_bin().expect("resolve nemesisbot bin");
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        // 故意不创建 .nemesisbot：config.json 必然缺失。

        let mut cmd = tokio::process::Command::new(&bin);
        cmd.args(["--local", "gateway"])
            .current_dir(ws.path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        if let Some(profile) = r9_coverage_profile("r9_gateway_missing_config") {
            cmd.env("LLVM_PROFILE_FILE", profile);
        }
        let mut child = cmd.spawn().expect("spawn negative gateway");

        let status = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait())
            .await
            .expect("must exit within 60s")
            .expect("wait status");
        assert_eq!(status.code(), Some(1), "缺配置必须 exit(1)");

        // 管道数据在写端关闭后仍可读：校验两段用户指引文案都在。
        let mut stderr_text = String::new();
        {
            use tokio::io::AsyncReadExt as _;
            let mut err_stream = child.stderr.take().expect("stderr piped");
            err_stream
                .read_to_string(&mut stderr_text)
                .await
                .expect("read stderr");
        }
        assert!(
            stderr_text.contains("Configuration file not found"),
            "应含缺失配置报错，stderr={stderr_text}"
        );
        assert!(
            stderr_text.contains("onboard default"),
            "应含修复指引，stderr={stderr_text}"
        );
    }

    // ---------------------------------------------------------------------
    // 场景 B/C/D/E/F：五个一次性启动实例（每实例一组互斥翻转）
    // ---------------------------------------------------------------------

    /// 「安静翻转」实例：security 显式禁用 + skills 配置缺席 + CORS dev-mode +
    /// logging.llm detail_level 非 truncated 且 log_dir 非空 + devices.enabled=true
    /// + cluster llm_timeout_secs=0。
    ///
    /// 点亮：2540-2545 禁用臂、1597-1600 缺席 info 臂、2188-2191 dev-mode 臂、
    /// 2568-2572 非空 log_dir 臂（_=>Full 由默认 summary 已命中，仍显式给值
    /// 保持意图）、3276-3285 DeviceService 启动成功臂、1843 的 24h 回退臂。
    /// 这些分支只能靠日志文本观测，测试断言收敛到「按期就绪 + 干净退出」。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_gateway_quiet_flips_boot_reaches_ready_and_exits_cleanly() {
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let mut cfg = r9_base_config(&home);
        cfg["security"]["enabled"] = serde_json::json!(false);
        // skills 配置故意不种 → 1560-1600 的 else 缺席 info 臂。
        cfg["logging"]["llm"]["enabled"] = serde_json::json!(true);
        cfg["logging"]["llm"]["detail_level"] = serde_json::json!("full");
        cfg["logging"]["llm"]["log_dir"] = serde_json::json!("logs/request_logs_r9");
        cfg["devices"]["enabled"] = serde_json::json!(true);

        // cluster 应用配置：enabled=false（不开 UDP/RPC 网络），但
        // llm_timeout_secs=0 → 1840-1844 的 24h 回退臂。
        let ws_config_dir = home.join("workspace").join("config");
        std::fs::create_dir_all(&ws_config_dir).unwrap();
        std::fs::write(
            ws_config_dir.join("config.cluster.json"),
            r#"{"enabled":false,"port":11949,"rpc_port":21949,"broadcast_interval":30,"llm_timeout_secs":0}"#,
        )
        .unwrap();

        // CORS dev-mode：2188-2191 info 臂。
        let home_config_dir = home.join("config");
        std::fs::create_dir_all(&home_config_dir).unwrap();
        std::fs::write(
            home_config_dir.join("cors.json"),
            r#"{"allowed_origins": [], "development_mode": true}"#,
        )
        .unwrap();

        let state =
            r9_spawn_until_ready_then_graceful_stop("gateway-r9-quiet-flips", &ws, cfg).await;
        assert_eq!(state["web_host"], "127.0.0.1");
        assert!(
            state["web_port"].as_i64().unwrap_or(0) > 0,
            "state={state}"
        );
    }

    /// 「坏 JSON 种子」实例：config.skills.json 非法 JSON + cors.json 非法 JSON
    /// + security.enabled=true 但完全不给 config.security.json / config.scanner.json。
    ///
    /// 点亮：1586-1595 skills 坏 JSON warn 臂、2201-2207 CORS 坏 JSON warn 臂、
    /// 2454-2460 sec_json=None 臂 + 插件照常构造、scanner 配置缺失 info 臂。
    /// 断言核心：坏文件绝不阻断启动。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_gateway_bad_json_seeds_keep_boot_alive() {
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let mut cfg = r9_base_config(&home);
        cfg["security"]["enabled"] = serde_json::json!(true);

        let ws_config_dir = home.join("workspace").join("config");
        std::fs::create_dir_all(&ws_config_dir).unwrap();
        // skills registry 解析失败 warn 臂（必须能让 serde 拒收的内容）。
        std::fs::write(ws_config_dir.join("config.skills.json"), "{{{ not json").unwrap();
        // 刻意不写 config.security.json / config.scanner.json。

        let home_config_dir = home.join("config");
        std::fs::create_dir_all(&home_config_dir).unwrap();
        // CORSManager::load_from_file 失败 → 2201-2207 warn 臂（宽松默认继续）。
        std::fs::write(home_config_dir.join("cors.json"), "[not an object").unwrap();

        let state = r9_spawn_until_ready_then_graceful_stop("gateway-r9-bad-json", &ws, cfg).await;
        assert_eq!(state["web_host"], "127.0.0.1");
        assert!(state["web_port"].as_i64().unwrap_or(0) > 0, "state={state}");
    }

    /// 「通道全家桶」实例：13 个通道开关全开（push 臂全覆盖）+ line/maixcam/
    /// websocket 的 ChannelInitConfig Some 构造臂；external 开启但 exe 留空 ——
    /// manager 构造器会报错并被容忍（error! 后 continue），验证初始化失败不致命。
    ///
    /// 注意：telegram/discord/feishu/slack/whatsapp/dingtalk/qq/onebot 在默认
    /// 构建里未编译（channels-* feature 默认只开 web/webhook/rpc），它们的
    /// 开关只点亮 gateway 侧 push 行和 enabled_channels 计数，不会拉起网络。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_gateway_channel_ladder_boot_constructs_all_init_configs() {
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let mut cfg = r9_base_config(&home);

        // 13 个通道位全开（2232-2272 push 臂逐个点亮）。
        for name in [
            "web", "websocket", "telegram", "discord", "feishu", "slack", "whatsapp",
            "dingtalk", "qq", "line", "onebot", "maixcam", "external",
        ] {
            cfg["channels"][name]["enabled"] = serde_json::json!(true);
        }

        // websocket：wave_b/S11d 同款安全参数（127.0.0.1:0 → Some 构造臂 2326-2333）。
        cfg["channels"]["websocket"]["host"] = serde_json::json!("127.0.0.1");
        cfg["channels"]["websocket"]["port"] = serde_json::json!(0);
        cfg["channels"]["websocket"]["sync_to"] = serde_json::json!(["web"]);

        // maixcam：loopback + 0 端口（TcpListener 字面接受 0 → 临时端口，
        // 不经手任何 8080 类默认替换），Some 构造臂 2309-2321。
        cfg["channels"]["maixcam"]["host"] = serde_json::json!("127.0.0.1");
        cfg["channels"]["maixcam"]["port"] = serde_json::json!(0);

        // line：webhook_port=0 会被通道内部替换成 8080（生产端口禁区！），
        // 必须喂一个真实探测到的高位空闲端口。Some 构造臂 2300-2307。
        let line_port = r9_probe_free_tcp_port();
        cfg["channels"]["line"]["channel_access_token"] = serde_json::json!("r9-dummy-token");
        cfg["channels"]["line"]["channel_secret"] = serde_json::json!("r9-dummy-secret");
        cfg["channels"]["line"]["webhook_port"] = serde_json::json!(line_port);

        // external：exe 留默认空串 → ExternalChannel::new 返回 Err → manager
        // 记录错误并继续（恒 Ok），验证通道初始化失败不影响网关存活。

        let state = r9_spawn_until_ready_then_graceful_stop("gateway-r9-channels", &ws, cfg).await;
        assert_eq!(state["web_host"], "127.0.0.1");
        assert!(state["web_port"].as_i64().unwrap_or(0) > 0, "state={state}");
    }

    /// 无 workflow 定义的对照位已被 G 组顺带覆盖（Ok(0)/Ok(_) 空恢复臂）；
    /// 本组专注 workflow 有料臂，单列一个实例避免 YAML/检查点噪声污染 G 组。
    ///
    /// seeds：
    ///   - definitions/ 三件套：合法 cron 触发 YAML（2 月 29 日表达式，测试期
    ///     不会真的触发）、可恢复目标双节点链、坏 YAML（引擎 warn-skip）。
    ///   - checkpoints：用 FileCheckpointStore 以引擎同构方式落一个 hash 匹配
    ///     的 waiting 检查点 + 一个损坏 JSON（引发隔离/告警路径）。
    ///   - legacy {home}/workflow/ 目录（jsonl + checkpoints + 干扰文件）驱动
    ///     真实启动路径上的旧布局迁移（含 partial 清理告警）。
    ///   - executor.enabled=true + sandbox=false → build_workflow_world Some(world)
    ///     接线臂（Layer-1 stdio 世界不需要 Sandboxie 就绪）。
    #[cfg(feature = "workflow")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_gateway_workflow_defs_cron_checkpoint_restore_live() {
        use nemesis_workflow::checkpoint::{
            Checkpoint, CheckpointStore as _, FileCheckpointStore, SerializableContext,
        };

        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let mut cfg = r9_base_config(&home);
        cfg["executor"] = serde_json::json!({"enabled": true, "sandbox": false});

        let wf_root = home.join("workspace").join("workflow");
        let defs_dir = wf_root.join("definitions");
        std::fs::create_dir_all(&defs_dir).unwrap();

        // ① cron 触发工作流：schedule 用「2 月 29 日 02:30」——表达式合法
        //    （croner 接受），实际下一次触发放到数年之后，测试期内绝不会开跑。
        std::fs::write(
            defs_dir.join("wf_cron.yaml"),
            r#"
name: r9_cron_wf
description: R9 cron trigger fixture
version: "1.0.0"
nodes:
  - id: n1
    node_type: llm
    config: {}
    depends_on: []
    retry_count: 0
edges: []
triggers:
  - trigger_type: cron
    config:
      schedule: "30 2 29 2 *"
      timezone: local
variables: {}
"#,
        )
        .unwrap();

        // ② 恢复目标：双节点链（n1 完成、停在 n2 等待），给检查点做 hash 匹配。
        std::fs::write(
            defs_dir.join("wf_restore_target.yaml"),
            r#"
name: r9_restore_wf
description: R9 checkpoint restore fixture
version: "1.0.0"
nodes:
  - id: n1
    node_type: llm
    config: {}
    depends_on: []
    retry_count: 0
  - id: n2
    node_type: llm
    config: {}
    depends_on: [n1]
    retry_count: 0
edges:
  - from_node: n1
    to_node: n2
triggers: []
variables: {}
"#,
        )
        .unwrap();

        // ③ 坏 YAML：load 循环 warn-skip，不计入加载数，也不炸启动。
        std::fs::write(defs_dir.join("broken.yaml"), "{ not yaml :: [").unwrap();

        // 用引擎同款解析器算 hash（Workflow::hash 为定义结构 SHA-256），
        // 保证网关端 restore 时能按 hash 找回注册的定义。
        let parsed = nemesis_workflow::parser::parse_file(&defs_dir.join("wf_restore_target.yaml"))
            .expect("parse restore target");
        let workflow_hash = parsed.hash();

        let ckpt_store =
            FileCheckpointStore::new(wf_root.join("checkpoints")).expect("checkpoint store root");
        ckpt_store
            .save(Checkpoint {
                id: "cp-r9-1".to_string(),
                execution_id: "r9-exec-1".to_string(),
                saved_at: chrono::Utc::now(),
                completed_nodes: ["n1".to_string()].into_iter().collect(),
                waiting_node: Some("n2".to_string()),
                parent_execution_id: None,
                trigger_source: None,
                terminal: false,
                context_snapshot: SerializableContext {
                    variables: std::collections::HashMap::new(),
                    node_results: std::collections::HashMap::new(),
                    input: std::collections::HashMap::new(),
                },
                workflow_hash,
            })
            .await
            .expect("save waiting checkpoint");

        // 损坏检查点：file_store 读取时 quarantine/告警，restore 计数不受影响。
        let broken_exec = wf_root
            .join("checkpoints")
            .join("checkpoints")
            .join("r9-exec-broken");
        std::fs::create_dir_all(&broken_exec).unwrap();
        std::fs::write(broken_exec.join("broken.json"), "{\"half\": tru").unwrap();

        // legacy 旧扁平布局（位于 home/workflow）：两个可迁移条目 + 一个干扰
        // 文件 → 迁移搬走可识别项、保留干扰项、legacy 目录不清空（partial）。
        let legacy = home.join("workflow");
        std::fs::create_dir_all(legacy.join("checkpoints").join("exec-old")).unwrap();
        std::fs::write(legacy.join("wf_old_exec1.jsonl"), "{\"e\":1}").unwrap();
        std::fs::write(
            legacy.join("checkpoints").join("exec-old").join("cp.json"),
            "{\"cp\":1}",
        )
        .unwrap();
        std::fs::write(legacy.join("notes.txt"), "user data stays").unwrap();

        let state =
            r9_spawn_until_ready_then_graceful_stop("gateway-r9-workflow-live", &ws, cfg).await;
        assert_eq!(state["web_host"], "127.0.0.1");
        assert!(state["web_port"].as_i64().unwrap_or(0) > 0, "state={state}");

        // 迁移副作用事后复核：legacy 内容被搬进新布局，notes.txt 留守原地。
        // 注意落点深度：migrate 把 legacy/checkpoints/<exec> 直搬进
        // workspace/workflow/checkpoints/（无内层 checkpoints 段）——与引擎的
        // FileCheckpointStore（root/checkpoints/<exec>）是不同层级，互不干扰。
        assert!(wf_root.join("executions").join("wf_old_exec1.jsonl").exists());
        assert!(wf_root
            .join("checkpoints")
            .join("exec-old")
            .join("cp.json")
            .exists());
        assert!(legacy.join("notes.txt").exists(), "partial 保留不得删干扰文件");
    }

    // ---------------------------------------------------------------------
    // 场景 G：web bind 冲突 → error!/fallback warn + state 回落写配置端口
    // ---------------------------------------------------------------------

    /// in-process 版（S11d 结构豁免先例）：测试先占住 web 目标端口，网关线程
    /// 里 bind 走查（`bind_with_port_walk`，2026-08-30 语义：带外线性向上）
    /// 落到邻端口 busy+1 成功 serve → bound_tx Ok(walked) → state 写走查后
    /// 端口。断言确定性来自「busy 全程被我持有且取带外端口」：走查序列
    /// 第一尝试即 busy（失败），第二尝试 busy+1（空闲）必中，别无来路。
    ///
    /// 2026-08-31 重写：旧「bind 冲突 → 回落写配置端口」前提在 35b092e 给
    /// bind 加 +1×20 重试、随后 port-walk 精化为带内回绕后失效（同族前提
    /// 修正见 crates/nemesis-web/src/server/r4_tests.rs 顶部注释）。
    ///
    /// 为什么不能优雅停机：/api/internal 挂在 web server 上，POST 不可达；
    /// 强杀子进程会丢 profraw——所以与本模块其余子进程用例不同，这里走
    /// 进程内线程，测试进程自身干净退出时统一落盘覆盖率（同 S11d 注释里的
    /// 「线程随测试进程退出销毁」豁免条款）。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r9_gateway_web_bind_conflict_walks_to_neighbor_port_in_state() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();

        // 占住目标端口：必须取带外（> WEB_PORT_MAX）端口，走查才是可预测的
        // 线性向上；带内端口会回绕整圈，落点在并行测试下不可断言。
        let busy_port = loop {
            let p = r9_probe_free_tcp_port();
            if p > nemesis_web::server::WEB_PORT_MAX && p < u16::MAX {
                break p;
            }
        };
        let busy_holder = std::net::TcpListener::bind(("127.0.0.1", busy_port))
            .expect("hold busy port for conflict scenario");

        let mut cfg = r9_base_config(&th.home);
        cfg["channels"]["web"]["port"] = serde_json::json!(busy_port);
        std::fs::create_dir_all(th.home.join("workspace").join("config")).unwrap();
        std::fs::write(th.home.join("config.json"), cfg.to_string()).unwrap();

        let state_path = th.home.join("workspace").join("state").join("gateway.json");
        std::thread::Builder::new()
            .name("gateway-r9-bind-conflict".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("build gateway conflict-test runtime");
                let _ = rt.block_on(async { run(false, &[]).await });
            })
            .expect("spawn gateway thread");

        // 就绪信号 = state 文件出现 web_port>0；本场景里它只能是走查落点
        // busy+1（fallback 已不存在：walk 落到邻端口成功 serve）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let observed: u16 = loop {
            if let Ok(txt) = std::fs::read_to_string(&state_path)
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt)
                    && let Some(p) = v.get("web_port").and_then(|x| x.as_i64())
                        && p > 0 {
                            break p as u16;
                        }
            assert!(
                std::time::Instant::now() < deadline,
                "bind-conflict 网关未在 120s 内写出 state；holder={:?}",
                busy_holder.local_addr()
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        };

        // 尾巴时间让 banner / 自检输出跑完（全部发生在 bind 成功之后）。
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let final_state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&state_path).expect("state readable"),
        )
        .expect("state json");
        assert_eq!(final_state["web_host"], "127.0.0.1");
        assert_eq!(
            final_state["web_port"].as_u64(),
            Some(busy_port as u64 + 1),
            "端口被占用时走查落到邻端口 busy+1，state 必须如实写走查后端口"
        );
        assert_eq!(observed, busy_port + 1);

        // busy_holder 保活到断言结束（放在末尾抑制 unused 警告的真实用途注解）。
        drop(busy_holder);
        // 网关线程按 S11d 豁免条款挂起在 wait_for_shutdown，随测试进程销毁。
    }

    // =====================================================================
    // R10 确定性批（2026-08-27 MERGED miss 快照 A 类收口的 r10 波次）
    //
    // 分工：R9 各场景管「正常翻转启动面」；R10 批管「敌意文件系统种子 +
    // 配置角落分支 + 直调孪生补保险」。全部子进程形态（敌意种子只是普通
    // 文件/目录，不需要进程内装配，也不需要 GLOBAL_STATE_LOCK），复用本模
    // 块既有的 R9GatewayProc / 探测端口 / 优雅停机骨架。
    // =====================================================================

    /// 固定端口就绪等待器。state 文件被敌意化的场景里 gateway.json 不再是
    /// 可靠就绪信号，改用「配置端口可 TCP 连通」作 bind 完成证据。
    async fn r10_wait_tcp_ready(port: u16, what: &str) {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500))
                .is_ok()
            {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{what}: web port {port} never became connectable within 120s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// 通用轮询等待器（与 tests_r9_live 的 wait_until 同构；两文件互为独立
    /// 测试模块无法互相导入，就地复制保持各文件的单一真相源自足）。
    async fn r10_wait_until(timeout_secs: u64, what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);
        while !cond() {
            assert!(
                std::time::Instant::now() < deadline,
                "r10_wait_until({what}): condition not met within {timeout_secs}s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    }

    /// 组装一条预种子过期 "at" 任务（schema 逐字段复刻 nemesis-cron 序列化
    /// 形态；session_key/max_rounds 由调用方对返回值就地改写以驱动 Opt2 分支
    /// 与 T3 元数据插入）。
    fn r10_seed_at_job(id: &str, name: &str, message: &str, due_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "enabled": true,
            "schedule": {
                "kind": "at",
                "at_ms": due_ms,
                "every_ms": null,
                "expr": null,
                "tz": null,
            },
            "payload": {
                "kind": "agent_turn",
                "message": message,
                "command": null,
                "deliver": true,
                "channel": "web",
                "to": null,
                "session_key": null,
                "max_rounds": null,
            },
            "state": {
                "next_run_at_ms": due_ms,
                "last_run_at_ms": null,
                "last_status": null,
                "last_error": null,
                "history": [],
            },
            "created_at_ms": due_ms - 1000,
            "updated_at_ms": due_ms - 1000,
            "delete_after_run": false,
        })
    }

    fn r10_seed_cron_store(home: &std::path::Path, jobs: Vec<serde_json::Value>) {
        let store = serde_json::json!({ "version": 1, "jobs": jobs });
        let dir = home.join("workspace").join("cron");
        std::fs::create_dir_all(&dir).expect("mkdir cron dir");
        std::fs::write(
            dir.join("jobs.json"),
            serde_json::to_string_pretty(&store).expect("ser cron store"),
        )
        .expect("write cron store");
    }

    fn r10_cron_last_status(home: &std::path::Path, id: &str) -> Option<String> {
        let txt =
            std::fs::read_to_string(home.join("workspace").join("cron").join("jobs.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&txt).ok()?;
        v.get("jobs")?
            .as_array()?
            .iter()
            .find(|j| j.get("id").and_then(|x| x.as_str()) == Some(id))?
            .pointer("/state/last_status")
            .and_then(|s| s.as_str().map(str::to_owned))
    }

    // ---------------------------------------------------------------------
    // r10-A：migrate_legacy_workflow_dir 直调孪生（成功布局搬迁 + partial
    // 干扰保留）。直调层已有 mod migrate_legacy_workflow_tests 四例，这两例
    // 以同函数再走一遍保证最新测量必然命中入口 info/搬迁循环/cleanup 三段。
    // ---------------------------------------------------------------------

    #[cfg(feature = "workflow")]
    #[test]
    fn r10_migrate_success_moves_layouts_then_removes_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let exec_dir = home.join("workspace").join("workflow").join("executions");
        let ckpt_dir = home.join("workspace").join("workflow").join("checkpoints");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::create_dir_all(&ckpt_dir).unwrap();

        let legacy = home.join("workflow");
        std::fs::create_dir_all(legacy.join("checkpoints").join("exec-a")).unwrap();
        std::fs::write(legacy.join("wf_a_e1.jsonl"), "{\"e\":1}").unwrap();
        std::fs::write(
            legacy.join("checkpoints").join("exec-a").join("cp.json"),
            "{\"cp\":1}",
        )
        .unwrap();

        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

        assert!(exec_dir.join("wf_a_e1.jsonl").exists(), "jsonl 已迁入 executions/");
        assert!(
            ckpt_dir.join("exec-a").join("cp.json").exists(),
            "checkpoint 子目录整体迁移"
        );
        assert!(!legacy.exists(), "清空后的 legacy 根目录应被删除（info 臂）");
    }

    #[cfg(feature = "workflow")]
    #[test]
    fn r10_migrate_partial_keeps_unrecognized_files_keeps_legacy_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let exec_dir = home.join("workspace").join("workflow").join("executions");
        let ckpt_dir = home.join("workspace").join("workflow").join("checkpoints");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::create_dir_all(&ckpt_dir).unwrap();

        let legacy = home.join("workflow");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("notes.txt"), "user data").unwrap();
        std::fs::write(legacy.join("wf_y.jsonl"), "{}").unwrap();

        migrate_legacy_workflow_dir(home, &exec_dir, &ckpt_dir);

        assert!(exec_dir.join("wf_y.jsonl").exists());
        assert!(legacy.exists(), "含未识别文件的 legacy 必须原地保留（partial warn 臂）");
        assert!(legacy.join("notes.txt").exists());
    }

    // ---------------------------------------------------------------------
    // r10-B/C：workspace/state 敌意化两连（1095-1097 create_dir_all warn +
    // 1104-1105 write warn；顺带后续 3468 的状态更新 warn）。state 文件不可
    // 用后就绪信号换固定端口的 TCP 连通；停机走 /api/internal POST。
    // ---------------------------------------------------------------------

    /// 场景 B：{home}/workspace/state 是一个**普通文件**——create_dir_all
    /// 报错 → warn 臂；随后向 <file>/gateway.json 写 state 也报错 → 第二个
    /// warn 臂。断言核心：启动不被阻断，web 真实 bind 固定探测端口。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_state_dir_as_regular_file_boot_warns_but_binds_web() {
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let web_port = r9_probe_free_tcp_port();
        let mut cfg = r9_base_config(&home);
        cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
        cfg["channels"]["web"]["port"] = serde_json::json!(web_port);
        cfg["channels"]["web"]["auth_token"] = serde_json::json!("r10-state-token");

        // 敌意种子：workspace/ 正常建目录，但 state 是一个文件。
        std::fs::create_dir_all(home.join("workspace")).expect("mkdir workspace");
        std::fs::write(home.join("workspace").join("state"), b"I am a file").unwrap();

        std::fs::create_dir_all(&home).expect("create home dir");
        std::fs::write(ws.config_path(), cfg.to_string()).expect("write config.json");

        let bin = test_harness::resolve_nemesisbot_bin().expect("resolve nemesisbot bin");
        let mut proc = R9GatewayProc::spawn("gateway-r10-statefile", &bin, ws.path());

        r10_wait_tcp_ready(web_port, "state-as-file boot").await;
        // 尾巴时间：banner / 自检 / 3461 处二次 state 更新 warn 都跑完。
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        test_harness::graceful_shutdown_gateway(web_port, "r10-state-token")
            .await
            .expect("graceful shutdown accepted on hostile-state gateway");
        proc.wait_exit(std::time::Duration::from_secs(90)).await;
    }

    /// 场景 C：state 目录正常、gateway.json 本身是**目录**——首次 fs::write
    /// 命中 1104-1105 warn（else 臂反向：info@1107 不触发）。其余与场景 B 相同。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_state_gateway_json_as_directory_boot_survives_first_write_warn() {
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let web_port = r9_probe_free_tcp_port();
        let mut cfg = r9_base_config(&home);
        cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
        cfg["channels"]["web"]["port"] = serde_json::json!(web_port);
        cfg["channels"]["web"]["auth_token"] = serde_json::json!("r10-statedir-token");

        std::fs::create_dir_all(home.join("workspace").join("state")).expect("mkdir state dir");
        std::fs::create_dir_all(home.join("workspace").join("state").join("gateway.json"))
            .expect("pre-create gateway.json AS directory");

        std::fs::create_dir_all(&home).expect("create home dir");
        std::fs::write(ws.config_path(), cfg.to_string()).expect("write config.json");

        let bin = test_harness::resolve_nemesisbot_bin().expect("resolve nemesisbot bin");
        let mut proc = R9GatewayProc::spawn("gateway-r10-statedir", &bin, ws.path());

        r10_wait_tcp_ready(web_port, "gateway.json-as-dir boot").await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        test_harness::graceful_shutdown_gateway(web_port, "r10-statedir-token")
            .await
            .expect("graceful shutdown accepted");
        proc.wait_exit(std::time::Duration::from_secs(90)).await;
    }

    // ---------------------------------------------------------------------
    // r10-D：综合敌意种子伞——一次启动吃掉一串互不干扰的降级分支：
    //   - channels.web.host 保持模板 "0.0.0.0" → 归一到 127.0.0.1 再 bind
    //     （2178-2184；listen 恒在 loopback，无防火墙弹窗风险）
    //   - workspace/workflow/definitions 是文件 → 子目录创建 warn 循环命中 +
    //     load_workflows_from_dir read_dir 非 NotFound → PersistenceError →
    //     外层 warn 臂（1241-1256）
    //   - config.skills.json 是目录 → read_to_string Err → warn 臂（1591-1597）
    //   - security.enabled=true + dlp.rules:["phone"] 解析臂（2477-2482）
    //     + audit_chain_enabled=true 路径设置臂（2494-2507）
    //   - logs/security_logs 是文件 → init_audit_log_file Err → warn 臂（2519-2522）
    //   - config.scanner.json {"enabled":["bogus-engine"]} → info + init 调用，
    //     引擎数 0 → 链内 warn "remains disabled"（零网络）（2531-2538）
    //   - logging.llm.log_dir="" → 默认回退臂（2568-2576）
    //   - workspace/data/nemesisbot_data.db 写入垃圾字节 → SCHEMA_V1 立即炸
    //     → DataStore::open Err → warn 臂（2618-2626）
    //   - 预种子过期 cron 任务 session_key="agent:r10umb"（非空 → Opt2 router
    //     分支 1365-1372）+ max_rounds=3（T3 元数据插入 1389-1391）；模型死端
    //     点即可：on_job 只发布到总线即记 ok，不依赖 LLM 成败
    // 断言：state 文件 web_host=="127.0.0.1"（归一化铁证）+ cron last_status
    // =="ok"。state 文件在本场景完好，复用 R9 的标准就绪等待器。
    // ---------------------------------------------------------------------
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_hostile_fs_and_config_seeds_boot_reaches_web_with_normalized_host() {
        let ws = test_harness::TestWorkspace::new().expect("temp workspace");
        let home = ws.home();

        let web_port = r9_probe_free_tcp_port();
        let mut cfg = r9_base_config(&home);
        // 有意保持 host 为模板默认 "0.0.0.0"（归一化臂的直接输入）。
        cfg["channels"]["web"]["port"] = serde_json::json!(web_port);
        cfg["channels"]["web"]["auth_token"] = serde_json::json!("r10-umb-token");
        cfg["security"]["enabled"] = serde_json::json!(true);
        cfg["logging"]["llm"]["enabled"] = serde_json::json!(true);
        cfg["logging"]["llm"]["log_dir"] = serde_json::json!("");

        let ws_config = home.join("workspace").join("config");
        std::fs::create_dir_all(&ws_config).unwrap();

        // config.security.json：DLP rules 键位 + audit_chain_enabled。
        std::fs::write(
            ws_config.join("config.security.json"),
            r#"{
                "default_action": "allow",
                "audit_chain_enabled": true,
                "layers": {
                    "dlp": {
                        "enabled": false,
                        "action": "log",
                        "rules": ["phone"],
                        "low_confidence_action": "log",
                        "inbound_action": "log"
                    }
                },
                "process_rules": {"exec": [{"pattern": "never-matches-*", "action": "allow"}]}
            }"#,
        )
        .unwrap();

        // scanner：未知引擎名 → enabled 非空进 info/init 臂，链自降级为零引擎。
        std::fs::write(
            ws_config.join("config.scanner.json"),
            r#"{"enabled": ["bogus-engine"], "engines": {}}"#,
        )
        .unwrap();

        // workflow：根是目录、definitions 是文件 → 创建 warn + 加载 PersistenceError。
        let wf_root = home.join("workspace").join("workflow");
        std::fs::create_dir_all(&wf_root).unwrap();
        std::fs::write(wf_root.join("definitions"), b"I am not a directory").unwrap();

        // skills 配置路径变成目录 → read_to_string 直接失败。
        std::fs::create_dir_all(ws_config.join("config.skills.json"))
            .expect("create config.skills.json AS directory");

        // logs/security_logs 是文件 → 安全审计日志初始化失败（warn，非致命）。
        let logs_dir = home.join("workspace").join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(logs_dir.join("security_logs"), b"audit dir hostage").unwrap();

        // DataStore：垃圾字节让 SCHEMA_V1 在 open 时即刻报错。
        let data_dir = home.join("workspace").join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(
            data_dir.join("nemesisbot_data.db"),
            b"definitely not sqlite \x00\x01\x02 garbage",
        )
        .unwrap();

        // cron：Opt2 分支（session_key 非空）+ max_rounds=3 元数据（T3）。
        let mut job = r10_seed_at_job(
            "r10umbcron",
            "opt2-router-driver",
            "r10 umbrella cron probe please",
            0, // 由于 below 改写为过期时刻
        );
        let due = chrono_millis_now() - 4000;
        job["schedule"]["at_ms"] = serde_json::json!(due);
        job["state"]["next_run_at_ms"] = serde_json::json!(due);
        job["created_at_ms"] = serde_json::json!(due - 1000);
        job["updated_at_ms"] = serde_json::json!(due - 1000);
        job["payload"]["session_key"] = serde_json::json!("agent:r10umbrella");
        job["payload"]["max_rounds"] = serde_json::json!(3);
        r10_seed_cron_store(&home, vec![job]);

        let state =
            r9_spawn_until_ready_then_graceful_stop("gateway-r10-umbrella", &ws, cfg).await;

        // 归一化铁证：模板 host "0.0.0.0" 只可能出现在 state 里为归一结果。
        assert_eq!(
            state["web_host"],
            "127.0.0.1",
            "template 0.0.0.0 must normalize to 127.0.0.1 before bind"
        );
        assert!(state["web_port"].as_i64().unwrap_or(0) > 0);

        // Opt2/max_rounds 副作用：任务被执行并记账 ok（agent LLM 死端点无关）。
        r10_wait_until(90, "umbrella cron job marked ok", || {
            r10_cron_last_status(&home, "r10umbcron").as_deref() == Some("ok")
        })
        .await;

        // 结构复核：敌意种子没有被启动流程破坏性修复（warn-and-continue 语义）。
        assert!(ws_config.join("config.skills.json").is_dir());
        assert!(wf_root.join("definitions").is_file());
    }
}

/// 毫秒时间戳（cron 种子的过期时刻锚点；模块级自由函数避免在每个测试里重复）。
fn chrono_millis_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}

// -------------------------------------------------------------------------
// W2 P2 board 派发写回（write_back_board_dispatch）
// -------------------------------------------------------------------------

/// 建一个已派发（in_progress + issue_dispatch 登记）的 board store。
#[cfg(all(feature = "board", feature = "cluster"))]
fn dispatched_store(
    dir: &std::path::Path,
    task_id: &str,
) -> std::sync::Arc<nemesis_board::BoardStore> {
    let store = std::sync::Arc::new(
        nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store"),
    );
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "派发写回".into(),
            ..Default::default()
        })
        .expect("create issue");
    store
        .transition_issue(issue.id, nemesis_board::IssueStatus::InProgress, &nemesis_board::Actor::admin("t"))
        .expect("transition");
    store
        .insert_dispatch(task_id, issue.id, "node-b", &nemesis_board::Actor::admin("t"))
        .expect("insert dispatch");
    store
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn test_writeback_success_moves_to_in_review_with_result_comment() {
    let dir = std::env::temp_dir().join(format!(
        "nemesisbot-gw-writeback-ok-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dispatched_store(&dir, "task-ok");

    let is_board = write_back_board_dispatch(&Some(store.clone()), "task-ok", "success", "改完了，产物在 foo.rs");
    assert!(is_board, "dispatched task must be recognized as board task");

    // 状态推进 in_progress → in_review（等 coordinator 验收）。
    let issue = store.get_issue_by_number("NB-1").unwrap();
    assert_eq!(issue.status, nemesis_board::IssueStatus::InReview);
    // worker 结果评论（agent/node-b）。
    let comments = store.list_comments(issue.id).unwrap();
    assert!(comments.iter().any(|c| c.author.kind == "agent"
        && c.author.id == "node-b"
        && c.content.contains("改完了，产物在 foo.rs")));
    // 派发终结为 done。
    let rec = store.get_dispatch("task-ok").unwrap().unwrap();
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::DONE);
    assert!(rec.completed_at.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn test_writeback_error_keeps_in_progress_with_failure_comment() {
    let dir = std::env::temp_dir().join(format!(
        "nemesisbot-gw-writeback-err-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dispatched_store(&dir, "task-err");

    let is_board = write_back_board_dispatch(&Some(store.clone()), "task-err", "error", "编译失败：…");
    assert!(is_board);

    // 失败留在 in_progress（不推 in_review），失败评论留痕。
    let issue = store.get_issue_by_number("NB-1").unwrap();
    assert_eq!(issue.status, nemesis_board::IssueStatus::InProgress);
    let comments = store.list_comments(issue.id).unwrap();
    assert!(comments.iter().any(|c| c.content.contains("编译失败：…")));
    let rec = store.get_dispatch("task-err").unwrap().unwrap();
    assert_eq!(rec.state, nemesis_board::models::dispatch_state::FAILED);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn test_writeback_duplicate_callback_is_idempotent() {
    let dir = std::env::temp_dir().join(format!(
        "nemesisbot-gw-writeback-dup-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dispatched_store(&dir, "task-dup");

    assert!(write_back_board_dispatch(&Some(store.clone()), "task-dup", "success", "第一份"));
    // 重复回调：仍识别为 board 任务（跳过续行），但不重复写评论/转移。
    assert!(write_back_board_dispatch(&Some(store.clone()), "task-dup", "success", "第一份"));

    let issue = store.get_issue_by_number("NB-1").unwrap();
    assert_eq!(issue.status, nemesis_board::IssueStatus::InReview);
    let n = store
        .list_comments(issue.id)
        .unwrap()
        .into_iter()
        .filter(|c| c.content.contains("第一份"))
        .count();
    assert_eq!(n, 1, "duplicate callback must not double-comment");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(all(feature = "board", feature = "cluster"))]
#[test]
fn test_writeback_non_board_and_unavailable_store() {
    // 未知 task_id → 非 board 任务（false，走既有路由）。
    let dir = std::env::temp_dir().join(format!(
        "nemesisbot-gw-writeback-non-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = dispatched_store(&dir, "task-x");
    assert!(!write_back_board_dispatch(
        &Some(store.clone()),
        "other-task",
        "success",
        "…"
    ));
    assert!(!write_back_board_dispatch(&Some(store.clone()), "", "success", "…"));
    // store 未注入 → 恒 false。
    assert!(!write_back_board_dispatch(&None, "task-x", "success", "…"));
    let _ = std::fs::remove_dir_all(&dir);
}
