// Configuration management for WebSocket client
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub server: ServerConfig,
    pub reconnect: ReconnectConfig,
    pub heartbeat: HeartbeatConfig,
    pub logging: LoggingConfig,
    pub ui: UiConfig,
    pub statistics: StatisticsConfig,
    #[serde(default)]
    pub message_rules: MessageRulesConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReconnectConfig {
    pub enabled: bool,
    pub max_attempts: u32,
    pub initial_delay: u64,
    pub max_delay: u64,
    #[serde(default = "default_delay_multiplier")]
    pub delay_multiplier: f64,
}

fn default_delay_multiplier() -> f64 {
    2.0
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HeartbeatConfig {
    pub enabled: bool,
    pub interval: u64,
    pub timeout: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoggingConfig {
    pub enabled: bool,
    pub file: String,
    pub level: String,
    pub log_messages: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiConfig {
    pub color: bool,
    pub show_timestamp: bool,
    pub show_stats: bool,
    #[serde(default = "default_prompt_style")]
    pub prompt_style: String,
}

fn default_prompt_style() -> String {
    "simple".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StatisticsConfig {
    pub enabled: bool,
    pub print_interval: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                url: "ws://127.0.0.1:49001/ws".to_string(),
                token: String::new(),
            },
            reconnect: ReconnectConfig {
                enabled: true,
                max_attempts: 0,
                initial_delay: 1,
                max_delay: 30,
                delay_multiplier: 2.0,
            },
            heartbeat: HeartbeatConfig {
                enabled: true,
                interval: 30,
                timeout: 10,
            },
            logging: LoggingConfig {
                enabled: true,
                file: "websocket_client.log".to_string(),
                level: "info".to_string(),
                log_messages: true,
            },
            ui: UiConfig {
                color: true,
                show_timestamp: true,
                show_stats: true,
                prompt_style: "simple".to_string(),
            },
            statistics: StatisticsConfig {
                enabled: true,
                print_interval: 0,
            },
            message_rules: MessageRulesConfig::default(),
        }
    }
}

impl Config {
    /// Load configuration from file
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to file
    pub fn save(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Get default config path
    pub fn get_default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("websocket_client")
            .join("config.toml")
    }

    /// Load from default path or create default config
    pub fn load_or_create_default() -> Self {
        let config_path = Self::get_default_path();

        if let Ok(config) = Self::load(config_path.to_str().unwrap_or("config.toml")) {
            config
        } else {
            // Create default config if not exists
            let config = Config::default();
            if let Some(parent) = config_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = config.save(config_path.to_str().unwrap_or("config.toml"));
            config
        }
    }
}

/// Message processing rules configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageRulesConfig {
    /// Enable message rule processing
    #[serde(default = "default_rules_enabled")]
    pub enabled: bool,
    /// List of message processing rules
    #[serde(default)]
    pub rules: Vec<MessageRule>,
}

fn default_rules_enabled() -> bool {
    true
}

impl Default for MessageRulesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: vec![
                // Rule 1: Memory threshold - skip message
                MessageRule {
                    name: "memory-threshold".to_string(),
                    description: "内存阈值优化中".to_string(),
                    pattern: "Memory threshold reached".to_string(),
                    replacement: String::new(),
                    enabled: true,
                    case_sensitive: false,
                    skip: true,
                },
                // Rule 2: API rate limit error (429)
                MessageRule {
                    name: "api-rate-limit".to_string(),
                    description: "API访问量过大，模型繁忙".to_string(),
                    pattern: "您的账户已达到速率限制".to_string(),
                    replacement: "【目前我有点忙，要不然你等会再叫我】".to_string(),
                    enabled: true,
                    case_sensitive: false,
                    skip: false,
                },
                // Rule 3: API 400 error - prompt parameter missing
                MessageRule {
                    name: "prompt-param-missing".to_string(),
                    description: "未正常接收到prompt参数".to_string(),
                    pattern: "未正常接收到prompt参数".to_string(),
                    replacement: "【你说什么，我没听清，可以再说一遍么】".to_string(),
                    enabled: true,
                    case_sensitive: false,
                    skip: false,
                },
                // Rule 4: Generic API error - LLM call failed
                MessageRule {
                    name: "llm-call-failed".to_string(),
                    description: "LLM调用失败".to_string(),
                    pattern: "LLM call failed after retries".to_string(),
                    replacement: "【我遇到一些技术问题，请稍后再试】".to_string(),
                    enabled: true,
                    case_sensitive: false,
                    skip: false,
                },
            ],
        }
    }
}

/// A single message processing rule
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessageRule {
    /// Unique rule name/identifier
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Pattern to match (supports substring matching)
    pub pattern: String,
    /// Replacement text when pattern matches
    pub replacement: String,
    /// Whether this rule is enabled
    #[serde(default = "default_rule_enabled")]
    pub enabled: bool,
    /// Case-sensitive matching
    #[serde(default)]
    pub case_sensitive: bool,
    /// Skip displaying this message entirely (instead of replacing)
    #[serde(default)]
    pub skip: bool,
}

fn default_rule_enabled() -> bool {
    true
}
