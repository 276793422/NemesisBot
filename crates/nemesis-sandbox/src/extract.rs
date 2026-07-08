//! Extract the NSIS-built installer with 7-Zip.
//!
//! 7-Zip (`7z.exe` + `7z.dll`) is **bundled** into this crate via
//! `include_bytes!` (LGPL; see `third_party/7z/LICENSE.txt`) so the Sandboxie
//! extraction is self-contained — no requirement that 7-Zip be pre-installed.
//! The bundled 7-Zip is written to `<runtime_dir>/7z/` on first use and reused.
//! A system 7-Zip in PATH / common install dirs is only a fallback.
//!
//! The public Classic `.exe` is pure NSIS (`SandboxieVS.nsi:5` SetCompressor
//! lzma), so `7z x` extracts it cleanly. Files MUST be extracted verbatim —
//! modifying a byte breaks both the OS Authenticode signature (driver won't
//! load) and the Sandboxie ECC `.sig` (SbieSvc rejects the client).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Bundled 7-Zip console + engine (LGPL; from `third_party/7z/`).
const SEVEN_ZIP_EXE: &[u8] = include_bytes!("../third_party/7z/7z.exe");
const SEVEN_ZIP_DLL: &[u8] = include_bytes!("../third_party/7z/7z.dll");

/// Resolve a usable 7z.exe path: prefer the bundled one (written to
/// `<runtime_dir>/7z/7z.exe` on first use), fall back to a system 7-Zip in
/// PATH / common install dirs (insurance).
pub fn resolve_seven_zip(runtime_dir: &Path) -> Result<PathBuf> {
    let bundled_dir = runtime_dir.join("7z");
    let bundled_exe = bundled_dir.join("7z.exe");
    if !bundled_exe.exists() {
        std::fs::create_dir_all(&bundled_dir)
            .with_context(|| format!("create bundled-7z dir {}", bundled_dir.display()))?;
        std::fs::write(&bundled_exe, SEVEN_ZIP_EXE)
            .with_context(|| format!("write bundled 7z.exe to {}", bundled_exe.display()))?;
        std::fs::write(bundled_dir.join("7z.dll"), SEVEN_ZIP_DLL)
            .with_context(|| format!("write bundled 7z.dll to {}", bundled_dir.display()))?;
        tracing::info!("[sandbox] extracted bundled 7-Zip to {}", bundled_dir.display());
    }
    if bundled_exe.exists() {
        return Ok(bundled_exe);
    }
    if let Some(p) = find_system_7z() {
        tracing::warn!(
            "[sandbox] bundled 7-Zip unavailable; falling back to system 7z at {}",
            p.display()
        );
        return Ok(p);
    }
    bail!("no 7-Zip available (bundled extraction failed and no system 7z found)");
}

/// Search PATH + common install dirs for a system 7z.exe (fallback only).
fn find_system_7z() -> Option<PathBuf> {
    if let Ok(out) = std::process::Command::new("where").arg("7z.exe").output() {
        if out.status.success() {
            if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                let p = PathBuf::from(line.trim());
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
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

/// Convenience: resolve 7z (bundled first) + extract in one call.
pub fn extract_release(installer: &Path, runtime_dir: &Path) -> Result<()> {
    let seven_zip = resolve_seven_zip(runtime_dir)?;
    extract(installer, runtime_dir, &seven_zip)
}
