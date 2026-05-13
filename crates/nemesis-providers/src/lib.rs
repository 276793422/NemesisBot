//! NemesisBot - LLM Providers
//!
//! Provider management, routing strategies, failover, and LLM driver implementations.

pub mod types;
pub mod failover;
pub mod router;
pub mod http_provider;
pub mod openai_compat;

pub mod model_ref;
pub mod tool_call_extract;
pub mod error_classifier;
pub mod cooldown;
pub mod anthropic;
pub mod claude_cli;
pub mod codex;
pub mod codex_cli;
pub mod codex_credentials;
pub mod github_copilot;
pub mod factory;
pub mod fallback_provider;
