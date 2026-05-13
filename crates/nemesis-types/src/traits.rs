//! Core traits for decoupling between modules.

use async_trait::async_trait;
use std::pin::Pin;

use crate::channel::{InboundMessage, OutboundMessage};
use crate::error::Result;
use crate::provider::{LlmRequest, LlmResponse, StreamChunk};
use crate::security::{Operation, SecurityVerdict};
use crate::tools::ToolDefinition;

/// Channel adapter trait for message I/O.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn name(&self) -> &str;
    async fn send(&self, msg: OutboundMessage) -> Result<()>;
    async fn start(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}

/// LLM Provider trait.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse>;
    async fn stream(&self, request: LlmRequest) -> Result<Pin<Box<dyn futures::Stream<Item = Result<StreamChunk>> + Send>>>;
}

/// Security checker trait.
#[async_trait]
pub trait SecurityChecker: Send + Sync {
    fn evaluate(&self, operation: &Operation) -> SecurityVerdict;
}

/// Tool trait for agent-callable tools.
#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, args: serde_json::Value, context: crate::tools::ToolContext) -> Result<String>;
}

/// Contextual tool with channel/chat context injection.
#[async_trait]
pub trait ContextualTool: Tool {
    fn set_context(&self, channel: &str, chat_id: &str);
}

/// Memory store trait.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn store(&self, entry: crate::memory::MemoryEntry) -> Result<()>;
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<crate::memory::MemoryEntry>>;
    async fn delete(&self, id: &str) -> Result<bool>;
}

/// Message bus trait for pub/sub.
#[async_trait]
pub trait MessageBus: Send + Sync {
    async fn publish_inbound(&self, msg: InboundMessage);
    async fn publish_outbound(&self, msg: OutboundMessage);
    fn subscribe_inbound(&self) -> tokio::sync::broadcast::Receiver<InboundMessage>;
    fn subscribe_outbound(&self) -> tokio::sync::broadcast::Receiver<OutboundMessage>;
}
