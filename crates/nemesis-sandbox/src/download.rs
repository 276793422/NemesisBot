//! Download the official Sandboxie Classic release + verify its SHA-256.
//!
//! Checksums are published as `sha256-checksums.txt` attached to the same
//! GitHub release (`.github/workflows/hash.yml`). We fetch that, parse the line
//! for our filename, and verify the downloaded installer against it.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// 下载 stall 检测窗：reqwest::get 内部 Client::new()
/// 零超时——半死源（TCP 已建但不发数据）让 `sandbox install` 永久挂死。
/// 不设总超时是刻意的：GitHub 直链实测低至 ~10KB/s，安装器 ~4.5MB 的合法
/// 慢速流不能被总超时误杀；read_timeout 只惩罚响应分片间隔超窗的 stall。
#[cfg(not(test))]
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(test)]
// 测试态 400ms：stall 回归测试经真实公开函数跑，等待必须秒级以内。
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_millis(400);
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .read_timeout(DOWNLOAD_READ_TIMEOUT)
        .build()
        .expect("download http client build")
}

/// Download `filename`'s expected SHA-256 from the checksums file.
/// Format per line: `<sha256> <filename>` (two-space separated).
pub async fn fetch_expected_sha256(checksums_url: &str, filename: &str) -> Result<String> {
    let text = download_client()
        .get(checksums_url)
        .send()
        .await
        .with_context(|| format!("fetch checksums {checksums_url}"))?
        // BUG-20261110-058：状态闸。无闸时 404/5xx 的错误页 body 被当
        // checksums 文本逐行解析 → 不匹配 → bail「filename not found」
        // 假根因，并把 download_release 的「无校验降级」误触发在 URL
        // 已死场景（降级拍板针对 checksums 服务不可得，不是安装器 404）。
        .error_for_status()
        .with_context(|| format!("checksums {checksums_url}"))?
        .text()
        .await
        .context("read checksums body")?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // `<hash> <file>` — split on whitespace, last token is filename.
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
    let bytes = download_client()
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetch {url}"))?
        // BUG-20261110-058：状态闸。无闸时 404/5xx 错误页字节被当安装器
        // 写盘（可覆盖 runtime_dir 里旧的好安装包），失败延迟到 7z 解压层
        // 才以「非档案」混淆报错——HTTP 层失败必须在 fetch 层诚实暴露。
        .error_for_status()
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cov_tests;

// Wave6B 覆盖率收尾：见 cov_wave6b_tests.rs 头注释。
#[cfg(test)]
mod cov_wave6b_tests;
