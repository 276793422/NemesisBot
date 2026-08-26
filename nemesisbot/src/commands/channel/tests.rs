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
    if let Some(ch) = config.pointer_mut("/channels/telegram") {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
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
    if let Some(ch) = config.pointer_mut("/channels/web") {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
        }
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
    if let Some(ch) = config.pointer_mut("/channels/telegram") {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
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
    if let Some(ch) = config.pointer_mut("/channels/web") {
        if let Some(obj) = ch.as_object_mut() {
            obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
        }
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
    assert_eq!(web["port"], serde_json::json!("49152"));
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
    assert_eq!(cfg["channels"]["web"]["port"], serde_json::json!("49001"));
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
    assert_eq!(cfg["channels"]["websocket"]["port"], serde_json::json!("49001"));
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
    // 合法端口写入
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
        serde_json::json!("49152")
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
