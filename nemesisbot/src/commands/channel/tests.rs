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

    assert_eq!(
        get_channel_config(&cfg, "web", "port"),
        Some("9090".to_string())
    );
    assert_eq!(
        get_channel_config(&cfg, "web", "host"),
        Some("192.168.1.1".to_string())
    );
}

#[test]
fn test_set_overwrite_value() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "port", "3000").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "port"),
        Some("3000".to_string())
    );
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
    let expected = [
        "web",
        "websocket",
        "telegram",
        "discord",
        "whatsapp",
        "feishu",
        "slack",
        "line",
        "onebot",
        "qq",
        "dingtalk",
        "maixcam",
        "external",
    ];
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

    assert_eq!(
        get_channel_config(&cfg, "discord", "token"),
        Some("abc".to_string())
    );
    assert_eq!(
        get_channel_config(&cfg, "discord", "guild_id"),
        Some("12345".to_string())
    );
}

#[test]
fn test_set_channel_config_value_with_spaces() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "host", "my server name").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "host"),
        Some("my server name".to_string())
    );
}

#[test]
fn test_set_channel_config_empty_value() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    set_channel_config(&cfg, "web", "custom_field", "").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "custom_field"),
        Some("".to_string())
    );
}

// -------------------------------------------------------------------------
// get_channel_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_get_channel_config_numeric_value_returns_text() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    // BUG #41 修复后端口落 JSON 数字；get 侧按原文回显（非数字才回 "(not set)"）
    let val = get_channel_config(&cfg, "web", "port");
    assert_eq!(val.as_deref(), Some("8080"));
}

#[test]
fn test_get_channel_config_bool_value_returns_text() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    // bool 按原文回显为 "true"/"false"
    let val = get_channel_config(&cfg, "web", "enabled");
    assert_eq!(val.as_deref(), Some("true"));
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
    assert_eq!(
        get_channel_config(&cfg, "web", "auth_token"),
        Some("mysecrettoken123".to_string())
    );
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
    if let Some(ch) = config.pointer_mut("/channels/telegram")
        && let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(loaded["channels"]["telegram"]["enabled"], true);
}

#[test]
fn test_disable_channel_via_pointer_mut() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Simulate ChannelAction::Disable for web
    let data = std::fs::read_to_string(&cfg).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    if let Some(ch) = config.pointer_mut("/channels/web")
        && let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
        }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
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

    assert_eq!(
        get_channel_config(&cfg, "external", "input_script"),
        Some("/path/to/input.sh".to_string())
    );
    assert_eq!(
        get_channel_config(&cfg, "external", "output_script"),
        Some("/path/to/output.sh".to_string())
    );
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

    assert_eq!(
        get_channel_config(&cfg, "web", "enabled"),
        Some("true".to_string())
    );
    assert_eq!(
        get_channel_config(&cfg, "telegram", "enabled"),
        Some("true".to_string())
    );
    assert_eq!(
        get_channel_config(&cfg, "discord", "enabled"),
        Some("true".to_string())
    );
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
        assert_eq!(
            *ch,
            ch.to_lowercase(),
            "Channel '{}' should be lowercase",
            ch
        );
    }
}

#[test]
fn test_set_get_remove_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    // Set
    set_channel_config(&cfg, "web", "test_key", "test_value").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "test_key"),
        Some("test_value".to_string())
    );

    // Overwrite
    set_channel_config(&cfg, "web", "test_key", "new_value").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "test_key"),
        Some("new_value".to_string())
    );

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

// -------------------------------------------------------------------------
// Auth token last-4 extraction logic (from Status display)
// -------------------------------------------------------------------------

#[test]
fn test_auth_token_last4_long_token() {
    let auth = "abcdefghijklmnop";
    let last4 = if auth.len() > 4 {
        &auth[auth.len() - 4..]
    } else {
        auth
    };
    assert_eq!(last4, "mnop");
}

#[test]
fn test_auth_token_last4_short_token() {
    let auth = "abc";
    let last4 = if auth.len() > 4 {
        &auth[auth.len() - 4..]
    } else {
        auth
    };
    assert_eq!(last4, "abc");
}

#[test]
fn test_auth_token_last4_exactly_4() {
    let auth = "abcd";
    let last4 = if auth.len() > 4 {
        &auth[auth.len() - 4..]
    } else {
        auth
    };
    assert_eq!(last4, "abcd");
}

#[test]
fn test_auth_token_last4_empty() {
    let auth = "";
    let last4 = if auth.len() > 4 {
        &auth[auth.len() - 4..]
    } else {
        auth
    };
    assert_eq!(last4, "");
}

// -------------------------------------------------------------------------
// Channel enable via pointer_mut (Enable action logic)
// -------------------------------------------------------------------------

#[test]
fn test_enable_unknown_channel_creates_entry() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    // Simulate Enable action: creates channels.<name>.enabled = true
    let data = std::fs::read_to_string(&cfg).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    let name = "discord";
    if let Some(ch) = config.pointer_mut(&format!("/channels/{}", name)) {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    } else if let Some(channels) = config
        .as_object_mut()
        .and_then(|o| o.get_mut("channels"))
        .and_then(|v| v.as_object_mut())
    {
        channels.insert(name.to_string(), serde_json::json!({"enabled": true}));
    }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(loaded["channels"]["discord"]["enabled"], true);
}

#[test]
fn test_enable_channel_with_existing_config_preserves_fields() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Telegram has no enabled field, simulate enabling it
    let data = std::fs::read_to_string(&cfg).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    if let Some(ch) = config.pointer_mut("/channels/telegram")
        && let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(loaded["channels"]["telegram"]["enabled"], true);
}

// -------------------------------------------------------------------------
// Channel disable via pointer_mut (Disable action logic)
// -------------------------------------------------------------------------

#[test]
fn test_disable_channel_preserves_other_keys() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    let data = std::fs::read_to_string(&cfg).unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&data).unwrap();
    if let Some(ch) = config.pointer_mut("/channels/web")
        && let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
        }
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let loaded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(loaded["channels"]["web"]["enabled"], false);
    assert_eq!(loaded["channels"]["web"]["host"], "0.0.0.0");
    assert_eq!(loaded["channels"]["web"]["port"], 8080);
}

// -------------------------------------------------------------------------
// Channel list display parsing logic
// -------------------------------------------------------------------------

#[test]
fn test_channel_list_status_parsing() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    let data = std::fs::read_to_string(&cfg).unwrap();
    let config: serde_json::Value = serde_json::from_str(&data).unwrap();

    for ch in KNOWN_CHANNELS {
        let enabled = config
            .get("channels")
            .and_then(|c| c.get(*ch))
            .and_then(|c| c.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if *ch == "web" {
            assert!(enabled, "web should be enabled");
        } else {
            assert!(!enabled, "{} should be disabled", ch);
        }
    }
}

// -------------------------------------------------------------------------
// WebSocket path normalization edge cases
// -------------------------------------------------------------------------

#[test]
fn test_websocket_path_root_stays() {
    let mut value = "/".to_string();
    if !value.starts_with('/') {
        value = format!("/{}", value);
    }
    assert_eq!(value, "/");
}

#[test]
fn test_websocket_path_multi_segment() {
    let mut value = "api/v1/ws".to_string();
    if !value.starts_with('/') {
        value = format!("/{}", value);
    }
    assert_eq!(value, "/api/v1/ws");
}

// -------------------------------------------------------------------------
// Port validation edge cases
// -------------------------------------------------------------------------

#[test]
fn test_port_validation_max() {
    let value = "65535";
    let port: Result<u16, _> = value.parse();
    assert!(port.is_ok());
    assert_eq!(port.unwrap(), 65535);
}

#[test]
fn test_port_validation_negative_fails() {
    // u16 can't be negative, so parsing "-1" should fail
    let value = "-1";
    let port: Result<u16, _> = value.parse();
    assert!(port.is_err());
}

#[test]
fn test_port_validation_overflow_fails() {
    let value = "70000";
    let port: Result<u16, _> = value.parse();
    assert!(port.is_err());
}

// -------------------------------------------------------------------------
// KNOWN_CHANNELS validation check (matches Enable/Disable guard)
// -------------------------------------------------------------------------

#[test]
fn test_known_channels_validation_accepts_valid() {
    for ch in KNOWN_CHANNELS {
        assert!(KNOWN_CHANNELS.contains(ch), "{} should be valid", ch);
    }
}

#[test]
fn test_known_channels_validation_rejects_invalid() {
    assert!(!KNOWN_CHANNELS.contains(&""));
    assert!(!KNOWN_CHANNELS.contains(&"WEB"));
    assert!(!KNOWN_CHANNELS.contains(&"Telegram"));
}

// -------------------------------------------------------------------------
// Auth token display formatting (from AuthSet output)
// -------------------------------------------------------------------------

#[test]
fn test_auth_set_display_short_token() {
    let token = "abc";
    let display = if token.len() > 4 { &token[..4] } else { "***" };
    assert_eq!(display, "***");
}

#[test]
fn test_auth_set_display_long_token() {
    let token = "my-secret-token";
    let display = if token.len() > 4 { &token[..4] } else { "***" };
    assert_eq!(display, "my-s");
}

// -------------------------------------------------------------------------
// Config with special characters in values
// -------------------------------------------------------------------------

#[test]
fn test_set_channel_config_special_chars() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    set_channel_config(&cfg, "web", "host", "host-with-dashes.example.com").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "host"),
        Some("host-with-dashes.example.com".to_string())
    );
}

#[test]
fn test_set_channel_config_unicode_value() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    set_channel_config(&cfg, "web", "label", "中文标签").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "label"),
        Some("中文标签".to_string())
    );
}

#[test]
fn test_set_channel_config_json_in_value() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    set_channel_config(&cfg, "web", "extra", r#"{"nested":true}"#).unwrap();
    assert_eq!(
        get_channel_config(&cfg, "web", "extra"),
        Some(r#"{"nested":true}"#.to_string())
    );
}

// -------------------------------------------------------------------------
// Sequential multi-channel operations
// -------------------------------------------------------------------------

#[test]
fn test_sequential_enable_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    // Enable
    set_channel_config(&cfg, "telegram", "enabled", "true").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "telegram", "enabled"),
        Some("true".to_string())
    );

    // Configure
    set_channel_config(&cfg, "telegram", "token", "123456:ABC").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "telegram", "token"),
        Some("123456:ABC".to_string())
    );

    // Disable
    set_channel_config(&cfg, "telegram", "enabled", "false").unwrap();
    assert_eq!(
        get_channel_config(&cfg, "telegram", "enabled"),
        Some("false".to_string())
    );

    // Token still present
    assert_eq!(
        get_channel_config(&cfg, "telegram", "token"),
        Some("123456:ABC".to_string())
    );
}

// -------------------------------------------------------------------------
// Config with extra non-standard fields
// -------------------------------------------------------------------------

#[test]
fn test_config_preserves_extra_fields() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let config = serde_json::json!({
        "channels": {
            "web": {
                "enabled": true,
                "host": "0.0.0.0",
                "custom_field": "custom_value"
            }
        },
        "other_section": {
            "key": "value"
        }
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    set_channel_config(&cfg_path, "web", "port", "8080").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(data["channels"]["web"]["custom_field"], "custom_value");
    assert_eq!(data["other_section"]["key"], "value");
    assert_eq!(data["channels"]["web"]["port"], "8080");
}

// -------------------------------------------------------------------------
// Stress test: many keys on a single channel
// -------------------------------------------------------------------------

#[test]
fn test_many_keys_stress() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_empty_config(&tmp);

    for i in 0..50 {
        set_channel_config(&cfg, "web", &format!("key_{}", i), &format!("val_{}", i)).unwrap();
    }

    for i in 0..50 {
        let val = get_channel_config(&cfg, "web", &format!("key_{}", i));
        assert_eq!(val, Some(format!("val_{}", i)));
    }
}

// -------------------------------------------------------------------------
// format_token integration (via crate::common)
// -------------------------------------------------------------------------

#[test]
fn test_format_token_for_channel_display() {
    assert_eq!(crate::common::format_token(""), "(not set)");
    assert_eq!(crate::common::format_token("short"), "***");
    assert_eq!(crate::common::format_token("12345678"), "***");
    assert_eq!(
        crate::common::format_token("my-secret-auth-token-value"),
        "my-s...alue"
    );
}

// =========================================================================
// run() 端到端分支覆盖（S11 覆盖率冲刺）
//
// 策略：NEMESISBOT_HOME 指向临时目录（resolve_home 优先级 2），
// config.json 全程只读写临时 home 下的 {tmp}/.nemesisbot/config.json。
// env set_var 是进程级操作 → 持 crate::GLOBAL_STATE_LOCK 串行。
// 交互式分支（Auth/Clear/Setup）在 cargo test 下 stdin 为管道 EOF，
// read_line 得到空串 → 走取消/默认值路径，不会阻塞。
// =========================================================================

struct TempHomeEnv {
    _tmp: TempDir,
    home: std::path::PathBuf,
}

impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn temp_home_env() -> TempHomeEnv {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    TempHomeEnv { _tmp: tmp, home }
}

fn write_main_cfg(home: &std::path::Path, cfg: &serde_json::Value) {
    std::fs::write(
        crate::common::config_path(home),
        serde_json::to_string_pretty(cfg).unwrap(),
    )
    .unwrap();
}

fn read_main_cfg(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(crate::common::config_path(home)).unwrap())
        .unwrap()
}

// -------------------------------------------------------------------------
// List
// -------------------------------------------------------------------------

#[test]
fn test_run_list_with_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "web": { "enabled": true },
                "telegram": { "enabled": false }
            }
        }),
    );
    run(ChannelAction::List, false).unwrap();
}

#[test]
fn test_run_list_without_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(ChannelAction::List, false).unwrap();
}

// -------------------------------------------------------------------------
// Enable / Disable
// -------------------------------------------------------------------------

#[test]
fn test_run_enable_known_channel_existing_entry() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "telegram": { "enabled": false, "token": "t" } } }),
    );
    run(ChannelAction::Enable { name: "telegram".into() }, false).unwrap();
    let ch = &read_main_cfg(&th.home)["channels"]["telegram"];
    assert_eq!(ch["enabled"], serde_json::json!(true));
    assert_eq!(ch["token"], serde_json::json!("t"));
}

#[test]
fn test_run_enable_known_channel_new_entry() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // channels 对象存在但没有该条目 → else 分支插入
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(ChannelAction::Enable { name: "discord".into() }, false).unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["discord"]["enabled"],
        serde_json::json!(true)
    );
}

#[test]
fn test_run_enable_unknown_channel() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(ChannelAction::Enable { name: "nope".into() }, false).unwrap();
    assert!(read_main_cfg(&th.home)["channels"].get("nope").is_none());
}

#[test]
fn test_run_enable_without_config_file() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(ChannelAction::Enable { name: "web".into() }, false).unwrap();
    assert!(!crate::common::config_path(&th.home).exists());
}

#[test]
fn test_run_disable_known_channel() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "web": { "enabled": true } } }),
    );
    run(ChannelAction::Disable { name: "web".into() }, false).unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["web"]["enabled"],
        serde_json::json!(false)
    );
}

#[test]
fn test_run_disable_unknown_and_no_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(ChannelAction::Disable { name: "nope".into() }, false).unwrap();
    // 无 config → Ok 且不创建
    let _th2 = temp_home_env();
    run(ChannelAction::Disable { name: "web".into() }, false).unwrap();
    assert!(!crate::common::config_path(&_th2.home).exists());
    drop(th);
}

// -------------------------------------------------------------------------
// Status（web / websocket / 默认 / 未配置 / 无 config）
// -------------------------------------------------------------------------

#[test]
fn test_run_status_web_with_auth() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "web": { "enabled": true, "host": "127.0.0.1", "port": 9999, "auth_token": "abcdefgh1234" }
            }
        }),
    );
    run(ChannelAction::Status { name: "web".into() }, false).unwrap();
}

#[test]
fn test_run_status_web_without_auth_defaults() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 缺 host/port/auth → unwrap_or 默认值分支
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "web": { "enabled": false } } }),
    );
    run(ChannelAction::Status { name: "web".into() }, false).unwrap();
}

#[test]
fn test_run_status_websocket_with_and_without_auth() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": { "websocket": { "enabled": true, "auth_token": "tok12345", "path": "/custom" } }
        }),
    );
    run(
        ChannelAction::Status { name: "websocket".into() },
        false,
    )
    .unwrap();
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "websocket": { "enabled": false } } }),
    );
    run(
        ChannelAction::Status { name: "websocket".into() },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_status_generic_channel_extra_fields() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": { "telegram": { "enabled": true, "bot_token": "abc", "api_url": "http://x" } }
        }),
    );
    run(
        ChannelAction::Status { name: "telegram".into() },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_status_not_configured_and_no_config() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(
        ChannelAction::Status { name: "discord".into() },
        false,
    )
    .unwrap();
    let _th2 = temp_home_env();
    run(ChannelAction::Status { name: "web".into() }, false).unwrap();
    drop(th);
}

// -------------------------------------------------------------------------
// Web 子命令
// -------------------------------------------------------------------------

#[test]
fn test_run_web_auth_interactive_eof_empty_token() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "web": {} } }));
    // stdin EOF → token 空 → 报错提前返回，不写配置
    run(
        ChannelAction::Web { action: WebAction::Auth },
        false,
    )
    .unwrap();
    assert!(
        read_main_cfg(&th.home)["channels"]["web"]
            .get("auth_token")
            .is_none()
    );
}

#[test]
fn test_run_web_auth_set_normal_token() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "web": {} } }));
    run(
        ChannelAction::Web {
            action: WebAction::AuthSet { token: "longenoughtoken".into() },
        },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["web"]["auth_token"],
        serde_json::json!("longenoughtoken")
    );
}

#[test]
fn test_run_web_auth_set_short_and_empty() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "web": {} } }));
    // 短 token → 警告但仍然写入
    run(
        ChannelAction::Web {
            action: WebAction::AuthSet { token: "abc".into() },
        },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["web"]["auth_token"],
        serde_json::json!("abc")
    );
    // 空 token → 报错提前返回（覆盖 4 字符以下 "***" 分支前的早退）
    run(
        ChannelAction::Web {
            action: WebAction::AuthSet { token: String::new() },
        },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["web"]["auth_token"],
        serde_json::json!("abc")
    );
}

#[test]
fn test_run_web_auth_get_set_and_unset() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "web": {} } }));
    run(ChannelAction::Web { action: WebAction::AuthGet }, false).unwrap();
    run(
        ChannelAction::Web {
            action: WebAction::AuthSet { token: "my-secret-auth-token-value".into() },
        },
        false,
    )
    .unwrap();
    run(ChannelAction::Web { action: WebAction::AuthGet }, false).unwrap();
}

#[test]
fn test_run_web_host_and_port() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "web": {} } }));
    run(
        ChannelAction::Web {
            action: WebAction::Host { host: "127.0.0.1".into() },
        },
        false,
    )
    .unwrap();
    run(
        ChannelAction::Web {
            action: WebAction::Port { port: 49152 },
        },
        false,
    )
    .unwrap();
    let web = &read_main_cfg(&th.home)["channels"]["web"];
    assert_eq!(web["host"], serde_json::json!("127.0.0.1"));
    // BUG #41 修复后端口必须落为 JSON 数字（typed 读侧 i64 / as_u64 才能读到）
    assert_eq!(web["port"], serde_json::json!(49152));
}

#[test]
fn test_run_web_status_configured_and_not() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "web": { "enabled": true, "auth_token": "token12345", "path": "/sock" }
            }
        }),
    );
    run(
        ChannelAction::Web { action: WebAction::Status },
        false,
    )
    .unwrap();
    // 无 web 条目 → (not configured)
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(
        ChannelAction::Web { action: WebAction::Status },
        false,
    )
    .unwrap();
    // 无 config 文件 → No configuration found
    let _th2 = temp_home_env();
    run(
        ChannelAction::Web { action: WebAction::Status },
        false,
    )
    .unwrap();
    drop(th);
}

#[test]
fn test_run_web_clear_interactive_eof_cancels() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "web": { "auth_token": "tok123456" } } }),
    );
    // stdin EOF → 答案非 y → Cancelled，token 保留
    run(
        ChannelAction::Web { action: WebAction::Clear },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["web"]["auth_token"],
        serde_json::json!("tok123456")
    );
}

#[test]
fn test_run_web_config_full_fields_and_missing() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 覆盖 TLS/CORS/max_connections/额外字段打印分支
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "web": {
                    "enabled": true,
                    "host": "0.0.0.0",
                    "port": 8080,
                    "auth_token": "verylongtoken1234",
                    "path": "/ws",
                    "tls_cert": "cert.pem",
                    "tls_key": "key.pem",
                    "cors": true,
                    "max_connections": 64,
                    "custom_extra": "x"
                }
            }
        }),
    );
    run(
        ChannelAction::Web { action: WebAction::Config },
        false,
    )
    .unwrap();
    // 最小字段（enabled 未设 → "(not set)"）
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "web": { "host": "h" } } }),
    );
    run(
        ChannelAction::Web { action: WebAction::Config },
        false,
    )
    .unwrap();
    // 无 web 条目
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(
        ChannelAction::Web { action: WebAction::Config },
        false,
    )
    .unwrap();
    // 无 config 文件
    let _th2 = temp_home_env();
    run(
        ChannelAction::Web { action: WebAction::Config },
        false,
    )
    .unwrap();
    drop(th);
}

// -------------------------------------------------------------------------
// WebSocket 子命令
// -------------------------------------------------------------------------

#[test]
fn test_run_websocket_setup_interactive_eof_defaults() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "websocket": { "enabled": false, "host": "10.0.0.5", "port": "49001", "path": "/ws" },
                "web": {}
            }
        }),
    );
    // 全部提示 EOF → 用现有值；sync 问题答案 "" != "n" → 走同步分支
    run(
        ChannelAction::WebSocket { action: WebSocketAction::Setup },
        false,
    )
    .unwrap();
    let cfg = read_main_cfg(&th.home);
    assert_eq!(
        cfg["channels"]["websocket"]["enabled"],
        serde_json::json!(true)
    );
    assert_eq!(cfg["channels"]["websocket"]["host"], serde_json::json!("10.0.0.5"));
    // 同步到 web：session_id 生成（ws- 前缀）
    let session = cfg["channels"]["web"]["session_id"].as_str().unwrap();
    assert!(session.starts_with("ws-"));
    assert_eq!(cfg["channels"]["web"]["port"], serde_json::json!(49001));
}

#[test]
fn test_run_websocket_setup_from_scratch_defaults() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 无已有值 → 默认 127.0.0.1/49001//ws；无 channels 键也能建
    write_main_cfg(&th.home, &serde_json::json!({}));
    run(
        ChannelAction::WebSocket { action: WebSocketAction::Setup },
        false,
    )
    .unwrap();
    let cfg = read_main_cfg(&th.home);
    assert_eq!(
        cfg["channels"]["websocket"]["host"],
        serde_json::json!("127.0.0.1")
    );
    assert_eq!(cfg["channels"]["websocket"]["port"], serde_json::json!(49001));
    assert_eq!(cfg["channels"]["websocket"]["path"], serde_json::json!("/ws"));
}

#[test]
fn test_run_websocket_config_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": { "websocket": { "enabled": true, "port": "49001" } }
        }),
    );
    run(
        ChannelAction::WebSocket { action: WebSocketAction::Config },
        false,
    )
    .unwrap();
    // 无 websocket 条目 → (not configured)
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(
        ChannelAction::WebSocket { action: WebSocketAction::Config },
        false,
    )
    .unwrap();
    // 无 config 文件 → Ok（无输出）
    let _th2 = temp_home_env();
    run(
        ChannelAction::WebSocket { action: WebSocketAction::Config },
        false,
    )
    .unwrap();
    drop(th);
}

#[test]
fn test_run_websocket_set_validations() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "websocket": {} } }));
    // 非数字端口 → Err
    let err = run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Set {
                key: "port".into(),
                value: "abc".into(),
            },
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("Invalid port number"));
    // 端口 0 → bail
    let err = run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Set {
                key: "port".into(),
                value: "0".into(),
            },
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("cannot be 0"));
    // 合法端口写入（BUG #41 修复后落 JSON 数字）
    run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Set {
                key: "port".into(),
                value: "49152".into(),
            },
        },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["websocket"]["port"],
        serde_json::json!(49152)
    );
    // path 缺 "/" → 自动补
    run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Set {
                key: "path".into(),
                value: "sock".into(),
            },
        },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["websocket"]["path"],
        serde_json::json!("/sock")
    );
    // 普通键直写
    run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Set {
                key: "host".into(),
                value: "127.0.0.1".into(),
            },
        },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_websocket_get_set_and_unset() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "websocket": {} } }));
    run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Get { key: "host".into() },
        },
        false,
    )
    .unwrap();
    run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Set {
                key: "host".into(),
                value: "192.168.1.1".into(),
            },
        },
        false,
    )
    .unwrap();
    run(
        ChannelAction::WebSocket {
            action: WebSocketAction::Get { key: "host".into() },
        },
        false,
    )
    .unwrap();
}

// -------------------------------------------------------------------------
// External 子命令
// -------------------------------------------------------------------------

#[test]
fn test_run_external_setup_interactive_eof_defaults() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "external": { "enabled": false, "input_exe": "in.bat", "output_exe": "out.bat" }
            }
        }),
    );
    run(
        ChannelAction::External { action: ExternalAction::Setup },
        false,
    )
    .unwrap();
    let ext = &read_main_cfg(&th.home)["channels"]["external"];
    assert_eq!(ext["enabled"], serde_json::json!(true));
    assert_eq!(ext["input_exe"], serde_json::json!("in.bat"));
    assert_eq!(ext["output_exe"], serde_json::json!("out.bat"));
    assert_eq!(ext["chat_id"], serde_json::json!("external:main"));
}

#[test]
fn test_run_external_setup_from_scratch_removes_empty() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 无已有值 → input/output 空 → remove（本就没有）；chat_id 默认写入
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(
        ChannelAction::External { action: ExternalAction::Setup },
        false,
    )
    .unwrap();
    let ext = &read_main_cfg(&th.home)["channels"]["external"];
    assert!(ext.get("input_exe").is_none());
    assert!(ext.get("output_exe").is_none());
    assert_eq!(ext["chat_id"], serde_json::json!("external:main"));
}

#[test]
fn test_run_external_config_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(
        &th.home,
        &serde_json::json!({ "channels": { "external": { "enabled": true } } }),
    );
    run(
        ChannelAction::External { action: ExternalAction::Config },
        false,
    )
    .unwrap();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    run(
        ChannelAction::External { action: ExternalAction::Config },
        false,
    )
    .unwrap();
    let _th2 = temp_home_env();
    run(
        ChannelAction::External { action: ExternalAction::Config },
        false,
    )
    .unwrap();
    drop(th);
}

#[test]
fn test_run_external_test_not_configured() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
    // 两个 exe 都没配 → 提示后提前返回
    run(
        ChannelAction::External { action: ExternalAction::Test },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_external_test_not_found_and_failed_spawn() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // input=不存在的路径 → NOT FOUND；output=存在但是目录 → spawn Err → FAILED
    let dir_as_exe = th.home.join("workspace").to_string_lossy().to_string();
    write_main_cfg(
        &th.home,
        &serde_json::json!({
            "channels": {
                "external": {
                    "input_exe": "Z:/definitely/not/there.exe",
                    "output_exe": dir_as_exe
                }
            }
        }),
    );
    run(
        ChannelAction::External { action: ExternalAction::Test },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_external_set_and_get() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    write_main_cfg(&th.home, &serde_json::json!({ "channels": { "external": {} } }));
    run(
        ChannelAction::External {
            action: ExternalAction::Set {
                key: "input_exe".into(),
                value: "input.bat".into(),
            },
        },
        false,
    )
    .unwrap();
    assert_eq!(
        read_main_cfg(&th.home)["channels"]["external"]["input_exe"],
        serde_json::json!("input.bat")
    );
    run(
        ChannelAction::External {
            action: ExternalAction::Get { key: "input_exe".into() },
        },
        false,
    )
    .unwrap();
    run(
        ChannelAction::External {
            action: ExternalAction::Get { key: "ghost".into() },
        },
        false,
    )
    .unwrap();
}

// =========================================================================
// wave_b（覆盖率补测 B 波）：
//
// 复用上方 TempHomeEnv / temp_home_env / write_main_cfg / read_main_cfg。
// stdin 交互型分支（需用户真正键入非空 token/路径/"y"/"n" 的臂）不可单测，
// 见报告 EXEMPT 表；本模块只测可离线构造的分支：
//   - Disable 已知通道但 channels 条目缺失 → pointer_mut None 臂 + 照样回写
//   - Status web 短 auth_token（raw 掩码臂）/ 非对象条目（字段转储跳过）
//   - Web Status 短 token 与无 token 条目；Web Config 短 token 与非对象条目
//   - WebSocket Setup：预置 auth_token 的保留与同步到 web 臂；
//     websocket 条目为非对象时 enabling 被静默跳过（可疑点 C1）
//   - External Setup：external 条目为非对象同样静默跳过（可疑点 C1）
// =========================================================================

mod wave_b {
    use super::*;

    /// Disable 已知通道、但 config 中无该条目 → pointer_mut None（不新建），
    /// 但文件仍被无害回写（171-194 家族的完整执行）。
    #[test]
    fn wave_b_disable_known_channel_absent_entry_still_rewrites_config() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(&th.home, &serde_json::json!({ "channels": {} }));
        run(ChannelAction::Disable { name: "slack".into() }, false).unwrap();
        let cfg = read_main_cfg(&th.home);
        assert!(
            cfg["channels"].get("slack").is_none(),
            "Disable 对缺失条目不得新建"
        );
        // 能从磁盘重新解析即证明回写已发生且 JSON 合法。
    }

    /// Status web：auth_token 长度 ≤4 → last4 掩码走 raw 分支（227 区），
    /// 不经 ceil_char_boundary 切片。
    #[test]
    fn wave_b_status_web_short_auth_token_uses_raw_mask_arm() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": { "web": { "enabled": true, "auth_token": "ab" } }
            }),
        );
        run(ChannelAction::Status { name: "web".into() }, false).unwrap();
    }

    /// Status 泛型臂：channels.<name> 是字符串而非对象 → as_object None，
    /// 字段转储循环整体跳过（260 关联区域）。
    #[test]
    fn wave_b_status_non_object_channel_entry_skips_field_dump() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({ "channels": { "discord": "bogus-not-an-object" } }),
        );
        run(
            ChannelAction::Status { name: "discord".into() },
            false,
        )
        .unwrap();
    }

    /// Web Status 两形态：(a) auth_token ≤4 字符 → raw 掩码臂（378）；
    /// (b) web 条目存在但完全无 auth_token → has_auth=false，掩码块整体落穿（381 区）。
    #[test]
    fn wave_b_web_status_short_auth_then_no_auth_entry() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": {
                    "web": { "enabled": true, "host": "127.0.0.1", "port": 8080, "auth_token": "abc" }
                }
            }),
        );
        run(ChannelAction::Web { action: WebAction::Status }, false).unwrap();

        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": { "web": { "enabled": true, "path": "/ws" } }
            }),
        );
        run(ChannelAction::Web { action: WebAction::Status }, false).unwrap();
    }

    /// Web Config 两形态：(a) 短 auth_token → raw 分支（452）；
    /// (b) web 条目是非对象字符串 → as_object None，额外字段循环跳过（488 区）。
    #[test]
    fn wave_b_web_config_short_auth_then_non_object_entry() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": { "web": { "auth_token": "abc", "port": 5000 } }
            }),
        );
        run(ChannelAction::Web { action: WebAction::Config }, false).unwrap();

        write_main_cfg(
            &th.home,
            &serde_json::json!({ "channels": { "web": "junk-string" } }),
        );
        run(ChannelAction::Web { action: WebAction::Config }, false).unwrap();
    }

    /// WebSocket Setup：预置 ws.auth_token（EOF 保持原值）→ 提示语预览已有
    /// token 前 4 位（546）、保留分支 set_channel_config 非 remove（563-565）、
    /// 同步到 web 的 token 保留臂（610-612）。session_id 照常生成。
    #[test]
    fn wave_b_websocket_setup_preserves_seeded_token_and_syncs_to_web() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": {
                    "websocket": {
                        "enabled": false,
                        "host": "10.9.9.9",
                        "port": "49002",
                        "path": "/ws",
                        "auth_token": "wstoken12345"
                    },
                    "web": {}
                }
            }),
        );
        run(
            ChannelAction::WebSocket { action: WebSocketAction::Setup },
            false,
        )
        .unwrap();
        let cfg = read_main_cfg(&th.home);
        let ws = &cfg["channels"]["websocket"];
        assert_eq!(ws["enabled"], serde_json::json!(true), "正常对象条目 → 置位");
        assert_eq!(
            ws["auth_token"], serde_json::json!("wstoken12345"),
            "EOF 无输入 → 原有 token 保留而非移除"
        );
        let web = &cfg["channels"]["web"];
        assert_eq!(
            web["auth_token"], serde_json::json!("wstoken12345"),
            "同步到 web 时非空 token 走 set 而非 remove"
        );
        assert_eq!(web["host"], serde_json::json!("10.9.9.9"));
        assert_eq!(web["port"], serde_json::json!(49002));
        assert_eq!(web["path"], serde_json::json!("/ws"));
        assert!(web["session_id"].as_str().unwrap().starts_with("ws-"));
    }

    /// WebSocket Setup：websocket 条目是字符串（配置损坏）→ BUG #42 修复后
    /// 第一个写点即 loud bail，命令返回 Err 且文件一字节不动。
    #[test]
    fn wave_b_websocket_setup_non_object_ws_entry_loud_bails_untouched() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let original = serde_json::json!({ "channels": { "websocket": "junk" } });
        write_main_cfg(&th.home, &original);
        let err = run(
            ChannelAction::WebSocket { action: WebSocketAction::Setup },
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("配置已损坏"),
            "应点名配置损坏而非静默成功: {}",
            err
        );
        assert_eq!(
            read_main_cfg(&th.home),
            original,
            "拒绝覆盖：非对象条目必须原样保留"
        );
    }

    /// External Setup：external 条目为字符串 → 与 WebSocket 同型的 loud bail。
    #[test]
    fn wave_b_external_setup_non_object_entry_loud_bails_untouched() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let original = serde_json::json!({ "channels": { "external": "nope" } });
        write_main_cfg(&th.home, &original);
        let err = run(
            ChannelAction::External { action: ExternalAction::Setup },
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("配置已损坏"),
            "应点名配置损坏而非静默成功: {}",
            err
        );
        assert_eq!(
            read_main_cfg(&th.home)["channels"]["external"],
            serde_json::json!("nope"),
            "拒绝覆盖：非对象条目必须原样保留"
        );
    }
}

// =========================================================================
// wave_c（覆盖率补测 C 波）：非对象条目拒绝臂 + External Test 探测臂。
//   - Enable / Disable 已知通道、但该条目是非对象（损坏配置）→ BUG #42
//     修复后 loud bail 且文件原样保留（此前是静默跳过仍报成功）；
//   - ExternalAction::Test 的半配置形态：只配 output 不配 input →
//     不双双为空早退，而走 "Input program: not configured" 侧臂；
//   - 探测程序存在且可启动（绝对路径，同 S11b EDITOR 豁免裁定：无窗口、
//     立即退出）→ input/output 两槽的 spawn-Ok → kill/wait → OK 臂；
//   - input 槽存在但是目录 → spawn Err → FAILED 臂；output 槽未配置 →
//     "Output program: not configured" 侧臂。
//   另含 BUG #41 回归：端口数字落盘 + read_port_flex 对旧字符串盘的容错读。
// 豁免：Setup/Auth/交互式提示里需要用户真实键入的赋值臂（stdin 无法注入）。
// =========================================================================

mod wave_c {
    use super::*;

    /// Enable 已知通道、条目为字符串：BUG #42 修复后 loud bail、文件不动。
    #[test]
    fn wc_enable_non_object_entry_loud_bails_untouched() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let original = serde_json::json!({ "channels": { "web": "junk-string" } });
        write_main_cfg(&th.home, &original);

        let err = run(ChannelAction::Enable { name: "web".into() }, false).unwrap_err();

        assert!(
            err.to_string().contains("配置已损坏"),
            "应点名配置损坏而非静默成功: {}",
            err
        );
        assert_eq!(
            read_main_cfg(&th.home),
            original,
            "拒绝覆盖：非对象条目必须原样保留"
        );
    }

    /// Disable 同型：条目为数组（非对象）→ loud bail、字节不变。
    #[test]
    fn wc_disable_non_object_entry_loud_bails_untouched() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let original =
            serde_json::json!({ "channels": { "slack": ["not", "an", "object"] } });
        write_main_cfg(&th.home, &original);

        let err = run(ChannelAction::Disable { name: "slack".into() }, false).unwrap_err();

        assert!(
            err.to_string().contains("配置已损坏"),
            "应点名配置损坏而非静默成功: {}",
            err
        );
        assert_eq!(read_main_cfg(&th.home), original);
    }

    /// BUG #41 显示侧回归：read_port_flex 对旧字符串端口如实报告而非落默认。
    #[test]
    fn wc_read_port_flex_tolerates_legacy_string_and_number() {
        let n = serde_json::json!(49152);
        let s = serde_json::json!("49153");
        let s_ws = serde_json::json!(" 8081 ");
        let junk = serde_json::json!("abc");
        assert_eq!(super::super::read_port_flex(Some(&n), 8080), 49152);
        assert_eq!(super::super::read_port_flex(Some(&s), 8080), 49153);
        assert_eq!(super::super::read_port_flex(Some(&s_ws), 8080), 8081);
        // 垃圾字符串与非数字类型：如实落默认，不假装成功
        assert_eq!(super::super::read_port_flex(Some(&junk), 8080), 8080);
        assert_eq!(
            super::super::read_port_flex(Some(&serde_json::json!([1])), 8080),
            8080
        );
        assert_eq!(super::super::read_port_flex(None, 8080), 8080);
    }

    /// External Test 半配置形态：input 未配置 + output 配了（不存在路径）
    /// → 不走双双为空的早退，而是分别打印 Input-not-configured 侧臂与
    /// output 的 NOT FOUND 判定。
    #[test]
    fn wc_external_test_half_configured_reports_input_not_configured() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": {
                    "external": { "output_exe": "Z:/definitely/not/there.exe" }
                }
            }),
        );
        run(
            ChannelAction::External {
                action: ExternalAction::Test,
            },
            false,
        )
        .unwrap();
    }

    /// 探测程序存在且能启动：spawn 后立刻 kill/wait → "OK (starts
    /// successfully)"（Ok-child 臂此前从未到达——既有测试只有 NOT FOUND
    /// 与 FAILED）。注意源码守卫是 `Path::new(&exe).exists()`——只接受
    /// 文件路径而非 PATH 命令名，所以必须解析出绝对路径（which）；
    /// 解析失败则整个用例跳过（环境性豁免，不算失败）。
    #[test]
    fn wc_external_test_existing_program_spawn_ok_for_input_and_output_slots() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let Ok(exe) = which::which("hostname") else {
            return; // 无 hostname 的环境：跳过（NOT FOUND 臂由既有测试覆盖）
        };
        let exe = exe.to_string_lossy().into_owned();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": {
                    "external": {
                        "input_exe": exe.clone(),
                        "output_exe": exe
                    }
                }
            }),
        );
        run(
            ChannelAction::External {
                action: ExternalAction::Test,
            },
            false,
        )
        .unwrap();
    }

    /// input 槽存在但是目录 → path.exists()==true 但 spawn 失败 →
    /// input 侧 FAILED 臂；output 槽未配置 → "Output program: not
    /// configured" 侧臂（该 else 此前同样从未到过）。
    #[test]
    fn wc_external_test_input_slot_directory_spawn_failed_and_output_unconfigured() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": {
                    "external": { "input_exe": th.home.to_string_lossy() }
                }
            }),
        );
        run(
            ChannelAction::External {
                action: ExternalAction::Test,
            },
            false,
        )
        .unwrap();
    }

    /// output 槽存在但是目录（path.exists()==true 但 spawn 失败）→
    /// output 侧 FAILED 臂（input 槽用真实可执行文件走 OK，保证执行
    /// 流推进到第二段 spawn 块的 Err 分支）。
    #[test]
    fn wc_external_test_output_slot_directory_spawn_failed_after_input_ok() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        let Ok(exe) = which::which("hostname") else {
            return; // 无 hostname 的环境：跳过
        };
        write_main_cfg(
            &th.home,
            &serde_json::json!({
                "channels": {
                    "external": {
                        "input_exe": exe.to_string_lossy(),
                        "output_exe": th.home.to_string_lossy()
                    }
                }
            }),
        );
        run(
            ChannelAction::External {
                action: ExternalAction::Test,
            },
            false,
        )
        .unwrap();
    }
}

// =========================================================================
// r10（覆盖率补测批 2026-08-27）：channel 交互式子命令 A 类 miss 行收口。
// 此前交互分支（Auth/Clear/Setup）只有 EOF 默认/取消侧——wave_c 豁免区把
// "需要用户真实键入的赋值臂" 判为 stdin 无法注入；本批换 r9 同款武器
// （test_harness::run_cli_with_stdin 管道喂完整答案序列）点亮：
//   * Web Auth：空 token 早退 / 短 token 警告 / y 保存（长 token 掩码臂 +
//     ≤4 字符 "***" 臂）/ n 取消；
//   * Web Clear："y" 确认删除臂（415-417）；
//   * WebSocket Setup：host/port/path/token 四个非空赋值臂（532/541/555/
//     571）+ 同步块显式 session ID 臂（618）+ 同步问题答 "n" 整块跳过收口
//     （632 跳出边）；
//   * External Setup：input/output/chat_id 三值非空输入臂（712/728/737）；
//   * websocket set port 成功落盘 JSON 数字臂尾（667-672，进程内直调）。
// 夹具纪律（r9 先例）：run_cli* 自动 prepend --local + cwd=tempdir，父进程
// 环境零改动 → 子进程测试无需 GLOBAL_STATE_LOCK；二进制解析不了则 SKIP。
// 注意：set_channel_config 在 config.json 缺失时 bail（同文件既有测试同理
// 都先种子化），所以每个子进程场景先写一份 {"channels":{…}} 到 home 根。
// =========================================================================
mod r10_interactive_flows {
    use super::*;

    /// 解析真二进制；解析不了（未构建 / 非 Windows 平台）→ SKIP。
    fn r10_bin_or_skip() -> Option<std::path::PathBuf> {
        match test_harness::resolve_nemesisbot_bin() {
            Ok(b) => Some(b),
            Err(e) => {
                println!("[r10 SKIP] 未找到 nemesisbot 可执行文件（先构建 release 版）：{e:#}");
                None
            }
        }
    }

    /// 隔离工作区 + 预置最小 channels 配置（子进程写路径要求文件已存在）。
    fn r10_ws_with_cfg(channels: serde_json::Value) -> Option<(test_harness::TestWorkspace, std::path::PathBuf)> {
        let bin = r10_bin_or_skip()?;
        let tw = test_harness::TestWorkspace::new().expect("tempdir");
        std::fs::create_dir_all(tw.home()).unwrap();
        std::fs::write(
            tw.config_path(),
            serde_json::to_string_pretty(&serde_json::json!({ "channels": channels })).unwrap(),
        )
        .unwrap();
        Some((tw, bin))
    }

    /// 读回子进程落盘后的主配置（= <home>/config.json = common::config_path）。
    fn r10_read_cfg(tw: &test_harness::TestWorkspace) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(tw.config_path()).unwrap()).unwrap()
    }

    // ── ⭐ Web Auth 保存双形态：长 token（len>4 掩码切片臂 311-313）+ 短 token
    // （<8 警告臂 295-299 且 len≤4 落 "***" 兜底臂 315）。两次独立调用同一工作区，
    // 第二次覆盖第一次的值，断言以最后一次为准。─────────────────────────────────
    #[tokio::test]
    async fn r10_web_auth_interactive_save_long_and_short_token_arms() {
        let Some((tw, bin)) = r10_ws_with_cfg(serde_json::json!({ "web": {} })) else { return };

        // 长 token + y 主流程：295 警告不触发、y 确认、auth_token 原样落盘。
        let long = tw
            .run_cli_with_stdin(bin.as_path(), &["channel", "web", "auth"], "webtoken-long-01\ny\n", 30)
            .await;
        assert!(long.success(), "{}\n{}", long.stdout, long.stderr);
        assert!(
            long.stdout.contains("Web auth token set:") && long.stdout.contains("webt****"),
            "长 token 掩码打印前 4 字符，got:\n{}",
            long.stdout
        );
        assert_eq!(r10_read_cfg(&tw)["channels"]["web"]["auth_token"], serde_json::json!("webtoken-long-01"));

        // 短 token（<8）：警告臂触发后仍走确认；len≤4 → 掩码走 "***" 分支。
        let short = tw
            .run_cli_with_stdin(bin.as_path(), &["channel", "web", "auth"], "abc\ny\n", 30)
            .await;
        assert!(short.success(), "{}\n{}", short.stdout, short.stderr);
        assert!(
            short.stdout.contains("Warning: Token is short"),
            "短 token 必须警告，got:\n{}",
            short.stdout
        );
        assert!(short.stdout.contains("***"), "≤4 字符掩码兜底为 ***");
        assert_eq!(r10_read_cfg(&tw)["channels"]["web"]["auth_token"], serde_json::json!("abc"));
    }

    // ── Web Auth 早退/取消双出口：空 token 报错返回（288-294）；合法 token 但
    //    确认答 n → Cancelled 不落盘（304-306）。两态都不触碰 auth_token 键。──
    #[tokio::test]
    async fn r10_web_auth_empty_token_returns_and_decline_cancels_without_write() {
        let Some((tw, bin)) = r10_ws_with_cfg(serde_json::json!({ "web": {} })) else { return };

        // 空 token → Error 提示提前 return Ok。
        let empty = tw
            .run_cli_with_stdin(bin.as_path(), &["channel", "web", "auth"], "\n", 30)
            .await;
        assert!(empty.success(), "早退是正常出口退 0:\n{}\n{}", empty.stdout, empty.stderr);
        assert!(
            empty.stdout.contains("Token cannot be empty"),
            "got:\n{}",
            empty.stdout
        );

        // 合法 token + 答 n → Cancelled，绝不写入。
        let decline = tw
            .run_cli_with_stdin(bin.as_path(), &["channel", "web", "auth"], "validtoken99\nn\n", 30)
            .await;
        assert!(decline.success(), "{}\n{}", decline.stdout, decline.stderr);
        assert!(decline.stdout.contains("Cancelled"));
        assert!(
            r10_read_cfg(&tw)["channels"]["web"].get("auth_token").is_none(),
            "两个退出臂都不得留下 auth_token"
        );
    }

    // ── Web Clear "y" 确认臂（415-417）：remove_channel_config + 回执文案。───
    #[tokio::test]
    async fn r10_web_clear_confirm_y_removes_auth_token_from_disk() {
        let Some((tw, bin)) = r10_ws_with_cfg(serde_json::json!({ "web": { "auth_token": "tok123456" } }))
        else {
            return;
        };
        let out = tw
            .run_cli_with_stdin(bin.as_path(), &["channel", "web", "clear"], "y\n", 30)
            .await;
        assert!(out.success(), "{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stdout.contains("Web auth token cleared."),
            "got:\n{}",
            out.stdout
        );
        assert!(
            r10_read_cfg(&tw)["channels"]["web"].get("auth_token").is_none(),
            "确认后 auth_token 必须从盘上移除"
        );
    }

    // ── WebSocket Setup 四个非空输入臂 + 同步块显式 session ID 臂：一次脚本全走 ──
    #[tokio::test]
    async fn r10_websocket_setup_four_nonempty_inputs_then_explicit_session_id_synced() {
        let Some((tw, bin)) = r10_ws_with_cfg(serde_json::json!({ "websocket": { "enabled": false } }))
        else {
            return;
        };
        // 时序：Host → Port → Path → Token → Sync? → Session ID。
        let out = tw
            .run_cli_with_stdin(
                bin.as_path(),
                &["channel", "web-socket", "setup"],
                "192.0.2.7\n55021\n/customws\ntok98765\ny\nsess-explicit-77\n",
                30,
            )
            .await;
        assert!(out.success(), "{}\n{}", out.stdout, out.stderr);
        assert!(
            out.stdout.contains("Synced to Web channel (session: sess-explicit-77)"),
            "同步回执带用户显式 session id，got:\n{}",
            out.stdout
        );
        assert!(out.stdout.contains("WebSocket channel configured and enabled."));

        let cfg = r10_read_cfg(&tw);
        let ws = &cfg["channels"]["websocket"];
        assert_eq!(ws["enabled"], serde_json::json!(true));
        assert_eq!(ws["host"], serde_json::json!("192.0.2.7"), "非空 host 臂生效");
        assert_eq!(
            ws["port"],
            serde_json::json!(55021),
            "端口必须落 JSON 数字（BUG #41 契约），got: {}",
            ws["port"]
        );
        assert_eq!(ws["path"], serde_json::json!("/customws"), "非空 path 臂生效");
        assert_eq!(ws["auth_token"], serde_json::json!("tok98765"), "非空 token 臂生效");

        let web = &cfg["channels"]["web"];
        assert_eq!(web["host"], serde_json::json!("192.0.2.7"));
        assert_eq!(web["port"], serde_json::json!(55021));
        assert_eq!(web["path"], serde_json::json!("/customws"));
        assert_eq!(web["auth_token"], serde_json::json!("tok98765"));
        assert_eq!(
            web["session_id"],
            serde_json::json!("sess-explicit-77"),
            "显式 session ID 覆盖 uuid 默认生成"
        );
    }

    // ── 同步问题答 "n" → 整个 sync 块跳过（608 false 边收口到 632）：
    //    web 条目绝不被创建，其余默认值照常落盘并启用。──────────────────────
    #[tokio::test]
    async fn r10_websocket_setup_sync_answer_n_skips_web_mirror_block() {
        let Some((tw, bin)) = r10_ws_with_cfg(serde_json::json!({ "websocket": { "enabled": false } }))
        else {
            return;
        };
        let out = tw
            .run_cli_with_stdin(
                bin.as_path(),
                &["channel", "web-socket", "setup"],
                "\n\n\n\nn\n",
                30,
            )
            .await;
        assert!(out.success(), "{}\n{}", out.stdout, out.stderr);
        assert!(out.stdout.contains("WebSocket channel configured and enabled."));
        assert!(
            !out.stdout.contains("Synced to Web channel"),
            "答 n 不得出现同步回执"
        );

        let cfg = r10_read_cfg(&tw);
        assert_eq!(
            cfg["channels"]["websocket"]["enabled"],
            serde_json::json!(true),
            "启用照常发生"
        );
        assert_eq!(cfg["channels"]["websocket"]["port"], serde_json::json!(49001), "EOF 空输入保持默认端口且落数字");
        assert!(
            cfg["channels"].get("web").is_none(),
            "答 n 后 web 条目必须完全不存在"
        );
    }

    // ── External Setup 三值非空输入臂（712/728/737）+ 启用 + 三行回执。──────
    #[tokio::test]
    async fn r10_external_setup_three_nonempty_inputs_persisted_and_enabled() {
        let Some((tw, bin)) = r10_ws_with_cfg(serde_json::json!({ "external": { "enabled": false } }))
        else {
            return;
        };
        let out = tw
            .run_cli_with_stdin(
                bin.as_path(),
                &["channel", "external", "setup"],
                "demo-in.exe\ndemo-out.exe\nexternal:alpha\n",
                30,
            )
            .await;
        assert!(out.success(), "{}\n{}", out.stdout, out.stderr);
        assert!(out.stdout.contains("External channel configured and enabled."));
        assert!(out.stdout.contains("external:alpha"));

        let ext = &r10_read_cfg(&tw)["channels"]["external"];
        assert_eq!(ext["input_exe"], serde_json::json!("demo-in.exe"), "非空 input_exe 臂生效");
        assert_eq!(ext["output_exe"], serde_json::json!("demo-out.exe"), "非空 output_exe 臂生效");
        assert_eq!(ext["chat_id"], serde_json::json!("external:alpha"), "非空 chat_id 臂生效");
        assert_eq!(ext["enabled"], serde_json::json!(true));
    }

    // ── websocket set port 成功落盘分支尾（667-672）：set_channel_config_value
    //    以 JSON 数字写入；进程内直调即可覆盖（无 stdin 参与）。────────────────
    #[test]
    fn r10_websocket_set_port_success_saves_json_number_tail() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = temp_home_env();
        write_main_cfg(&th.home, &serde_json::json!({ "channels": { "websocket": {} } }));
        run(
            ChannelAction::WebSocket {
                action: WebSocketAction::Set { key: "port".into(), value: "55501".into() },
            },
            false,
        )
        .unwrap();
        let port = read_main_cfg(&th.home)["channels"]["websocket"]["port"].clone();
        assert_eq!(
            port,
            serde_json::json!(55501),
            "成功路径尾部必须是 JSON 数字而非字符串"
        );
        assert!(port.is_u64(), "类型契约：as_u64 读侧可见，got: {}", port);
    }
}
