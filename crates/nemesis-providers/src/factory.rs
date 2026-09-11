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
    /// Per-model 请求超时秒数（P3A 超时对齐）。0 = lane 默认（600s）。
    pub timeout_secs: u64,
}

/// Factory configuration for resolving providers.
#[derive(Debug, Clone, Default)]
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
    /// 显式协议类型（模型条目 protocol 字段，LLM 协议选择器）：非空时钉死
    /// wire 协议，优先于 llm_ref 前缀推断（显式 > 推断）。canonical 值：
    /// `anthropic`（别名 `claude`）/ `openai`（chat/completions）/ `responses`
    /// （OpenAI Responses API）。空 = 按前缀推断（现状行为）。未知值 loud 报错。
    pub protocol: String,
    /// Additional headers for HTTP provider.
    pub headers: HashMap<String, String>,
    /// Per-model 单请求超时秒数（P3A 超时对齐，2026-09-12）：来自模型条目
    /// extra `timeout_secs`（provider_resolver 提取）。0 = 未设置，落 lane
    /// 默认 600s——此前 anthropic/codex/http-compat 各自写死 120s，与
    /// openai 兼容 lane 的 600s 口径分裂（评审 LLM 连续精确 120s 超时根因）。
    pub timeout_secs: u64,
}

/// 全 lane 统一的单请求默认超时（超时阶梯最内层；外层链见 CLAUDE.md）。
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// 0（未设置）落 lane 默认，>0 透传用户显式配置。
fn effective_timeout(configured: u64) -> u64 {
    if configured > 0 {
        configured
    } else {
        DEFAULT_TIMEOUT_SECS
    }
}

/// Resolve a provider selection from factory config.
pub fn resolve_provider_selection(cfg: &FactoryConfig) -> Result<ProviderSelection, String> {
    // 裸模型名（无 "provider/" 前缀，含 provider 解析为空的 "/name" 形态）没有
    // 可信 provider 归属：默认 openai 会把它们误路由到 Codex Responses API
    // （POST {base}/responses + 模型重映射 gpt-5.2，第三方 OpenAI 兼容端点全灭）。
    // 默认落 HttpCompat（chat/completions 是 OpenAI 兼容通用语）；显式
    // "openai/x" 前缀不受影响，仍走 Codex。生产实证（2026-09-11）：dashboard
    // 裸名 glm-5.3-flash → "auth failure for provider codex/gpt-5.2: status 401"。
    let model_ref = parse_model_ref(&cfg.llm_ref, "http-compat")
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
        timeout_secs: cfg.timeout_secs,
    };

    // Handle special providers first（CLI 型是本地进程不是 wire 协议，
    // 不受显式 protocol 影响）
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

    // 显式协议覆盖（LLM 协议选择器，2026-09-11 拍板语义）：openai =
    // chat/completions（业界通行），responses = OpenAI Responses API
    // （内部 CodexProvider），anthropic = Claude 消息协议。与旧前缀语义的
    // 分歧是刻意的：显式选择就该是用户读到的含义；旧前缀路径（protocol
    // 为空）行为不变。provider 前缀继续管 api_base 默认推断/价目表/显示。
    if !cfg.protocol.is_empty() {
        // 值集归一走单一真相源 nemesis_types::capability::normalize_model_protocol。
        let protocol = nemesis_types::capability::normalize_model_protocol(&cfg.protocol)?;
        match protocol.as_str() {
            "anthropic" => {
                sel.provider_type = ProviderType::Anthropic;
                if sel.api_base.is_empty() {
                    sel.api_base = "https://api.anthropic.com".to_string();
                }
            }
            "openai" => {
                sel.provider_type = ProviderType::HttpCompat;
                if sel.api_base.is_empty() {
                    sel.api_base = "https://api.openai.com/v1".to_string();
                }
            }
            "responses" => {
                sel.provider_type = ProviderType::Codex;
                if sel.api_base.is_empty() {
                    sel.api_base = "https://chatgpt.com/backend-api/codex".to_string();
                }
            }
            _ => unreachable!("normalize_model_protocol only returns the canonical set"),
        }
        return Ok(sel);
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
                timeout_secs: effective_timeout(sel.timeout_secs),
            };
            Arc::new(AnthropicProvider::new(anthropic_cfg))
        }
        ProviderType::Codex => {
            let codex_cfg = CodexConfig {
                api_key: sel.api_key,
                account_id: sel.account_id,
                default_model: sel.model,
                base_url: sel.api_base,
                timeout_secs: effective_timeout(sel.timeout_secs),
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
                timeout_secs: effective_timeout(sel.timeout_secs),
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
