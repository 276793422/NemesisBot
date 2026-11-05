//! NemesisBot Web Module
//!
//! Web server with WebSocket support, SSE event hub, session management,
//! three-level dispatch protocol, and REST API handlers.
//!
//! # Module structure
//!
//! - `server` — WebServer, route registration, SSE stream, health check, message processing
//! - `api_handlers` — REST API endpoints (status, logs, scanner, config)
//! - `cors` — CORS middleware configuration
//! - `events` — SSE EventHub
//! - `history` — Chat history types
//! - `protocol` — Three-level dispatch protocol (type -> module -> cmd)
//! - `session` — Session management with DashMap
//! - `websocket_handler` — WebSocket connection handling, send queue, message dispatch

pub mod api_handlers;
pub mod api_usage;
pub mod chat_dedup;
pub mod chat_event_log;
pub mod conv_router;
pub mod cors;
pub mod events;
pub mod handlers;
pub mod history;
pub mod history_filter;
pub mod internal;
pub mod llm_bridge;
// 入站 OpenAI 兼容端点（POST /v1/chat/completions）。
pub mod openai_compat;
pub mod pricing_sync;
// LLM 代理请求用量记账（裸转发 lane + /api/chat/stream 的落账出口；
// 代理请求用量记账）。
pub mod protocol;
// LLM 代理调用方 key 鉴权面（per-caller key 匹配 + mtime 缓存热生效；
// LLM代理调用方key管理）。
pub(crate) mod proxy_keys;
pub(crate) mod proxy_usage;
#[cfg(feature = "terminal")]
pub mod pty;
#[cfg(feature = "vm-display")]
pub mod vm_display;
// （集群无人值守硬化）：跨节点批复桥 master 侧（注册表 +
// 看板评论回源）。cluster 门（nemesis-cluster optional）。
pub mod relay;
#[cfg(feature = "cluster")]
pub mod remote_approval;
pub mod sdk_embed;
pub mod server;
pub mod session;
pub mod session_bindings;
// 版本自切换通道（selfupdate feature 门控 apply 编排；journal/check/boot
// 收口不门控——任何构建都要能清残留现场）。
pub mod selfupdate;
pub mod share;
#[cfg(feature = "skins")]
pub mod skins;
pub mod sse_chat;
// 语音 realtime 热态 + TTS 接力注册（无条件编译；真实现挂 voice feature）。
pub mod voice_relay;
pub mod websocket_handler;
#[cfg(feature = "workflow")]
pub mod workflow_chat;
#[cfg(feature = "workflow")]
pub mod workflow_chat_reply_observer;
pub mod ws_router;

pub use conv_router::ConvRouter;
pub use conv_router::SharedConvRouter;
pub use cors::CORSConfig;
pub use cors::CORSManager;
pub use events::EventHub;
#[cfg(feature = "forge")]
pub use llm_bridge::ForgeProviderBridge;
pub use llm_bridge::ProviderAdapter;
pub use protocol::ProtocolMessage;
pub use server::DirectoryStaticFiles;
pub use server::StaticFiles;
pub use server::WebServer;
pub use server::WebServerConfig;
pub use session::SessionManager;
pub use websocket_handler::IncomingMessage;
pub use ws_router::ModuleHandler;
pub use ws_router::RequestContext;
pub use ws_router::WsRouter;

// Previously-written extra coverage modules for the top-level handlers were
// never `mod`-declared, so they did not compile or run.
#[cfg(all(test, feature = "workflow"))]
mod api_usage_extra_tests;
// proxy_usage 单测（AppState 构造依赖 workflow 的 ChatSecretStore，闸同
// api_usage_extra_tests）。
#[cfg(all(test, feature = "forge"))]
mod llm_bridge_extra_tests;
#[cfg(all(test, feature = "workflow"))]
mod proxy_usage_extra_tests;
// proxy_keys 单测（AppState 构造依赖 workflow 的 ChatSecretStore，闸同
// proxy_usage_extra_tests）。
#[cfg(all(test, feature = "workflow"))]
mod proxy_keys_extra_tests;
// 入站过滤链 history 过滤器（BUG 项目会话历史加载修复）。
#[cfg(test)]
mod history_filter_tests;

// （改进项三期）：chat.send 写入侧幂等键——LRU 容量 / LRU
// touch / sidecar 持久化往返 / 损坏 sidecar 降级 / 纯图 seq=0 形态。
#[cfg(test)]
mod chat_dedup_tests;

// Home 单例测试竞态锁（test_home.rs 头注）：重定向 home 的测试与经单例
// 读写 chat_log 的测试全 crate 串行化。
#[cfg(test)]
mod test_home;

#[cfg(test)]
mod sse_chat_extra_tests;
#[cfg(all(test, feature = "workflow"))]
mod workflow_chat_extra_tests;

// (, quality-hardening goal 冲刺 web 批次 2): workflow_chat
// history unknown-index / happy-read arms + send unknown-index arm.
#[cfg(all(test, feature = "workflow"))]
mod workflow_chat_s10b_tests;

// (, quality-hardening goal 冲刺 web 批次 2): ProviderAdapter
// tool_calls/options/tools mapping arms + provider-Err arm (llm_bridge.rs
// itself is compiled unconditionally, so no feature gate needed).
#[cfg(test)]
mod llm_bridge_s10b_tests;

// wire 语义闸（/ 根修）：chat_streamed 尊重 stream_preferred
// 裁决 + 三条真实 lane 判据钉死。
#[cfg(test)]
mod llm_bridge_stream_gate_tests;
