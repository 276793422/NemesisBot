//! Provider factory (create provider from config).

use crate::anthropic::{AnthropicConfig, AnthropicProvider};
use crate::claude_cli::{ClaudeCliConfig, ClaudeCliProvider};
use crate::codex::{CodexConfig, CodexProvider};
use crate::codex_cli::{CodexCliConfig, CodexCliProvider};
use crate::github_copilot::{GitHubCopilotConfig, GitHubCopilotProvider};
use crate::http_provider::{HttpProvider, HttpProviderConfig};
use crate::model_ref::{normalize_provider, parse_model_ref};
use crate::router::LLMProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Provider type identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    HttpCompat,
    Anthropic,
    Codex,
    ClaudeCli,
    CodexCli,
    GitHubCopilot,
}

/// Resolved provider selection from config.
#[derive(Debug, Clone)]
pub struct ProviderSelection {
    pub provider_type: ProviderType,
    pub api_key: String,
    pub api_base: String,
    pub model: String,
    pub workspace: String,
    pub connect_mode: String,
    pub account_id: String,
}

/// Factory configuration for resolving providers.
#[derive(Debug, Clone)]
pub struct FactoryConfig {
    /// The LLM reference string (e.g. "anthropic/claude-sonnet", "claude-cli/claude-code").
    pub llm_ref: String,
    /// API key override.
    pub api_key: String,
    /// API base URL override.
    pub api_base: String,
    /// Workspace path for CLI providers.
    pub workspace: String,
    /// Connect mode for GitHub Copilot.
    pub connect_mode: String,
    /// Account ID for Codex providers.
    pub account_id: String,
    /// Additional headers for HTTP provider.
    pub headers: HashMap<String, String>,
}

impl Default for FactoryConfig {
    fn default() -> Self {
        Self {
            llm_ref: String::new(),
            api_key: String::new(),
            api_base: String::new(),
            workspace: String::new(),
            connect_mode: String::new(),
            account_id: String::new(),
            headers: HashMap::new(),
        }
    }
}

/// Resolve a provider selection from factory config.
pub fn resolve_provider_selection(cfg: &FactoryConfig) -> Result<ProviderSelection, String> {
    let model_ref = parse_model_ref(&cfg.llm_ref, "openai")
        .ok_or_else(|| "empty LLM reference".to_string())?;

    let provider_name = normalize_provider(&model_ref.provider);

    let mut sel = ProviderSelection {
        provider_type: ProviderType::HttpCompat,
        api_key: cfg.api_key.clone(),
        api_base: cfg.api_base.clone(),
        model: model_ref.model.clone(),
        workspace: if cfg.workspace.is_empty() {
            ".".to_string()
        } else {
            cfg.workspace.clone()
        },
        connect_mode: cfg.connect_mode.clone(),
        account_id: cfg.account_id.clone(),
    };

    // Handle special providers first
    match provider_name.as_str() {
        "claude-cli" | "claude-code" | "claudecode" | "claudecodec" => {
            sel.provider_type = ProviderType::ClaudeCli;
            return Ok(sel);
        }
        "codex-cli" | "codex-code" => {
            sel.provider_type = ProviderType::CodexCli;
            return Ok(sel);
        }
        "github_copilot" | "copilot" => {
            sel.provider_type = ProviderType::GitHubCopilot;
            return Ok(sel);
        }
        _ => {}
    }

    // Handle standard providers
    match provider_name.as_str() {
        "anthropic" => {
            sel.provider_type = ProviderType::Anthropic;
            if sel.api_base.is_empty() {
                sel.api_base = "https://api.anthropic.com".to_string();
            }
        }
        "openai" => {
            sel.provider_type = ProviderType::Codex;
            if sel.api_base.is_empty() {
                sel.api_base = "https://chatgpt.com/backend-api/codex".to_string();
            }
        }
        _ => {
            sel.provider_type = ProviderType::HttpCompat;
            if sel.api_key.is_empty() {
                return Err(format!(
                    "no API key configured for provider: {} (model: {})\n\
                     Use: nemesisbot model add --model {}/{} --key <YOUR_KEY> --default",
                    provider_name, model_ref.model, provider_name, model_ref.model
                ));
            }
        }
    }

    Ok(sel)
}

/// Create a provider from factory config.
pub fn create_provider(cfg: &FactoryConfig) -> Result<Arc<dyn LLMProvider>, String> {
    let sel = resolve_provider_selection(cfg)?;

    let provider: Arc<dyn LLMProvider> = match sel.provider_type {
        ProviderType::Anthropic => {
            let anthropic_cfg = AnthropicConfig {
                api_key: sel.api_key,
                base_url: sel.api_base,
                default_model: sel.model,
                ..Default::default()
            };
            Arc::new(AnthropicProvider::new(anthropic_cfg))
        }
        ProviderType::Codex => {
            let codex_cfg = CodexConfig {
                api_key: sel.api_key,
                account_id: sel.account_id,
                default_model: sel.model,
                base_url: sel.api_base,
                ..Default::default()
            };
            Arc::new(CodexProvider::new(codex_cfg))
        }
        ProviderType::ClaudeCli => {
            let cli_cfg = ClaudeCliConfig {
                workspace: sel.workspace,
                ..Default::default()
            };
            Arc::new(ClaudeCliProvider::new(cli_cfg))
        }
        ProviderType::CodexCli => {
            let cli_cfg = CodexCliConfig {
                workspace: sel.workspace,
                ..Default::default()
            };
            Arc::new(CodexCliProvider::new(cli_cfg))
        }
        ProviderType::GitHubCopilot => {
            let copilot_cfg = GitHubCopilotConfig {
                uri: sel.api_base,
                connect_mode: sel.connect_mode,
                default_model: sel.model,
                ..Default::default()
            };
            Arc::new(GitHubCopilotProvider::new(copilot_cfg))
        }
        ProviderType::HttpCompat => {
            let http_cfg = HttpProviderConfig {
                name: "http-compat".to_string(),
                base_url: sel.api_base,
                api_key: sel.api_key,
                default_model: sel.model,
                timeout_secs: 120,
                headers: cfg.headers.clone(),
                proxy: None,
                preserve_prefix: false,
            };
            Arc::new(HttpProvider::new(http_cfg))
        }
    };

    Ok(provider)
}

#[cfg(test)]
mod tests;
