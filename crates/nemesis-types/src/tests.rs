//! Integration tests for nemesis-types

#[test]
fn test_library_imports() {
    use crate::agent::SessionKey;
    use crate::memory::MemoryType;
    use crate::security::RiskLevel;

    // Test basic functionality
    assert_eq!(MemoryType::ShortTerm.to_string(), "short_term");
    assert!(RiskLevel::Low < RiskLevel::Medium);

    let session_key = SessionKey::new("web", "chat-1");
    assert_eq!(session_key.0, "web:chat-1");
}

#[test]
fn test_error_variants() {
    use crate::error::NemesisError;

    let config_err = NemesisError::Config("test".to_string());
    assert!(config_err.to_string().contains("Configuration error"));

    let security_err = NemesisError::Security("violation".to_string());
    assert!(security_err.to_string().contains("Security violation"));

    let not_found_err = NemesisError::NotFound("resource".to_string());
    assert!(not_found_err.to_string().contains("Not found"));
}

#[test]
fn test_constants() {
    use crate::constants::{
        BUS_CHANNEL_CAPACITY, CLUSTER_CONTINUATION_PREFIX, RPC_CHANNEL_TIMEOUT_SECS,
        is_internal_channel,
    };

    // BUG-20261029-184：零消费死常量已删除，仅钉活常量单一真相源值。
    assert_eq!(CLUSTER_CONTINUATION_PREFIX, "cluster_continuation:");
    assert_eq!(RPC_CHANNEL_TIMEOUT_SECS, 86400);
    assert_eq!(BUS_CHANNEL_CAPACITY, 1024);

    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(!is_internal_channel("web"));
}
