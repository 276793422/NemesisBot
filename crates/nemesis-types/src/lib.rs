//! NemesisBot - Core shared types
//!
//! This crate defines all shared types, traits, error types, and constants
//! used across the NemesisBot system.

pub mod error;
pub mod channel;
pub mod agent;
pub mod security;
pub mod memory;
pub mod workflow;
pub mod tools;
pub mod cluster;
pub mod forge;
pub mod provider;
pub mod config;
pub mod traits;
pub mod constants;
pub mod utils;
pub mod capability;

// Re-export commonly used constants
pub use constants::{
    BUS_CHANNEL_CAPACITY, CLEANUP_INTERVAL_SECS, CLUSTER_CONTINUATION_PREFIX,
    CLUSTER_DIR, CONFIG_FILE, DEFAULT_MAX_CONTEXT_TOKENS, DEFAULT_MAX_ITERATIONS,
    FORGE_DIR, IDENTITY_FILE, INTERNAL_CHANNELS, PEER_CHAT_TIMEOUT_SECS,
    RPC_CHANNEL_TIMEOUT_SECS, RPC_CLIENT_TIMEOUT_SECS, RPC_PREFIX,
    SCANNER_CONFIG_FILE, SKILLS_DIR, SOUL_FILE, USER_FILE,
    WORKSPACE_DIR, is_internal_channel,
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
