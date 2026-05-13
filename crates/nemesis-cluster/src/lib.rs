//! NemesisBot - Cluster System
//!
//! Distributed cluster with node discovery, task management, and RPC protocol types.

pub mod types;
pub mod cluster;
pub mod cluster_config;
pub mod task_manager;
pub mod rpc_types;
pub mod actions_schema;
pub mod config_loader;
pub mod continuation_store;
pub mod logger;
pub mod network;
pub mod registry;
pub mod task_result_store;

pub mod discovery;
pub mod handlers;
pub mod rpc;
pub mod transport;

// Re-export commonly used types
pub use task_manager::{TaskManager, TaskStore, InMemoryTaskStore};
