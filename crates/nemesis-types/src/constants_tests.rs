//! Tests for constants.rs

#[test]
fn test_constants_values() {
    // Test that constants have expected values
    assert_eq!(crate::CONFIG_FILE, "config.json");
    assert_eq!(crate::WORKSPACE_DIR, ".nemesisbot");
    assert_eq!(crate::IDENTITY_FILE, "IDENTITY.md");
    assert_eq!(crate::SOUL_FILE, "SOUL.md");
    assert_eq!(crate::USER_FILE, "USER.md");
    assert_eq!(crate::RPC_PREFIX, "[rpc:");
    assert_eq!(crate::CLUSTER_CONTINUATION_PREFIX, "cluster_continuation:");
    assert_eq!(crate::DEFAULT_MAX_ITERATIONS, 60);
    assert_eq!(crate::DEFAULT_MAX_CONTEXT_TOKENS, 128_000);
    assert_eq!(crate::RPC_CLIENT_TIMEOUT_SECS, 3600);
    assert_eq!(crate::PEER_CHAT_TIMEOUT_SECS, 3540);
    assert_eq!(crate::RPC_CHANNEL_TIMEOUT_SECS, 86400);
    assert_eq!(crate::BUS_CHANNEL_CAPACITY, 1024);
    assert_eq!(crate::CLEANUP_INTERVAL_SECS, 30);
    assert_eq!(crate::SCANNER_CONFIG_FILE, "config.scanner.json");
    assert_eq!(crate::FORGE_DIR, "forge");
    assert_eq!(crate::CLUSTER_DIR, "cluster");
    assert_eq!(crate::SKILLS_DIR, "Skills");
}

#[test]
fn test_internal_channels() {
    // Test INTERNAL_CHANNELS constant
    assert!(crate::INTERNAL_CHANNELS.contains(&"cli"));
    assert!(crate::INTERNAL_CHANNELS.contains(&"system"));
    assert!(crate::INTERNAL_CHANNELS.contains(&"subagent"));
    assert!(!crate::INTERNAL_CHANNELS.contains(&"web"));
    assert!(!crate::INTERNAL_CHANNELS.contains(&"telegram"));
}

#[test]
fn test_is_internal_channel() {
    // Test is_internal_channel function
    assert!(crate::is_internal_channel("cli"));
    assert!(crate::is_internal_channel("system"));
    assert!(crate::is_internal_channel("subagent"));
    assert!(!crate::is_internal_channel("web"));
    assert!(!crate::is_internal_channel("telegram"));
    assert!(!crate::is_internal_channel("discord"));
}
