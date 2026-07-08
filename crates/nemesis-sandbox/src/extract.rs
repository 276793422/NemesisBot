//! Extract the NSIS-built installer with 7-Zip.
//!
//! The public Classic `.exe` is pure NSIS (`SandboxieVS.nsi:5` SetCompressor
//! lzma), so `7z x` extracts it cleanly to the runtime dir. Files MUST be
//! extracted verbatim — modifying a byte breaks both the OS Authenticode
//! signature (driver won't load) and the Sandboxie ECC `.sig` (SbieSvc rejects
//! the client).
//!
//! L2.0: locates 7z by searching PATH + common install dirs. Bundling 7z.exe
//! (`include_bytes!`) is a follow-up so the crate is self-contained.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Find a usable 7z executable: PATH (`where 7z.exe`) then common install dirs.
pub fn find_7z() -> Option<PathBuf> {
    // 1. `where` on PATH.
    if let Ok(out) = std::process::Command::new("where").arg("7z.exe").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().next() {
                let p = PathBuf::from(line.trim());
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    // 2. Common Windows install locations.
    for candidate in [
        r"C:\Program Files\7-Zip\7z.exe",
        r"C:\Program Files (x86)\7-Zip\7z.exe",
    ] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Extract `installer` into `runtime_dir` using `seven_zip`. Overwrites existing.
pub fn extract(installer: &Path, runtime_dir: &Path, seven_zip: &Path) -> Result<()> {
    // `7z x <installer> -o<dir> -y` — extract with full paths, yes to all.
    // `-aoa` = overwrite all (7z rename/overwrite flag).
    let output = std::process::Command::new(seven_zip)
        .arg("x")
        .arg(installer)
        .arg(format!("-o{}", runtime_dir.display()))
        .arg("-y")
        .output()
        .with_context(|| format!("spawn 7z at {}", seven_zip.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "7z extraction failed (status {}): stderr={stderr}; stdout={stdout}",
            output.status
        );
    }
    tracing::info!(
        "[sandbox] extracted {} -> {}",
        installer.display(),
        runtime_dir.display()
    );
    Ok(())
}
