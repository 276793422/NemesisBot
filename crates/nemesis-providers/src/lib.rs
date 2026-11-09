//! NemesisBot - LLM Providers
//!
//! Provider management, routing strategies, failover, and LLM driver implementations.

pub mod failover;
pub mod http_provider;
pub mod null_provider;
pub mod openai_compat;
pub mod router;
pub mod stream_util;
pub mod tool_call_repair;
pub mod types;

pub mod anthropic;
pub mod claude_cli;
pub mod cli_child;
pub mod codex;
pub mod codex_cli;
pub mod codex_credentials;
pub mod cooldown;
pub mod default_slot;
/// 强度翻译（U1）：reasoning_effort → openai 兼容请求体补丁（HttpCompat lane）。
pub mod effort_translate;
pub mod error_classifier;
pub mod factory;
pub mod fallback_provider;
pub mod github_copilot;
/// 图像生成独立 lane（images-openai；集群专业职能框架）。
pub mod images;
pub mod model_ref;
pub mod think_strip;
pub mod tool_call_extract;
