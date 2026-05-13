//! Transport sub-module for low-level TCP connection management.
//!
//! Provides:
//! - `frame` — Length-prefixed binary framing (sync + async)
//! - `conn` — TCP connection wrappers (`Connection` sync, `TcpConn` async, `WireMessage`)
//! - `pool` — Connection pools (`ConnectionPool` sync, `Pool` async)
//! - `rpc_transport` — High-level RPC transport over pool

pub mod conn;
pub mod frame;
pub mod pool;
pub mod rpc_transport;

// Sync types (backward-compatible)
pub use conn::Connection;
pub use frame::TransportFrame;
pub use pool::ConnectionPool;

// Async types (full Go feature parity)
pub use conn::{TcpConn, TcpConnConfig, WireMessage};
pub use frame::{AsyncFrameReader, MAX_FRAME_SIZE};
pub use pool::{AsyncPoolConfig, Pool, PoolStats};
pub use rpc_transport::RpcTransport;
