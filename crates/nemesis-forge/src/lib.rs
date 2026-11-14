//! NemesisBot - Forge Self-Learning Framework
//!
//! Collector + Reflector + Registry + Sanitizer + Pipeline + Learning.

/// 手动/蒸馏注册表文件名（Forge 自有库；Forge::new 与闭环引擎跨库互查同源引用）。
pub const MANUAL_REGISTRY_FILE: &str = "registry.json";

/// 闭环学习注册表文件名（ctx.rs 装配点与本 crate 跨库互查同源引用）。
pub const LEARNING_REGISTRY_FILE: &str = "learning_registry.json";

pub mod bridge;
pub mod collector;
pub mod config;
pub mod cycle_store;
pub mod experience_store;
pub mod exporter;
pub mod forge;
pub mod forge_tools;
pub mod learning_engine;
pub mod mcp_installer;
pub mod monitor;
pub mod optimize;
pub mod pattern;
pub mod pipeline;
pub mod reflector;
pub mod reflector_llm;
pub mod registry;
pub mod replay;
pub mod report;
pub mod sanitizer;
pub mod skill_tests;
pub mod syncer;
pub mod test_runner;
pub mod trace;
pub mod trace_store;
pub mod types;
pub mod validator;

#[cfg(test)]
mod test_support;
