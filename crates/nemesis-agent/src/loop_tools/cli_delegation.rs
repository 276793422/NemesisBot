//! T5 (U13 / A7+A9): shared CLI-delegation execution layer.
//!
//! H7's ClaudeCodeTool and I4's CodexTool grew ~150 lines of byte-identical
//! spawn/timeout/tree-kill/CREATE_NO_WINDOW/structured-error plumbing (the A9
//! duplication debt). This module is the single home for that logic; the two
//! tools shrink to "argument construction + config" shells around
//! [`run_cli_delegation`].
//!
//! T5 also adds the FIXED (non-interactive) permission tiers the original U13
//! item required — config-driven, NEVER model-selectable (absent from the
//! tool schema on purpose):
//! - claude: `agents.claude_code_tool.permission_mode` → `--permission-mode`
//!   (CLI camelCase enum acceptEdits/auto/bypassPermissions/manual/dontAsk/
//!   plan, default acceptEdits; legacy snake_case config values map over)
//! - codex: `agents.codex_tool.sandbox` → `--sandbox <kebab>` (enum
//!   read_only/workspace_write/danger_full_access, default read_only) +
//!   implied `--ask-for-approval never`

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// One delegation request: everything needed to spawn the CLI. The arg
/// vector is pre-assembled by the caller (the tool shell) — this layer does
/// NOT know about tool-specific flags.
pub struct CliDelegationSpec<'a> {
    /// Resolved CLI path (already probed by the tool shell).
    pub cli: &'a str,
    /// Human label for error messages, e.g. "claude CLI".
    pub cli_label: &'a str,
    /// Full argument vector, including the prompt and permission flags.
    pub args: Vec<String>,
    /// Wall-clock budget in seconds.
    pub timeout_secs: u64,
    /// Working directory for the child.
    pub cwd: &'a Path,
}

/// Unified CLI delegation runner: spawn → collect output → timeout with
/// process-tree kill → structured errors. Extracted verbatim from the two
/// per-tool copies (A9).
pub async fn run_cli_delegation(spec: CliDelegationSpec<'_>) -> Result<String, String> {
    let mut cmd = Command::new(spec.cli);
    // Timeout safety: if tokio::time::timeout drops the output() future
    // at the deadline, the spawned child must die with it — without
    // kill_on_drop the CLI process would outlive the tool call as an
    // orphan (second-pass review fix).
    cmd.kill_on_drop(true);
    cmd.args(&spec.args)
        .current_dir(spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // Never open a console window (project background-process rule).
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    // Spawn explicitly so a timeout KILLS THE PROCESS TREE: killing
    // only the direct child (bat wrapper) leaves grandchildren holding
    // the inherited stdout/stderr pipes open, and output() futures hang
    // on pipe close until every holder exits (Windows inheritance
    // classic — this is why kill_on_drop alone did not shorten the
    // timeout path).
    let child = cmd
        .spawn()
        .map_err(|e| format!("Error: failed to spawn {}: {}", spec.cli_label, e))?;
    // Capture the pid BEFORE moving `child` into wait_with_output (the
    // timeout arm needs it for the tree kill — Windows only; POSIX arm above
    // relies on kill_on_drop, hence the cfg_attr to keep non-Windows builds
    // warning-free).
    #[cfg_attr(not(windows), allow(unused_variables))]
    let child_pid = child.id();
    let out = match tokio::time::timeout(
        Duration::from_secs(spec.timeout_secs),
        child.wait_with_output(),
    )
    .await
    {
        Ok(r) => r.map_err(|e| format!("Error: {} output wait failed: {}", spec.cli_label, e))?,
        Err(_) => {
            #[cfg(windows)]
            {
                // Tree-kill (the CLI may spawn workers that inherit the
                // pipes).
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new("taskkill")
                    .args([
                        "/PID",
                        &child_pid.unwrap_or_default().to_string(),
                        "/T",
                        "/F",
                    ])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(0x0800_0000)
                    .status();
            }
            #[cfg(not(windows))]
            {
                // POSIX: `child` was moved into wait_with_output;
                // kill_on_drop(true) (set at spawn) kills it when the
                // dropped future's Child reaper runs. Worker reaping is
                // the CLI's own responsibility on POSIX.
            }
            return Err(format!(
                "Error: {} delegation timed out after {}s",
                spec.cli_label, spec.timeout_secs
            ));
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Ok(format!(
            "Error: {} exited with {}.\nstdout:\n{}\nstderr:\n{}",
            spec.cli_label, out.status, stdout, stderr
        ));
    }
    Ok(stdout.trim().to_string())
}

/// Resolve the cwd for a delegation: the session_key is NOT a path in
/// general (e.g. "web:chat123"), so only use it as a cwd base when its
/// parent actually exists on disk; otherwise fall back to the process cwd.
pub fn delegation_cwd(session_key: &str) -> PathBuf {
    let p = Path::new(session_key).parent().map(|p| p.to_path_buf());
    match p {
        Some(dir) if dir.is_dir() => dir,
        _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    }
}

/// Locate a CLI on PATH: `where <name>` (Windows) / `which <name>`.
/// Returns the resolved path, or None when not installed.
///
/// Windows nuance (V4 真机揭的 bug，2026-08-23）：npm 风格安装会在 PATH 上
/// 同时落一个**无扩展名的 POSIX sh shim** 和一个 `.cmd` 包装（`where claude`
/// 先吐 shim）。`Command::new(<无扩展名>)` 在 Windows 上必失败（os error 193
/// "不是有效的 Win32 应用程序"）——所以这里不能盲取第一行，要优先挑带
/// Windows 可执行扩展名的候选。
pub fn find_cli_on_path(name: &str) -> Option<String> {
    let finder = if cfg!(windows) { "where" } else { "which" };
    let out = std::process::Command::new(finder)
        .arg(name)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = s
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let first = lines.first().copied()?;
    #[cfg(windows)]
    {
        if let Some(picked) = pick_windows_exec_candidate(&lines) {
            return Some(picked);
        }
    }
    // 非 Windows（或 Windows 上没有任何带可执行扩展名的候选——保留旧行为，
    // 让 spawn 错误如实暴露而不是误报"未安装"）。
    Some(first.to_string())
}

/// Windows 可执行扩展名（`where` 候选筛选用）。
#[cfg(windows)]
const WINDOWS_EXEC_EXTENSIONS: [&str; 4] = ["exe", "cmd", "bat", "com"];

/// 从 `where` 输出行里挑第一个带 Windows 可执行扩展名的候选；
/// 全都不带扩展名（如纯 shim 布局）则 None（调用方回退第一行）。
#[cfg(windows)]
fn pick_windows_exec_candidate(lines: &[&str]) -> Option<String> {
    lines
        .iter()
        .find(|l| {
            std::path::Path::new(l)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| WINDOWS_EXEC_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
        .map(|l| l.to_string())
}

// ---------------------------------------------------------------------------
// T5 fixed permission tiers (config-driven, NOT model-selectable)
// ---------------------------------------------------------------------------

/// Valid claude `--permission-mode` values — the CLI's own camelCase set
/// (verified against claude CLI 2.1.240 `--help`: acceptEdits / auto /
/// bypassPermissions / manual / dontAsk / plan). V4 真机揭的第二个 bug
/// (2026-08-23)：T5 原来硬编码 snake_case（default/accept_edits/...），
/// 真实 CLI 直接拒收 `accept_edits`（exit 2 "invalid choice"），委派从未
/// 在真机上跑通过——mock 测试钉的是错误假设。Default `acceptEdits` —
/// the non-interactive-safe tier.
pub const CC_PERMISSION_MODES: [&str; 6] = [
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "manual",
    "dontAsk",
    "plan",
];
pub const CC_PERMISSION_MODE_DEFAULT: &str = "acceptEdits";

/// Legacy snake_case aliases (T5 era config values / old docs). The feature
/// never worked end-to-end with these (the CLI rejects them), but configs in
/// the wild may carry them — map instead of silently degrading to default.
const CC_PERMISSION_MODE_LEGACY_ALIASES: [(&str, &str); 4] = [
    ("default", "acceptEdits"),
    ("accept_edits", "acceptEdits"),
    ("bypass_permissions", "bypassPermissions"),
    ("plan", "plan"),
];

/// Normalize a configured claude permission mode: exact camelCase match wins;
/// legacy snake_case maps over; empty/unknown → default (fail-safe, matching
/// the crate's graceful config-degradation style).
pub fn resolve_cc_permission_mode(cfg: &str) -> &'static str {
    if let Some(m) = CC_PERMISSION_MODES.iter().find(|m| **m == cfg) {
        return m;
    }
    if let Some((_, mapped)) = CC_PERMISSION_MODE_LEGACY_ALIASES
        .iter()
        .find(|(old, _)| *old == cfg)
    {
        return CC_PERMISSION_MODES
            .iter()
            .find(|m| *m == mapped)
            .unwrap_or(&CC_PERMISSION_MODE_DEFAULT);
    }
    CC_PERMISSION_MODE_DEFAULT
}

/// Valid codex sandbox tiers (config side, snake_case).
pub const CODEX_SANDBOX_MODES: [&str; 3] = ["read_only", "workspace_write", "danger_full_access"];
pub const CODEX_SANDBOX_DEFAULT: &str = "read_only";

/// Normalize a configured codex sandbox tier: empty/unknown → default.
pub fn resolve_codex_sandbox(cfg: &str) -> &'static str {
    match CODEX_SANDBOX_MODES.iter().find(|m| **m == cfg) {
        Some(m) => m,
        None => CODEX_SANDBOX_DEFAULT,
    }
}

/// Map a validated snake_case sandbox tier to codex's kebab-case CLI value
/// (`--sandbox read-only|workspace-write|danger-full-access`). Anything not
/// in the enum maps to the default tier's CLI form (callers pass an
/// already-normalized value in production; the fallback covers raw strings).
pub fn codex_sandbox_kebab(snake: &str) -> &'static str {
    match snake {
        "workspace_write" => "workspace-write",
        "danger_full_access" => "danger-full-access",
        _ => "read-only",
    }
}

#[cfg(test)]
pub(crate) mod tests;
