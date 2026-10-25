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
pub mod background_registry;
pub mod capture_sink;
pub mod cc_hooks;
pub mod chat_log;
pub mod checkpoint;
/// ②typed checks 块捕获（诚实边界收口 2026-10-19）：检查类工具（exec /
/// run_checks）真实执行后的管线侧确定性记账——交付证据 `$.delivery.checks`
/// 的捕获源，见模块头诚实边界。
pub mod checks_capture;
// P30（WS14）Canvas widget：```canvas 块检出 + JS 语法预检（终答判定处消费）。
pub mod canvas;
/// T2（内部专项总计划 L1 codemode）：`code_run` QuickJS 脚本工具——脚本内
/// tool() 过完整 dispatch 瀑布；编译闸 `codemode` feature + 运行闸
/// `agents.codemode.enabled`（默认开，Tools 页可关，开关热生效）+
/// min_tier big 双闸。**非 Android
/// only**：rquickjs-sys 无 android bindings（依赖声明在 target 门控表，
/// 与本 cfg 同收口——android 上 codemode feature 在但无依赖，模块缺席）。
#[cfg(all(feature = "codemode", not(target_os = "android")))]
pub mod code_run;
pub mod context;
/// A3 委派持续操控（2026-10-18）：delegate 四工具 + 协议 adapter（claude
/// stream-json / codex app-server）。运行闸 `agents.delegate.enabled`（默认
/// 关）+ CLI 在场探测；复用 B4 后台注册表复用件；estop 联动。
pub mod delegate_registry;
pub mod discipline;
pub mod estop;
pub mod event_ledger;
pub mod executor_pipe;
pub mod executor_transport;
pub mod formatter;
pub mod fs_watcher;
pub mod history;
// U20 会话全文检索（FTS5）——随 history-search feature（rusqlite）进出。
#[cfg(feature = "history-search")]
pub mod history_search;
pub mod hooks;
pub mod image_attach;
pub mod image_downscale;
pub mod image_path_detector;
pub mod inbox;
pub mod instance;
pub mod interrupt_replay;
pub mod r#loop;
pub mod loop_continuation;
pub mod loop_executor;
pub mod loop_tools;
pub mod mcp_bridge;
pub mod memory;
pub mod message_preprocess;
pub mod probe;
pub mod prompt;
pub mod prune;
pub mod registry;
pub mod remote_executor_tool;
pub mod replay;
pub mod request_logger;
pub mod request_logger_observer;
pub mod ringbuffer;
pub mod session;
pub mod session_fork;
pub mod skills_digest;
pub mod spill;
pub mod todo_closeout;
/// P8（集群无人值守硬化 A2）：会话 token 预算闸——marker 文案单一真相源 +
/// per-session 累计账本（loop 侧接线在 `r#loop::token_budget`）。
pub mod token_budget;
pub mod tool_adapter;
pub mod tool_doc_folding;
pub mod tool_event_hook;
/// T1（扩展计划 D3）：工具收据——防幻觉执行证明（HMAC-SHA256）。
pub mod tool_receipts;
pub mod turn_guard;
/// L8（内部专项总计划重评估）：工具调用三态 WAL（崩溃恢复续轮）。
pub mod turn_wal;
pub mod types;
pub mod user_profile;
pub mod workspace_instructions;
/// WS9/P22：workspace 级写租约（文件锁 + 持有者 sidecar；写类工具包装）。
pub mod workspace_lease;

#[cfg(test)]
mod history_tests;

#[cfg(test)]
/// P8（集群无人值守硬化 A2）：token 预算账本（seed 防双计/增量/隔离/软闩/
/// 容量上限/文案 marker）测试（独立测试文件）。
mod token_budget_tests;

#[cfg(test)]
/// WS9/P22：租约互斥/宽限/死进程释放/开关旁路测试（独立测试文件）。
mod workspace_lease_tests;

// S9 (quality-hardening goal 冲刺 S9): 测试共享 helper（thread-local tracing
// subscriber），声明式挂载指向独立文件，无内联测试。
#[cfg(test)]
mod test_support;

pub use background_registry::BackgroundProcessRegistry;
pub use capture_sink::{CaptureSink, SessionWriteCapture, ToolCapture};
pub use context::RequestContext;
pub use estop::EstopState;
pub use image_attach::LlmImage;
pub use instance::AgentInstance;
pub use loop_continuation::{
    ContinuationData, ContinuationManager, ContinuationSnapshot, ContinuationStore,
    ContinuationToolResult, handle_cluster_continuation,
};
pub use loop_executor::{Observer, ObserverEvent, ObserverUsageInfo, ToolResult};
pub use loop_tools::ClusterRpcChannelConfig;
pub use loop_tools::ClusterRpcConfig;
pub use loop_tools::ClusterRpcTool;
pub use loop_tools::SharedToolConfig;
pub use loop_tools::WorkspaceBoundary;
pub use loop_tools::register_default_tools;
pub use loop_tools::register_extended_tools;
pub use loop_tools::register_shared_tools;
pub use loop_tools::setup_cluster_rpc_channel;
pub use memory::ConversationMemory;
pub use registry::AgentRegistry;
pub use remote_executor_tool::{
    DaclSpawnFn, DaclSpawnHook, DaclSpawnOutcome, DaclSpawnRequest, ExecutorChannel, MOVE_TOOLS,
    PersistentCallError, PersistentExecutor, PersistentRoute, RemoteExecutorTool, StrictGate,
    UserlandFallback,
};
pub use request_logger::RequestLogger;
pub use request_logger_observer::RequestLoggerObserver;
pub use ringbuffer::RingBuffer;
pub use session::{Session, SessionManager, SessionStore, StoredMessage, StoredSession};
pub use session::{
    estimate_tokens, estimate_tokens_for_turns, force_compress_turns, is_internal_channel,
};
pub use todo_closeout::TodoCloseoutHook;
pub use types::*;
pub use workspace_lease::{LEASE_WRITE_TOOLS, LeaseGuardTool, WorkspaceLease};
