//! KmdUtil.exe command builders + the registry writes KmdUtil does NOT do.
//!
//! Commands mirror the official NSIS install sequence
//! (`Sandboxie/install/SandboxieVS.nsi:1564-1573`) and the KmdUtil verb parser
//! (`Sandboxie/install/kmdutil/kmdutil.c:40-50`). All install/delete verbs need
//! admin (they open SC_MANAGER_CREATE_SERVICE) — call via the elevation helper
//! or from an already-elevated process.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::{DRIVER_SERVICE, FILTER_ALTITUDE, USERMODE_SERVICE};

/// Build the `KmdUtil install SbieDrv ...` command (kernel mini-filter driver).
pub fn install_driver(kmdutil: &Path, sbiedrv_sys: &Path, sbiemsg_dll: &Path) -> Command {
    let mut c = Command::new(kmdutil);
    c.arg("install")
        .arg(DRIVER_SERVICE)
        .arg(sbiedrv_sys)
        .arg("type=kernel")
        .arg("start=demand")
        .arg(format!("msgfile={}", sbiemsg_dll.display()))
        .arg(format!("altitude={FILTER_ALTITUDE}"));
    c
}

/// Build the `KmdUtil install SbieSvc ...` command (user-mode service).
/// KmdUtil auto-quotes the binary path (`kmdutil.c:408-417`).
pub fn install_service(kmdutil: &Path, sbiesvc_exe: &Path, sbiemsg_dll: &Path) -> Command {
    let mut c = Command::new(kmdutil);
    c.arg("install")
        .arg(USERMODE_SERVICE)
        .arg(sbiesvc_exe)
        .arg("type=own")
        .arg("start=auto")
        .arg("display=Sandboxie Service")
        .arg("group=UIGroup")
        .arg(format!("msgfile={}", sbiemsg_dll.display()));
    c
}

pub fn start(kmdutil: &Path, name: &str) -> Command {
    let mut c = Command::new(kmdutil);
    c.arg("start").arg(name);
    c
}

pub fn stop(kmdutil: &Path, name: &str) -> Command {
    let mut c = Command::new(kmdutil);
    c.arg("stop").arg(name);
    c
}

pub fn delete(kmdutil: &Path, name: &str) -> Command {
    let mut c = Command::new(kmdutil);
    c.arg("delete").arg(name);
    c
}

/// Run a KmdUtil command, returning its combined status/stderr as a Result.
/// `tolerant`: when true, a non-zero exit is logged but not propagated (used for
/// `stop`/`delete` of services that may already be gone).
pub fn run(mut cmd: Command, tolerant: bool) -> Result<()> {
    let output = cmd
        .output()
        .with_context(|| format!("spawn {}", format_command(&cmd)))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let msg = format!(
            "kmdutil failed (status {}): cmd={}; stderr={stderr}; stdout={stdout}",
            output.status,
            format_command(&cmd)
        );
        if tolerant {
            tracing::warn!("[sandbox] (tolerant) {msg}");
            return Ok(());
        }
        anyhow::bail!("{msg}");
    }
    tracing::debug!(
        "[sandbox] kmdutil ok: {} | stdout={stdout}",
        format_command(&cmd)
    );
    Ok(())
}

fn format_command(cmd: &Command) -> String {
    let program = cmd.get_program().to_string_lossy();
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    format!("{} {}", program, args.join(" "))
}

// ---------------------------------------------------------------------------
// Registry writes KmdUtil does NOT do (from SandboxieVS.nsi:1564-1573)
// ---------------------------------------------------------------------------

/// Set `HKLM\...\Services\SbieDrv\IniPath = <ini_path>` so Sandboxie.ini lives
/// under our app home instead of `C:\Windows` (`core/drv/conf.c:256-269`).
/// Uses `reg.exe` (L2.0 scaffolding — `winreg`/FFI is a cleanup option).
pub fn set_ini_path(ini_path: &Path) -> Result<()> {
    reg_add_sz(
        &format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{DRIVER_SERVICE}"),
        "IniPath",
        &ini_path.to_string_lossy(),
    )
}

/// Write the two SbieSvc service-key DWORDs the NSIS installer writes
/// (`SandboxieVS.nsi:1571-1573`): `Language=0`, `PreferExternalManifest=1`.
pub fn set_sbiesvc_service_key_dwounds() -> Result<()> {
    let key = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{USERMODE_SERVICE}");
    reg_add_dword(&key, "Language", "0")?;
    reg_add_dword(&key, "PreferExternalManifest", "1")?;
    Ok(())
}

fn reg_add_sz(key: &str, value: &str, data: &str) -> Result<()> {
    let out = std::process::Command::new("reg")
        .arg("add")
        .arg(key)
        .arg("/v")
        .arg(value)
        .arg("/t")
        .arg("REG_SZ")
        .arg("/d")
        .arg(data)
        .arg("/f")
        .output()
        .with_context(|| format!("reg add {key} {value}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "reg add {key}\\{value} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

fn reg_add_dword(key: &str, value: &str, data: &str) -> Result<()> {
    let out = std::process::Command::new("reg")
        .arg("add")
        .arg(key)
        .arg("/v")
        .arg(value)
        .arg("/t")
        .arg("REG_DWORD")
        .arg("/d")
        .arg(data)
        .arg("/f")
        .output()
        .with_context(|| format!("reg add {key} {value}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "reg add {key}\\{value} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}
