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

/// 安装器成功体读取上限（BUG-20261123-043）：`http_cap::read_body_capped`
/// 流式帽。2GiB 与 embedding/插件下载族口径一致（BUG-20261116-027 /
/// BUG-20261119-183）——Sandboxie Classic 安装器 ~4.5MB，帽只拦失控/恶意
/// 源的巨大 body（旧无帽 `.bytes()` 是 273/005 横扫的漏网残留）。
const INSTALLER_BODY_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

/// checksums 文本成功体读取上限（BUG-20261123-043 同族）：sha256sums.txt
/// KB 级，4MB 宽裕。
const CHECKSUMS_BODY_LIMIT: u64 = 4 * 1024 * 1024;

/// 帽读错误 → anyhow 映射：传输错误必须保留 `reqwest::Error` 本体
/// （source chain 完整——stall 回归测试 `downcast_ref::<reqwest::Error>()`
/// 依赖可达，`{err:#}` 才能走到根因 `operation timed out`）；超限转诚实
/// 消息（与 `read_body_capped` 文案同形）。
fn cap_read_err(e: nemesis_utils::http_cap::BodyReadError) -> anyhow::Error {
    match e {
        nemesis_utils::http_cap::BodyReadError::Transport(err) => anyhow::Error::new(err),
        nemesis_utils::http_cap::BodyReadError::ExceedsLimit { cap } => anyhow::Error::msg(
            format!("response exceeds limit (>{cap} bytes); aborted mid-read"),
        ),
    }
}

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
    let checksums_resp = download_client()
        .get(checksums_url)
        .send()
        .await
        .with_context(|| format!("fetch checksums {checksums_url}"))?
        // BUG-20261110-058：状态闸。无闸时 404/5xx 的错误页 body 被当
        // checksums 文本逐行解析 → 不匹配 → bail「filename not found」
        // 假根因，并把 download_release 的「无校验降级」误触发在 URL
        // 已死场景（降级拍板针对 checksums 服务不可得，不是安装器 404）。
        .error_for_status()
        .with_context(|| format!("checksums {checksums_url}"))?;
    // BUG-20261123-043：成功体有帽读（`http_cap::read_body_capped_typed`
    // 单一真相源）——无帽 `.text()` 是 273/005 横扫的漏网残留。
    let checksums_bytes =
        nemesis_utils::http_cap::read_body_capped_typed(checksums_resp, CHECKSUMS_BODY_LIMIT)
            .await
            .map_err(cap_read_err)
            .context("read checksums body")?;
    let text = String::from_utf8_lossy(&checksums_bytes).into_owned();
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
    download_and_verify_with_cap(url, expected_sha256, dest, INSTALLER_BODY_LIMIT).await
}

/// [`download_and_verify`] 的帽参数化形态（BUG-20261123-043 测试接缝：
/// 2GiB 生产帽无法在测试里用真实超大 body 越过，行为级回归锁经小帽
/// 注入跑同一条读路径）。
async fn download_and_verify_with_cap(
    url: &str,
    expected_sha256: Option<&str>,
    dest: &Path,
    body_limit: u64,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create dir {}", parent.display()))?;
    }
    let installer_resp = download_client()
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetch {url}"))?
        // BUG-20261110-058：状态闸。无闸时 404/5xx 错误页字节被当安装器
        // 写盘（可覆盖 runtime_dir 里旧的好安装包），失败延迟到 7z 解压层
        // 才以「非档案」混淆报错——HTTP 层失败必须在 fetch 层诚实暴露。
        .error_for_status()
        .with_context(|| format!("fetch {url}"))?;
    // BUG-20261123-043：安装器成功体有帽读（`http_cap::read_body_capped_typed`
    // 单一真相源）——超限在读入中途断开，写盘发生在读全+验签之后，
    // 帽闸 abort 意味着 dest 不会被触碰。
    let bytes = nemesis_utils::http_cap::read_body_capped_typed(installer_resp, body_limit)
        .await
        .map_err(cap_read_err)
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
