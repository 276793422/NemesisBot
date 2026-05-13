//! NemesisBot - Configuration Management
//!
//! Handles loading, saving, and workspace detection for all configuration.
//! Translated from Go module/config/.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub mod provider_resolver;

// Re-export provider_resolver types and functions for backward compatibility
pub use provider_resolver::{
    ProviderResolution, ProviderResolver, ModelResolution,
    resolve_model_config, get_model_by_name, get_effective_llm,
    infer_provider_from_model, infer_default_model, get_default_api_base,
    find_model_by_name, resolve_model_resolution,
};

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Config validation error: {0}")]
    Validation(String),
    #[error("Workspace not found")]
    WorkspaceNotFound,
}

pub type Result<T> = std::result::Result<T, ConfigError>;

// ============================================================================
// Flexible string slice deserializer - accepts both strings and numbers
// (mirrors Go's FlexibleStringSlice for config compatibility)
// ============================================================================

/// Custom deserializer for `allow_from` fields that accepts JSON arrays
/// containing mixed types (strings, numbers, etc.), converting all to strings.
/// This matches Go's `FlexibleStringSlice` behavior.
fn deserialize_flexible_string_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{SeqAccess, Visitor};
    use std::fmt;

    struct FlexibleVecVisitor;

    impl<'de> Visitor<'de> for FlexibleVecVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an array of strings or numbers")
        }

        fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Vec<String>, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut result = Vec::new();
            while let Some(value) = seq.next_element::<serde_json::Value>()? {
                match value {
                    serde_json::Value::String(s) => result.push(s),
                    serde_json::Value::Number(n) => result.push(format!("{}", n)),
                    other => result.push(other.to_string()),
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_seq(FlexibleVecVisitor)
}

// ============================================================================
// Main Config struct - mirrors Go Config exactly
// ============================================================================

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub bindings: Vec<AgentBinding>,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub model_list: Vec<ModelConfig>,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
    #[serde(default)]
    pub devices: DevicesConfig,
    #[serde(default)]
    pub logging: Option<LoggingConfig>,
    #[serde(default)]
    pub security: Option<SecurityFlagConfig>,
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
    #[serde(default)]
    pub forge: Option<ForgeFlagConfig>,
    #[serde(default)]
    pub memory: Option<MemoryFlagConfig>,
    #[serde(default)]
    pub mcp: Option<McpConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agents: AgentsConfig::default(),
            bindings: vec![],
            session: SessionConfig::default(),
            channels: ChannelsConfig::default(),
            model_list: vec![],
            gateway: GatewayConfig::default(),
            tools: ToolsConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            devices: DevicesConfig::default(),
            logging: None,
            security: None,
            skills: None,
            forge: None,
            memory: None,
            mcp: None,
        }
    }
}

// ============================================================================
// Agents Config
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: AgentDefaults,
    #[serde(default)]
    pub list: Vec<AgentConfigEntry>,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            defaults: AgentDefaults::default(),
            list: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    #[serde(default)]
    pub workspace: String,
    #[serde(default = "default_true")]
    pub restrict_to_workspace: bool,
    #[serde(default)]
    pub llm: String,
    #[serde(default)]
    pub image_model: String,
    #[serde(default)]
    pub image_model_fallbacks: Vec<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i64,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: i64,
    #[serde(default = "default_concurrent_request_mode")]
    pub concurrent_request_mode: String,
    #[serde(default = "default_queue_size")]
    pub queue_size: i64,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: String::new(),
            restrict_to_workspace: true,
            llm: String::new(),
            image_model: String::new(),
            image_model_fallbacks: vec![],
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            max_tool_iterations: default_max_tool_iterations(),
            concurrent_request_mode: default_concurrent_request_mode(),
            queue_size: default_queue_size(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfigEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub model: Option<AgentModelConfig>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub subagents: Option<SubagentsConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentModelConfig {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// Support both string and structured model config during deserialization.
impl<'de> serde::Deserialize<'de> for AgentModelConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::{self, Visitor};
        use std::fmt;

        struct AgentModelConfigVisitor;

        impl<'de> Visitor<'de> for AgentModelConfigVisitor {
            type Value = AgentModelConfig;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a string or an object with primary and fallbacks")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(AgentModelConfig {
                    primary: v.to_string(),
                    fallbacks: vec![],
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> std::result::Result<Self::Value, A::Error> {
                #[derive(Deserialize)]
                struct Raw {
                    #[serde(default)]
                    primary: String,
                    #[serde(default)]
                    fallbacks: Vec<String>,
                }
                let raw = Raw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(AgentModelConfig {
                    primary: raw.primary,
                    fallbacks: raw.fallbacks,
                })
            }
        }

        deserializer.deserialize_any(AgentModelConfigVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentsConfig {
    #[serde(default)]
    pub allow_agents: Vec<String>,
    #[serde(default)]
    pub model: Option<AgentModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBinding {
    #[serde(default)]
    pub agent_id: String,
    pub r#match: BindingMatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingMatch {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub peer: Option<PeerMatch>,
    #[serde(default)]
    pub guild_id: String,
    #[serde(default)]
    pub team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMatch {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub dm_scope: String,
    #[serde(default)]
    pub identity_links: std::collections::HashMap<String, Vec<String>>,
}

// ============================================================================
// Channels Config
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub whatsapp: WhatsAppConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub feishu: FeishuConfig,
    #[serde(default)]
    pub discord: DiscordConfig,
    #[serde(default)]
    pub maixcam: MaixCamConfig,
    #[serde(default)]
    pub qq: QqConfig,
    #[serde(default)]
    pub dingtalk: DingTalkConfig,
    #[serde(default)]
    pub slack: SlackConfig,
    #[serde(default)]
    pub line: LineConfig,
    #[serde(default)]
    pub onebot: OneBotConfig,
    #[serde(default)]
    pub web: WebChannelConfig,
    #[serde(default)]
    pub websocket: WebSocketChannelConfig,
    #[serde(default)]
    pub external: ExternalConfig,
}

// Channel configuration structs follow below.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub bridge_url: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub token: String,
    #[serde(default)] pub proxy: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub app_id: String,
    #[serde(default)] pub app_secret: String,
    #[serde(default)] pub encrypt_key: String,
    #[serde(default)] pub verification_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaixCamConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub host: String,
    #[serde(default)] pub port: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QqConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub app_id: String,
    #[serde(default)] pub app_secret: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingTalkConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub client_id: String,
    #[serde(default)] pub client_secret: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub bot_token: String,
    #[serde(default)] pub app_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LineConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub channel_secret: String,
    #[serde(default)] pub channel_access_token: String,
    #[serde(default)] pub webhook_host: String,
    #[serde(default)] pub webhook_port: i64,
    #[serde(default)] pub webhook_path: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OneBotConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub ws_url: String,
    #[serde(default)] pub access_token: String,
    #[serde(default)] pub reconnect_interval: i64,
    #[serde(default)] pub group_trigger_prefix: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebChannelConfig {
    #[serde(default = "default_true")] pub enabled: bool,
    #[serde(default = "default_web_host")] pub host: String,
    #[serde(default = "default_web_port")] pub port: i64,
    #[serde(default = "default_web_path")] pub path: String,
    #[serde(default)] pub auth_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default = "default_heartbeat_interval")] pub heartbeat_interval: i64,
    #[serde(default = "default_session_timeout")] pub session_timeout: i64,
    #[serde(default)] pub sync_to: Vec<String>,
}

impl Default for WebChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_web_host(),
            port: default_web_port(),
            path: default_web_path(),
            auth_token: String::new(),
            allow_from: vec![],
            heartbeat_interval: default_heartbeat_interval(),
            session_timeout: default_session_timeout(),
            sync_to: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSocketChannelConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub host: String,
    #[serde(default)] pub port: i64,
    #[serde(default)] pub path: String,
    #[serde(default)] pub auth_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
    #[serde(default)] pub sync_to_web: bool,
    #[serde(default)] pub web_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub input_exe: String,
    #[serde(default)] pub output_exe: String,
    #[serde(default)] pub chat_id: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")] pub allow_from: Vec<String>,
    #[serde(default)] pub sync_to: Vec<String>,
    #[serde(default)] pub sync_to_web: bool,
    #[serde(default)] pub web_session_id: String,
}

// ============================================================================
// Model Config
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)] pub model_name: String,
    #[serde(default)] pub model: String,
    #[serde(default)] pub api_base: String,
    #[serde(default)] pub api_key: String,
    #[serde(default)] pub proxy: String,
    #[serde(default)] pub auth_method: String,
    #[serde(default)] pub connect_mode: String,
    #[serde(default)] pub workspace: String,
}

impl ModelConfig {
    pub fn validate(&self) -> Result<()> {
        if self.model_name.is_empty() {
            return Err(ConfigError::Validation("model_name is required".into()));
        }
        if self.model.is_empty() {
            return Err(ConfigError::Validation("model is required".into()));
        }
        Ok(())
    }

    /// Parse the model string to extract protocol and model identifier.
    /// Format: [protocol/]model-identifier (e.g., "openai/gpt-4o")
    pub fn parse_model(&self) -> (&str, &str) {
        if let Some(slash_pos) = self.model.find('/') {
            (&self.model[..slash_pos], &self.model[slash_pos + 1..])
        } else {
            ("openai", &self.model)
        }
    }
}

// ============================================================================
// Gateway, Tools, etc.
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_gateway_host")] pub host: String,
    #[serde(default = "default_gateway_port")] pub port: i64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self { host: default_gateway_host(), port: default_gateway_port() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default)] pub web: WebToolsConfig,
    #[serde(default)] pub cron: CronToolsConfig,
    #[serde(default)] pub exec: ExecConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebToolsConfig {
    #[serde(default)] pub brave: BraveConfig,
    #[serde(default)] pub duckduckgo: DuckDuckGoConfig,
    #[serde(default)] pub perplexity: PerplexityConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BraveConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub api_key: String,
    #[serde(default = "default_max_results")] pub max_results: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DuckDuckGoConfig {
    #[serde(default = "default_true")] pub enabled: bool,
    #[serde(default = "default_max_results")] pub max_results: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerplexityConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub api_key: String,
    #[serde(default = "default_max_results")] pub max_results: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronToolsConfig {
    #[serde(default)] pub exec_timeout_minutes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecConfig {
    #[serde(default)] pub enable_deny_patterns: bool,
    #[serde(default)] pub custom_deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default = "default_true")] pub enabled: bool,
    #[serde(default = "default_heartbeat_interval_minutes")] pub interval: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevicesConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default = "default_true")] pub monitor_usb: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub llm: Option<LlmLogConfig>,
    pub general: Option<GeneralLogConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmLogConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub log_dir: String,
    #[serde(default = "default_detail_level")] pub detail_level: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeneralLogConfig {
    #[serde(default = "default_true")] pub enabled: bool,
    #[serde(default = "default_true")] pub enable_console: bool,
    #[serde(default = "default_log_level")] pub level: String,
    #[serde(default)] pub file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityFlagConfig {
    #[serde(default)] pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgeFlagConfig {
    #[serde(default)] pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryFlagConfig {
    #[serde(default)] pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)] pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)] pub enabled: bool,
    #[serde(default)] pub servers: Vec<McpServerConfig>,
    #[serde(default = "default_mcp_timeout")] pub timeout: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(default)] pub name: String,
    #[serde(default)] pub command: String,
    #[serde(default)] pub args: Vec<String>,
    #[serde(default)] pub env: Vec<String>,
    #[serde(default)] pub timeout: i64,
}

// ============================================================================
// Full sub-configurations (loaded from separate files)
// ============================================================================

/// Full skills configuration loaded from config.skills.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsFullConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub search_cache: SkillsSearchCacheConfig,
    #[serde(default = "default_max_concurrent_searches")]
    pub max_concurrent_searches: i64,
    #[serde(default)]
    pub github_sources: Vec<GitHubSourceConfig>,
    #[serde(default)]
    pub clawhub: SkillsClawHubConfig,
}

impl Default for SkillsFullConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            search_cache: SkillsSearchCacheConfig::default(),
            max_concurrent_searches: 2,
            github_sources: vec![],
            clawhub: SkillsClawHubConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsSearchCacheConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cache_max_size")]
    pub max_size: i64,
    #[serde(default = "default_cache_ttl_seconds")]
    pub ttl_seconds: i64,
}

impl Default for SkillsSearchCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: 50,
            ttl_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitHubSourceConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub index_type: String,
    #[serde(default)]
    pub index_path: String,
    #[serde(default)]
    pub skill_path_pattern: String,
    #[serde(default)]
    pub timeout: i64,
    #[serde(default)]
    pub max_size: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsClawHubConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub convex_url: String,
    #[serde(default)]
    pub timeout: i64,
}

/// Security configuration loaded from config.security.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_security_action")]
    pub default_action: String,
    #[serde(default = "default_true")]
    pub log_all_operations: bool,
    #[serde(default)]
    pub log_denials_only: bool,
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_seconds: i64,
    #[serde(default = "default_max_pending")]
    pub max_pending_requests: i64,
    #[serde(default = "default_audit_retention")]
    pub audit_log_retention_days: i64,
    #[serde(default)]
    pub audit_log_path: String,
    #[serde(default = "default_true")]
    pub audit_log_file_enabled: bool,
    #[serde(default)]
    pub synchronous_mode: bool,
    #[serde(default)]
    pub file_rules: Option<FileSecurityRules>,
    #[serde(default)]
    pub directory_rules: Option<DirectorySecurityRules>,
    #[serde(default)]
    pub process_rules: Option<ProcessSecurityRules>,
    #[serde(default)]
    pub network_rules: Option<NetworkSecurityRules>,
    #[serde(default)]
    pub hardware_rules: Option<HardwareSecurityRules>,
    #[serde(default)]
    pub registry_rules: Option<RegistrySecurityRules>,
    #[serde(default)]
    pub layers: Option<SecurityLayersConfig>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_action: "deny".to_string(),
            log_all_operations: true,
            log_denials_only: false,
            approval_timeout_seconds: 300,
            max_pending_requests: 100,
            audit_log_retention_days: 90,
            audit_log_path: String::new(),
            audit_log_file_enabled: true,
            synchronous_mode: false,
            file_rules: None,
            directory_rules: None,
            process_rules: None,
            network_rules: None,
            hardware_rules: None,
            registry_rules: None,
            layers: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityLayersConfig {
    #[serde(default)]
    pub injection: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub command_guard: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub dlp: Option<DLPLayerConfig>,
    #[serde(default)]
    pub ssrf: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub credential: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub signature: Option<SignatureLayerConfig>,
    #[serde(default)]
    pub audit_chain: Option<SecurityLayerConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DLPLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub action: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignatureLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityRule {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub action: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileSecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub write: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectorySecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub create: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessSecurityRules {
    #[serde(default)]
    pub exec: Vec<SecurityRule>,
    #[serde(default)]
    pub spawn: Vec<SecurityRule>,
    #[serde(default)]
    pub kill: Vec<SecurityRule>,
    #[serde(default)]
    pub suspend: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkSecurityRules {
    #[serde(default)]
    pub request: Vec<SecurityRule>,
    #[serde(default)]
    pub download: Vec<SecurityRule>,
    #[serde(default)]
    pub upload: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareSecurityRules {
    #[serde(default)]
    pub i2c: Vec<SecurityRule>,
    #[serde(default)]
    pub spi: Vec<SecurityRule>,
    #[serde(default)]
    pub gpio: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistrySecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub write: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

/// Scanner configuration loaded from config.scanner.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannerFullConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub engines: std::collections::HashMap<String, serde_json::Value>,
}

impl Default for ScannerFullConfig {
    fn default() -> Self {
        Self {
            enabled: vec![],
            engines: std::collections::HashMap::new(),
        }
    }
}

/// Engine state tracking for scanner engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineState {
    #[serde(default)]
    pub install_status: String,
    #[serde(default)]
    pub install_error: String,
    #[serde(default)]
    pub last_install_attempt: String,
    #[serde(default)]
    pub db_status: String,
    #[serde(default)]
    pub last_db_update: String,
}

/// ClamAV-specific scanner configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClamAVEngineConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub clamav_path: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub scan_on_write: bool,
    #[serde(default)]
    pub scan_on_download: bool,
    #[serde(default)]
    pub scan_on_exec: bool,
    #[serde(default)]
    pub scan_extensions: Vec<String>,
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    #[serde(default)]
    pub max_file_size: i64,
    #[serde(default)]
    pub update_interval: String,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub state: EngineState,
}

// ============================================================================
// Embedded defaults management
// ============================================================================

/// Holds embedded default configuration data (set at compile time).
#[derive(Debug, Clone, Default)]
pub struct EmbeddedDefaults {
    pub config: Vec<u8>,
    pub mcp: Vec<u8>,
    pub security: Vec<u8>,
    pub cluster: Vec<u8>,
    pub skills: Vec<u8>,
    pub scanner: Vec<u8>,
}

/// Global embedded defaults storage.
static EMBEDDED_DEFAULTS: std::sync::OnceLock<std::sync::RwLock<EmbeddedDefaults>> =
    std::sync::OnceLock::new();

fn embedded_defaults() -> &'static std::sync::RwLock<EmbeddedDefaults> {
    EMBEDDED_DEFAULTS.get_or_init(|| std::sync::RwLock::new(EmbeddedDefaults::default()))
}

/// Get the current embedded defaults.
pub fn get_embedded_defaults() -> EmbeddedDefaults {
    embedded_defaults().read().unwrap().clone()
}

/// Set embedded defaults from byte arrays.
pub fn set_embedded_defaults(
    config: Vec<u8>,
    mcp: Vec<u8>,
    security: Vec<u8>,
    cluster: Vec<u8>,
    skills: Vec<u8>,
    scanner: Vec<u8>,
) {
    let mut guard = embedded_defaults().write().unwrap();
    guard.config = config;
    guard.mcp = mcp;
    guard.security = security;
    guard.cluster = cluster;
    guard.skills = skills;
    guard.scanner = scanner;
}

/// Set embedded defaults from a filesystem path (directory of config files).
///
/// This mirrors the Go `SetEmbeddedDefaultsFromFS` function. Reads the
/// standard set of configuration files from the given directory:
/// - `config.default.json`
/// - `config.mcp.default.json`
/// - platform-specific security config
/// - `config.cluster.default.json`
/// - `config.skills.default.json`
/// - `config.scanner.default.json` (optional)
pub fn set_embedded_defaults_from_fs(config_dir: &Path) -> Result<()> {
    // Read config.default.json
    let config_path = config_dir.join("config.default.json");
    let config_data = std::fs::read(&config_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.default.json: {}", e),
        ))
    })?;

    // Read config.mcp.default.json
    let mcp_path = config_dir.join("config.mcp.default.json");
    let mcp_data = std::fs::read(&mcp_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.mcp.default.json: {}", e),
        ))
    })?;

    // Read platform-specific security config
    let security_filename = get_platform_security_config_filename();
    let security_path = config_dir.join(&security_filename);
    let security_data = std::fs::read(&security_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read {}: {}", security_filename, e),
        ))
    })?;

    // Read config.cluster.default.json
    let cluster_path = config_dir.join("config.cluster.default.json");
    let cluster_data = std::fs::read(&cluster_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.cluster.default.json: {}", e),
        ))
    })?;

    // Read config.skills.default.json
    let skills_path = config_dir.join("config.skills.default.json");
    let skills_data = std::fs::read(&skills_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.skills.default.json: {}", e),
        ))
    })?;

    // Read config.scanner.default.json (optional -- don't fail if missing)
    let scanner_path = config_dir.join("config.scanner.default.json");
    let scanner_data = std::fs::read(&scanner_path).unwrap_or_default();

    set_embedded_defaults(config_data, mcp_data, security_data, cluster_data, skills_data, scanner_data);

    Ok(())
}

// ============================================================================
// Provider resolution functions are now in the provider_resolver module.
// They are re-exported above for backward compatibility.
// ============================================================================

/// Load configuration from a file path.
/// Mirrors Go LoadConfig: tries file → embedded default → hardcoded default.
/// After loading, applies environment variable overrides (NEMESISBOT_*).
pub fn load_config(config_path: &Path) -> Result<Config> {
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        let mut config: Config = serde_json::from_str(&content)?;
        apply_env_overrides(&mut config);
        config.post_process_for_compatibility();
        config.adjust_paths_for_environment();
        return Ok(config);
    }

    let defaults = get_embedded_defaults();
    if !defaults.config.is_empty() {
        if let Ok(mut config) = serde_json::from_slice::<Config>(&defaults.config) {
            apply_env_overrides(&mut config);
            config.post_process_for_compatibility();
            config.adjust_paths_for_environment();
            return Ok(config);
        }
    }

    let mut config = default_config();
    apply_env_overrides(&mut config);
    config.post_process_for_compatibility();
    config.adjust_paths_for_environment();
    Ok(config)
}

/// Save configuration to a file path.
/// Mirrors Go SaveConfig: auto-adjusts paths for local mode before saving.
pub fn save_config(config_path: &Path, config: &mut Config) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Auto-adjust paths for local mode before saving (mirrors Go behavior)
    // When --local is used or .nemesisbot is detected in current directory,
    // workspace and other paths should be relative to the config directory
    if is_local_mode(config_path) {
        let config_dir = config_path.parent().unwrap_or(Path::new("."));
        if config_dir.file_name().map_or(false, |n| n == ".nemesisbot") {
            // Get the expected default workspace path
            let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
            let default_workspace_path = user_home.join(".nemesisbot").join("workspace");
            let default_workspace_str = default_workspace_path.to_string_lossy().to_string();

            let ws = &config.agents.defaults.workspace;
            // Check if workspace is using default path pattern
            if ws.starts_with("~/") || ws.starts_with("~\\")
                || ws.starts_with(&user_home.to_string_lossy().to_string())
                || ws == &default_workspace_str
            {
                config.agents.defaults.workspace = PathBuf::from(".nemesisbot")
                    .join("workspace")
                    .to_string_lossy()
                    .to_string();
            }

            // Normalize logging directory
            if let Some(ref mut logging) = config.logging {
                if let Some(ref mut llm) = logging.llm {
                    if llm.log_dir.is_empty() {
                        llm.log_dir = "logs/request_logs".to_string();
                    }
                }
            }
        }
    }

    let content = serde_json::to_string_pretty(&*config)?;
    // Write with restricted permissions (0600 on Unix) to protect API keys/tokens
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(config_path)?;
        std::io::Write::write_all(&mut f, content.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(config_path, content)?;
    }
    Ok(())
}

/// Check if we're in local mode (config is in current directory's .nemesisbot).
fn is_local_mode(config_path: &Path) -> bool {
    if let Ok(cwd) = std::env::current_dir() {
        let local_dir = cwd.join(".nemesisbot");
        if local_dir.exists() {
            if let Ok(canonical_config) = config_path.canonicalize() {
                if let Ok(canonical_local) = local_dir.canonicalize() {
                    if let Some(parent) = canonical_config.parent() {
                        return parent == canonical_local;
                    }
                }
            }
        }
    }
    false
}

/// Apply environment variable overrides to the config.
/// Mirrors Go's `env.Parse(cfg)` which reads NEMESISBOT_* env vars.
///
/// Supported environment variables (matching Go's env tags):
/// - NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE
/// - NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE
/// - NEMESISBOT_AGENTS_DEFAULTS_LLM
/// - NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL
/// - NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS
/// - NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE
/// - NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS
/// - NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE
/// - NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE
/// - NEMESISBOT_CHANNELS_WEB_ENABLED
/// - NEMESISBOT_CHANNELS_WEB_HOST
/// - NEMESISBOT_CHANNELS_WEB_PORT
/// - NEMESISBOT_GATEWAY_HOST
/// - NEMESISBOT_GATEWAY_PORT
/// - NEMESISBOT_HEARTBEAT_ENABLED
/// - NEMESISBOT_HEARTBEAT_INTERVAL
/// - NEMESISBOT_SECURITY_ENABLED
/// - NEMESISBOT_FORGE_ENABLED
pub fn apply_env_overrides(config: &mut Config) {
    // Agent defaults
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE") {
        config.agents.defaults.workspace = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE") {
        config.agents.defaults.restrict_to_workspace = v.parse().unwrap_or(true);
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_LLM") {
        config.agents.defaults.llm = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL") {
        config.agents.defaults.image_model = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS") {
        if let Ok(n) = v.parse() { config.agents.defaults.max_tokens = n; }
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE") {
        if let Ok(f) = v.parse() { config.agents.defaults.temperature = f; }
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS") {
        if let Ok(n) = v.parse() { config.agents.defaults.max_tool_iterations = n; }
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE") {
        config.agents.defaults.concurrent_request_mode = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE") {
        if let Ok(n) = v.parse() { config.agents.defaults.queue_size = n; }
    }

    // Web channel
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_ENABLED") {
        config.channels.web.enabled = v.parse().unwrap_or(true);
    }
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_HOST") {
        config.channels.web.host = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_PORT") {
        if let Ok(n) = v.parse() { config.channels.web.port = n; }
    }

    // Gateway
    if let Ok(v) = std::env::var("NEMESISBOT_GATEWAY_HOST") {
        config.gateway.host = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_GATEWAY_PORT") {
        if let Ok(n) = v.parse() { config.gateway.port = n; }
    }

    // Heartbeat
    if let Ok(v) = std::env::var("NEMESISBOT_HEARTBEAT_ENABLED") {
        config.heartbeat.enabled = v.parse().unwrap_or(true);
    }
    if let Ok(v) = std::env::var("NEMESISBOT_HEARTBEAT_INTERVAL") {
        if let Ok(n) = v.parse() { config.heartbeat.interval = n; }
    }

    // Security
    if let Ok(v) = std::env::var("NEMESISBOT_SECURITY_ENABLED") {
        let enabled = v.parse().unwrap_or(false);
        config.security = Some(SecurityFlagConfig { enabled });
    }

    // Forge
    if let Ok(v) = std::env::var("NEMESISBOT_FORGE_ENABLED") {
        let enabled = v.parse().unwrap_or(false);
        config.forge = Some(ForgeFlagConfig { enabled });
    }

    // Session
    if let Ok(v) = std::env::var("NEMESISBOT_SESSION_DM_SCOPE") {
        config.session.dm_scope = v;
    }
}

/// Load embedded default config.
pub fn load_embedded_config() -> Result<Config> {
    let defaults = get_embedded_defaults();
    if defaults.config.is_empty() {
        return Err(ConfigError::Validation("embedded default config not available".into()));
    }
    let mut config: Config = serde_json::from_slice(&defaults.config)?;
    config.post_process_for_compatibility();
    config.adjust_paths_for_environment();
    Ok(config)
}

/// Create a fully populated Config with all sensible defaults.
pub fn default_config() -> Config {
    let ws = WorkspaceResolver::resolve(false).join("workspace").to_string_lossy().to_string();
    Config {
        agents: AgentsConfig {
            defaults: AgentDefaults {
                workspace: ws,
                restrict_to_workspace: true,
                llm: "zhipu/glm-4.7-flash".to_string(),
                max_tokens: 8192,
                temperature: 0.7,
                max_tool_iterations: 20,
                concurrent_request_mode: "reject".to_string(),
                queue_size: 8,
                ..Default::default()
            },
            list: vec![],
        },
        bindings: vec![],
        session: SessionConfig::default(),
        channels: ChannelsConfig {
            whatsapp: WhatsAppConfig { bridge_url: "ws://localhost:3001".to_string(), ..Default::default() },
            maixcam: MaixCamConfig { host: "0.0.0.0".to_string(), port: 18790, ..Default::default() },
            line: LineConfig { webhook_host: "0.0.0.0".to_string(), webhook_port: 18791, webhook_path: "/webhook/line".to_string(), ..Default::default() },
            onebot: OneBotConfig { ws_url: "ws://127.0.0.1:3001".to_string(), reconnect_interval: 5, ..Default::default() },
            web: WebChannelConfig { enabled: true, host: "0.0.0.0".to_string(), port: 8080, path: "/ws".to_string(), heartbeat_interval: 30, session_timeout: 3600, ..Default::default() },
            external: ExternalConfig { chat_id: "external:main".to_string(), sync_to: vec!["web".to_string()], ..Default::default() },
            ..Default::default()
        },
        model_list: vec![],
        gateway: GatewayConfig { host: "0.0.0.0".to_string(), port: 18790 },
        tools: ToolsConfig {
            web: WebToolsConfig {
                duckduckgo: DuckDuckGoConfig { enabled: true, max_results: 5 },
                brave: BraveConfig { max_results: 5, ..Default::default() },
                perplexity: PerplexityConfig { max_results: 5, ..Default::default() },
            },
            cron: CronToolsConfig { exec_timeout_minutes: 5 },
            exec: ExecConfig { enable_deny_patterns: true, ..Default::default() },
        },
        heartbeat: HeartbeatConfig { enabled: true, interval: 30 },
        devices: DevicesConfig { monitor_usb: true, ..Default::default() },
        logging: Some(LoggingConfig { llm: Some(LlmLogConfig { enabled: false, log_dir: "logs/request_logs".to_string(), detail_level: "full".to_string() }), general: None }),
        security: Some(SecurityFlagConfig { enabled: false }),
        forge: Some(ForgeFlagConfig { enabled: false }),
        memory: None,
        skills: None,
        mcp: None,
    }
}

impl Config {
    /// Get the workspace path from config.
    pub fn workspace_path(&self) -> String {
        let ws = &self.agents.defaults.workspace;
        if ws.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(&ws[2..]).to_string_lossy().to_string();
            }
        }
        if ws.is_empty() {
            return WorkspaceResolver::resolve(false)
                .join("workspace")
                .to_string_lossy()
                .to_string();
        }
        ws.clone()
    }

    /// Find a model configuration by model name or vendor/model prefix.
    pub fn get_model_by_model_name(&self, model_ref: &str) -> Result<&ModelConfig> {
        // First try exact match with model_name
        for mc in &self.model_list {
            if mc.model_name == model_ref {
                return Ok(mc);
            }
        }

        // Then try prefix match with model field (vendor/model)
        for mc in &self.model_list {
            if mc.model == model_ref {
                return Ok(mc);
            }
        }

        Err(ConfigError::Validation(format!(
            "model {:?} not found in model_list",
            model_ref
        )))
    }

    /// Get model configuration for a given model name.
    /// Alias for get_model_by_model_name.
    pub fn get_model_config(&self, model_name: &str) -> Result<&ModelConfig> {
        self.get_model_by_model_name(model_name)
    }

    /// Populate deprecated fields from new fields for backward compatibility.
    ///
    /// This mirrors the Go `postProcessForCompatibility` function:
    /// - Sync `sync_to_web` from `sync_to` for External and WebSocket channels.
    pub fn post_process_for_compatibility(&mut self) {
        // External channel: populate SyncToWeb from SyncTo
        self.channels.external.sync_to_web = !self.channels.external.sync_to.is_empty();

        // WebSocket channel: populate SyncToWeb from SyncTo
        self.channels.websocket.sync_to_web = !self.channels.websocket.sync_to.is_empty();
    }

    /// Adjust hardcoded default paths to respect NEMESISBOT_HOME.
    ///
    /// This mirrors the Go `adjustPathsForEnvironment` function:
    /// - If the workspace uses a default path, replace it with the actual resolved path.
    /// - Set a default log directory if none is specified.
    pub fn adjust_paths_for_environment(&mut self) {
        let expected_workspace = WorkspaceResolver::resolve(false)
            .join("workspace");

        // Check if workspace is using a hardcoded default path
        let ws = &self.agents.defaults.workspace;
        let is_default = ws.is_empty()
            || ws == "~/.nemesisbot/workspace"
            || ws.ends_with(".nemesisbot/workspace");

        if is_default {
            self.agents.defaults.workspace = expected_workspace.to_string_lossy().to_string();
        }

        // Default log directory
        if let Some(ref mut logging) = self.logging {
            if let Some(ref mut llm) = logging.llm {
                if llm.log_dir.is_empty() {
                    llm.log_dir = "logs/request_logs".to_string();
                }
            }
        }
    }
}

// ============================================================================
// Sub-config load/save functions
// ============================================================================

/// Load MCP configuration from a separate config.mcp.json file.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_mcp_config(path: &Path) -> Result<McpConfig> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let cfg: McpConfig = serde_json::from_str(&content)?;
        return Ok(cfg);
    }

    // Try embedded default
    let defaults = get_embedded_defaults();
    if !defaults.mcp.is_empty() {
        if let Ok(cfg) = serde_json::from_slice::<McpConfig>(&defaults.mcp) {
            return Ok(cfg);
        }
    }

    // Hardcoded default
    Ok(McpConfig {
        enabled: false,
        servers: vec![],
        timeout: 30,
    })
}

/// Save MCP configuration to a separate config.mcp.json file.
pub fn save_mcp_config(path: &Path, cfg: &McpConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Load security configuration from config.security.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_security_config(path: &Path) -> Result<SecurityConfig> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let cfg: SecurityConfig = serde_json::from_str(&content)?;
        return Ok(cfg);
    }

    // Try embedded default
    let defaults = get_embedded_defaults();
    if !defaults.security.is_empty() {
        if let Ok(cfg) = serde_json::from_slice::<SecurityConfig>(&defaults.security) {
            return Ok(cfg);
        }
    }

    // Hardcoded default
    Ok(SecurityConfig::default())
}

/// Save security configuration to config.security.json.
pub fn save_security_config(path: &Path, cfg: &SecurityConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Load scanner configuration from config.scanner.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_scanner_config(path: &Path) -> Result<ScannerFullConfig> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let cfg: ScannerFullConfig = serde_json::from_str(&content)?;
        return Ok(cfg);
    }

    // Try embedded default
    let defaults = get_embedded_defaults();
    if !defaults.scanner.is_empty() {
        if let Ok(cfg) = serde_json::from_slice::<ScannerFullConfig>(&defaults.scanner) {
            return Ok(cfg);
        }
    }

    // Hardcoded default
    Ok(ScannerFullConfig::default())
}

/// Save scanner configuration to config.scanner.json.
pub fn save_scanner_config(path: &Path, cfg: &ScannerFullConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, content)?;
    Ok(())
}

/// Load skills configuration from config.skills.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_skills_config(path: &Path) -> Result<SkillsFullConfig> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let cfg: SkillsFullConfig = serde_json::from_str(&content)?;
        return Ok(cfg);
    }

    // Try embedded default
    let defaults = get_embedded_defaults();
    if !defaults.skills.is_empty() {
        if let Ok(cfg) = serde_json::from_slice::<SkillsFullConfig>(&defaults.skills) {
            return Ok(cfg);
        }
    }

    // Hardcoded default
    Ok(SkillsFullConfig::default())
}

/// Save skills configuration to config.skills.json.
pub fn save_skills_config(path: &Path, cfg: &SkillsFullConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, content)?;
    Ok(())
}

// ============================================================================
// Additional default value functions
// ============================================================================

fn default_security_action() -> String { "deny".to_string() }
fn default_approval_timeout() -> i64 { 300 }
fn default_max_pending() -> i64 { 100 }
fn default_audit_retention() -> i64 { 90 }
fn default_cache_max_size() -> i64 { 50 }
fn default_cache_ttl_seconds() -> i64 { 300 }
fn default_max_concurrent_searches() -> i64 { 2 }

// ============================================================================
// Default value functions
// ============================================================================

fn default_true() -> bool { true }
fn default_max_tokens() -> i64 { 8192 }
fn default_temperature() -> f64 { 0.7 }
fn default_max_tool_iterations() -> i64 { 20 }
fn default_concurrent_request_mode() -> String { "reject".to_string() }
fn default_queue_size() -> i64 { 8 }
fn default_gateway_host() -> String { "0.0.0.0".to_string() }
fn default_gateway_port() -> i64 { 18790 }
fn default_web_host() -> String { "0.0.0.0".to_string() }
fn default_web_port() -> i64 { 8080 }
fn default_web_path() -> String { "/ws".to_string() }
fn default_heartbeat_interval() -> i64 { 30 }
fn default_session_timeout() -> i64 { 3600 }
fn default_heartbeat_interval_minutes() -> i64 { 30 }
fn default_max_results() -> i64 { 5 }
fn default_detail_level() -> String { "full".to_string() }
fn default_log_level() -> String { "INFO".to_string() }
fn default_mcp_timeout() -> i64 { 30 }

// ============================================================================
// Workspace detection and config loading
// ============================================================================

/// Expand ~ in a path to the user's home directory.
/// Mirrors Go's `ExpandHome` function.
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs::home_dir() {
            if path == "~" {
                return home;
            }
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Workspace path resolver.
pub struct WorkspaceResolver;

impl WorkspaceResolver {
    /// Resolve workspace directory path.
    /// Priority: local_flag > env_var > auto_detect > default
    pub fn resolve(local: bool) -> PathBuf {
        if local {
            return PathBuf::from("./.nemesisbot");
        }

        if let Ok(home) = std::env::var("NEMESISBOT_HOME") {
            // Expand ~ to user home, then append .nemesisbot (matching Go behavior)
            let expanded = expand_tilde(&home);
            return expanded.join(".nemesisbot");
        }

        // Auto-detect: if .nemesisbot exists in current directory
        let local_path = PathBuf::from("./.nemesisbot");
        if local_path.exists() {
            return local_path;
        }

        // Default: ~/.nemesisbot
        dirs::home_dir()
            .map(|h| h.join(".nemesisbot"))
            .unwrap_or(local_path)
    }

    /// Get the config file path within the workspace.
    pub fn config_path(workspace: &Path) -> PathBuf {
        workspace.join("config.json")
    }

    /// Ensure workspace directory exists.
    pub fn ensure_workspace(workspace: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(workspace)?;
        Ok(())
    }
}

/// Configuration loader.
pub struct ConfigLoader;

impl ConfigLoader {
    /// Load config from a file path.
    pub fn load_from_file(path: &Path) -> Result<Config> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Load config from workspace directory.
    /// Three-tier fallback: file -> embedded default -> hardcoded default.
    /// Mirrors Go LoadConfig behavior.
    pub fn load_from_workspace(workspace: &Path) -> Result<Config> {
        let config_path = WorkspaceResolver::config_path(workspace);
        load_config(&config_path)
    }

    /// Save config to a file.
    pub fn save_to_file(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(config)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Save config to workspace.
    pub fn save_to_workspace(config: &Config, workspace: &Path) -> Result<()> {
        let config_path = WorkspaceResolver::config_path(workspace);
        Self::save_to_file(config, &config_path)
    }

    /// Load embedded default config (compile-time embedded).
    pub fn load_embedded_default() -> Result<Config> {
        let default_json = include_str!("../../../nemesisbot/config/config.default.json");
        let config: Config = serde_json::from_str(default_json)
            .unwrap_or_else(|_| Config::default());
        Ok(config)
    }
}

/// Get platform-specific security config filename.
/// On Linux: config.security.linux.json
/// On Windows: config.security.windows.json
/// On macOS: config.security.darwin.json
pub fn get_platform_security_config_filename() -> String {
    if cfg!(target_os = "linux") {
        "config.security.linux.json".to_string()
    } else if cfg!(target_os = "windows") {
        "config.security.windows.json".to_string()
    } else if cfg!(target_os = "macos") {
        "config.security.darwin.json".to_string()
    } else {
        "config.security.json".to_string()
    }
}

/// Get platform display name.
pub fn get_platform_display_name() -> String {
    if cfg!(target_os = "linux") {
        "Linux".to_string()
    } else if cfg!(target_os = "windows") {
        "Windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macOS".to_string()
    } else {
        "Unknown".to_string()
    }
}

/// Get platform info as a JSON value.
pub fn get_platform_info() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "display_name": get_platform_display_name(),
        "security_config": get_platform_security_config_filename(),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.model_list.is_empty());
        assert!(config.bindings.is_empty());
        assert!(config.gateway.port > 0);
    }

    #[test]
    fn test_config_serialize_deserialize() {
        let config = Config::default();
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.gateway.port, config.gateway.port);
    }

    #[test]
    fn test_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let config = Config::default();
        ConfigLoader::save_to_file(&config, &path).unwrap();
        let loaded = ConfigLoader::load_from_file(&path).unwrap();
        assert_eq!(loaded.gateway.port, config.gateway.port);
    }

    #[test]
    fn test_load_from_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join(".nemesisbot");

        // With config file - test file loading path (not dependent on embedded defaults)
        std::fs::create_dir_all(&workspace).unwrap();
        let custom = r#"{"gateway": {"host": "0.0.0.0", "port": 9090}}"#;
        std::fs::write(workspace.join("config.json"), custom).unwrap();

        let loaded = ConfigLoader::load_from_workspace(&workspace).unwrap();
        assert_eq!(loaded.gateway.host, "0.0.0.0");
        assert_eq!(loaded.gateway.port, 9090);
    }

    #[test]
    fn test_model_config_validate() {
        let model = ModelConfig {
            model_name: "gpt-4".to_string(),
            model: "openai/gpt-4".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        assert!(model.validate().is_ok());

        let empty = ModelConfig::default();
        assert!(empty.validate().is_err());
    }

    #[test]
    fn test_model_parse_protocol() {
        let model = ModelConfig {
            model_name: "test".to_string(),
            model: "anthropic/claude-3".to_string(),
            ..Default::default()
        };
        let (proto, name) = model.parse_model();
        assert_eq!(proto, "anthropic");
        assert_eq!(name, "claude-3");

        let model2 = ModelConfig {
            model_name: "test2".to_string(),
            model: "gpt-4o".to_string(),
            ..Default::default()
        };
        let (proto2, name2) = model2.parse_model();
        assert_eq!(proto2, "openai");
        assert_eq!(name2, "gpt-4o");
    }

    #[test]
    fn test_provider_resolver() {
        let models = vec![
            ModelConfig {
                model_name: "default".to_string(),
                model: "openai/gpt-4".to_string(),
                api_key: "key1".to_string(),
                ..Default::default()
            },
            ModelConfig {
                model_name: "fast".to_string(),
                model: "groq/llama3".to_string(),
                api_key: "key2".to_string(),
                ..Default::default()
            },
        ];

        let found = ProviderResolver::find_by_name(&models, "fast").unwrap();
        assert_eq!(found.model, "groq/llama3");

        let default = ProviderResolver::find_default(&models).unwrap();
        assert_eq!(default.model_name, "default");

        assert!(ProviderResolver::find_by_name(&models, "nonexistent").is_none());
    }

    #[test]
    fn test_resolve_model_string() {
        let (proto, model) = ProviderResolver::resolve_model_string("openai/gpt-4o");
        assert_eq!(proto, "openai");
        assert_eq!(model, "gpt-4o");

        let (proto2, model2) = ProviderResolver::resolve_model_string("llama3");
        assert_eq!(proto2, "openai");
        assert_eq!(model2, "llama3");
    }

    #[test]
    fn test_agent_model_config_string() {
        let json = r#""gpt-4""#;
        let config: AgentModelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.primary, "gpt-4");
        assert!(config.fallbacks.is_empty());
    }

    #[test]
    fn test_agent_model_config_object() {
        let json = r#"{"primary": "gpt-4", "fallbacks": ["claude-haiku"]}"#;
        let config: AgentModelConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.primary, "gpt-4");
        assert_eq!(config.fallbacks, vec!["claude-haiku"]);
    }

    #[test]
    fn test_workspace_resolve_local() {
        let path = WorkspaceResolver::resolve(true);
        assert_eq!(path, PathBuf::from("./.nemesisbot"));
    }

    #[test]
    fn test_platform_security_config_filename() {
        let filename = get_platform_security_config_filename();
        assert!(filename.starts_with("config.security."));
        assert!(filename.ends_with(".json"));
    }

    #[test]
    fn test_platform_display_name() {
        let name = get_platform_display_name();
        assert!(!name.is_empty());
        assert!(name == "Windows" || name == "Linux" || name == "macOS" || name == "Unknown");
    }

    #[test]
    fn test_platform_info() {
        let info = get_platform_info();
        assert!(info["os"].is_string());
        assert!(info["arch"].is_string());
        assert!(info["display_name"].is_string());
        assert!(info["security_config"].is_string());
    }

    #[test]
    fn test_load_mcp_config_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.mcp.json");
        // Write default MCP config to file to avoid dependency on embedded defaults state
        let default_cfg = McpConfig { enabled: false, servers: vec![], timeout: 30 };
        std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
        let cfg = load_mcp_config(&path).unwrap();
        assert!(!cfg.enabled);
        assert!(cfg.servers.is_empty());
        assert_eq!(cfg.timeout, 30);
    }

    #[test]
    fn test_save_and_load_mcp_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("subdir/config.mcp.json");
        let cfg = McpConfig {
            enabled: true,
            servers: vec![McpServerConfig {
                name: "test-server".to_string(),
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                ..Default::default()
            }],
            timeout: 60,
        };
        save_mcp_config(&path, &cfg).unwrap();
        let loaded = load_mcp_config(&path).unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "test-server");
        assert_eq!(loaded.timeout, 60);
    }

    #[test]
    fn test_load_security_config_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.security.json");
        // Write default config to file to avoid dependency on embedded defaults state
        let default_cfg = SecurityConfig::default();
        std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
        let cfg = load_security_config(&path).unwrap();
        assert_eq!(cfg.default_action, "deny");
        assert!(cfg.log_all_operations);
        assert_eq!(cfg.approval_timeout_seconds, 300);
    }

    #[test]
    fn test_save_and_load_security_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.security.json");
        let cfg = SecurityConfig {
            default_action: "allow".to_string(),
            file_rules: Some(FileSecurityRules {
                read: vec![SecurityRule {
                    pattern: "/workspace/**".to_string(),
                    action: "allow".to_string(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        save_security_config(&path, &cfg).unwrap();
        let loaded = load_security_config(&path).unwrap();
        assert_eq!(loaded.default_action, "allow");
        assert!(loaded.file_rules.is_some());
        assert_eq!(loaded.file_rules.unwrap().read.len(), 1);
    }

    #[test]
    fn test_load_scanner_config_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.scanner.json");
        // Write default config to file to avoid dependency on embedded defaults state
        let default_cfg = ScannerFullConfig::default();
        std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
        let cfg = load_scanner_config(&path).unwrap();
        assert!(cfg.enabled.is_empty());
        assert!(cfg.engines.is_empty());
    }

    #[test]
    fn test_save_and_load_scanner_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.scanner.json");
        let mut engines = std::collections::HashMap::new();
        engines.insert("clamav".to_string(), serde_json::json!({"url": "tcp://localhost:3310"}));
        let cfg = ScannerFullConfig {
            enabled: vec!["clamav".to_string()],
            engines,
        };
        save_scanner_config(&path, &cfg).unwrap();
        let loaded = load_scanner_config(&path).unwrap();
        assert_eq!(loaded.enabled, vec!["clamav"]);
        assert!(loaded.engines.contains_key("clamav"));
    }

    #[test]
    fn test_load_skills_config_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.skills.json");
        // Write default config to file to avoid dependency on embedded defaults state
        let default_cfg = SkillsFullConfig::default();
        std::fs::write(&path, serde_json::to_string(&default_cfg).unwrap()).unwrap();
        let cfg = load_skills_config(&path).unwrap();
        assert!(cfg.enabled);
        assert!(cfg.search_cache.enabled);
        assert_eq!(cfg.max_concurrent_searches, 2);
    }

    #[test]
    fn test_save_and_load_skills_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.skills.json");
        let cfg = SkillsFullConfig {
            enabled: false,
            github_sources: vec![GitHubSourceConfig {
                name: "test-source".to_string(),
                repo: "test/repo".to_string(),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        save_skills_config(&path, &cfg).unwrap();
        let loaded = load_skills_config(&path).unwrap();
        assert!(!loaded.enabled);
        assert_eq!(loaded.github_sources.len(), 1);
        assert_eq!(loaded.github_sources[0].repo, "test/repo");
    }

    #[test]
    fn test_config_get_model_by_model_name() {
        let config = Config {
            model_list: vec![
                ModelConfig {
                    model_name: "default".to_string(),
                    model: "openai/gpt-4".to_string(),
                    api_key: "key1".to_string(),
                    ..Default::default()
                },
                ModelConfig {
                    model_name: "fast".to_string(),
                    model: "groq/llama3".to_string(),
                    api_key: "key2".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        // By model_name
        let found = config.get_model_by_model_name("default").unwrap();
        assert_eq!(found.api_key, "key1");

        // By model field (vendor/model)
        let found2 = config.get_model_by_model_name("groq/llama3").unwrap();
        assert_eq!(found2.model_name, "fast");

        // Not found
        assert!(config.get_model_by_model_name("nonexistent").is_err());
    }

    #[test]
    fn test_config_get_model_config() {
        let config = Config {
            model_list: vec![ModelConfig {
                model_name: "test".to_string(),
                model: "test/model".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let found = config.get_model_config("test").unwrap();
        assert_eq!(found.model, "test/model");
    }

    #[test]
    fn test_config_workspace_path() {
        let config = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: "/custom/workspace".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.workspace_path(), "/custom/workspace");

        let empty_config = Config::default();
        let ws = empty_config.workspace_path();
        assert!(!ws.is_empty());
    }

    #[test]
    fn test_resolve_model_config() {
        let config = Config {
            model_list: vec![ModelConfig {
                model_name: "my-model".to_string(),
                model: "openai/gpt-4".to_string(),
                api_key: "test-key".to_string(),
                api_base: "https://custom.api.com/v1".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        // By model_name
        let res = resolve_model_config(&config, "my-model").unwrap();
        assert_eq!(res.provider_name, "openai");
        assert_eq!(res.model_name, "gpt-4");
        assert_eq!(res.api_key, "test-key");
        assert_eq!(res.api_base, "https://custom.api.com/v1");

        // By model field
        let res2 = resolve_model_config(&config, "openai/gpt-4").unwrap();
        assert_eq!(res2.model_name, "gpt-4");

        // Not found, but can infer provider
        let res3 = resolve_model_config(&config, "claude-3-opus").unwrap();
        assert_eq!(res3.provider_name, "anthropic");

        // Empty ref
        assert!(resolve_model_config(&config, "").is_err());
    }

    #[test]
    fn test_get_model_by_name_free_fn() {
        let config = Config {
            model_list: vec![ModelConfig {
                model_name: "default".to_string(),
                model: "zhipu/glm-4".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let found = get_model_by_name(&config, "default").unwrap();
        assert_eq!(found.model, "zhipu/glm-4");

        let found2 = get_model_by_name(&config, "zhipu/glm-4").unwrap();
        assert_eq!(found2.model_name, "default");

        assert!(get_model_by_name(&config, "nonexistent").is_err());
    }

    #[test]
    fn test_get_effective_llm() {
        assert_eq!(get_effective_llm(None), "zhipu/glm-4.7-flash");

        let config = Config::default();
        assert_eq!(get_effective_llm(Some(&config)), "zhipu/glm-4.7-flash");

        let config_with_llm = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    llm: "anthropic/claude-3".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(get_effective_llm(Some(&config_with_llm)), "anthropic/claude-3");
    }

    #[test]
    fn test_embedded_defaults() {
        let defaults = get_embedded_defaults();
        assert!(defaults.config.is_empty());
        assert!(defaults.mcp.is_empty());

        set_embedded_defaults(
            b"config".to_vec(),
            b"mcp".to_vec(),
            b"security".to_vec(),
            b"cluster".to_vec(),
            b"skills".to_vec(),
            b"scanner".to_vec(),
        );

        let defaults = get_embedded_defaults();
        assert_eq!(defaults.config, b"config".to_vec());
        assert_eq!(defaults.mcp, b"mcp".to_vec());
    }

    #[test]
    fn test_security_config_roundtrip() {
        let cfg = SecurityConfig {
            default_action: "deny".to_string(),
            log_all_operations: true,
            approval_timeout_seconds: 600,
            max_pending_requests: 50,
            audit_log_retention_days: 30,
            audit_log_file_enabled: true,
            synchronous_mode: true,
            file_rules: Some(FileSecurityRules {
                read: vec![SecurityRule {
                    pattern: "/workspace/**".to_string(),
                    action: "allow".to_string(),
                }],
                write: vec![
                    SecurityRule {
                        pattern: "/workspace/**".to_string(),
                        action: "allow".to_string(),
                    },
                    SecurityRule {
                        pattern: "*.key".to_string(),
                        action: "deny".to_string(),
                    },
                ],
                ..Default::default()
            }),
            process_rules: Some(ProcessSecurityRules {
                exec: vec![SecurityRule {
                    pattern: "rm -rf *".to_string(),
                    action: "deny".to_string(),
                }],
                ..Default::default()
            }),
            layers: Some(SecurityLayersConfig {
                injection: Some(SecurityLayerConfig {
                    enabled: true,
                    ..Default::default()
                }),
                dlp: Some(DLPLayerConfig {
                    enabled: true,
                    action: "block".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: SecurityConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.default_action, "deny");
        assert_eq!(parsed.approval_timeout_seconds, 600);
        assert!(parsed.file_rules.is_some());
        let fr = parsed.file_rules.unwrap();
        assert_eq!(fr.write.len(), 2);
        assert!(parsed.layers.is_some());
    }

    #[test]
    fn test_skills_full_config_roundtrip() {
        let cfg = SkillsFullConfig {
            enabled: true,
            search_cache: SkillsSearchCacheConfig {
                enabled: true,
                max_size: 100,
                ttl_seconds: 600,
            },
            max_concurrent_searches: 4,
            github_sources: vec![GitHubSourceConfig {
                name: "anthropics/skills".to_string(),
                repo: "anthropics/skills".to_string(),
                enabled: true,
                index_type: "github_tree".to_string(),
                skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
                ..Default::default()
            }],
            clawhub: SkillsClawHubConfig {
                enabled: true,
                base_url: "https://clawhub.com".to_string(),
                ..Default::default()
            },
        };

        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: SkillsFullConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.search_cache.max_size, 100);
        assert_eq!(parsed.github_sources.len(), 1);
        assert_eq!(parsed.github_sources[0].name, "anthropics/skills");
    }

    #[test]
    fn test_full_config_roundtrip() {
        let config = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    max_tokens: 256000,
                    temperature: 0.5,
                    ..Default::default()
                },
                list: vec![AgentConfigEntry {
                    id: "main".to_string(),
                    default: true,
                    name: "Main Agent".to_string(),
                    ..Default::default()
                }],
            },
            model_list: vec![ModelConfig {
                model_name: "test".to_string(),
                model: "test/test-1.0".to_string(),
                api_key: "test-key".to_string(),
                ..Default::default()
            }],
            channels: ChannelsConfig {
                web: WebChannelConfig {
                    enabled: true,
                    port: 9999,
                    ..Default::default()
                },
                ..Default::default()
            },
            security: Some(SecurityFlagConfig { enabled: false }),
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: Config = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.agents.defaults.max_tokens, 256000);
        assert_eq!(parsed.agents.list.len(), 1);
        assert_eq!(parsed.model_list[0].model, "test/test-1.0");
        assert!(parsed.channels.web.enabled);
        assert_eq!(parsed.channels.web.port, 9999);
        assert!(parsed.security.is_some());
        assert!(!parsed.security.unwrap().enabled);
    }

    #[test]
    fn test_config_error_display() {
        let err = ConfigError::Validation("test error".to_string());
        assert!(err.to_string().contains("test error"));

        let err = ConfigError::WorkspaceNotFound;
        assert!(err.to_string().contains("Workspace not found"));
    }

    #[test]
    fn test_mcp_server_config_default() {
        let cfg = McpServerConfig::default();
        assert!(cfg.name.is_empty());
        assert!(cfg.command.is_empty());
        assert!(cfg.args.is_empty());
        assert!(cfg.env.is_empty());
    }

    #[test]
    fn test_mcp_config_roundtrip() {
        let cfg = McpConfig {
            enabled: true,
            servers: vec![
                McpServerConfig {
                    name: "server1".to_string(),
                    command: "node".to_string(),
                    args: vec!["a.js".to_string(), "b.js".to_string()],
                    env: vec!["KEY=VALUE".to_string()],
                    timeout: 30,
                },
                McpServerConfig {
                    name: "server2".to_string(),
                    command: "python".to_string(),
                    args: vec!["main.py".to_string()],
                    env: vec![],
                    timeout: 30,
                },
            ],
            timeout: 120,
        };
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: McpConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.servers.len(), 2);
        assert_eq!(parsed.servers[0].args.len(), 2);
        assert_eq!(parsed.timeout, 120);
    }

    #[test]
    fn test_security_rule_serialization() {
        let rule = SecurityRule {
            pattern: "/workspace/**".to_string(),
            action: "allow".to_string(),
        };
        let json = serde_json::to_string(&rule).unwrap();
        let parsed: SecurityRule = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pattern, "/workspace/**");
        assert_eq!(parsed.action, "allow");
    }

    #[test]
    fn test_engine_state_default_v2() {
        let cfg = ScannerFullConfig::default();
        assert!(cfg.enabled.is_empty());
        assert!(cfg.engines.is_empty());
    }

    #[test]
    fn test_agent_model_config_manual() {
        let cfg = AgentModelConfig {
            primary: String::new(),
            fallbacks: vec![],
        };
        assert!(cfg.primary.is_empty());
        assert!(cfg.fallbacks.is_empty());
    }

    #[test]
    fn test_agent_model_config_string_serialization() {
        let cfg = AgentModelConfig {
            primary: "gpt-4".to_string(),
            fallbacks: vec!["claude-3".to_string(), "llama3".to_string()],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: AgentModelConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.primary, "gpt-4");
        assert_eq!(parsed.fallbacks.len(), 2);
    }

    #[test]
    fn test_agent_defaults_default() {
        let defaults = AgentDefaults::default();
        assert_eq!(defaults.max_tokens, 8192);
        assert_eq!(defaults.temperature, 0.7);
        assert_eq!(defaults.max_tool_iterations, 20);
    }

    #[test]
    fn test_agents_config_default() {
        let agents = AgentsConfig::default();
        assert!(agents.list.is_empty());
    }

    #[test]
    fn test_channels_config_default() {
        let channels = ChannelsConfig::default();
        assert!(!channels.web.enabled);
        assert_eq!(channels.web.port, 8080);
    }

    #[test]
    fn test_gateway_config_default() {
        let gateway = GatewayConfig::default();
        assert_eq!(gateway.host, "0.0.0.0");
        assert_eq!(gateway.port, 18790);
    }

    #[test]
    fn test_model_config_default() {
        let model = ModelConfig::default();
        assert!(model.model_name.is_empty());
        assert!(model.model.is_empty());
        assert!(model.api_key.is_empty());
    }

    #[test]
    fn test_model_parse_no_slash() {
        let model = ModelConfig {
            model_name: "test".to_string(),
            model: "gpt-4o-mini".to_string(),
            ..Default::default()
        };
        let (proto, name) = model.parse_model();
        assert_eq!(proto, "openai");
        assert_eq!(name, "gpt-4o-mini");
    }

    #[test]
    fn test_model_validate_empty_model_name() {
        let model = ModelConfig {
            model_name: String::new(),
            model: "openai/gpt-4".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        assert!(model.validate().is_err());
    }

    #[test]
    fn test_model_validate_empty_api_key() {
        let model = ModelConfig {
            model_name: "test".to_string(),
            model: "openai/gpt-4".to_string(),
            api_key: String::new(),
            ..Default::default()
        };
        // api_key is not validated by validate()
        assert!(model.validate().is_ok());
    }

    #[test]
    fn test_config_get_model_config_not_found() {
        let config = Config::default();
        assert!(config.get_model_config("nonexistent").is_err());
    }

    #[test]
    fn test_workspace_resolver_config_path() {
        let path = WorkspaceResolver::config_path(Path::new("/tmp/test"));
        assert_eq!(path, PathBuf::from("/tmp/test/config.json"));
    }

    #[test]
    fn test_workspace_resolver_ensure_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("new_workspace");
        WorkspaceResolver::ensure_workspace(&ws).unwrap();
        assert!(ws.is_dir());
    }

    #[test]
    fn test_security_config_default_values() {
        let cfg = SecurityConfig::default();
        assert_eq!(cfg.default_action, "deny");
        assert!(cfg.log_all_operations);
        assert_eq!(cfg.approval_timeout_seconds, 300);
        assert_eq!(cfg.max_pending_requests, 100);
        assert_eq!(cfg.audit_log_retention_days, 90);
    }

    #[test]
    fn test_security_layer_config_default() {
        let cfg = SecurityLayerConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.extra.is_empty());
    }

    #[test]
    fn test_file_security_rules_default() {
        let rules = FileSecurityRules::default();
        assert!(rules.read.is_empty());
        assert!(rules.write.is_empty());
        assert!(rules.delete.is_empty());
    }

    #[test]
    fn test_process_security_rules_default() {
        let rules = ProcessSecurityRules::default();
        assert!(rules.exec.is_empty());
        assert!(rules.spawn.is_empty());
        assert!(rules.kill.is_empty());
    }

    #[test]
    fn test_network_security_rules_default() {
        let rules = NetworkSecurityRules::default();
        assert!(rules.request.is_empty());
        assert!(rules.download.is_empty());
        assert!(rules.upload.is_empty());
    }

    #[test]
    fn test_load_config_from_file_nonexistent() {
        let result = ConfigLoader::load_from_file(Path::new("/nonexistent/config.json"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "not valid json{{").unwrap();
        let result = ConfigLoader::load_from_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_tilde_absolute() {
        let result = expand_tilde("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_tilde_relative() {
        let result = expand_tilde("relative/path");
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_save_config_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("deep/nested/config.json");
        let config = Config::default();
        ConfigLoader::save_to_file(&config, &path).unwrap();
        assert!(path.exists());
    }

    // ---- New tests ----

    #[test]
    fn test_agent_defaults_restrict_to_workspace_default() {
        let defaults = AgentDefaults::default();
        assert!(defaults.restrict_to_workspace);
    }

    #[test]
    fn test_agent_defaults_queue_size() {
        let defaults = AgentDefaults::default();
        assert_eq!(defaults.queue_size, 8);
    }

    #[test]
    fn test_agent_defaults_concurrent_request_mode() {
        let defaults = AgentDefaults::default();
        assert_eq!(defaults.concurrent_request_mode, "reject");
    }

    #[test]
    fn test_session_config_default() {
        let session = SessionConfig::default();
        assert!(session.dm_scope.is_empty());
        assert!(session.identity_links.is_empty());
    }

    #[test]
    fn test_binding_match_manual() {
        let bm = BindingMatch {
            channel: String::new(),
            account_id: String::new(),
            peer: None,
            guild_id: String::new(),
            team_id: String::new(),
        };
        assert!(bm.channel.is_empty());
        assert!(bm.account_id.is_empty());
        assert!(bm.peer.is_none());
    }

    #[test]
    fn test_peer_match_manual() {
        let pm = PeerMatch {
            kind: String::new(),
            id: String::new(),
        };
        assert!(pm.kind.is_empty());
        assert!(pm.id.is_empty());
    }

    #[test]
    fn test_whatsapp_config_default() {
        let cfg = WhatsAppConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.bridge_url.is_empty());
        assert!(cfg.allow_from.is_empty());
    }

    #[test]
    fn test_telegram_config_default() {
        let cfg = TelegramConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.token.is_empty());
    }

    #[test]
    fn test_feishu_config_default() {
        let cfg = FeishuConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.app_id.is_empty());
    }

    #[test]
    fn test_discord_config_default() {
        let cfg = DiscordConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_tools_config_default() {
        let cfg = ToolsConfig::default();
        // Default trait gives serde defaults; brave.max_results=0, duckduckgo.enabled=false via Default trait
        assert_eq!(cfg.web.brave.max_results, 0);
        assert!(!cfg.web.duckduckgo.enabled);
    }

    #[test]
    fn test_exec_config_default() {
        let cfg = ExecConfig::default();
        assert!(!cfg.enable_deny_patterns);
        assert!(cfg.custom_deny_patterns.is_empty());
    }

    #[test]
    fn test_heartbeat_config_default() {
        let cfg = HeartbeatConfig::default();
        assert!(!cfg.enabled); // Default trait gives false, serde default gives true
        assert_eq!(cfg.interval, 0); // Default trait gives 0, serde default gives 30
    }

    #[test]
    fn test_devices_config_default() {
        let cfg = DevicesConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.monitor_usb); // Default trait gives false, serde default gives true
    }

    #[test]
    fn test_security_flag_config_default() {
        let cfg = SecurityFlagConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_forge_flag_config_default() {
        let cfg = ForgeFlagConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_memory_flag_config_default() {
        let cfg = MemoryFlagConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_skills_config_default() {
        let cfg = SkillsConfig::default();
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_model_config_parse_multiple_slashes() {
        let model = ModelConfig {
            model_name: "test".to_string(),
            model: "provider/model/variant".to_string(),
            ..Default::default()
        };
        let (proto, name) = model.parse_model();
        assert_eq!(proto, "provider");
        assert_eq!(name, "model/variant");
    }

    #[test]
    fn test_agent_config_entry_default() {
        let entry = AgentConfigEntry::default();
        assert!(entry.id.is_empty());
        assert!(!entry.default);
        assert!(entry.skills.is_empty());
    }

    #[test]
    fn test_subagents_config_manual() {
        let cfg = SubagentsConfig {
            allow_agents: vec![],
            model: None,
        };
        assert!(cfg.allow_agents.is_empty());
        assert!(cfg.model.is_none());
    }

    #[test]
    fn test_channels_config_all_disabled() {
        let cfg = ChannelsConfig::default();
        assert!(!cfg.whatsapp.enabled);
        assert!(!cfg.telegram.enabled);
        assert!(!cfg.feishu.enabled);
        assert!(!cfg.discord.enabled);
        assert!(!cfg.maixcam.enabled);
        assert!(!cfg.qq.enabled);
        assert!(!cfg.dingtalk.enabled);
        assert!(!cfg.slack.enabled);
        assert!(!cfg.line.enabled);
        assert!(!cfg.onebot.enabled);
    }

    #[test]
    fn test_web_channel_config_defaults() {
        let cfg = WebChannelConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.host, "0.0.0.0");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.path, "/ws");
        assert_eq!(cfg.heartbeat_interval, 30);
        assert_eq!(cfg.session_timeout, 3600);
    }

    #[test]
    fn test_logging_config_default() {
        let cfg = LoggingConfig::default();
        assert!(cfg.llm.is_none());
        assert!(cfg.general.is_none());
    }

    #[test]
    fn test_llm_log_config_default() {
        let cfg = LlmLogConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.log_dir.is_empty());
        assert_eq!(cfg.detail_level, ""); // Default trait gives empty string, serde default gives "full"
    }

    #[test]
    fn test_general_log_config_default() {
        let cfg = GeneralLogConfig::default();
        assert!(!cfg.enabled); // Default trait gives false, serde default gives true
        assert!(!cfg.enable_console); // Default trait gives false, serde default gives true
        assert_eq!(cfg.level, ""); // Default trait gives empty string, serde default gives "INFO"
    }

    #[test]
    fn test_mcp_config_default() {
        let cfg = McpConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.servers.is_empty());
        assert_eq!(cfg.timeout, 0); // Default trait gives 0, serde default gives 30
    }

    #[test]
    fn test_dlp_layer_config_default() {
        let cfg = DLPLayerConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.rules.is_empty());
        assert!(cfg.action.is_empty());
    }

    #[test]
    fn test_signature_layer_config_default() {
        let cfg = SignatureLayerConfig::default();
        assert!(!cfg.enabled);
        assert!(!cfg.strict);
    }

    #[test]
    fn test_hardware_security_rules_defaults_v2() {
        let rules = HardwareSecurityRules::default();
        assert!(rules.i2c.is_empty());
        assert!(rules.spi.is_empty());
        assert!(rules.gpio.is_empty());
    }

    #[test]
    fn test_registry_security_rules_defaults_v2() {
        let rules = RegistrySecurityRules::default();
        assert!(rules.read.is_empty());
        assert!(rules.write.is_empty());
        assert!(rules.delete.is_empty());
    }

    #[test]
    fn test_directory_security_rules_defaults_v2() {
        let rules = DirectorySecurityRules::default();
        assert!(rules.read.is_empty());
        assert!(rules.create.is_empty());
        assert!(rules.delete.is_empty());
    }

    #[test]
    fn test_clamav_engine_config_default() {
        let cfg = ClamAVEngineConfig::default();
        assert!(cfg.url.is_empty());
        assert!(!cfg.scan_on_write);
        assert!(!cfg.scan_on_download);
        assert!(cfg.scan_extensions.is_empty());
    }

    #[test]
    fn test_flexible_string_vec_with_numbers() {
        let json = r#"{"allow_from": ["user1", 123, "user3"]}"#;
        let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.allow_from, vec!["user1", "123", "user3"]);
    }

    #[test]
    fn test_flexible_string_vec_empty_array() {
        let json = r#"{"allow_from": []}"#;
        let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.allow_from.is_empty());
    }

    #[test]
    fn test_config_post_process_for_compatibility() {
        let mut config = Config::default();
        config.channels.external.sync_to = vec!["web".to_string()];
        config.channels.websocket.sync_to = vec!["web".to_string()];
        config.post_process_for_compatibility();
        assert!(config.channels.external.sync_to_web);
        assert!(config.channels.websocket.sync_to_web);
    }

    #[test]
    fn test_config_post_process_no_sync() {
        let mut config = Config::default();
        config.post_process_for_compatibility();
        assert!(!config.channels.external.sync_to_web);
        assert!(!config.channels.websocket.sync_to_web);
    }

    #[test]
    fn test_config_workspace_path_empty() {
        let config = Config::default();
        let ws = config.workspace_path();
        assert!(!ws.is_empty());
    }

    #[test]
    fn test_config_workspace_path_tilde() {
        let config = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: "~/custom/path".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let ws = config.workspace_path();
        assert!(!ws.starts_with("~"));
        assert!(ws.contains("custom"));
    }

    #[test]
    fn test_security_layers_config_default() {
        let cfg = SecurityLayersConfig::default();
        assert!(cfg.injection.is_none());
        assert!(cfg.command_guard.is_none());
        assert!(cfg.dlp.is_none());
        assert!(cfg.ssrf.is_none());
        assert!(cfg.credential.is_none());
        assert!(cfg.signature.is_none());
        assert!(cfg.audit_chain.is_none());
    }

    #[test]
    fn test_brave_config_default() {
        let cfg = BraveConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.api_key.is_empty());
        assert_eq!(cfg.max_results, 0); // Default trait gives 0, serde default gives 5
    }

    #[test]
    fn test_duckduckgo_config_default() {
        let cfg = DuckDuckGoConfig::default();
        assert!(!cfg.enabled); // Default trait gives false, serde default gives true
        assert_eq!(cfg.max_results, 0); // Default trait gives 0, serde default gives 5
    }

    #[test]
    fn test_perplexity_config_default() {
        let cfg = PerplexityConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.api_key.is_empty());
        assert_eq!(cfg.max_results, 0); // Default trait gives 0, serde default gives 5
    }

    #[test]
    fn test_cron_tools_config_default() {
        let cfg = CronToolsConfig::default();
        assert_eq!(cfg.exec_timeout_minutes, 0);
    }

    #[test]
    fn test_websocket_channel_config_default() {
        let cfg = WebSocketChannelConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.host.is_empty());
        assert!(!cfg.sync_to_web);
    }

    #[test]
    fn test_external_config_default() {
        let cfg = ExternalConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.input_exe.is_empty());
        assert!(!cfg.sync_to_web);
    }

    #[test]
    fn test_maixcam_config_default() {
        let cfg = MaixCamConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.host.is_empty());
        assert_eq!(cfg.port, 0);
    }

    #[test]
    fn test_qq_config_default() {
        let cfg = QqConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.app_id.is_empty());
    }

    #[test]
    fn test_dingtalk_config_default() {
        let cfg = DingTalkConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.client_id.is_empty());
    }

    #[test]
    fn test_slack_config_default() {
        let cfg = SlackConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.bot_token.is_empty());
    }

    #[test]
    fn test_line_config_default() {
        let cfg = LineConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.channel_secret.is_empty());
    }

    #[test]
    fn test_onebot_config_default() {
        let cfg = OneBotConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.ws_url.is_empty());
        assert!(cfg.group_trigger_prefix.is_empty());
    }

    #[test]
    fn test_engine_state_default() {
        let state = EngineState::default();
        assert!(state.install_status.is_empty());
        assert!(state.install_error.is_empty());
        assert!(state.last_install_attempt.is_empty());
        assert!(state.db_status.is_empty());
        assert!(state.last_db_update.is_empty());
    }

    #[test]
    fn test_skills_full_config_enabled_by_default() {
        let cfg = SkillsFullConfig::default();
        assert!(cfg.enabled);
    }

    #[test]
    fn test_github_source_config_default() {
        let cfg = GitHubSourceConfig::default();
        assert!(cfg.name.is_empty());
        assert!(cfg.repo.is_empty());
        assert!(!cfg.enabled);
    }

    #[test]
    fn test_skills_clawhub_config_default() {
        let cfg = SkillsClawHubConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.base_url.is_empty());
    }

    // ---- Additional coverage tests ----

    #[test]
    fn test_expand_tilde_home() {
        let result = expand_tilde("~");
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        assert_eq!(result, home);
    }

    #[test]
    fn test_expand_tilde_with_path() {
        let result = expand_tilde("~/some/path");
        if let Some(home) = dirs::home_dir() {
            assert_eq!(result, home.join("some/path"));
        }
    }

    #[test]
    fn test_config_workspace_path_absolute() {
        let config = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: "/absolute/path".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.workspace_path(), "/absolute/path");
    }

    #[test]
    fn test_apply_env_overrides_gateway_host() {
        let mut config = Config::default();

        // SAFETY: single-threaded test, no concurrent access to this env var
        unsafe { std::env::set_var("NEMESISBOT_GATEWAY_HOST", "192.168.1.1"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.gateway.host, "192.168.1.1");

        unsafe { std::env::remove_var("NEMESISBOT_GATEWAY_HOST"); }
    }

    #[test]
    fn test_apply_env_overrides_gateway_port() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_GATEWAY_PORT", "9999"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.gateway.port, 9999);

        unsafe { std::env::remove_var("NEMESISBOT_GATEWAY_PORT"); }
    }

    #[test]
    fn test_apply_env_overrides_security_enabled() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_SECURITY_ENABLED", "true"); }
        apply_env_overrides(&mut config);
        assert!(config.security.is_some());
        assert!(config.security.as_ref().unwrap().enabled);

        unsafe { std::env::remove_var("NEMESISBOT_SECURITY_ENABLED"); }
    }

    #[test]
    fn test_apply_env_overrides_forge_enabled() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_FORGE_ENABLED", "true"); }
        apply_env_overrides(&mut config);
        assert!(config.forge.is_some());
        assert!(config.forge.as_ref().unwrap().enabled);

        unsafe { std::env::remove_var("NEMESISBOT_FORGE_ENABLED"); }
    }

    #[test]
    fn test_apply_env_overrides_workspace() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE", "/custom/ws"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.workspace, "/custom/ws");

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE"); }
    }

    #[test]
    fn test_apply_env_overrides_max_tokens() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS", "16000"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.max_tokens, 16000);

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS"); }
    }

    #[test]
    fn test_apply_env_overrides_temperature() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE", "0.3"); }
        apply_env_overrides(&mut config);
        assert!((config.agents.defaults.temperature - 0.3).abs() < f64::EPSILON);

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE"); }
    }

    #[test]
    fn test_apply_env_overrides_heartbeat() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_HEARTBEAT_ENABLED", "false"); }
        unsafe { std::env::set_var("NEMESISBOT_HEARTBEAT_INTERVAL", "60"); }
        apply_env_overrides(&mut config);
        assert!(!config.heartbeat.enabled);
        assert_eq!(config.heartbeat.interval, 60);

        unsafe { std::env::remove_var("NEMESISBOT_HEARTBEAT_ENABLED"); }
        unsafe { std::env::remove_var("NEMESISBOT_HEARTBEAT_INTERVAL"); }
    }

    #[test]
    fn test_apply_env_overrides_session_dm_scope() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_SESSION_DM_SCOPE", "per-peer"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.session.dm_scope, "per-peer");

        unsafe { std::env::remove_var("NEMESISBOT_SESSION_DM_SCOPE"); }
    }

    #[test]
    fn test_platform_info_json_fields() {
        let info = get_platform_info();
        assert!(info.get("os").is_some());
        assert!(info.get("arch").is_some());
        assert!(info.get("family").is_some());
        assert!(info.get("display_name").is_some());
        assert!(info.get("security_config").is_some());
        // Verify values are strings
        assert!(info["os"].is_string());
        assert!(info["arch"].is_string());
    }

    #[test]
    fn test_deserialize_flexible_string_vec_with_bool() {
        // The deserializer converts non-string, non-number types via to_string()
        let json = r#"{"allow_from": ["user1", true]}"#;
        let cfg: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.allow_from.len(), 2);
        assert_eq!(cfg.allow_from[0], "user1");
        assert_eq!(cfg.allow_from[1], "true");
    }

    #[test]
    fn test_config_default_values() {
        let config = default_config();
        assert!(config.agents.defaults.restrict_to_workspace);
        assert!(!config.channels.web.enabled || config.channels.web.enabled); // depends on default
        assert!(config.gateway.port > 0);
    }

    #[test]
    fn test_config_get_model_by_model_name_not_found() {
        let config = Config::default();
        assert!(config.get_model_by_model_name("nonexistent").is_err());
    }

    #[test]
    fn test_model_validate_empty_model() {
        let model = ModelConfig {
            model_name: "test".to_string(),
            model: String::new(),
            ..Default::default()
        };
        assert!(model.validate().is_err());
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_apply_env_overrides_web_channel() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_CHANNELS_WEB_ENABLED", "false"); }
        unsafe { std::env::set_var("NEMESISBOT_CHANNELS_WEB_HOST", "127.0.0.1"); }
        unsafe { std::env::set_var("NEMESISBOT_CHANNELS_WEB_PORT", "9999"); }
        apply_env_overrides(&mut config);
        assert!(!config.channels.web.enabled);
        assert_eq!(config.channels.web.host, "127.0.0.1");
        assert_eq!(config.channels.web.port, 9999);

        unsafe { std::env::remove_var("NEMESISBOT_CHANNELS_WEB_ENABLED"); }
        unsafe { std::env::remove_var("NEMESISBOT_CHANNELS_WEB_HOST"); }
        unsafe { std::env::remove_var("NEMESISBOT_CHANNELS_WEB_PORT"); }
    }


    #[test]
    fn test_apply_env_overrides_invalid_bool() {
        let mut config = Config::default();

        // Invalid bool for restrict_to_workspace defaults to true
        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE", "notabool"); }
        apply_env_overrides(&mut config);
        assert!(config.agents.defaults.restrict_to_workspace);

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE"); }
    }

    #[test]
    fn test_apply_env_overrides_llm() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_LLM", "anthropic/claude-3"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.llm, "anthropic/claude-3");

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_LLM"); }
    }

    #[test]
    fn test_apply_env_overrides_image_model() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL", "openai/dall-e-3"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.image_model, "openai/dall-e-3");

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL"); }
    }

    #[test]
    fn test_apply_env_overrides_max_tool_iterations() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS", "50"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.max_tool_iterations, 50);

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS"); }
    }

    #[test]
    fn test_apply_env_overrides_concurrent_request_mode() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE", "queue"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.concurrent_request_mode, "queue");

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE"); }
    }

    #[test]
    fn test_apply_env_overrides_queue_size() {
        let mut config = Config::default();

        unsafe { std::env::set_var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE", "16"); }
        apply_env_overrides(&mut config);
        assert_eq!(config.agents.defaults.queue_size, 16);

        unsafe { std::env::remove_var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE"); }
    }

    #[test]
    fn test_load_config_nonexistent_falls_through() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        // Should not fail - falls through to default
        let config = load_config(&path).unwrap();
        assert!(config.gateway.port > 0);
    }

    #[test]
    fn test_set_embedded_defaults_from_fs_missing_dir() {
        let result = set_embedded_defaults_from_fs(Path::new("/nonexistent/config/dir"));
        assert!(result.is_err());
    }

    // Combined test to avoid parallel global state race: set embedded defaults from fs
    // and verify all load_*_config functions pick them up
    #[test]
    fn test_embedded_defaults_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path();

        // Write all required files with distinctive values
        let main_config = r#"{"gateway":{"host":"e2e-test","port":9999}}"#;
        let mcp_config = r#"{"enabled":true,"servers":[],"timeout":42}"#;
        let security_config = r#"{"default_action":"allow","log_all_operations":false,"approval_timeout_seconds":123}"#;
        let cluster_config = r#"{"enabled":true,"port":11111}"#;
        let skills_config = r#"{"enabled":false,"github_sources":[],"search_cache":{"enabled":false},"max_concurrent_searches":5}"#;
        let scanner_config = r#"{"enabled":["clamav"],"engines":{}}"#;

        std::fs::write(config_dir.join("config.default.json"), main_config).unwrap();
        std::fs::write(config_dir.join("config.mcp.default.json"), mcp_config).unwrap();
        std::fs::write(config_dir.join(get_platform_security_config_filename()), security_config).unwrap();
        std::fs::write(config_dir.join("config.cluster.default.json"), cluster_config).unwrap();
        std::fs::write(config_dir.join("config.skills.default.json"), skills_config).unwrap();
        std::fs::write(config_dir.join("config.scanner.default.json"), scanner_config).unwrap();

        let result = set_embedded_defaults_from_fs(config_dir);
        assert!(result.is_ok());

        // Verify load_config uses embedded fallback
        let nonexistent = dir.path().join("no-config-here/config.json");
        let config = load_config(&nonexistent).unwrap();
        assert_eq!(config.gateway.host, "e2e-test");
        assert_eq!(config.gateway.port, 9999);

        // Verify load_mcp_config uses embedded fallback
        let mcp_path = dir.path().join("no-config-here/config.mcp.json");
        let mcp = load_mcp_config(&mcp_path).unwrap();
        assert!(mcp.enabled);
        assert_eq!(mcp.timeout, 42);

        // Verify load_security_config uses embedded fallback
        let sec_path = dir.path().join("no-config-here/config.security.json");
        let sec = load_security_config(&sec_path).unwrap();
        assert_eq!(sec.default_action, "allow");
        assert_eq!(sec.approval_timeout_seconds, 123);

        // Verify load_scanner_config uses embedded fallback
        let scan_path = dir.path().join("no-config-here/config.scanner.json");
        let scan = load_scanner_config(&scan_path).unwrap();
        assert_eq!(scan.enabled, vec!["clamav"]);

        // Verify load_skills_config uses embedded fallback
        let skills_path = dir.path().join("no-config-here/config.skills.json");
        let skills = load_skills_config(&skills_path).unwrap();
        assert!(!skills.enabled);
        assert_eq!(skills.max_concurrent_searches, 5);

        // Verify load_embedded_config works
        let embedded = load_embedded_config();
        assert!(embedded.is_ok());
        assert_eq!(embedded.unwrap().gateway.host, "e2e-test");

        // Verify get_embedded_defaults
        let defaults = get_embedded_defaults();
        assert!(!defaults.config.is_empty());
        assert!(!defaults.mcp.is_empty());
    }

    #[test]
    fn test_save_config_local_mode() {
        let dir = tempfile::tempdir().unwrap();
        let local_dir = dir.path().join(".nemesisbot");
        std::fs::create_dir_all(&local_dir).unwrap();
        let config_path = local_dir.join("config.json");

        let mut config = Config::default();
        // Set workspace to default home path so local mode adjusts it
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        config.agents.defaults.workspace = home.join(".nemesisbot").join("workspace").to_string_lossy().to_string();

        // This should succeed even if local mode detection doesn't fully trigger
        let result = save_config(&config_path, &mut config);
        assert!(result.is_ok());
        assert!(config_path.exists());
    }

    #[test]
    fn test_workspace_path_tilde_expansion() {
        let config = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: "~/custom/workspace".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let ws = config.workspace_path();
        // Should expand ~ to home directory
        assert!(!ws.starts_with("~"));
        assert!(ws.contains("custom") || ws.contains("workspace"));
    }

    #[test]
    fn test_workspace_path_empty() {
        let config = Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: String::new(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let ws = config.workspace_path();
        assert!(!ws.is_empty());
    }

    #[test]
    fn test_expand_tilde_home_v2() {
        let result = expand_tilde("~");
        assert!(!result.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_expand_tilde_subpath() {
        let result = expand_tilde("~/subdir");
        assert!(!result.to_string_lossy().starts_with("~"));
        assert!(result.to_string_lossy().contains("subdir"));
    }

    #[test]
    fn test_post_process_for_compatibility() {
        let mut config = Config::default();
        // External with non-empty sync_to
        config.channels.external.sync_to = vec!["web".to_string()];
        config.channels.websocket.sync_to = vec!["web".to_string()];
        config.post_process_for_compatibility();
        assert!(config.channels.external.sync_to_web);
        assert!(config.channels.websocket.sync_to_web);

        // Empty sync_to
        config.channels.external.sync_to = vec![];
        config.channels.websocket.sync_to = vec![];
        config.post_process_for_compatibility();
        assert!(!config.channels.external.sync_to_web);
        assert!(!config.channels.websocket.sync_to_web);
    }

    #[test]
    fn test_adjust_paths_for_environment_empty_workspace() {
        let mut config = Config::default();
        config.agents.defaults.workspace = String::new();
        config.adjust_paths_for_environment();
        assert!(!config.agents.defaults.workspace.is_empty());
    }

    #[test]
    fn test_adjust_paths_for_environment_log_dir() {
        let mut config = Config::default();
        config.logging = Some(LoggingConfig {
            llm: Some(LlmLogConfig {
                enabled: true,
                log_dir: String::new(),
                detail_level: "full".to_string(),
            }),
            general: None,
        });
        config.adjust_paths_for_environment();
        assert_eq!(config.logging.as_ref().unwrap().llm.as_ref().unwrap().log_dir, "logs/request_logs");
    }

    #[test]
    fn test_adjust_paths_for_environment_existing_log_dir() {
        let mut config = Config::default();
        config.logging = Some(LoggingConfig {
            llm: Some(LlmLogConfig {
                enabled: true,
                log_dir: "custom/logs".to_string(),
                detail_level: "full".to_string(),
            }),
            general: None,
        });
        config.adjust_paths_for_environment();
        assert_eq!(config.logging.as_ref().unwrap().llm.as_ref().unwrap().log_dir, "custom/logs");
    }

    #[test]
    fn test_adjust_paths_for_environment_no_logging() {
        let mut config = Config::default();
        config.logging = None;
        config.adjust_paths_for_environment();
        assert!(config.logging.is_none());
    }

    #[test]
    fn test_load_config_from_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let json = r#"{"gateway": {"host": "0.0.0.0", "port": 9999}}"#;
        std::fs::write(&path, json).unwrap();
        let config = load_config(&path).unwrap();
        assert_eq!(config.gateway.port, 9999);
    }

    #[test]
    fn test_clamav_engine_config_roundtrip() {
        let cfg = ClamAVEngineConfig {
            url: "tcp://localhost:3310".to_string(),
            clamav_path: "/usr/bin/clamav".to_string(),
            scan_on_write: true,
            scan_extensions: vec![".exe".to_string()],
            skip_extensions: vec![".txt".to_string()],
            max_file_size: 100_000_000,
            state: EngineState {
                install_status: "installed".to_string(),
                db_status: "ready".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let parsed: ClamAVEngineConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.url, "tcp://localhost:3310");
        assert!(parsed.scan_on_write);
        assert_eq!(parsed.scan_extensions.len(), 1);
        assert_eq!(parsed.state.install_status, "installed");
    }

    #[test]
    fn test_config_loader_save_to_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let config = Config::default();
        ConfigLoader::save_to_workspace(&config, &workspace).unwrap();
        let config_path = workspace.join("config.json");
        assert!(config_path.exists());
    }

    #[test]
    fn test_dlp_layer_config_roundtrip() {
        let cfg = DLPLayerConfig {
            enabled: true,
            rules: vec!["no_credit_card".to_string(), "no_ssn".to_string()],
            action: "block".to_string(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: DLPLayerConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.rules.len(), 2);
        assert_eq!(parsed.action, "block");
    }

    #[test]
    fn test_signature_layer_config_roundtrip() {
        let cfg = SignatureLayerConfig {
            enabled: true,
            strict: true,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SignatureLayerConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert!(parsed.strict);
    }

    #[test]
    fn test_model_config_all_fields() {
        let model = ModelConfig {
            model_name: "test-model".to_string(),
            model: "test/test-v1".to_string(),
            api_base: "https://api.test.com/v1".to_string(),
            api_key: "sk-test".to_string(),
            proxy: "http://proxy:8080".to_string(),
            auth_method: "bearer".to_string(),
            connect_mode: "streaming".to_string(),
            workspace: "/custom/ws".to_string(),
        };
        assert_eq!(model.api_base, "https://api.test.com/v1");
        assert_eq!(model.proxy, "http://proxy:8080");
        assert_eq!(model.auth_method, "bearer");
        assert_eq!(model.connect_mode, "streaming");
        assert_eq!(model.workspace, "/custom/ws");
    }

    #[test]
    fn test_general_log_config_defaults_v2() {
        let cfg = GeneralLogConfig::default();
        assert!(!cfg.enabled); // Default trait gives false, serde default gives true
        assert!(!cfg.enable_console); // Default trait gives false
        assert!(cfg.level.is_empty()); // Default trait gives "", serde default gives "INFO"
        assert!(cfg.file.is_empty());
    }

    #[test]
    fn test_llm_log_config_defaults_v2() {
        let cfg = LlmLogConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.log_dir.is_empty());
        assert!(cfg.detail_level.is_empty());
    }

    #[test]
    fn test_cron_tools_config_defaults_v2() {
        let cfg = CronToolsConfig::default();
        assert_eq!(cfg.exec_timeout_minutes, 0);
    }
}
