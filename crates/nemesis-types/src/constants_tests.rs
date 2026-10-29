//! Tests for constants.rs

#[test]
fn test_constants_values() {
    // Test that constants have expected values
    // （BUG-20261029-184：零消费死常量已删除，仅钉活常量单一真相源值。）
    assert_eq!(crate::CLUSTER_CONTINUATION_PREFIX, "cluster_continuation:");
    assert_eq!(
        crate::constants::SUBAGENT_CONTINUATION_PREFIX,
        "subagent_continuation:"
    );
    assert_eq!(crate::RPC_CHANNEL_TIMEOUT_SECS, 86400);
    assert_eq!(crate::BUS_CHANNEL_CAPACITY, 1024);
    assert_eq!(crate::CLEANUP_INTERVAL_SECS, 30);
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
