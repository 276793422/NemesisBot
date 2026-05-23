use super::*;

// --- SecurityConfig ---

#[test]
fn test_security_config_default() {
    let cfg = SecurityConfig::default();
    assert!(cfg.enabled);
    assert!(cfg.restrict_to_workspace);
    assert!(!cfg.audit_chain_enabled);
}

#[test]
fn test_security_config_serialize_deserialize() {
    let cfg = SecurityConfig::default();
    let json = serde_json::to_string(&cfg).unwrap();
    let cfg2: SecurityConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg.enabled, cfg2.enabled);
    assert_eq!(cfg.restrict_to_workspace, cfg2.restrict_to_workspace);
    assert_eq!(cfg.audit_chain_enabled, cfg2.audit_chain_enabled);
}

#[test]
fn test_security_config_deserialize_empty_object() {
    // All fields have defaults, so empty object should work.
    let json = "{}";
    let cfg: SecurityConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled); // default_true
    assert!(!cfg.restrict_to_workspace); // Default trait => false
    assert!(!cfg.audit_chain_enabled);
}

#[test]
fn test_security_config_deserialize_explicit_false() {
    let json = r#"{"enabled": false, "restrict_to_workspace": false}"#;
    let cfg: SecurityConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled);
    assert!(!cfg.restrict_to_workspace);
}

#[test]
fn test_security_config_clone() {
    let cfg = SecurityConfig::default();
    let cfg2 = cfg.clone();
    assert_eq!(cfg.enabled, cfg2.enabled);
}

// --- ProviderSettings ---

#[test]
fn test_provider_settings_default() {
    let ps = ProviderSettings::default();
    assert!(ps.default_model.is_none());
    assert!(ps.providers.is_empty());
    assert!(ps.routing_strategy.is_empty());
}

#[test]
fn test_provider_settings_deserialize_empty() {
    // providers field is required (no #[serde(default)])
    let json = r#"{"providers": []}"#;
    let ps: ProviderSettings = serde_json::from_str(json).unwrap();
    assert!(ps.default_model.is_none());
    assert!(ps.providers.is_empty());
    assert!(ps.routing_strategy.is_empty());
}

#[test]
fn test_provider_settings_deserialize_missing_providers_fails() {
    let json = "{}";
    let result = serde_json::from_str::<ProviderSettings>(json);
    assert!(result.is_err());
}

#[test]
fn test_provider_settings_with_values() {
    let json = r#"{
        "default_model": "gpt-4",
        "providers": [{"provider": "openai", "model": "gpt-4", "api_key": "sk-test", "base_url": null, "is_default": true}],
        "routing_strategy": "round-robin"
    }"#;
    let ps: ProviderSettings = serde_json::from_str(json).unwrap();
    assert_eq!(ps.default_model, Some("gpt-4".to_string()));
    assert_eq!(ps.providers.len(), 1);
    assert_eq!(ps.routing_strategy, "round-robin");
}

#[test]
fn test_provider_settings_serialize_deserialize_roundtrip() {
    let ps = ProviderSettings::default();
    let json = serde_json::to_string(&ps).unwrap();
    let ps2: ProviderSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(ps.default_model, ps2.default_model);
    assert_eq!(ps.providers.len(), ps2.providers.len());
}

// --- ChannelsConfig ---

#[test]
fn test_channels_config_default() {
    let cfg = ChannelsConfig::default();
    assert!(cfg.enabled_channels.is_empty());
}

#[test]
fn test_channels_config_with_channels() {
    let json = r#"{"enabled_channels": ["web", "discord", "telegram"]}"#;
    let cfg: ChannelsConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.enabled_channels, vec!["web", "discord", "telegram"]);
}

// --- ForgeConfig ---

#[test]
fn test_forge_config_default() {
    let cfg = ForgeConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_forge_config_deserialize_empty() {
    let json = "{}";
    let cfg: ForgeConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled);
}

#[test]
fn test_forge_config_enabled() {
    let json = r#"{"enabled": true}"#;
    let cfg: ForgeConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
}

// --- ClusterConfig ---

#[test]
fn test_cluster_config_default() {
    let cfg = ClusterConfig::default();
    assert!(!cfg.enabled);
    assert!(cfg.name.is_none());
    assert!(cfg.role.is_none());
}

#[test]
fn test_cluster_config_with_values() {
    let json = r#"{"enabled": true, "name": "bot1", "role": "worker"}"#;
    let cfg: ClusterConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
    assert_eq!(cfg.name, Some("bot1".to_string()));
    assert_eq!(cfg.role, Some("worker".to_string()));
}

#[test]
fn test_cluster_config_deserialize_empty() {
    let json = "{}";
    let cfg: ClusterConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.enabled);
    assert!(cfg.name.is_none());
}

// --- MemoryConfig ---

#[test]
fn test_memory_config_default() {
    let cfg = MemoryConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_memory_config_enabled() {
    let json = r#"{"enabled": true}"#;
    let cfg: MemoryConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
}

// --- WorkflowConfig ---

#[test]
fn test_workflow_config_default() {
    let cfg = WorkflowConfig::default();
    assert!(!cfg.enabled);
}

#[test]
fn test_workflow_config_enabled() {
    let json = r#"{"enabled": true}"#;
    let cfg: WorkflowConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.enabled);
}

// --- WorkflowConfig ---

#[test]
fn test_logging_config_default() {
    let cfg = LoggingConfig::default();
    assert!(cfg.level.is_none());
    assert!(cfg.format.is_none());
}

#[test]
fn test_logging_config_with_values() {
    let json = r#"{"level": "debug", "format": "json"}"#;
    let cfg: LoggingConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.level, Some("debug".to_string()));
    assert_eq!(cfg.format, Some("json".to_string()));
}

fn app_config_with_defaults() -> AppConfig {
    serde_json::from_str("{}").unwrap()
}

// --- AppConfig ---

#[test]
fn test_app_config_default() {
    let cfg = app_config_with_defaults();
    // All sub-configs should use their defaults
    assert!(cfg.security.enabled);
    assert!(cfg.security.restrict_to_workspace);
    assert!(cfg.provider.default_model.is_none());
    assert!(cfg.channels.enabled_channels.is_empty());
    assert!(!cfg.forge.enabled);
    assert!(!cfg.cluster.enabled);
    assert!(!cfg.memory.enabled);
    assert!(!cfg.workflow.enabled);
}

#[test]
fn test_app_config_deserialize_empty_object() {
    let json = "{}";
    let cfg: AppConfig = serde_json::from_str(json).unwrap();
    assert!(cfg.security.enabled);
    assert!(!cfg.forge.enabled);
    assert!(!cfg.cluster.enabled);
}

#[test]
fn test_app_config_full_roundtrip() {
    let json = r#"{
        "security": {"enabled": false, "restrict_to_workspace": false, "audit_chain_enabled": true},
        "provider": {"default_model": "gpt-4", "providers": [], "routing_strategy": ""},
        "channels": {"enabled_channels": ["web"]},
        "forge": {"enabled": true},
        "cluster": {"enabled": true, "name": "node1", "role": "master"},
        "memory": {"enabled": true},
        "workflow": {"enabled": true},
        "logging": {"level": "info", "format": "text"}
    }"#;
    let cfg: AppConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.security.enabled);
    assert!(!cfg.security.restrict_to_workspace);
    assert!(cfg.security.audit_chain_enabled);
    assert_eq!(cfg.provider.default_model, Some("gpt-4".to_string()));
    assert_eq!(cfg.channels.enabled_channels, vec!["web"]);
    assert!(cfg.forge.enabled);
    assert!(cfg.cluster.enabled);
    assert_eq!(cfg.cluster.name, Some("node1".to_string()));
    assert_eq!(cfg.cluster.role, Some("master".to_string()));
    assert!(cfg.memory.enabled);
    assert!(cfg.workflow.enabled);
    assert_eq!(cfg.logging.level, Some("info".to_string()));
    assert_eq!(cfg.logging.format, Some("text".to_string()));

    // Roundtrip
    let json2 = serde_json::to_string(&cfg).unwrap();
    let cfg2: AppConfig = serde_json::from_str(&json2).unwrap();
    assert_eq!(cfg.security.enabled, cfg2.security.enabled);
    assert_eq!(cfg.cluster.name, cfg2.cluster.name);
}

#[test]
fn test_app_config_partial_override() {
    // Only override some fields, others should use defaults
    let json = r#"{
        "security": {"enabled": false},
        "forge": {"enabled": true}
    }"#;
    let cfg: AppConfig = serde_json::from_str(json).unwrap();
    assert!(!cfg.security.enabled);
    assert!(!cfg.security.restrict_to_workspace); // serde default for bool is false
    assert!(cfg.forge.enabled);
    assert!(!cfg.cluster.enabled);
}

#[test]
fn test_app_config_clone() {
    let cfg = app_config_with_defaults();
    let cfg2 = cfg.clone();
    assert_eq!(cfg.security.enabled, cfg2.security.enabled);
}

#[test]
fn test_app_config_serialize_contains_all_fields() {
    let cfg = app_config_with_defaults();
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    // Verify all top-level sections are present
    assert!(json.contains("\"security\""));
    assert!(json.contains("\"provider\""));
    assert!(json.contains("\"channels\""));
    assert!(json.contains("\"forge\""));
    assert!(json.contains("\"cluster\""));
    assert!(json.contains("\"memory\""));
    assert!(json.contains("\"workflow\""));
    assert!(json.contains("\"logging\""));
}

// --- Helper functions ---

#[test]
fn test_default_true_returns_true() {
    assert!(default_true());
}
