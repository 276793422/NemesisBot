//! NemesisBot - Cluster System
//!
//! Distributed cluster with node discovery, task management, and RPC protocol types.

pub mod actions_schema;
pub mod cluster;
pub mod cluster_config;
pub mod cluster_log;
pub mod cluster_log_reader;
pub mod cluster_task;
pub mod config_loader;
pub mod continuation_store;
pub mod diagnostics;
pub mod logger;
pub mod network;
pub mod registry;
pub mod rpc_types;
pub mod task_manager;
pub mod task_result_store;
pub mod types;

pub mod discovery;
pub mod handlers;
pub mod rpc;
pub mod transport;

// Re-export commonly used types
pub use cluster_task::{ClusterTask, ClusterTaskList, ClusterWorkQueue, TaskSource, TaskStatus};
pub use task_manager::{InMemoryTaskStore, TaskManager, TaskStore};
