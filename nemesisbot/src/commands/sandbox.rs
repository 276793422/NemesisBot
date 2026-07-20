//! Sandbox command — manage the Sandboxie driver + service install.
//!
//! L2.0 scope: `install` / `uninstall` / `status`. Does NOT touch the executor
//! yet (that's L2.1+: named-pipe transport + Start.exe spawn). See
//! `docs/PLAN/2026-07-09_sandboxie-integration.md`.
//!
//! Install/uninstall need admin (KmdUtil opens SC_MANAGER_CREATE_SERVICE). The
//! non-elevated flow re-launches self elevated via ShellExecuteW("runas") with
//! a hidden `--internal` flag; the elevated child runs KmdUtil synchronously;
//! the parent polls `service_state` to confirm the side effect (relaunch is
//! fire-and-forget — no exit code).

use std::time::Duration;

use anyhow::Result;
use clap::Subcommand;

use crate::common;
use nemesis_sandbox::status::ServiceState;

#[derive(Subcommand, Debug)]
pub enum SandboxCommand {
    /// Download + extract the Sandboxie runtime files (no admin / no UAC — just files).
    /// Use `start` to activate the engine (install driver + service).
    Install,
    /// Deactivate the engine: stop + uninstall the driver + service (needs admin → UAC).
    /// --purge also deletes the acquired files (full removal).
    Stop {
        #[arg(long, hide = true)]
        internal: bool,
        #[arg(long)]
        purge: bool,
    },
    /// Show Sandboxie install / service status.
    Status,
    /// List pending workspace files in the box (written by the sandboxed executor,
    /// not yet committed to real disk).
    Pending,
    /// Commit pending files from the box to the real workspace.
    Commit {
        /// Commit ALL pending workspace files.
        #[arg(long)]
        all: bool,
        /// Commit only files whose real path contains one of these (case-insensitive).
        /// Ignored when --all is set.
        files: Vec<String>,
    },
    /// Delete the box's contents (discard pending). Asks before discarding if
    /// there are pending workspace files; --force skips the prompt.
    Clear {
        #[arg(long)]
        force: bool,
    },
    /// Activate the engine: install driver + service + write ini + start SbieSvc.
    /// Needs admin (kernel driver) → triggers UAC. Requires `install` (files) first.
    Start {
        #[arg(long, hide = true)]
        internal: bool,
    },
}

pub async fn run(action: SandboxCommand, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    match action {
        SandboxCommand::Install => install(&paths).await,
        SandboxCommand::Stop { internal, purge } => stop(&paths, local, internal, purge),
        SandboxCommand::Status => status(&paths),
        SandboxCommand::Pending => pending(&paths, local),
        SandboxCommand::Commit { all, files } => commit(&paths, local, all, files),
        SandboxCommand::Clear { force } => clear(&paths, local, force),
        SandboxCommand::Start { internal } => start(&paths, local, internal),
    }
}

// ---------------------------------------------------------------------------
// pending / commit / clear — manual workspace-commit (L2.3)
// ---------------------------------------------------------------------------

/// %USERPROFILE% — the box's `user/<marker>/` subtree maps here.
fn user_profile() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir())
        .expect("USERPROFILE / home dir")
}

/// The workspace whose subtree is committable (matches what the gateway uses).
fn workspace_dir(local: bool) -> std::path::PathBuf {
    common::resolve_home(local).join("workspace")
}

fn format_size(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{}K", n / 1024)
    } else {
        format!("{}M", n / (1024 * 1024))
    }
}

fn pending(paths: &nemesis_sandbox::SandboxPaths, local: bool) -> Result<()> {
    let ws = workspace_dir(local);
    let up = user_profile();
    let pending =
        nemesis_sandbox::pending::pending_workspace(&paths.box_root, &ws, &up)?;
    if pending.is_empty() {
        println!(
            "No pending workspace files in box {}.",
            paths.box_root.display()
        );
        return Ok(());
    }
    println!("Pending workspace files ({}):", pending.len());
    for p in &pending {
        let rel = p.real_path.strip_prefix(&ws).unwrap_or(&p.real_path);
        println!("  {:>8}  {}", format_size(p.size), rel.display());
    }
    println!("\nCommit with: nemesisbot sandbox commit --all");
    Ok(())
}

fn commit(
    paths: &nemesis_sandbox::SandboxPaths,
    local: bool,
    all: bool,
    files: Vec<String>,
) -> Result<()> {
    let ws = workspace_dir(local);
    let up = user_profile();
    let pending =
        nemesis_sandbox::pending::pending_workspace(&paths.box_root, &ws, &up)?;
    if pending.is_empty() {
        println!("No pending workspace files to commit.");
        return Ok(());
    }
    let to_commit: Vec<&nemesis_sandbox::pending::PendingFile> = if all {
        pending.iter().collect()
    } else {
        let needles: Vec<String> = files.iter().map(|s| s.to_lowercase()).collect();
        pending
            .iter()
            .filter(|p| {
                let rp = p.real_path.to_string_lossy().to_lowercase();
                needles.iter().any(|n| rp.contains(n))
            })
            .collect()
    };
    if to_commit.is_empty() {
        println!("No pending files matched. Use --all or check `nemesisbot sandbox pending`.");
        return Ok(());
    }
    let mut total = 0u64;
    let mut ok = 0usize;
    for p in &to_commit {
        match nemesis_sandbox::pending::commit_file(p) {
            Err(e) => println!("  FAILED {}: {e}", p.real_path.display()),
            Ok(n) => {
                total += n;
                ok += 1;
                println!("  committed {} ({} bytes)", p.real_path.display(), n);
            }
        }
    }
    println!("Committed {ok}/{} file(s), {} bytes.", to_commit.len(), total);
    Ok(())
}

fn clear(paths: &nemesis_sandbox::SandboxPaths, local: bool, force: bool) -> Result<()> {
    use std::io::Write as _;

    let ws = workspace_dir(local);
    let up = user_profile();
    let pending =
        nemesis_sandbox::pending::pending_workspace(&paths.box_root, &ws, &up)?;
    if !pending.is_empty() && !force {
        println!(
            "{} pending workspace file(s) will be LOST when the box is cleared.",
            pending.len()
        );
        print!(
            "Commit all before clearing? [y=commit+clear / n=clear-without-commit / a=abort] (default a): "
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        match line.trim().to_lowercase().as_str() {
            "y" => {
                let mut n = 0usize;
                for p in &pending {
                    if nemesis_sandbox::pending::commit_file(p).is_ok() {
                        n += 1;
                    }
                }
                println!("Committed {n}/{} before clearing.", pending.len());
            }
            "n" => { /* clear without commit */ }
            _ => {
                println!("Aborted — box not cleared.");
                return Ok(());
            }
        }
    }
    println!("Clearing box contents...");
    nemesis_sandbox::pending::delete_box_contents(
        &paths.start_exe(),
        nemesis_sandbox::DEFAULT_BOX_NAME,
    )?;
    println!("Box cleared.");
    Ok(())
}

/// Activate the engine: install driver + service + ini + start SbieSvc. Needs
/// admin (kernel driver) → UAC self-relaunch. Requires files acquired (`install`).
fn start(paths: &nemesis_sandbox::SandboxPaths, local: bool, internal: bool) -> Result<()> {
    if internal {
        println!("[sandbox] activating engine (elevated child)...");
        nemesis_sandbox::install::start(paths)?;
        println!("[sandbox] engine activated — SbieSvc RUNNING.");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC...");
        let exe = std::env::current_exe()?;
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &relaunch_args("start", local))?;
        println!("[sandbox] elevated activator launched; waiting for SbieSvc (up to 120s)...");
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::Running,
            Duration::from_secs(120),
        );
        if !matches!(state, ServiceState::Running) {
            anyhow::bail!(
                "activate did not complete (SbieSvc state={state:?}); check the UAC prompt"
            );
        }
        println!("[sandbox] engine activated — SbieSvc RUNNING.");
        return Ok(());
    }
    println!("[sandbox] activating engine (already elevated)...");
    nemesis_sandbox::install::start(paths)?;
    println!("[sandbox] engine activated — SbieSvc RUNNING.");
    Ok(())
}

/// Build the argv for the elevated self-relaunch (`sandbox <subcmd> --internal [--local]`).
fn relaunch_args(subcmd: &str, local: bool) -> Vec<String> {
    let mut v = vec!["sandbox".to_string(), subcmd.to_string(), "--internal".to_string()];
    if local {
        v.push("--local".to_string());
    }
    v
}

async fn install(paths: &nemesis_sandbox::SandboxPaths) -> Result<()> {
    // Acquire files only (download + extract). No driver/service/ini, no UAC.
    println!("[sandbox] acquiring Sandboxie files (download + extract, no UAC)...");
    nemesis_sandbox::install::install(paths).await?;
    println!(
        "[sandbox] files acquired at {}.\nRun `nemesisbot sandbox start` (or the dashboard 启动 button) to activate the engine (installs driver, triggers UAC).",
        paths.runtime_dir.display()
    );
    Ok(())
}

/// Deactivate the engine: stop + uninstall driver + service. --purge also removes
/// the acquired files. Needs admin → UAC self-relaunch.
fn stop(
    paths: &nemesis_sandbox::SandboxPaths,
    local: bool,
    internal: bool,
    purge: bool,
) -> Result<()> {
    if internal {
        println!(
            "[sandbox] deactivating engine (elevated child){}...",
            if purge { " + purging files" } else { "" }
        );
        nemesis_sandbox::install::stop(paths, purge)?;
        println!("[sandbox] engine deactivated.");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC...");
        let exe = std::env::current_exe()?;
        let mut args = relaunch_args("stop", local);
        if purge {
            args.push("--purge".to_string());
        }
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &args)?;
        println!("[sandbox] elevated deactivator launched; waiting for SbieSvc to disappear (up to 60s)...");
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::NotFound,
            Duration::from_secs(60),
        );
        println!("[sandbox] SbieSvc state after stop: {state:?}");
        return Ok(());
    }
    println!("[sandbox] deactivating engine (already elevated)...");
    nemesis_sandbox::install::stop(paths, purge)?;
    println!("[sandbox] engine deactivated.");
    Ok(())
}

/// gateway 退出时卸载 sandbox 驱动 + 服务（install::stop，非 purge — 保留文件
/// 供下次 start 免重下载）。gateway 通常未提权 → relaunch_elevated 起 UAC 子
/// 进程 fire-and-forget（用户需点 UAC 确认，gateway 不等）；已提权则同步卸载。
/// BSOD 风险（驱动装卸）由 install::stop 的 tolerant + KmdUtil 内置重试兜底；
/// 调用时机在 gateway shutdown 序列靠后（agent loop 已停、per-call executor 已退）。
pub async fn stop_for_shutdown(home: &std::path::Path) -> Result<()> {
    let paths = nemesis_sandbox::SandboxPaths::new(home);
    if nemesis_sandbox::elevation::is_elevated() {
        nemesis_sandbox::install::stop(&paths, false)?;
        println!("[sandbox] engine deactivated on gateway shutdown.");
    } else {
        let exe = std::env::current_exe()?;
        let args = vec!["sandbox".to_string(), "stop".to_string(), "--internal".to_string()];
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &args)?;
        println!("[sandbox] elevated deactivator launched on shutdown (UAC prompt).");
    }
    Ok(())
}

fn status(paths: &nemesis_sandbox::SandboxPaths) -> Result<()> {
    let sbiesvc = nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE);
    let sbiedrv = nemesis_sandbox::status::service_state(nemesis_sandbox::DRIVER_SERVICE);
    let start_exe = paths.start_exe();
    let ready = matches!(sbiesvc, ServiceState::Running) && start_exe.exists();

    println!("Sandboxie status");
    println!("  SbieSvc (service): {sbiesvc:?}");
    println!("  SbieDrv (driver):  {sbiedrv:?}");
    println!(
        "  Start.exe:         {} [{}]",
        start_exe.display(),
        if start_exe.exists() { "present" } else { "MISSING" }
    );
    println!(
        "  Sandboxie.ini:     {} [{}]",
        paths.ini_path.display(),
        if paths.ini_path.exists() { "present" } else { "absent" }
    );
    println!(
        "  runtime dir:       {} [{}]",
        paths.runtime_dir.display(),
        if paths.runtime_dir.exists() { "present" } else { "absent" }
    );
    println!("  sandbox ready:     {ready}");
    Ok(())
}
