//! NemesisBot - Skills System
//!
//! Skill loading, remote registry, linting, quality scoring, signing,
//! security checking, and management.

pub mod types;
pub mod loader;
pub mod lint;
pub mod quality;
pub mod registry;
pub mod signer;
pub mod security_check;
pub mod installer;
pub mod clawhub_registry;
pub mod github_registry;
pub mod github_tree;
pub mod search_cache;
pub mod mock_registry;
