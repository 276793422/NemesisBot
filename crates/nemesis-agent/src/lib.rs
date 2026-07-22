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

pub mod args_validator;
pub mod capture_sink;
pub mod chat_log;
pub mod checkpoint;
pub mod context;
pub mod estop;
pub mod executor_pipe;
pub mod instance;
pub mod r#loop;
pub mod loop_continuation;
pub mod loop_executor;
pub mod loop_tools;
pub mod mcp_bridge;
pub mod memory;
pub mod message_preprocess;
pub mod probe;
pub mod registry;
pub mod remote_executor_tool;
pub mod request_logger;
pub mod request_logger_observer;
pub mod ringbuffer;
pub mod session;
pub mod tool_adapter;
pub mod turn_guard;
pub mod types;

pub use capture_sink::{CaptureSink, SessionWriteCapture, ToolCapture};
pub use context::RequestContext;
pub use estop::EstopState;
pub use instance::AgentInstance;
pub use loop_continuation::{
    ContinuationData, ContinuationManager, ContinuationSnapshot, ContinuationStore,
    ContinuationToolResult, handle_cluster_continuation,
};
pub use loop_executor::{
    AgentLoopExecutor, ConcurrentMode, ContextualTool, ExecutorConfig, FallbackCandidate,
    FallbackExecutor, FallbackResult, Observer, ObserverEvent, SessionPersistence, ToolResult,
};
pub use loop_tools::ClusterRpcChannelConfig;
pub use loop_tools::ClusterRpcConfig;
pub use loop_tools::ClusterRpcTool;
pub use loop_tools::SharedToolConfig;
pub use loop_tools::register_default_tools;
pub use loop_tools::register_extended_tools;
pub use loop_tools::register_shared_tools;
pub use loop_tools::setup_cluster_rpc_channel;
pub use memory::ConversationMemory;
pub use registry::AgentRegistry;
pub use remote_executor_tool::{ExecutorChannel, MOVE_TOOLS, RemoteExecutorTool};
pub use request_logger::RequestLogger;
pub use request_logger_observer::RequestLoggerObserver;
pub use ringbuffer::RingBuffer;
pub use session::{
    NullNotifier, Session, SessionManager, SessionStore, StoredMessage, StoredSession,
    SummarizationNotifier, Summarizer,
};
pub use session::{
    estimate_tokens, estimate_tokens_for_turns, force_compress_turns, is_internal_channel,
};
pub use types::*;
