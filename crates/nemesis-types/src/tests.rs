//! Integration tests for nemesis-types

#[test]
fn test_library_imports() {
    use crate::agent::{AgentConfig, AgentMessage, MessageRole, SessionKey};
    use crate::channel::{ChannelUser, InboundMessage, OutboundMessage};
    use crate::cluster::{NodeInfo, Task};
    use crate::config::AppConfig;
    use crate::error::{NemesisError, Result};
    use crate::forge::{Artifact, Experience};
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
        BUS_CHANNEL_CAPACITY, CLUSTER_CONTINUATION_PREFIX, CLUSTER_DIR, CONFIG_FILE,
        DEFAULT_MAX_CONTEXT_TOKENS, DEFAULT_MAX_ITERATIONS, FORGE_DIR, IDENTITY_FILE,
        PEER_CHAT_TIMEOUT_SECS, RPC_CHANNEL_TIMEOUT_SECS, RPC_CLIENT_TIMEOUT_SECS, RPC_PREFIX,
        SCANNER_CONFIG_FILE, SKILLS_DIR, SOUL_FILE, USER_FILE, WORKSPACE_DIR, is_internal_channel,
    };

    assert_eq!(CONFIG_FILE, "config.json");
    assert_eq!(WORKSPACE_DIR, ".nemesisbot");
    assert_eq!(IDENTITY_FILE, "IDENTITY.md");
    assert_eq!(SOUL_FILE, "SOUL.md");
    assert_eq!(USER_FILE, "USER.md");
    assert_eq!(RPC_PREFIX, "[rpc:");
    assert_eq!(CLUSTER_CONTINUATION_PREFIX, "cluster_continuation:");
    assert_eq!(DEFAULT_MAX_ITERATIONS, 60);
    assert_eq!(DEFAULT_MAX_CONTEXT_TOKENS, 128_000);
    assert_eq!(RPC_CLIENT_TIMEOUT_SECS, 3600);
    assert_eq!(PEER_CHAT_TIMEOUT_SECS, 3540);
    assert_eq!(RPC_CHANNEL_TIMEOUT_SECS, 86400);
    assert_eq!(BUS_CHANNEL_CAPACITY, 1024);
    assert_eq!(SCANNER_CONFIG_FILE, "config.scanner.json");
    assert_eq!(FORGE_DIR, "forge");
    assert_eq!(CLUSTER_DIR, "cluster");
    assert_eq!(SKILLS_DIR, "Skills");

    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(!is_internal_channel("web"));
}
