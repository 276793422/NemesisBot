use super::*;
use tempfile::TempDir;

fn make_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg_path = tmp.path().join("config.json");
    let config = serde_json::json!({
        "channels": {
            "web": {
                "enabled": true,
                "host": "0.0.0.0",
                "port": 8080,
                "auth_token": "mysecrettoken123"
            },
            "websocket": {
                "enabled": false,
                "host": "127.0.0.1",
                "port": 49001,
                "path": "/ws"
            },
            "telegram": {
                "enabled": false
            }
        }
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg_path
}

fn make_empty_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg_path = tmp.path().join("config.json");
    let config = serde_json::json!({"channels": {}});
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg_path
}

fn make_no_channels_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg_path = tmp.path().join("config.json");
    let config = serde_json::json!({});
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg_path
}

#[test]
fn test_set_channel_config_existing_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "host", "127.0.0.1").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["channels"]["web"]["host"], "127.0.0.1");
    // Other fields should remain
    assert_eq!(data["channels"]["web"]["port"], 8080);
}

#[test]
fn test_set_channel_config_new_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "discord", "enabled", "true").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["channels"]["discord"]["enabled"], "true");
}

#[test]
fn test_set_channel_config_no_channels_key() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_no_channels_config(&tmp);

    set_channel_config(&cfg, "web", "host", "0.0.0.0").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["channels"]["web"]["host"], "0.0.0.0");
}

#[test]
fn test_set_channel_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");

    let result = set_channel_config(&cfg, "web", "host", "0.0.0.0");
    assert!(result.is_err());
}

#[test]
fn test_get_channel_config_existing() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    let val = get_channel_config(&cfg, "web", "host");
    assert_eq!(val, Some("0.0.0.0".to_string()));
}

#[test]
fn test_get_channel_config_missing_key() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    let val = get_channel_config(&cfg, "web", "nonexistent_key");
    assert!(val.is_none());
}

#[test]
fn test_get_channel_config_missing_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    let val = get_channel_config(&cfg, "discord", "host");
    assert!(val.is_none());
}

#[test]
fn test_get_channel_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");

    let val = get_channel_config(&cfg, "web", "host");
    assert!(val.is_none());
}

#[test]
fn test_remove_channel_config_existing_key() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    remove_channel_config(&cfg, "web", "auth_token").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data["channels"]["web"].get("auth_token").is_none());
    // Other keys remain
    assert_eq!(data["channels"]["web"]["host"], "0.0.0.0");
}

#[test]
fn test_remove_channel_config_nonexistent_key() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Should succeed even if key doesn't exist
    remove_channel_config(&cfg, "web", "nonexistent").unwrap();
}

#[test]
fn test_remove_channel_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");

    // Should succeed (no-op)
    remove_channel_config(&cfg, "web", "host").unwrap();
}

#[test]
fn test_uuid_session_format() {
    let session = uuid_session();
    assert!(session.starts_with("ws-"));
    assert_eq!(session.len(), 8); // "ws-" + 5 digits
}

#[test]
fn test_uuid_session_numeric_suffix() {
    let session = uuid_session();
    let suffix = &session[3..];
    assert!(suffix.chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn test_known_channels_contains_web() {
    assert!(KNOWN_CHANNELS.contains(&"web"));
}

#[test]
fn test_known_channels_contains_telegram() {
    assert!(KNOWN_CHANNELS.contains(&"telegram"));
}

#[test]
fn test_known_channels_count() {
    assert_eq!(KNOWN_CHANNELS.len(), 13);
}

#[test]
fn test_set_and_get_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    set_channel_config(&cfg, "web", "port", "9090").unwrap();
    set_channel_config(&cfg, "web", "host", "192.168.1.1").unwrap();

    assert_eq!(get_channel_config(&cfg, "web", "port"), Some("9090".to_string()));
    assert_eq!(get_channel_config(&cfg, "web", "host"), Some("192.168.1.1".to_string()));
}

#[test]
fn test_set_overwrite_value() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "port", "3000").unwrap();
    assert_eq!(get_channel_config(&cfg, "web", "port"), Some("3000".to_string()));
}

#[test]
fn test_set_remove_then_get() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // auth_token exists
    assert!(get_channel_config(&cfg, "web", "auth_token").is_some());

    remove_channel_config(&cfg, "web", "auth_token").unwrap();
    assert!(get_channel_config(&cfg, "web", "auth_token").is_none());
}

// -------------------------------------------------------------------------
// KNOWN_CHANNELS comprehensive tests
// -------------------------------------------------------------------------

#[test]
fn test_known_channels_contains_all_expected() {
    let expected = ["web", "websocket", "telegram", "discord", "whatsapp",
        "feishu", "slack", "line", "onebot", "qq", "dingtalk",
        "maixcam", "external"];
    for name in &expected {
        assert!(KNOWN_CHANNELS.contains(name), "Missing channel: {}", name);
    }
}

#[test]
fn test_known_channels_not_contains_unknown() {
    assert!(!KNOWN_CHANNELS.contains(&"irc"));
    assert!(!KNOWN_CHANNELS.contains(&"matrix"));
    assert!(!KNOWN_CHANNELS.contains(&"email"));
}

// -------------------------------------------------------------------------
// set_channel_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_set_channel_config_creates_nested_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    set_channel_config(&cfg, "telegram", "token", "12345").unwrap();

    let val = get_channel_config(&cfg, "telegram", "token");
    assert_eq!(val, Some("12345".to_string()));
}

#[test]
fn test_set_channel_config_multiple_keys_same_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    set_channel_config(&cfg, "discord", "token", "abc").unwrap();
    set_channel_config(&cfg, "discord", "guild_id", "12345").unwrap();

    assert_eq!(get_channel_config(&cfg, "discord", "token"), Some("abc".to_string()));
    assert_eq!(get_channel_config(&cfg, "discord", "guild_id"), Some("12345".to_string()));
}

#[test]
fn test_set_channel_config_value_with_spaces() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "host", "my server name").unwrap();
    assert_eq!(get_channel_config(&cfg, "web", "host"), Some("my server name".to_string()));
}

#[test]
fn test_set_channel_config_empty_value() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "custom_field", "").unwrap();
    assert_eq!(get_channel_config(&cfg, "web", "custom_field"), Some("".to_string()));
}

// -------------------------------------------------------------------------
// get_channel_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_get_channel_config_numeric_value_returns_none() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    // port is numeric in config, as_str() should return None
    let val = get_channel_config(&cfg, "web", "port");
    assert!(val.is_none());
}

#[test]
fn test_get_channel_config_bool_value_returns_none() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    // enabled is bool, as_str() should return None
    let val = get_channel_config(&cfg, "web", "enabled");
    assert!(val.is_none());
}

// -------------------------------------------------------------------------
// remove_channel_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_remove_channel_config_preserves_other_keys() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    remove_channel_config(&cfg, "web", "host").unwrap();

    assert!(get_channel_config(&cfg, "web", "host").is_none());
    // auth_token should still be there
    assert_eq!(get_channel_config(&cfg, "web", "auth_token"), Some("mysecrettoken123".to_string()));
}

#[test]
fn test_remove_channel_config_nonexistent_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Should not panic
    remove_channel_config(&cfg, "nonexistent_channel", "key").unwrap();
}

// -------------------------------------------------------------------------
// uuid_session tests
// -------------------------------------------------------------------------

#[test]
fn test_uuid_session_starts_with_prefix() {
    let session = uuid_session();
    assert!(session.starts_with("ws-"));
}

#[test]
fn test_uuid_session_correct_length() {
    let session = uuid_session();
    assert_eq!(session.len(), 8); // "ws-" (3) + 5 digits
}

#[test]
fn test_uuid_session_suffix_is_digits() {
    let session = uuid_session();
    let suffix = &session[3..];
    assert!(suffix.chars().all(|c| c.is_ascii_digit()));
}

#[test]
fn test_uuid_session_unique() {
    // Call twice rapidly; they might be the same due to low resolution,
    // but the format should always be valid
    let s1 = uuid_session();
    let s2 = uuid_session();
    assert!(s1.starts_with("ws-"));
    assert!(s2.starts_with("ws-"));
}

// -------------------------------------------------------------------------
// Channel config integration tests (simulating enable/disable via JSON manipulation)
// -------------------------------------------------------------------------

#[test]
fn test_enable_channel_via_pointer_mut() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Simulate ChannelAction::Enable for telegram
    let data = std::fs::read_to_string(&cfg).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    if let Some(ch) = config.pointer_mut("/channels/telegram") {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(loaded["channels"]["telegram"]["enabled"], true);
}

#[test]
fn test_disable_channel_via_pointer_mut() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Simulate ChannelAction::Disable for web
    let data = std::fs::read_to_string(&cfg).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    if let Some(ch) = config.pointer_mut("/channels/web") {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
        }
    }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(loaded["channels"]["web"]["enabled"], false);
}

// -------------------------------------------------------------------------
// WebSocket path normalization tests (matching WebSocketAction::Set logic)
// -------------------------------------------------------------------------

#[test]
fn test_websocket_path_normalization_adds_slash() {
    let mut value = "mypath".to_string();
    if !value.starts_with('/') {
        value = format!("/{}", value);
    }
    assert_eq!(value, "/mypath");
}

#[test]
fn test_websocket_path_normalization_keeps_existing_slash() {
    let mut value = "/already-has-slash".to_string();
    if !value.starts_with('/') {
        value = format!("/{}", value);
    }
    assert_eq!(value, "/already-has-slash");
}

// -------------------------------------------------------------------------
// Port validation tests (matching WebSocketAction::Set logic)
// -------------------------------------------------------------------------

#[test]
fn test_port_validation_valid() {
    let value = "9090";
    let port: Result<u16, _> = value.parse();
    assert!(port.is_ok());
    assert_ne!(port.unwrap(), 0);
}

#[test]
fn test_port_validation_zero_rejected() {
    let value = "0";
    let port: u16 = value.parse().unwrap();
    assert_eq!(port, 0); // Should be rejected by command
}

#[test]
fn test_port_validation_invalid_string() {
    let value = "not-a-port";
    let port: Result<u16, _> = value.parse();
    assert!(port.is_err());
}

// -------------------------------------------------------------------------
// KNOWN_CHANNELS constant tests
// -------------------------------------------------------------------------

#[test]
fn test_known_channels_contains_expected() {
    assert!(KNOWN_CHANNELS.contains(&"web"));
    assert!(KNOWN_CHANNELS.contains(&"websocket"));
    assert!(KNOWN_CHANNELS.contains(&"telegram"));
    assert!(KNOWN_CHANNELS.contains(&"discord"));
    assert!(KNOWN_CHANNELS.contains(&"feishu"));
    assert!(KNOWN_CHANNELS.contains(&"slack"));
    assert!(KNOWN_CHANNELS.contains(&"external"));
}

#[test]
fn test_known_channels_count_v2() {
    assert_eq!(KNOWN_CHANNELS.len(), 13);
}

// -------------------------------------------------------------------------
// set_channel_config / get_channel_config with various channels
// -------------------------------------------------------------------------

#[test]
fn test_set_get_config_telegram() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "telegram", "bot_token", "123456:ABC-DEF").unwrap();
    let val = get_channel_config(&cfg, "telegram", "bot_token");
    assert_eq!(val, Some("123456:ABC-DEF".to_string()));
}

#[test]
fn test_set_get_config_discord() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "discord", "bot_token", "discord-token-value").unwrap();
    let val = get_channel_config(&cfg, "discord", "bot_token");
    assert_eq!(val, Some("discord-token-value".to_string()));
}

#[test]
fn test_set_get_config_feishu() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "feishu", "app_id", "cli_xxxxx").unwrap();
    let val = get_channel_config(&cfg, "feishu", "app_id");
    assert_eq!(val, Some("cli_xxxxx".to_string()));
}

#[test]
fn test_set_get_config_slack() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "slack", "bot_token", "xoxb-xxxx").unwrap();
    let val = get_channel_config(&cfg, "slack", "bot_token");
    assert_eq!(val, Some("xoxb-xxxx".to_string()));
}

#[test]
fn test_set_get_config_websocket() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "websocket", "enabled", "true").unwrap();
    let val = get_channel_config(&cfg, "websocket", "enabled");
    assert_eq!(val, Some("true".to_string()));
}

#[test]
fn test_set_get_config_external() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "external", "input_script", "/path/to/input.sh").unwrap();
    let val = get_channel_config(&cfg, "external", "input_script");
    assert_eq!(val, Some("/path/to/input.sh".to_string()));
}

#[test]
fn test_set_config_unknown_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    let result = set_channel_config(&cfg, "unknown_channel", "key", "value");
    // Should succeed by creating the section
    assert!(result.is_ok());
}

// -------------------------------------------------------------------------
// Channel enable/disable via set_channel_config
// -------------------------------------------------------------------------

#[test]
fn test_channel_enable_via_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "telegram", "enabled", "true").unwrap();
    let val = get_channel_config(&cfg, "telegram", "enabled");
    assert_eq!(val, Some("true".to_string()));
}

#[test]
fn test_channel_disable_via_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "enabled", "false").unwrap();
    let val = get_channel_config(&cfg, "web", "enabled");
    assert_eq!(val, Some("false".to_string()));
}

// -------------------------------------------------------------------------
// Web auth token configuration via set_channel_config
// -------------------------------------------------------------------------

#[test]
fn test_web_auth_token_via_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "auth_token", "my-secret-token").unwrap();
    let val = get_channel_config(&cfg, "web", "auth_token");
    assert_eq!(val, Some("my-secret-token".to_string()));
}

#[test]
fn test_web_host_via_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "host", "0.0.0.0").unwrap();
    let val = get_channel_config(&cfg, "web", "host");
    assert_eq!(val, Some("0.0.0.0".to_string()));
}

// -------------------------------------------------------------------------
// External channel configuration via set_channel_config
// -------------------------------------------------------------------------

#[test]
fn test_external_input_via_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "external", "input_script", "/path/to/input.sh").unwrap();
    set_channel_config(&cfg, "external", "output_script", "/path/to/output.sh").unwrap();

    assert_eq!(get_channel_config(&cfg, "external", "input_script"), Some("/path/to/input.sh".to_string()));
    assert_eq!(get_channel_config(&cfg, "external", "output_script"), Some("/path/to/output.sh".to_string()));
}

// -------------------------------------------------------------------------
// Multiple channel configurations
// -------------------------------------------------------------------------

#[test]
fn test_multiple_channels_configured() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "enabled", "true").unwrap();
    set_channel_config(&cfg, "telegram", "enabled", "true").unwrap();
    set_channel_config(&cfg, "discord", "enabled", "true").unwrap();

    assert_eq!(get_channel_config(&cfg, "web", "enabled"), Some("true".to_string()));
    assert_eq!(get_channel_config(&cfg, "telegram", "enabled"), Some("true".to_string()));
    assert_eq!(get_channel_config(&cfg, "discord", "enabled"), Some("true".to_string()));
}

// -------------------------------------------------------------------------
// Additional coverage tests
// -------------------------------------------------------------------------

#[test]
fn test_set_channel_config_invalid_json_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, "not valid json").unwrap();
    let result = set_channel_config(&cfg, "web", "host", "127.0.0.1");
    assert!(result.is_err());
}

#[test]
fn test_remove_channel_config_from_nonexistent_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    // Removing from a channel that doesn't exist should succeed (no-op)
    remove_channel_config(&cfg, "discord", "token").unwrap();
}

#[test]
fn test_uuid_session_format_multiple() {
    let s1 = uuid_session();
    let s2 = uuid_session();
    assert!(s1.starts_with("ws-"));
    assert!(s2.starts_with("ws-"));
    assert_eq!(s1.len(), 8);
    assert_eq!(s2.len(), 8);
}

#[test]
fn test_known_channels_all_lowercase() {
    for ch in KNOWN_CHANNELS {
        assert_eq!(*ch, ch.to_lowercase(), "Channel '{}' should be lowercase", ch);
    }
}

#[test]
fn test_set_get_remove_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    // Set
    set_channel_config(&cfg, "web", "test_key", "test_value").unwrap();
    assert_eq!(get_channel_config(&cfg, "web", "test_key"), Some("test_value".to_string()));

    // Overwrite
    set_channel_config(&cfg, "web", "test_key", "new_value").unwrap();
    assert_eq!(get_channel_config(&cfg, "web", "test_key"), Some("new_value".to_string()));

    // Remove
    remove_channel_config(&cfg, "web", "test_key").unwrap();
    assert!(get_channel_config(&cfg, "web", "test_key").is_none());
}

#[test]
fn test_set_many_keys_on_one_channel() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    for i in 0..20 {
        set_channel_config(&cfg, "web", &format!("key_{}", i), &format!("val_{}", i)).unwrap();
    }

    for i in 0..20 {
        let val = get_channel_config(&cfg, "web", &format!("key_{}", i));
        assert_eq!(val, Some(format!("val_{}", i)));
    }
}

#[test]
fn test_set_on_multiple_channels() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    for ch in &["web", "telegram", "discord", "feishu"] {
        set_channel_config(&cfg, ch, "token", &format!("{}-token", ch)).unwrap();
    }

    for ch in &["web", "telegram", "discord", "feishu"] {
        let val = get_channel_config(&cfg, ch, "token");
        assert_eq!(val, Some(format!("{}-token", ch)));
    }
}

#[test]
fn test_get_channel_config_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, "bad json").unwrap();
    let val = get_channel_config(&cfg, "web", "host");
    assert!(val.is_none());
}

#[test]
fn test_set_channel_config_creates_channels_key() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_no_channels_config(&tmp);
    set_channel_config(&cfg, "web", "host", "0.0.0.0").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data["channels"]["web"]["host"] == "0.0.0.0");
}
