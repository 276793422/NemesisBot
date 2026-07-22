//! Write Sandboxie.ini with the NemesisBox definition.
//!
//! Directives verified against the source research (2026-07-09):
//! - `Enabled=y` is REQUIRED (`core/drv/conf_user.c:381` — Start.exe gates on
//!   `SbieApi_IsBoxEnabled`).
//! - `AllowNetworkAccess=n` blocks egress (the `BlockNetAccess` directive does
//!   NOT exist — `core/drv/wfp.c:701`).
//! - `DropAdminRights=y` strips admin SIDs (`core/drv/token.c:558`).
//! - `OpenPipePath=\Device\NamedPipe\<box>_*` opens our IPC pipe
//!   (`core/drv/file.c`; examples throughout `install/Templates.ini`).
//! - `ForceProcess`/`ForceFolder` MUST NOT be set (would auto-box the gateway).
//!
//! The ini is written to `SandboxPaths::ini_path` (redirected via the SbieDrv
//! IniPath registry value), never `C:\Windows\Sandboxie.ini`.

use std::path::Path;

use anyhow::{Context, Result};

/// Write the NemesisBox definition to `ini_path`.
pub fn write_sandboxie_ini(ini_path: &Path, box_name: &str, box_root: &Path) -> Result<()> {
    if let Some(parent) = ini_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create ini dir {}", parent.display()))?;
    }
    let box_root_nt = format!(r"\??\{}", box_root.display());
    let content = format!(
        "[GlobalSettings]\n\
         \n\
         # Prevent SbieCtrl (the Classic GUI) from auto-launching when a sandboxed\n\
         # program starts — without this, SbieSvc auto-starts SbieCtrl.exe on every\n\
         # boxed spawn (sbieiniserver.cpp:1543 SbieCtrl_EnableAutoStart), popping its\n\
         # window + message dialogs. Headless use must keep this at n.\n\
         [UserSettings_Default]\n\
         SbieCtrl_EnableAutoStart=n\n\
         \n\
         [{box_name}]\n\
         Enabled=y\n\
         AllowNetworkAccess=n\n\
         DropAdminRights=y\n\
         OpenPipePath=\\Device\\NamedPipe\\{box_name}_*\n\
         FileRootPath={box_root_nt}\n"
    );
    std::fs::write(ini_path, content).with_context(|| format!("write {}", ini_path.display()))?;
    tracing::info!(
        "[sandbox] wrote {} (box={box_name}, root={})",
        ini_path.display(),
        box_root.display()
    );
    Ok(())
}
