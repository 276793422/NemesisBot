//! RPC sub-module for cluster peer-to-peer communication.

pub mod client;
pub mod peer_chat_handler;
pub mod server;

pub use client::{LocalNetworkInterface, PeerResolver, RpcClient, RpcClientError};
pub use peer_chat_handler::{
    LlmChannel, PeerChatHandler, PeerChatRequest, PeerChatResult, RpcMeta,
    TaskResultPersister,
};
pub use server::{RpcServer, RpcServerConfig, RpcHandlerFn};

/// Trait for the RPC channel that bridges cluster RPC and the LLM pipeline.
///
/// This is the Rust equivalent of Go's `channels.RPCChannel`. The concrete
/// implementation lives in the channels module; the cluster only depends on
/// this trait to avoid circular imports.
pub trait RpcChannel: Send + Sync + std::fmt::Debug {
    /// Submit a message for LLM processing. Returns a receiver for the response.
    fn input(
        &self,
        session_key: &str,
        content: &str,
        correlation_id: &str,
    ) -> Result<tokio::sync::oneshot::Receiver<String>, String>;
}
