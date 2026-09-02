//! Language → language-server mapping + PATH probe (L1 / U19).
//!
//! One row per language family; each row names the server command the
//! manager spawns (`--stdio` servers get the flag baked into `args`).
//! The table is data — adding a language is one row, no code changes.
//!
//! Probe semantics follow the CC/Codex tool registration pattern: probe at
//! registration time, no server found for a language ⇒ that language is
//! simply unavailable (clear error on query; tool not registered at all
//! when NO server for ANY language exists).

use std::path::{Path, PathBuf};

/// Languages we can route to a server. One `Lang` per distinct server
/// process type (TypeScript covers .ts/.tsx/.js/.jsx — same
/// typescript-language-server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Go,
    TypeScript,
    Python,
    C,
}

impl Lang {
    pub fn label(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Go => "go",
            Lang::TypeScript => "typescript/javascript",
            Lang::Python => "python",
            Lang::C => "c/c++",
        }
    }
}

/// One language-server spec: which extensions route here and how to spawn.
pub struct ServerSpec {
    pub lang: Lang,
    pub extensions: &'static [&'static str],
    pub command: &'static str,
    pub args: &'static [&'static str],
}

/// The known-language table. Servers are the mainstream default for each
/// language; absent servers are probed away at registration/first use.
pub const SERVERS: &[ServerSpec] = &[
    ServerSpec {
        lang: Lang::Rust,
        extensions: &["rs"],
        command: "rust-analyzer",
        args: &[],
    },
    ServerSpec {
        lang: Lang::Go,
        extensions: &["go"],
        command: "gopls",
        args: &[],
    },
    ServerSpec {
        lang: Lang::TypeScript,
        extensions: &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        command: "typescript-language-server",
        args: &["--stdio"],
    },
    ServerSpec {
        lang: Lang::Python,
        extensions: &["py"],
        command: "pyright-langserver",
        args: &["--stdio"],
    },
    ServerSpec {
        lang: Lang::C,
        extensions: &["c", "h", "cpp", "hpp", "cc"],
        command: "clangd",
        args: &[],
    },
];

/// Route a file path to its language by extension. `None` = unsupported
/// file type (the tool surfaces a clear error listing the supported set).
pub fn lang_for_path(path: &Path) -> Option<Lang> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    SERVERS
        .iter()
        .find(|s| s.extensions.contains(&ext.as_str()))
        .map(|s| s.lang)
}

/// The server spec for a language (table lookup; always Some for a Lang
/// that came from [`lang_for_path`]).
pub fn spec_for(lang: Lang) -> Option<&'static ServerSpec> {
    SERVERS.iter().find(|s| s.lang == lang)
}

/// Locate a command on PATH (absolute path wins as-is). `None` = absent.
pub fn find_command(command: &str) -> Option<PathBuf> {
    // An explicit path (contains a separator) is used directly; `which`
    // handles PATHEXT resolution on Windows and x-bit checks on POSIX.
    which::which(command).ok()
}

/// Whether the server for `lang` is installed.
pub fn server_available(lang: Lang) -> bool {
    spec_for(lang).is_some_and(|spec| find_command(spec.command).is_some())
}

/// Probe which languages have an installed server. Called once at tool
/// registration (OnceLock-cached by the caller) — empty result ⇒ the LSP
/// tool is not registered at all.
pub fn probe_available() -> Vec<Lang> {
    SERVERS
        .iter()
        .map(|s| s.lang)
        .filter(|l| server_available(*l))
        .collect()
}

#[cfg(test)]
mod tests;
