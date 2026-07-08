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
    /// Download + install the Sandboxie driver + service (needs admin → triggers UAC).
    Install {
        /// Internal: run the install synchronously in THIS already-elevated process.
        #[arg(long, hide = true)]
        internal: bool,
    },
    /// Stop + delete the Sandboxie driver + service (needs admin → triggers UAC).
    Uninstall {
        #[arg(long, hide = true)]
        internal: bool,
    },
    /// Show Sandboxie install / service status.
    Status,
}

pub async fn run(action: SandboxCommand, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    match action {
        SandboxCommand::Install { internal } => install(&paths, local, internal).await,
        SandboxCommand::Uninstall { internal } => uninstall(&paths, local, internal),
        SandboxCommand::Status => status(&paths),
    }
}

/// Build the argv for the elevated self-relaunch (`sandbox <subcmd> --internal [--local]`).
fn relaunch_args(subcmd: &str, local: bool) -> Vec<String> {
    let mut v = vec!["sandbox".to_string(), subcmd.to_string(), "--internal".to_string()];
    if local {
        v.push("--local".to_string());
    }
    v
}

async fn install(
    paths: &nemesis_sandbox::SandboxPaths,
    local: bool,
    internal: bool,
) -> Result<()> {
    if internal {
        println!("[sandbox] installing (elevated child)...");
        nemesis_sandbox::install::install(paths).await?;
        println!("[sandbox] install complete.");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC...");
        let exe = std::env::current_exe()?;
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &relaunch_args("install", local))?;
        println!("[sandbox] elevated installer launched; waiting for SbieSvc (up to 120s)...");
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::Running,
            Duration::from_secs(120),
        );
        match state {
            ServiceState::Running => println!("[sandbox] install OK — SbieSvc RUNNING."),
            _ => anyhow::bail!(
                "install did not complete (SbieSvc state={state:?}); check the UAC prompt, \
                 7z availability (PATH or C:\\Program Files\\7-Zip), and network/download"
            ),
        }
        return Ok(());
    }
    println!("[sandbox] installing (already elevated)...");
    nemesis_sandbox::install::install(paths).await?;
    println!("[sandbox] install complete.");
    Ok(())
}

fn uninstall(paths: &nemesis_sandbox::SandboxPaths, local: bool, internal: bool) -> Result<()> {
    if internal {
        println!("[sandbox] uninstalling (elevated child)...");
        nemesis_sandbox::install::uninstall(paths)?;
        println!("[sandbox] uninstall complete.");
        return Ok(());
    }
    if !nemesis_sandbox::elevation::is_elevated() {
        println!("[sandbox] not elevated — requesting UAC...");
        let exe = std::env::current_exe()?;
        nemesis_sandbox::elevation::relaunch_elevated(&exe, &relaunch_args("uninstall", local))?;
        println!("[sandbox] elevated uninstaller launched; waiting for SbieSvc to disappear (up to 60s)...");
        let state = nemesis_sandbox::install::wait_for_state(
            nemesis_sandbox::USERMODE_SERVICE,
            ServiceState::NotFound,
            Duration::from_secs(60),
        );
        println!("[sandbox] SbieSvc state after uninstall: {state:?}");
        return Ok(());
    }
    println!("[sandbox] uninstalling (already elevated)...");
    nemesis_sandbox::install::uninstall(paths)?;
    println!("[sandbox] uninstall complete.");
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
