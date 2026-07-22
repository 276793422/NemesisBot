//! Extract the NSIS-built Sandboxie installer with 7-Zip.
//!
//! 7-Zip is **not bundled** (keeps the binary ~2.4MB smaller). Instead,
//! [`resolve_seven_zip`] looks for a local 7z first — a previously-downloaded
//! copy in `runtime/7z/`, then a system install in PATH / `Program Files` — and
//! only downloads `7z.zip` (7z.exe + 7z.dll + LICENSE, LGPL) from the project's
//! GitHub when neither is present. This keeps the binary lean while letting any
//! user enable the sandbox later without re-downloading the binary: just run
//! `sandbox install`, which fetches 7z + Sandboxie on the spot.
//!
//! The public Classic `.exe` is pure NSIS (`SandboxieVS.nsi:5` SetCompressor
//! lzma), so `7z x` extracts it cleanly. Files MUST be extracted verbatim —
//! modifying a byte breaks both the OS Authenticode signature (driver won't
//! load) and the Sandboxie ECC `.sig` (SbieSvc rejects the client).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Where we fetch 7z.zip (7z.exe + 7z.dll + LICENSE.txt, LGPL) when no local
/// 7-Zip is found. Hosted in the project repo so it's under our control.
const SEVEN_ZIP_ZIP_URL: &str = "https://raw.githubusercontent.com/276793422/NemesisBot_Rust/refs/heads/main/test-tools/bins/7z.zip";

/// Resolve a usable 7z.exe: (1) a previously-downloaded copy in
/// `runtime/7z/7z.exe`, (2) a system 7-Zip in PATH / common install dirs,
/// (3) download `7z.zip` from GitHub + unzip into `runtime/7z/`. Returns the
/// 7z.exe path.
pub async fn resolve_seven_zip(runtime_dir: &Path) -> Result<PathBuf> {
    let bundled_exe = runtime_dir.join("7z").join("7z.exe");
    // 1. Previously downloaded.
    if bundled_exe.exists() {
        tracing::debug!("[sandbox] using cached 7z at {}", bundled_exe.display());
        return Ok(bundled_exe);
    }
    // 2. System 7-Zip (user already has 7-Zip installed).
    if let Some(p) = find_system_7z() {
        tracing::info!(
            "[sandbox] using system 7-Zip at {} (no download needed)",
            p.display()
        );
        return Ok(p);
    }
    // 3. Download + unzip.
    tracing::info!("[sandbox] no local 7-Zip; downloading from {SEVEN_ZIP_ZIP_URL}");
    download_and_unzip_7z(runtime_dir).await?;
    if bundled_exe.exists() {
        Ok(bundled_exe)
    } else {
        bail!(
            "7z download/extract finished but 7z.exe not at {} (unexpected 7z.zip layout)",
            bundled_exe.display()
        );
    }
}

/// Download `7z.zip` + unzip into `runtime_dir` (preserves the archive's `7z/`
/// subdir → `runtime_dir/7z/7z.exe`). Logs the SHA-256 for audit; integrity is
/// backstopped by Sandboxie's own signature check on the files it later extracts.
async fn download_and_unzip_7z(runtime_dir: &Path) -> Result<()> {
    tokio::fs::create_dir_all(runtime_dir)
        .await
        .with_context(|| format!("create runtime dir {}", runtime_dir.display()))?;

    // Download with a timeout + retry — raw.githubusercontent.com (Fastly CDN)
    // occasionally resets mid-transfer; a fresh connection usually completes.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("build http client")?;
    let bytes = {
        let mut last: Option<anyhow::Error> = None;
        let mut got: Option<bytes::Bytes> = None;
        for attempt in 1..=3u32 {
            match client.get(SEVEN_ZIP_ZIP_URL).send().await {
                Ok(resp) => match resp.bytes().await {
                    Ok(b) => {
                        got = Some(b);
                        break;
                    }
                    Err(e) => last = Some(anyhow::anyhow!("read body (attempt {attempt}): {e}")),
                },
                Err(e) => last = Some(anyhow::anyhow!("GET (attempt {attempt}): {e}")),
            }
            tracing::warn!(
                "[sandbox] 7z.zip download attempt {attempt} failed: {:?}; retrying",
                last.as_ref().map(|e| e.to_string())
            );
            if attempt < 3 {
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
        match got {
            Some(b) => b,
            None => return Err(last.context("fetch 7z.zip (3 attempts)")?),
        }
    };
    {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(&bytes);
        tracing::info!(
            "[sandbox] downloaded 7z.zip ({} bytes, sha256={:x})",
            bytes.len(),
            h.finalize()
        );
    }
    let zip_path = runtime_dir.join("7z.zip");
    tokio::fs::write(&zip_path, &bytes)
        .await
        .with_context(|| format!("write {}", zip_path.display()))?;

    // Unzip (sync file I/O → spawn_blocking so we don't stall the async runtime).
    // Clone zip_path into the closure; the original is reused for cleanup below.
    let dest = runtime_dir.to_path_buf();
    let zip_for_closure = zip_path.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&zip_for_closure)
            .with_context(|| format!("open {}", zip_for_closure.display()))?;
        let mut archive = zip::ZipArchive::new(file).context("open zip archive")?;
        archive
            .extract(&dest)
            .with_context(|| format!("extract zip -> {}", dest.display()))?;
        Ok(())
    })
    .await
    .context("unzip task join")??;

    // Keep runtime/ clean — the zip has served its purpose.
    let _ = tokio::fs::remove_file(&zip_path).await;
    Ok(())
}

/// Probe 7z availability WITHOUT downloading: cached `runtime/7z/7z.exe`, then a
/// system 7z in PATH / common install dirs. Returns (available, source).
pub fn seven_zip_status(runtime_dir: &Path) -> (bool, &'static str) {
    if runtime_dir.join("7z").join("7z.exe").exists() {
        return (true, "cached");
    }
    if find_system_7z().is_some() {
        return (true, "system");
    }
    (false, "none")
}

/// Search PATH + common install dirs for a system 7z.exe (used only when no
/// cached/downloaded copy exists yet).
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

/// Convenience: resolve 7z (local-first, download if needed) + extract the
/// Sandboxie installer in one call.
pub async fn extract_release(installer: &Path, runtime_dir: &Path) -> Result<()> {
    let seven_zip = resolve_seven_zip(runtime_dir).await?;
    extract(installer, runtime_dir, &seven_zip)
}

#[cfg(test)]
mod tests;
