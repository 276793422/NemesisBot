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

// A1 iroh 跨网 P2P 叠加层（M4 起 cluster-roaming feature 已删，恒编译；
// 运行面在 plugin_roaming C-ABI 插件，缺席 = 诚实降级）。
pub mod roaming;

/// iroh 地址前缀（`iroh:<64位小写hex>`）——形态判定单一真相源。
///
/// 住 lib.rs（无 feature 门）：形态仲裁是纯字符串语义，未跑漫游的节点
/// 同样会**收到**漫游节点的 announce（混合版本农场），地址不得按 TCP 口径
/// 拼接损坏。`roaming::bridge` re-export 同一常量。
pub const IROH_ADDR_PREFIX: &str = "iroh:";

/// 是否 iroh 形态地址（前缀判断；拨号分派第一闸 + 地址仲裁禁拼 `:port` 依据）。
pub fn is_roaming_address(addr: &str) -> bool {
    addr.trim_start().starts_with(IROH_ADDR_PREFIX)
}

// Re-export commonly used types
pub use cluster_task::{
    CancelOutcome, ClusterTask, ClusterTaskList, ClusterWorkQueue, SELFCHECK_PROMPT_MARKER,
    TaskSource, TaskStatus,
};
pub use task_manager::{InMemoryTaskStore, TaskManager, TaskStore};
