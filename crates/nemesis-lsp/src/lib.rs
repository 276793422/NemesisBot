//! nemesis-lsp: minimal LSP client (L1 / U19; C7 adds the write-capable ops).
//!
//! Gives the agent four semantic code queries — definition / references /
//! implementation / hover — plus rename and code-action listing (C7) by
//! driving real language servers (rust-analyzer, gopls,
//! typescript-language-server, pyright, clangd) over stdio. Rename stops at
//! computed per-file new text: writing to disk is the agent tool's job, so
//! every write crosses the security pipeline (8-layer gate) unchanged.
//!
//! Lifecycle model: one server process per (language, project root), spawned
//! on first query and cached. Idle sessions are reaped lazily (checked on
//! each query — no background thread), and `shutdown_all` closes everything
//! gracefully (`shutdown` request → `exit` notification → kill).
//!
//! Registration semantics (the delegation-tool PATH-probe pattern) live in the agent
//! layer: the tool is registered only when config opts in AND at least one
//! language server exists on PATH.

pub mod install;
pub mod manager;
pub mod proto;
pub mod registry;

pub use manager::{AppliedFileEdit, LspManager, LspOp, RenameOutcome};
