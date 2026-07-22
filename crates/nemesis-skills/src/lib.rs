//! NemesisBot - Skills System
//!
//! Skill loading, remote registry, linting, quality scoring, signing,
//! security checking, and management.

pub mod clawhub_registry;
pub mod github_registry;
pub mod github_tree;
pub mod installer;
pub mod lint;
pub mod loader;
pub mod mock_registry;
pub mod modelscope_registry;
pub mod quality;
pub mod registry;
pub mod search_cache;
pub mod security_check;
#[cfg(feature = "security")]
pub mod signer;
pub mod types;
