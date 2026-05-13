//! ClamAV integration modules.
//!
//! Provides ClamAV client, daemon management, scanner, updater, config
//! generation, and security pipeline hook.

pub mod client;
pub mod config;
pub mod daemon;
pub mod manager;
pub mod scanner;
pub mod updater;
pub mod hook;
