//! Process management module - Subprocess lifecycle, handshake, and platform abstraction.
//!
//! Provides the ProcessManager, PlatformExecutor, and Handshake subsystems
//! for managing child processes in desktop mode.

pub mod executor;
pub mod handshake;
pub mod manager;

pub use executor::{ChildProcess, ExecutorConfig, PlatformExecutor, ProcessStatus};
pub use handshake::{HandshakeResult, PROTOCOL_VERSION, PipeMessage};
pub use manager::ProcessManager;
