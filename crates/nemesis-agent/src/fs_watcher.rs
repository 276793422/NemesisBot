//! I1 (devtool-upgrade 阶段 3): workspace fs watcher — notify-based
//! recursive watch over the workspace root, with debounce + ignore table.
//!
//! Two consumers (both wired through `AgentLoop::start_fs_watcher`):
//! 1. **Instruction chain** — a create/modify/remove of any AGENTS.md or
//!    CLAUDE.md under the workspace calls `invalidate_context_digests()`
//!    (round-5 note: digest state is stateless — sections re-read from disk
//!    on every build — so external instruction edits already surface next
//!    build; the invalidate call is the kept anchor, same shape as the
//!    dispatch path's touch-driven H5 call).
//! 2. **External-change notice** — other Create/Modify events land in the
//!    loop's `external_changes` buffer and surface as a one-shot
//!    `<external_changes>` section in the next context snapshot.
//!
//! Noise discipline (why this doesn't drown the model in self-inflicted
//! notices):
//! - **Ignore table** drops runtime-owned paths (`logs/`, `target/`,
//!   `.git/`, `*.jsonl`, …) — the agent's own spill/session/checkpoint
//!   writes never surface.
//! - **Self-write window** — the dispatch path records every successful
//!   `write_file`/`edit_file` path; watcher events for the same path within
//!   [`SELF_WRITE_WINDOW`] are dropped (the agent knows what it just
//!   wrote). Honest hole: `exec`-driven writes (shell redirects) can't be
//!   attributed and may still surface — bounded by the per-flush cap.
//! - **Caps** — ≤10 paths per flush, ≤32 buffered between builds.
//! - **One-shot** — the buffer drains on the next build.
//!
//! Failure policy: startup failure (watch handle exhaustion, permission)
//! is returned to the caller which warns ONCE and never retries — a broken
//! watcher must not degrade the loop.

use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

/// Quiescence window before a batch of events flushes (reset on every
/// event). Tests use a shorter window via [`start_with_debounce`].
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Max external-change paths surfaced per flush (plan: ≤10 条).
pub const MAX_EXTERNAL_CHANGES: usize = 10;

/// Max paths buffered on the loop between builds (several flushes' worth —
/// the drain is one-shot, so a burst between two turns is bounded).
pub const MAX_BUFFERED_CHANGES: usize = 32;

/// How long after an agent-authored write the watcher's event for the same
/// path is treated as self-inflicted (dropped, not surfaced).
pub const SELF_WRITE_WINDOW: Duration = Duration::from_secs(20);

/// Instruction-file callback (consumer 1).
pub type Callback = Arc<dyn Fn() + Send + Sync>;
/// External-change callback, receives the workspace-relative path.
pub type PathCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Whether a WORKSPACE-RELATIVE path is dropped by the ignore table.
/// Callers must strip the workspace root first (a workspace that happens to
/// live under a dir named `target` must not be ignored wholesale).
///
/// Built-ins delegate to the SHARED truth `nemesis_path::is_workspace_ignored`
/// (I2 收敛：watch 过滤与 @补全遍历用同一张表，不再各写一份漂移)；
/// `extra` carries user-configured rules (`FsWatcherConfig.ignore`), where
/// bare names match any component and `*.ext` matches by extension.
pub fn is_ignored(rel: &Path, extra: &[String]) -> bool {
    if nemesis_path::is_workspace_ignored(rel) {
        return true;
    }
    extra.iter().any(|rule| {
        let rule = rule.as_str();
        if let Some(ext) = rule.strip_prefix("*.") {
            rel.extension().and_then(|e| e.to_str()) == Some(ext)
        } else {
            rel.components()
                .any(|c| c.as_os_str().to_string_lossy() == rule)
        }
    })
}

/// Whether the path is an instruction-chain file (any depth — the recursive
/// watch covers the chain root → cwd layers automatically).
fn is_instruction_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("AGENTS.md") | Some("CLAUDE.md")
    )
}

/// Normalize a workspace-relative path for self-write matching: forward
/// slashes + lowercase (Windows is case-insensitive; the watcher and the
/// tool args may disagree on separators/case).
pub fn normalize_workspace_rel(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

/// Render the one-shot external-changes section for the merged context
/// snapshot (plan shape: `<external_changes>` tag + hint line).
pub fn render_external_changes_section(paths: &[String]) -> String {
    let list = paths
        .iter()
        .map(|p| format!("- {}", p))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "<external_changes>\n外部修改了以下文件（工作区内、非本会话操作）：\n{list}\n</external_changes>\n(外部工具或用户在会话进行中修改了这些文件；如需最新内容请重新读取，不要凭旧记忆作答。)"
    )
}

/// Keep-alive handle: dropping it shuts the watcher down (the notify handle
/// is dropped and the debouncer thread's channel disconnects).
pub struct WatcherHandle {
    _watcher: notify::RecommendedWatcher,
    _tx: mpsc::Sender<(PathBuf, bool)>,
}

#[derive(Clone, Copy, PartialEq)]
enum EvClass {
    Change,
    Remove,
}

/// Start a watcher over `root` (recursive). `Ok(None)` when disabled by
/// config; `Err` on startup failure (caller warns once + gives up).
pub fn start(
    root: &Path,
    cfg: &nemesis_config::FsWatcherConfig,
    on_instruction_change: Callback,
    on_external_change: PathCallback,
) -> Result<Option<WatcherHandle>, String> {
    start_with_debounce(
        root,
        cfg,
        DEFAULT_DEBOUNCE,
        on_instruction_change,
        on_external_change,
    )
}

/// [`start`] with an explicit debounce window (tests shrink it).
pub fn start_with_debounce(
    root: &Path,
    cfg: &nemesis_config::FsWatcherConfig,
    debounce: Duration,
    on_instruction_change: Callback,
    on_external_change: PathCallback,
) -> Result<Option<WatcherHandle>, String> {
    if !cfg.enabled {
        return Ok(None);
    }
    let (tx, rx) = mpsc::channel::<(PathBuf, bool)>();
    let root = root.to_path_buf();
    let watch_root = root.clone();
    let extra = cfg.ignore.clone();
    let event_tx = tx.clone();
    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            let Ok(event) = res else { return };
            // Classify once per event; Access/Other kinds are noise.
            let class = match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) => EvClass::Change,
                EventKind::Remove(_) => EvClass::Remove,
                _ => return,
            };
            for path in event.paths {
                let Ok(rel) = path.strip_prefix(&root).map(|r| r.to_path_buf()) else {
                    continue; // outside the workspace — not ours
                };
                if is_ignored(&rel, &extra) {
                    continue;
                }
                // Drop non-file targets at ingest when they still exist (dirs,
                // created-then-deleted transients). Removed paths can't be
                // stat'ed — kept for the instruction chain (deletion invalidates).
                if class == EvClass::Change && !path.is_file() {
                    continue;
                }
                let _ = event_tx.send((rel, class == EvClass::Remove));
            }
        })
        .map_err(|e| format!("fs watcher init failed: {}", e))?;
    watcher
        .watch(&watch_root, RecursiveMode::Recursive)
        .map_err(|e| format!("fs watcher watch({}) failed: {}", watch_root.display(), e))?;

    // Debouncer: reset-on-event quiescence window; flush dedupes paths.
    std::thread::Builder::new()
        .name("fs-watcher-debounce".into())
        .spawn(move || {
            let mut pending: Vec<(PathBuf, bool)> = Vec::new();
            let mut seen: HashSet<PathBuf> = HashSet::new();
            loop {
                match rx.recv_timeout(debounce) {
                    Ok((path, removed)) => {
                        if seen.insert(path.clone()) {
                            pending.push((path, removed));
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if pending.is_empty() {
                            continue; // steady-state tick, nothing to flush
                        }
                        flush(&mut pending, &on_instruction_change, &on_external_change);
                        seen.clear();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|e| format!("fs watcher thread spawn failed: {}", e))?;

    Ok(Some(WatcherHandle {
        _watcher: watcher,
        _tx: tx,
    }))
}

/// Drain one debounced batch: instruction files → consumer 1 (once), other
/// changed files → consumer 2 (capped). Removed non-instruction files are
/// NOT surfaced (a deleted file isn't "modified content" — the model re-read
/// would fail anyway).
fn flush(
    pending: &mut Vec<(PathBuf, bool)>,
    on_instruction_change: &Callback,
    on_external_change: &PathCallback,
) {
    let mut instruction = false;
    let mut external: Vec<String> = Vec::new();
    for (rel, removed) in pending.drain(..) {
        if is_instruction_file(&rel) {
            instruction = true;
        } else if !removed && external.len() < MAX_EXTERNAL_CHANGES {
            external.push(rel.display().to_string());
        }
    }
    if instruction {
        tracing::info!(
            "[fs_watcher] instruction-chain file changed externally — invalidating context digests"
        );
        on_instruction_change();
    }
    for p in external {
        on_external_change(&p);
    }
}

#[cfg(test)]
mod tests;
