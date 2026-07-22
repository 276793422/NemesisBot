//! NemesisBot - MCP (Model Context Protocol)
//!
//! JSON-RPC based protocol implementation for MCP tool integration.
//! Provides both a client (to connect to external MCP servers) and a server
//! (to expose local tools via the MCP protocol).

pub mod adapter;
pub mod client;
pub mod http_transport;
pub mod manager;
pub mod server;
pub mod stdio_transport;
pub mod transport;
pub mod types;
