//! NemesisBot - Agent Engine
//!
//! Core agent loop, instance management, conversation memory, and request context.
//!
//! # Architecture
//!
//! The agent engine processes messages through a multi-step loop:
//!
//! 1. Receive an inbound message
//! 2. Build conversation context from history
//! 3. Call the LLM provider
//! 4. If the response contains tool calls, execute them and feed results back
//! 5. Repeat until the LLM returns a plain text response or max turns is reached
//!
//! # Key Types
//!
//! - [`AgentInstance`] manages conversation history and agent state for a session
//! - [`AgentLoop`] is the core execution loop that drives LLM + tool interactions
//! - [`ConversationMemory`] manages context window sizing and message summarization
//! - [`RequestContext`] carries per-request metadata (channel, session, correlation ID)

pub mod types;
pub mod instance;
pub mod r#loop;
pub mod memory;
pub mod context;
pub mod loop_executor;
pub mod loop_tools;
pub mod session;
pub mod ringbuffer;
pub mod registry;
pub mod request_logger;
pub mod request_logger_observer;
pub mod loop_continuation;

pub use types::*;
pub use instance::AgentInstance;
pub use memory::ConversationMemory;
pub use context::RequestContext;
pub use loop_executor::{
    AgentLoopExecutor, ExecutorConfig,
    ConcurrentMode, ObserverEvent, Observer,
    FallbackCandidate, FallbackResult,
    ContextualTool, ToolResult, FallbackExecutor,
    SessionPersistence,
};
pub use loop_tools::register_default_tools;
pub use loop_tools::register_shared_tools;
pub use loop_tools::register_extended_tools;
pub use loop_tools::setup_cluster_rpc_channel;
pub use loop_tools::ClusterRpcChannelConfig;
pub use loop_tools::SharedToolConfig;
pub use session::{Session, SessionManager, SessionStore, StoredSession, StoredMessage, Summarizer, NullNotifier, SummarizationNotifier};
pub use session::{estimate_tokens, estimate_tokens_for_turns, force_compress_turns, is_internal_channel};
pub use ringbuffer::RingBuffer;
pub use registry::AgentRegistry;
pub use request_logger::RequestLogger;
pub use request_logger_observer::RequestLoggerObserver;
pub use loop_continuation::{
    ContinuationManager, ContinuationData, ContinuationSnapshot,
    ContinuationStore, ContinuationToolResult,
    handle_cluster_continuation,
};
