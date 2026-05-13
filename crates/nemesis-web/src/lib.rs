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

pub mod server;
pub mod api_handlers;
pub mod cors;
pub mod events;
pub mod history;
pub mod protocol;
pub mod session;
pub mod sse_chat;
pub mod websocket_handler;

pub use events::EventHub;
pub use protocol::ProtocolMessage;
pub use session::SessionManager;
pub use server::WebServer;
pub use server::WebServerConfig;
pub use websocket_handler::IncomingMessage;
pub use cors::CORSConfig;
pub use cors::CORSManager;
