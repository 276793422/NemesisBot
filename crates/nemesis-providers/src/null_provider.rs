//! 无 LLM 降级 provider（双击直启 goal，2026-09-17）。
//!
//! 网关必须能在「用户还没配置任何模型」的全新安装上启动（双击直启：先见
//! Dashboard、后配 LLM）。factory 对缺 key loud 拒绝是正确设计（防半配置
//! 静默裸奔），但**启动装配点**不能被它杀死——装配点改用
//! [`crate::factory::create_provider_or_null`]，构造失败时拿到本 provider：
//! 每次调用诚实报「未配置模型，请到 Dashboard → 模型管理配置并设为默认」，
//! 用户配好并 set_default 热切（canonical_swap_params）后立即恢复。
//! 一次性 CLI 入口（`run` / `model probe` / Dashboard 连通性测试）保持严格
//! create_provider——它们就该看见错误。

use crate::failover::FailoverError;
use crate::http_provider::StreamChunk;
use crate::router::LLMProvider;
use crate::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use async_trait::async_trait;
use std::sync::Arc;

const NO_LLM_MESSAGE: &str = "未配置模型：请到 Dashboard → 模型管理添加模型并设为默认（或 CLI `nemesisbot model add --model <vendor/model> --key <key> --default`），配置后无需重启";

/// 永远诚实报「未配置模型」的哑 provider。
pub struct NullProvider {
    /// factory 侧的原始构造失败原因（日志/诊断用；用户面向文案统一为
    /// NO_LLM_MESSAGE 指路，原始错误在启动日志已有 warn）。
    pub reason: String,
}

impl NullProvider {
    pub fn new(reason: String) -> Self {
        Self { reason }
    }

    fn error(&self) -> FailoverError {
        FailoverError::Unknown {
            provider: "null".to_string(),
            message: if self.reason.is_empty() {
                NO_LLM_MESSAGE.to_string()
            } else {
                format!("{}（启动时装配失败原因: {}）", NO_LLM_MESSAGE, self.reason)
            },
        }
    }
}

#[async_trait]
impl LLMProvider for NullProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Err(self.error())
    }

    fn chat_stream(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> tokio::sync::mpsc::Receiver<Result<StreamChunk, FailoverError>> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let err = self.error();
        let _ = tx.try_send(Err(err));
        rx
    }

    fn default_model(&self) -> &str {
        ""
    }

    fn name(&self) -> &str {
        "null"
    }
}

/// 便捷构造（Arc 包装）。
pub fn null_provider(reason: impl Into<String>) -> Arc<dyn LLMProvider> {
    Arc::new(NullProvider::new(reason.into()))
}

#[cfg(test)]
mod tests;
