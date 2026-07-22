//! Sandboxie integration — install/use/uninstall the official Sandboxie Classic
//! kernel driver + service under our app's control, so the executor (Layer 1)
//! can run sandboxed.
//!
//! Layer 2 of the executor-separation plan. See
//! `docs/PLAN/2026-07-09_sandboxie-integration.md`.
//!
//! L2.0 scope (this crate's first cut): download + verify + 7z extract the
//! official release; KmdUtil install/uninstall/start/stop the driver + service;
//! write Sandboxie.ini (NemesisBox). Does NOT touch the executor yet — that's
//! L2.1+ (named-pipe transport, Start.exe spawn).
//!
//! Windows-only at runtime; non-Windows compiles to stubs that return errors
//! (so `cargo check --workspace` stays green cross-platform).

use std::path::{Path, PathBuf};

pub mod download;
pub mod elevation;
pub mod extract;
pub mod ini;
pub mod install;
pub mod kmdutil;
pub mod pending;
pub mod status;

// ---------------------------------------------------------------------------
// Constants (from C:/AI/NemesisBot/Sandboxie/ source research, 2026-07-09)
// ---------------------------------------------------------------------------

/// Plus release tag (the GitHub release tag differs from the Classic file version).
pub const RELEASE_TAG: &str = "v1.17.9";
/// Classic file version embedded in the binary (`common/my_version.h:27-29`).
pub const CLASSIC_VERSION: &str = "5.72.9";
/// Exact asset filename to download.
pub const INSTALLER_FILENAME: &str = "Sandboxie-Classic-x64-v5.72.9.exe";

/// Download URL (tag = Plus version, file = Classic version).
pub const INSTALLER_URL: &str = "https://github.com/sandboxie-plus/Sandboxie/releases/download/v1.17.9/Sandboxie-Classic-x64-v5.72.9.exe";
/// SHA-256 checksums file attached to the same release (`.github/workflows/hash.yml`).
pub const CHECKSUMS_URL: &str =
    "https://github.com/sandboxie-plus/Sandboxie/releases/download/v1.17.9/sha256-checksums.txt";

/// Kernel driver service name.
pub const DRIVER_SERVICE: &str = "SbieDrv";
/// User-mode service name.
pub const USERMODE_SERVICE: &str = "SbieSvc";
/// Mini-filter altitude (`common/my_version.h:92`) — do not change.
pub const FILTER_ALTITUDE: &str = "86900";
/// Default box name we launch the executor into.
pub const DEFAULT_BOX_NAME: &str = "NemesisBox";

// ---------------------------------------------------------------------------
// Paths — everything Sandboxie lives under <home>/workspace/tools/sandboxie/,
// never C:\Windows
// ---------------------------------------------------------------------------

/// Resolved on-disk locations for the Sandboxie install under our app home.
/// `IniPath` is redirected here via the SbieDrv service-key registry value
/// (`core/drv/conf.c:256-269`), so no file lands in `C:\Windows`.
#[derive(Debug, Clone)]
pub struct SandboxPaths {
    /// Where the extracted runtime binaries (SbieDrv.sys, SbieSvc.exe, Start.exe,
    /// KmdUtil.exe, ...) live.
    pub runtime_dir: PathBuf,
    /// Sandboxie.ini location (redirected via IniPath).
    pub ini_path: PathBuf,
    /// Box virtual-FS root (where the box mirrors paths).
    pub box_root: PathBuf,
}

impl SandboxPaths {
    /// Build paths under `<home>/workspace/tools/sandboxie/` — same `workspace/tools/`
    /// convention as ClamAV (`workspace/tools/clamav/`) and voice models. Keeps all
    /// Sandboxie files out of `C:\Windows` (IniPath registry redirect points here).
    pub fn new(home: &Path) -> Self {
        let base = home.join("workspace").join("tools").join("sandboxie");
        Self {
            runtime_dir: base.join("runtime"),
            ini_path: base.join("Sandboxie.ini"),
            box_root: base.join("box").join(DEFAULT_BOX_NAME),
        }
    }

    pub fn kmdutil(&self) -> PathBuf {
        self.runtime_dir.join("KmdUtil.exe")
    }
    pub fn start_exe(&self) -> PathBuf {
        self.runtime_dir.join("Start.exe")
    }
    pub fn sbiedrv_sys(&self) -> PathBuf {
        self.runtime_dir.join("SbieDrv.sys")
    }
    pub fn sbiesvc_exe(&self) -> PathBuf {
        self.runtime_dir.join("SbieSvc.exe")
    }
    pub fn sbiemsg_dll(&self) -> PathBuf {
        self.runtime_dir.join("SbieMsg.dll")
    }

    /// Verify the minimum runtime file set exists after extraction.
    pub fn verify_runtime(&self) -> anyhow::Result<()> {
        for (name, path) in [
            ("SbieDrv.sys", self.sbiedrv_sys()),
            ("SbieSvc.exe", self.sbiesvc_exe()),
            ("SbieMsg.dll", self.sbiemsg_dll()),
            ("KmdUtil.exe", self.kmdutil()),
            ("Start.exe", self.start_exe()),
        ] {
            if !path.exists() {
                anyhow::bail!(
                    "expected runtime file missing after extract: {name} at {}",
                    path.display()
                );
            }
        }
        Ok(())
    }
}
