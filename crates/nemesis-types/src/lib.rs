//! NemesisBot - Core shared types
//!
//! This crate defines all shared types, traits, error types, and constants
//! used across the NemesisBot system.

pub mod agent;
pub mod approval_journal;
pub mod capability;
pub mod channel;
pub mod cluster;
pub mod config;
pub mod constants;
pub mod error;
pub mod forge;
pub mod memory;
pub mod provider;
pub mod security;
pub mod tools;
pub mod traits;
pub mod utils;
pub mod workflow;

// Re-export commonly used constants
// （BUG-20261029-184：零消费死常量已删除；运行参数唯一真相源在各子系统，
// 见 constants.rs 模块注释。）
pub use constants::{
    BUS_CHANNEL_CAPACITY, CLEANUP_INTERVAL_SECS, CLUSTER_CONTINUATION_PREFIX, INTERNAL_CHANNELS,
    RPC_CHANNEL_TIMEOUT_SECS, is_internal_channel,
};

// Test modules
#[cfg(test)]
mod tests;

#[cfg(test)]
mod additional_tests;

#[cfg(test)]
mod constants_tests;

#[cfg(test)]
mod memory_tests;
