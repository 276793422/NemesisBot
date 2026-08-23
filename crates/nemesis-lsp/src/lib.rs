//! nemesis-lsp: minimal read-only LSP client (L1 / U19).
//!
//! Gives the agent four semantic code queries — definition / references /
//! implementation / hover — by driving real language servers (rust-analyzer,
//! gopls, typescript-language-server, pyright, clangd) over stdio. Read-only
//! by construction: no didOpen/didChange, no edits, no commands.
//!
//! Lifecycle model: one server process per (language, project root), spawned
//! on first query and cached. Idle sessions are reaped lazily (checked on
//! each query — no background thread), and `shutdown_all` closes everything
//! gracefully (`shutdown` request → `exit` notification → kill).
//!
//! Registration semantics (the CC/Codex probe pattern) live in the agent
//! layer: the tool is registered only when config opts in AND at least one
//! language server exists on PATH.

pub mod manager;
pub mod proto;
pub mod registry;

pub use manager::{LspManager, LspOp};
