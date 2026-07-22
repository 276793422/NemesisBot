//! Download the official Sandboxie Classic release + verify its SHA-256.
//!
//! Checksums are published as `sha256-checksums.txt` attached to the same
//! GitHub release (`.github/workflows/hash.yml`). We fetch that, parse the line
//! for our filename, and verify the downloaded installer against it.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// Download `filename`'s expected SHA-256 from the checksums file.
/// Format per line: `<sha256>  <filename>` (two-space separated).
pub async fn fetch_expected_sha256(checksums_url: &str, filename: &str) -> Result<String> {
    let text = reqwest::get(checksums_url)
        .await
        .with_context(|| format!("fetch checksums {checksums_url}"))?
        .text()
        .await
        .context("read checksums body")?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `<hash>  <file>` — split on whitespace, last token is filename.
        let mut parts = line.split_whitespace();
        let hash = parts.next().unwrap_or("");
        let file = parts.next().unwrap_or("");
        if file == filename && !hash.is_empty() {
            return Ok(hash.to_lowercase());
        }
    }
    bail!("filename `{filename}` not found in checksums file");
}

/// Download `url` to `dest`, verifying against `expected_sha256` (if given).
/// Returns the destination path on success.
pub async fn download_and_verify(
    url: &str,
    expected_sha256: Option<&str>,
    dest: &Path,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let bytes = reqwest::get(url)
        .await
        .with_context(|| format!("fetch {url}"))?
        .bytes()
        .await
        .context("read installer body")?;

    if let Some(expected) = expected_sha256 {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected.to_lowercase() {
            bail!(
                "SHA-256 mismatch for {}: expected {expected}, got {actual}",
                dest.display()
            );
        }
        tracing::info!(
            "[sandbox] downloaded + verified {} ({} bytes, sha256={actual})",
            dest.display(),
            bytes.len()
        );
    } else {
        tracing::warn!(
            "[sandbox] downloaded {} ({} bytes) WITHOUT sha256 verification (no expected hash)",
            dest.display(),
            bytes.len()
        );
    }

    tokio::fs::write(dest, &bytes)
        .await
        .with_context(|| format!("write {}", dest.display()))?;
    Ok(())
}

/// Download the Sandboxie Classic installer into `runtime_dir`, verifying its
/// SHA-256 against the release's checksums file. Returns the installer path.
pub async fn download_release(
    installer_url: &str,
    checksums_url: &str,
    filename: &str,
    runtime_dir: &Path,
) -> Result<std::path::PathBuf> {
    let dest = runtime_dir.join(filename);
    let expected = fetch_expected_sha256(checksums_url, filename)
        .await
        .map_err(|e| {
            tracing::warn!(
                "[sandbox] could not fetch/parse checksums ({e}); proceeding WITHOUT verification"
            );
            e
        })
        .ok();
    download_and_verify(installer_url, expected.as_deref(), &dest).await?;
    Ok(dest)
}
