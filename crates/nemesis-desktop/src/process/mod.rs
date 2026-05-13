//! Process management module - Subprocess lifecycle, handshake, and platform abstraction.
//!
//! Provides the ProcessManager, PlatformExecutor, and Handshake subsystems
//! for managing child processes in desktop mode.

pub mod executor;
pub mod manager;
pub mod handshake;

pub use executor::{PlatformExecutor, ExecutorConfig, ChildProcess, ProcessStatus};
pub use manager::ProcessManager;
pub use handshake::{HandshakeResult, PipeMessage, PROTOCOL_VERSION};
