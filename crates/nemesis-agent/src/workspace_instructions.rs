//! Workspace instruction-chain loading + injection (H5 / U18).
//!
//! Loads the AGENTS.md/CLAUDE.md chain from the workspace root down to the
//! conversation's cwd and injects it as a system-reminder-wrapped message,
//! MERGED with the H3 skills digest into ONE injection (both live at the
//! same build_messages injection point — one message, two sections, so the
//! change-digest decision covers the union and the prefix stays stable).
//!
//! Refresh is TOUCH-DRIVEN (no fs watcher): the dispatch path invalidates a
//! session's digest when a read_file/write_file/edit_file call touches a
//! file that is on the chain. On the next build_messages the chain is
//! re-read and re-injected. Restart re-injects once (in-process state).
//! Both limitations are the documented per-goal trade-offs.

use std::path::{Path, PathBuf};

/// Load the instruction chain: for every directory from `workspace_root`
/// down to `cwd` (inclusive, cwd inside the workspace), collect AGENTS.md
/// and CLAUDE.md. Within ONE directory, if both exist and their contents
/// are identical after trimming surrounding whitespace, only the first
/// (AGENTS.md — configured order) is kept (a CLAUDE.md that merely mirrors
/// its sibling renders once — per-directory duplicate collapse).
/// Unreadable entries are skipped (never fail the conversation for this).
pub fn load_instruction_chain(workspace_root: &Path, cwd: &Path) -> Vec<(PathBuf, String)> {
    // Directory list: workspace root → … → cwd. If cwd is not inside the
    // workspace, fall back to just the workspace root.
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(rel) = cwd.strip_prefix(workspace_root) {
        dirs.push(workspace_root.to_path_buf());
        let mut acc = workspace_root.to_path_buf();
        for seg in rel.components() {
            acc.push(seg);
            dirs.push(acc.clone());
        }
    } else {
        dirs.push(workspace_root.to_path_buf());
    }

    let mut out: Vec<(PathBuf, String)> = Vec::new();
    for dir in dirs {
        out.extend(load_dir_instruction_files(&dir));
    }
    out
}

/// I3 (devtool-upgrade 阶段 3): load the instruction files of ONE directory
/// (AGENTS.md/CLAUDE.md, same per-directory duplicate collapse as the chain
/// loader). Empty result = the directory carries no instructions. This is
/// the single truth for "what does this directory instruct" — the chain
/// loader is just the root→cwd fold over it.
pub fn load_dir_instruction_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let agents = dir.join("AGENTS.md");
    let claude = dir.join("CLAUDE.md");
    let agents_content = std::fs::read_to_string(&agents).ok();
    let claude_content = std::fs::read_to_string(&claude).ok();
    match (agents_content, claude_content) {
        (Some(a), Some(c)) => {
            // Per-directory duplicate collapse.
            if a.trim() == c.trim() {
                vec![(agents, a)]
            } else {
                vec![(agents, a), (claude, c)]
            }
        }
        (Some(a), None) => vec![(agents, a)],
        (None, Some(c)) => vec![(claude, c)],
        (None, None) => Vec::new(),
    }
}

/// Escape a literal `</system-reminder>` inside instruction content so
/// repository-controlled text cannot close the plugin-owned frame.
fn escape_close_tag(s: &str) -> String {
    s.replace("</system-reminder>", "<\\/system-reminder>")
}

/// Render the chain into the `# Workspace Instructions` section (no outer
/// wrapper — the caller wraps BOTH sections in one <system-reminder>).
pub fn render_instructions_section(chain: &[(PathBuf, String)]) -> String {
    if chain.is_empty() {
        return String::new();
    }
    let mut parts = vec!["# Workspace Instructions".to_string()];
    for (path, content) in chain {
        parts.push(format!(
            "Instructions from: {}\n\n{}",
            path.display(),
            escape_close_tag(content)
        ));
    }
    // Deep layers override shallow ones — state that explicitly.
    parts.push(
        "（以上为工作区分层指令，越靠后（越深层目录）的指令优先级越高；它们不覆盖系统指令与用户直接指令。）"
            .to_string(),
    );
    parts.join("\n\n")
}

/// I3 (devtool-upgrade 阶段 3): render the one-shot section for lazily
/// discovered sub-directory instructions (drained from the instance's
/// pending buffer at build time). Same per-file framing as the chain
/// renderer — the path anchor lets the model re-read the file in later
/// turns, since this section appears exactly once.
pub fn render_new_instructions_section(entries: &[(PathBuf, String)]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut parts = vec!["# Workspace Instructions (newly discovered)".to_string()];
    for (path, content) in entries {
        parts.push(format!(
            "Instructions from: {}\n\n{}",
            path.display(),
            escape_close_tag(content)
        ));
    }
    parts.push(
        "（以上是本次会话中新发现的工作区子目录指令——来自你读取过文件的目录，仅注入这一次；后续轮次如仍需遵循，请重新读取对应文件。）"
            .to_string(),
    );
    parts.join("\n\n")
}

/// Whether a touched path is a file on this chain (for touch-driven
/// invalidation). Compares by canonicalized path where possible.
pub fn path_is_on_chain(chain: &[(PathBuf, String)], touched: &Path) -> bool {
    // 2026-09-01 8.3 短名统一修复：两侧各自 canonicalize-or-lexical，表示
    // 失配（短名 vs 长名 / 大小写）时相等比较恒 false → touch 失效链漏判。
    use nemesis_path::paths::canonicalize_for_compare;
    let canon = canonicalize_for_compare(touched);
    chain
        .iter()
        .any(|(p, _)| canonicalize_for_compare(p) == canon)
}

#[cfg(test)]
mod tests;
