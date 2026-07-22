//! WebSocket module - Internal WebSocket communication between parent and child processes.
//!
//! Provides Server, Client, Dispatcher, and Protocol types for
//! parent-child process communication using JSON-RPC 2.0 messages.

pub mod client;
pub mod dispatcher;
pub mod protocol;
pub mod server;

pub use client::WebSocketClient;
pub use dispatcher::{Dispatcher, HandlerFunc, NotificationFunc};
pub use protocol::{ErrorPayload, Message, VERSION};
pub use server::WebSocketServer;
