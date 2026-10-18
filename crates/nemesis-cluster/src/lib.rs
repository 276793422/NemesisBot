//! NemesisBot - Cluster System
//!
//! Distributed cluster with node discovery, task management, and RPC protocol types.

pub mod actions_schema;
pub mod changeset;
pub mod cluster;
pub mod cluster_config;
pub mod cluster_log;
pub mod cluster_log_reader;
pub mod cluster_task;
pub mod config_loader;
pub mod continuation_store;
pub mod diagnostics;
pub mod envelope;
pub mod exec_workspace;
pub(crate) mod fsutil;
pub mod logger;
pub mod network;
pub mod outbox;
pub mod pair;
pub mod registry;
pub mod rpc_types;
pub mod task_manager;
pub mod task_result_store;
mod task_store_sqlite;
pub mod transfer;
pub mod types;

pub mod discovery;
pub mod handlers;
pub mod rpc;
pub mod transport;

// A1 iroh 跨网 P2P 叠加层（feature=cluster-roaming，默认关——LAN 用户零新依赖）。
#[cfg(feature = "cluster-roaming")]
pub mod roaming;

// Re-export commonly used types
pub use cluster_task::{
    CancelOutcome, ClusterTask, ClusterTaskList, ClusterWorkQueue, SELFCHECK_PROMPT_MARKER,
    TaskSource, TaskStatus,
};
pub use task_manager::{InMemoryTaskStore, TaskManager, TaskStore};
