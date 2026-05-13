//! WebSocket module - Internal WebSocket communication between parent and child processes.
//!
//! Provides Server, Client, Dispatcher, and Protocol types for
//! parent-child process communication using JSON-RPC 2.0 messages.

pub mod protocol;
pub mod dispatcher;
pub mod server;
pub mod client;

pub use protocol::{Message, ErrorPayload, VERSION};
pub use dispatcher::{Dispatcher, HandlerFunc, NotificationFunc};
pub use server::WebSocketServer;
pub use client::WebSocketClient;
