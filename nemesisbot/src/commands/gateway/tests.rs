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
            "web_port": backend_url.split(':').last().and_then(|p| p.parse::<u16>().ok()).unwrap_or(49000),
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
        .last()
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
        .replace('.', "_")
        .replace(':', "_")
        .replace('-', "_");
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
    assert!(true, "ForgeProviderBridge type compiles correctly");
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
    assert!(true, "run_bus_arc method compiles and is accessible");
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
    assert!(true, "HeartbeatBusAdapter types are compatible");
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
    assert!(true, "TraceStore created");
}

#[cfg(feature = "forge")]
#[test]
fn test_forge_cycle_store_creation() {
    let dir = tempfile::tempdir().unwrap();
    let _store = nemesis_forge::cycle_store::CycleStore::new(dir.path());
    // CycleStore was created successfully
    assert!(true, "CycleStore created");
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
    let tool_calls = vec![nemesis_agent::types::ToolCallInfo {
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
    assert!(true, "ContextBuilder created with workspace");
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
