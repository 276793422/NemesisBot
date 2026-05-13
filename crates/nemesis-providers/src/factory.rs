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
mod tests {
    use super::*;

    #[test]
    fn test_resolve_anthropic() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet-4-5".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::Anthropic);
        assert_eq!(sel.model, "claude-sonnet-4-5");
        assert_eq!(sel.api_base, "https://api.anthropic.com");
    }

    #[test]
    fn test_resolve_claude_cli() {
        let cfg = FactoryConfig {
            llm_ref: "claude-cli/claude-code".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
    }

    #[test]
    fn test_resolve_codex_cli() {
        let cfg = FactoryConfig {
            llm_ref: "codex-cli/codex".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::CodexCli);
    }

    #[test]
    fn test_resolve_github_copilot() {
        let cfg = FactoryConfig {
            llm_ref: "copilot/gpt-4.1".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
    }

    #[test]
    fn test_resolve_http_compat() {
        let cfg = FactoryConfig {
            llm_ref: "deepseek/deepseek-chat".to_string(),
            api_key: "test-key".to_string(),
            api_base: "https://api.deepseek.com/v1".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::HttpCompat);
    }

    #[test]
    fn test_resolve_http_compat_no_key() {
        let cfg = FactoryConfig {
            llm_ref: "deepseek/deepseek-chat".to_string(),
            ..Default::default()
        };
        let result = resolve_provider_selection(&cfg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no API key"));
    }

    #[test]
    fn test_resolve_openai() {
        let cfg = FactoryConfig {
            llm_ref: "openai/gpt-4o".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::Codex);
    }

    #[test]
    fn test_resolve_empty_ref() {
        let cfg = FactoryConfig {
            llm_ref: String::new(),
            ..Default::default()
        };
        let result = resolve_provider_selection(&cfg);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_provider_anthropic() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "anthropic");
    }

    #[test]
    fn test_create_provider_http() {
        let cfg = FactoryConfig {
            llm_ref: "deepseek/chat".to_string(),
            api_key: "test-key".to_string(),
            api_base: "https://api.deepseek.com/v1".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "http-compat");
    }

    #[test]
    fn test_create_provider_claude_cli() {
        let cfg = FactoryConfig {
            llm_ref: "claude-cli/claude-code".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "claude-cli");
    }

    #[test]
    fn test_create_provider_codex_cli() {
        let cfg = FactoryConfig {
            llm_ref: "codex-cli/default".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "codex-cli");
    }

    #[test]
    fn test_create_provider_copilot() {
        let cfg = FactoryConfig {
            llm_ref: "copilot/gpt-4.1".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "github-copilot");
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_factory_config_default() {
        let cfg = FactoryConfig::default();
        assert!(cfg.llm_ref.is_empty());
        assert!(cfg.api_key.is_empty());
        assert!(cfg.api_base.is_empty());
        assert!(cfg.workspace.is_empty());
        assert!(cfg.connect_mode.is_empty());
        assert!(cfg.account_id.is_empty());
        assert!(cfg.headers.is_empty());
    }

    #[test]
    fn test_resolve_provider_selection_anthropic_default_base() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_base, "https://api.anthropic.com");
    }

    #[test]
    fn test_resolve_provider_selection_anthropic_custom_base() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "key".to_string(),
            api_base: "https://custom-proxy.com".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_base, "https://custom-proxy.com");
    }

    #[test]
    fn test_resolve_provider_selection_openai_default_base() {
        let cfg = FactoryConfig {
            llm_ref: "openai/gpt-4o".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_base, "https://chatgpt.com/backend-api/codex");
    }

    #[test]
    fn test_resolve_provider_selection_workspace_default() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.workspace, ".");
    }

    #[test]
    fn test_resolve_provider_selection_workspace_custom() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude".to_string(),
            api_key: "key".to_string(),
            workspace: "/custom/workspace".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.workspace, "/custom/workspace");
    }

    #[test]
    fn test_resolve_claude_code_alias() {
        let cfg = FactoryConfig {
            llm_ref: "claude-code/claude".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
    }

    #[test]
    fn test_resolve_claudecodec_alias() {
        let cfg = FactoryConfig {
            llm_ref: "claudecodec/default".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
    }

    #[test]
    fn test_resolve_codex_code_alias() {
        let cfg = FactoryConfig {
            llm_ref: "codex-code/default".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::CodexCli);
    }

    #[test]
    fn test_resolve_github_copilot_alias() {
        let cfg = FactoryConfig {
            llm_ref: "github_copilot/gpt-4".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
    }

    #[test]
    fn test_create_provider_codex() {
        let cfg = FactoryConfig {
            llm_ref: "openai/gpt-4o".to_string(),
            api_key: "test-key".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "codex");
    }

    #[test]
    fn test_provider_selection_debug() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude".to_string(),
            api_key: "key".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        let debug_str = format!("{:?}", sel);
        assert!(debug_str.contains("Anthropic"));
    }

    #[test]
    fn test_provider_type_equality() {
        assert_eq!(ProviderType::HttpCompat, ProviderType::HttpCompat);
        assert_ne!(ProviderType::HttpCompat, ProviderType::Anthropic);
    }

    #[test]
    fn test_resolve_whitespace_only_ref() {
        let cfg = FactoryConfig {
            llm_ref: "   ".to_string(),
            ..Default::default()
        };
        let result = resolve_provider_selection(&cfg);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_http_compat_with_key_and_base() {
        let cfg = FactoryConfig {
            llm_ref: "zhipu/glm-4".to_string(),
            api_key: "test-key".to_string(),
            api_base: "https://open.bigmodel.cn/api/paas/v4".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.provider_type, ProviderType::HttpCompat);
        assert_eq!(sel.model, "glm-4");
        assert_eq!(sel.api_base, "https://open.bigmodel.cn/api/paas/v4");
    }

    #[test]
    fn test_resolve_http_compat_missing_key_error_message() {
        let cfg = FactoryConfig {
            llm_ref: "somevendor/model-x".to_string(),
            api_base: "https://api.vendor.com".to_string(),
            ..Default::default()
        };
        let result = resolve_provider_selection(&cfg);
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("no API key"));
        assert!(err_msg.contains("somevendor"));
        assert!(err_msg.contains("model-x"));
    }

    #[test]
    fn test_resolve_provider_selection_preserves_api_key() {
        let cfg = FactoryConfig {
            llm_ref: "anthropic/claude-sonnet".to_string(),
            api_key: "sk-ant-test123".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.api_key, "sk-ant-test123");
    }

    #[test]
    fn test_resolve_provider_selection_preserves_connect_mode() {
        let cfg = FactoryConfig {
            llm_ref: "copilot/gpt-4".to_string(),
            connect_mode: "ide".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.connect_mode, "ide");
    }

    #[test]
    fn test_resolve_provider_selection_preserves_account_id() {
        let cfg = FactoryConfig {
            llm_ref: "openai/gpt-4o".to_string(),
            api_key: "test".to_string(),
            account_id: "org-12345".to_string(),
            ..Default::default()
        };
        let sel = resolve_provider_selection(&cfg).unwrap();
        assert_eq!(sel.account_id, "org-12345");
    }

    #[test]
    fn test_factory_config_with_headers() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Custom".to_string(), "value".to_string());
        let cfg = FactoryConfig {
            llm_ref: "deepseek/chat".to_string(),
            api_key: "test".to_string(),
            api_base: "https://api.deepseek.com".to_string(),
            headers,
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "http-compat");
    }

    #[test]
    fn test_create_provider_http_with_key() {
        let cfg = FactoryConfig {
            llm_ref: "test/testai-1.1".to_string(),
            api_key: "test-key".to_string(),
            api_base: "http://127.0.0.1:8080/v1".to_string(),
            ..Default::default()
        };
        let provider = create_provider(&cfg).unwrap();
        assert_eq!(provider.name(), "http-compat");
        assert_eq!(provider.default_model(), "testai-1.1");
    }

    #[test]
    fn test_provider_type_copy() {
        // Verify ProviderType is Copy (can be used multiple times)
        let pt = ProviderType::Anthropic;
        let pt2 = pt;
        assert_eq!(pt, pt2);
    }

    #[test]
    fn test_provider_selection_debug_format() {
        let sel = ProviderSelection {
            provider_type: ProviderType::Anthropic,
            api_key: "key".to_string(),
            api_base: "https://api.anthropic.com".to_string(),
            model: "claude-3".to_string(),
            workspace: ".".to_string(),
            connect_mode: String::new(),
            account_id: String::new(),
        };
        let debug = format!("{:?}", sel);
        assert!(debug.contains("Anthropic"));
        assert!(debug.contains("claude-3"));
    }
}
