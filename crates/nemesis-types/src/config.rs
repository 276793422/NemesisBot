//! Main configuration types.

use serde::{Deserialize, Serialize};

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub provider: ProviderSettings,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub forge: ForgeConfig,
    #[serde(default)]
    pub cluster: ClusterConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub workflow: WorkflowConfig,
    #[serde(default)]
    pub vector: VectorConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub restrict_to_workspace: bool,
    #[serde(default)]
    pub audit_chain_enabled: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            restrict_to_workspace: true,
            audit_chain_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderSettings {
    pub default_model: Option<String>,
    pub providers: Vec<super::provider::ProviderConfig>,
    #[serde(default)]
    pub routing_strategy: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelsConfig {
    pub enabled_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeConfig {
    #[serde(default)]
    pub enabled: bool,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self { enabled: false }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterConfig {
    #[serde(default)]
    pub enabled: bool,
    pub name: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorConfig {
    #[serde(default = "default_embedding_tier")]
    pub embedding_tier: String,
    #[serde(default = "default_local_dim")]
    pub local_dim: usize,
    pub plugin_path: Option<String>,
    pub plugin_model_path: Option<String>,
    pub api_model: Option<String>,
}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            embedding_tier: default_embedding_tier(),
            local_dim: default_local_dim(),
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: Option<String>,
    pub format: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_embedding_tier() -> String {
    "auto".to_string()
}

fn default_local_dim() -> usize {
    256
}

#[cfg(test)]
mod tests {
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

    // --- VectorConfig ---

    #[test]
    fn test_vector_config_default() {
        let cfg = VectorConfig::default();
        assert_eq!(cfg.embedding_tier, "auto");
        assert_eq!(cfg.local_dim, 256);
        assert!(cfg.plugin_path.is_none());
        assert!(cfg.plugin_model_path.is_none());
        assert!(cfg.api_model.is_none());
    }

    #[test]
    fn test_vector_config_deserialize_empty() {
        let json = "{}";
        let cfg: VectorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.embedding_tier, "auto");
        assert_eq!(cfg.local_dim, 256);
    }

    #[test]
    fn test_vector_config_with_values() {
        let json = r#"{
            "embedding_tier": "local",
            "local_dim": 512,
            "plugin_path": "/path/to/plugin",
            "plugin_model_path": "/path/to/model",
            "api_model": "text-embedding-3-small"
        }"#;
        let cfg: VectorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.embedding_tier, "local");
        assert_eq!(cfg.local_dim, 512);
        assert_eq!(cfg.plugin_path, Some("/path/to/plugin".to_string()));
        assert_eq!(cfg.plugin_model_path, Some("/path/to/model".to_string()));
        assert_eq!(cfg.api_model, Some("text-embedding-3-small".to_string()));
    }

    #[test]
    fn test_vector_config_serialize_roundtrip() {
        let cfg = VectorConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let cfg2: VectorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg.embedding_tier, cfg2.embedding_tier);
        assert_eq!(cfg.local_dim, cfg2.local_dim);
    }

    // --- LoggingConfig ---

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
        assert_eq!(cfg.vector.embedding_tier, "auto");
        assert_eq!(cfg.vector.local_dim, 256);
    }

    #[test]
    fn test_app_config_deserialize_empty_object() {
        let json = "{}";
        let cfg: AppConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.security.enabled);
        assert!(!cfg.forge.enabled);
        assert!(!cfg.cluster.enabled);
        assert_eq!(cfg.vector.embedding_tier, "auto");
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
            "vector": {"embedding_tier": "api", "local_dim": 1024},
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
        assert_eq!(cfg.vector.embedding_tier, "api");
        assert_eq!(cfg.vector.local_dim, 1024);
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
        assert_eq!(cfg.vector.embedding_tier, "auto");
    }

    #[test]
    fn test_app_config_clone() {
        let cfg = app_config_with_defaults();
        let cfg2 = cfg.clone();
        assert_eq!(cfg.security.enabled, cfg2.security.enabled);
        assert_eq!(cfg.vector.embedding_tier, cfg2.vector.embedding_tier);
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
        assert!(json.contains("\"vector\""));
        assert!(json.contains("\"logging\""));
    }

    // --- Helper functions ---

    #[test]
    fn test_default_true_returns_true() {
        assert!(default_true());
    }

    #[test]
    fn test_default_embedding_tier_returns_auto() {
        assert_eq!(default_embedding_tier(), "auto");
    }

    #[test]
    fn test_default_local_dim_returns_256() {
        assert_eq!(default_local_dim(), 256);
    }
}
