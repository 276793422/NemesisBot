//! Bootstrap module — auto-setup on first run
//!
//! On first launch:
//!   1. Creates default config.toml next to exe (if missing)
//!   2. Downloads sherpa-onnx runtime shared libraries from GitHub releases
//!   3. Extracts needed libraries to exe directory
//!
//! Then calls sherpa::init() to load the library at runtime.
//!
//! Voice pipeline (TTS/STT) is only supported on Windows.

use anyhow::{bail, Result};
#[cfg(target_os = "windows")]
use anyhow::Context;
#[cfg(target_os = "windows")]
use std::fs;
use std::path::Path;
#[cfg(target_os = "windows")]
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Windows-only constants
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
const SHERPA_VERSION: &str = "1.13.2";

#[cfg(target_os = "windows")]
const SHERPA_RELEASE_NAME: &str = "sherpa-onnx-v1.13.2-win-x64-shared-MD-Release";

#[cfg(target_os = "windows")]
const REQUIRED_LIBS: &[&str] = &[
    "sherpa-onnx-c-api.dll",
    "onnxruntime.dll",
    "onnxruntime_providers_shared.dll",
];

#[cfg(target_os = "windows")]
const DEFAULT_CONFIG: &str = include_str!("../config.toml");

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns the list of required shared library names for the current platform.
/// Returns an empty slice on non-Windows platforms.
pub fn required_lib_names() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        REQUIRED_LIBS
    }
    #[cfg(not(target_os = "windows"))]
    {
        &[]
    }
}

/// Returns the default config.toml content.
#[cfg(target_os = "windows")]
pub fn default_config_toml() -> &'static str {
    DEFAULT_CONFIG
}

/// Full auto-setup: config + libraries. Libraries placed next to exe.
#[cfg(target_os = "windows")]
pub fn run(config_path: &Path) -> Result<bool> {
    let dir = exe_dir()?;
    run_in_dir(config_path, &dir)
}

/// Load sherpa-onnx runtime library. Call this before using any engine.
#[cfg(target_os = "windows")]
pub fn init_sherpa(lib_dir: &Path) -> Result<()> {
    let main_lib = lib_dir.join(REQUIRED_LIBS[0]);
    if !main_lib.exists() {
        bail!(
            "Voice runtime not found at {}. Run: nemesisbot voice setup",
            lib_dir.display()
        );
    }
    crate::sherpa::init(&main_lib)
}

/// Non-Windows stub.
#[cfg(not(target_os = "windows"))]
pub fn init_sherpa(_lib_dir: &Path) -> Result<()> {
    bail!("Voice pipeline is only supported on Windows")
}

/// Non-Windows stub.
#[cfg(not(target_os = "windows"))]
pub fn run(_config_path: &Path) -> Result<bool> {
    bail!("Voice pipeline is only supported on Windows")
}

/// Full auto-setup with explicit library directory.
#[cfg(target_os = "windows")]
pub fn run_in_dir(config_path: &Path, lib_dir: &Path) -> Result<bool> {
    if !config_path.exists() {
        tracing::info!("[setup] Creating default config: {}", config_path.display());
        fs::create_dir_all(lib_dir)?;
        fs::write(config_path, DEFAULT_CONFIG)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    }

    // Read proxy from config
    let proxy_url = crate::config::AppConfig::load_or_default(config_path)
        .models.proxy.url.clone();

    let all_present = REQUIRED_LIBS.iter().all(|lib| lib_dir.join(lib).exists());

    if all_present {
        tracing::info!("[setup] Runtime libraries found.");
    } else {
        tracing::info!("[setup] Downloading sherpa-onnx v{} runtime ...", SHERPA_VERSION);
        download_runtime_libs(lib_dir, &proxy_url)?;
        tracing::info!("[setup] Runtime libraries ready.");
    }

    let main_lib = lib_dir.join(REQUIRED_LIBS[0]);
    crate::sherpa::init(&main_lib)?;
    tracing::info!("[setup] sherpa-onnx v{} loaded.", SHERPA_VERSION);

    Ok(!all_present)
}

/// Non-Windows stub.
#[cfg(not(target_os = "windows"))]
pub fn run_in_dir(_config_path: &Path, _lib_dir: &Path) -> Result<bool> {
    bail!("Voice pipeline is only supported on Windows")
}

// ---------------------------------------------------------------------------
// Download & extract (Windows only) — uses async reqwest to avoid
// reqwest::blocking stack overflow (it creates a tokio runtime on the
// calling thread, exhausting the default 2 MB stack).
// ---------------------------------------------------------------------------

#[cfg(all(target_os = "windows", feature = "download"))]
fn download_runtime_libs(exe_dir: &Path, proxy_url: &str) -> Result<()> {
    let exe_dir = exe_dir.to_path_buf();
    let proxy_url = proxy_url.to_string();
    // Spawn a standalone thread to avoid "Cannot start a runtime from within a runtime"
    // (the caller may already be inside a tokio runtime) and to avoid reqwest::blocking
    // stack overflow (async reqwest + our own runtime is much lighter on stack).
    let handle = std::thread::spawn(move || -> Result<()> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .context("Failed to create tokio runtime")?;

        let urls = [
            format!(
                "https://github.com/k2-fsa/sherpa-onnx/releases/download/v{0}/{1}.tar.bz2",
                SHERPA_VERSION, SHERPA_RELEASE_NAME
            ),
            format!(
                "https://hf-mirror.com/datasets/csukuangfj/sherpa-onnx/resolve/main/{0}.tar.bz2",
                SHERPA_RELEASE_NAME
            ),
        ];

        let mut last_error = None;
        for url in &urls {
            tracing::info!("Trying: {}", url);
            match rt.block_on(try_download_and_extract(url, &exe_dir, &proxy_url)) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    tracing::warn!("Failed: {}", e);
                    last_error = Some(e);
                }
            }
        }

        bail!(
            "All download sources failed. Last error: {}. \
             Please download sherpa-onnx v{} manually and extract these libraries next to the exe: {}",
            last_error.unwrap_or_else(|| anyhow::anyhow!("unknown error")),
            SHERPA_VERSION,
            REQUIRED_LIBS.join(", ")
        )
    });

    handle.join()
        .map_err(|_| anyhow::anyhow!("Download thread panicked"))?
}

#[cfg(all(target_os = "windows", not(feature = "download")))]
fn download_runtime_libs(_exe_dir: &Path, _proxy_url: &str) -> Result<()> {
    bail!(
        "Runtime libraries not found. Build with 'download' feature (default) \
         for automatic download, or copy these libraries next to the exe: {}",
        REQUIRED_LIBS.join(", ")
    )
}

#[cfg(all(target_os = "windows", feature = "download"))]
fn format_speed(bytes_per_sec: f64) -> String {
    if bytes_per_sec >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes_per_sec / (1024.0 * 1024.0))
    } else if bytes_per_sec >= 1024.0 {
        format!("{:.0} KB", bytes_per_sec / 1024.0)
    } else {
        format!("{:.0} B", bytes_per_sec)
    }
}

#[cfg(all(target_os = "windows", feature = "download"))]
async fn try_download_and_extract(url: &str, exe_dir: &Path, proxy_url: &str) -> Result<()> {
    let temp_dir = std::env::temp_dir().join("nemesis-voice-setup");
    fs::create_dir_all(&temp_dir)?;

    let archive_name = format!("{}.tar.bz2", SHERPA_RELEASE_NAME);
    let archive_path = temp_dir.join(&archive_name);
    let part_path = temp_dir.join(format!("{}.part", archive_name));

    // Use .part file for download; only promote to final name on success
    let need_download = !archive_path.exists()
        || fs::metadata(&archive_path).map(|m| m.len()).unwrap_or(0) == 0;

    if need_download {
        tracing::info!("Downloading {} ...", archive_name);

        let mut client_builder = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36 Edg/148.0.0.0")
            .timeout(std::time::Duration::from_secs(1800))
            .connect_timeout(std::time::Duration::from_secs(30));

        if !proxy_url.trim().is_empty() {
            let proxy = reqwest::Proxy::all(proxy_url)
                .with_context(|| format!("Invalid proxy URL: {}", proxy_url))?;
            tracing::info!("Using proxy: {}", proxy_url);
            client_builder = client_builder.proxy(proxy);
        }

        let client = client_builder.build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(url)
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7")
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8,en-GB;q=0.7,en-US;q=0.6")
            .header("Accept-Encoding", "gzip, deflate, br, zstd")
            .header("Sec-Ch-Ua", r#""Chromium";v="148", "Microsoft Edge";v="148", "Not/A)Brand";v="99""#)
            .header("Sec-Ch-Ua-Mobile", "?0")
            .header("Sec-Ch-Ua-Platform", r#""Windows""#)
            .header("Sec-Fetch-Site", "none")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-User", "?1")
            .header("Sec-Fetch-Dest", "document")
            .header("Upgrade-Insecure-Requests", "1")
            .send()
            .await
            .with_context(|| format!("Download failed: {}", url))?;

        if !response.status().is_success() {
            bail!("HTTP {}", response.status());
        }

        let total_size = response.content_length();
        let mut file = fs::File::create(&part_path)
            .with_context(|| format!("Failed to create temp file: {}", part_path.display()))?;
        let mut downloaded: u64 = 0;
        let mut last_report = std::time::Instant::now();
        let mut last_downloaded: u64 = 0;

        use std::io::Write;
        let mut stream = response.bytes_stream();
        use futures::StreamExt;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Failed to read download chunk")?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;

            if last_report.elapsed() >= std::time::Duration::from_secs(5) {
                let elapsed = last_report.elapsed().as_secs_f64();
                let chunk_bytes = downloaded - last_downloaded;
                let speed = if elapsed > 0.0 { chunk_bytes as f64 / elapsed } else { 0.0 };
                let speed_str = format_speed(speed);
                if let Some(total) = total_size {
                    let pct = downloaded as f64 / total as f64 * 100.0;
                    tracing::info!(
                        "{:.1} / {:.1} MB ({: >3.0}%)  {}/s",
                        downloaded as f64 / (1024.0 * 1024.0),
                        total as f64 / (1024.0 * 1024.0),
                        pct,
                        speed_str
                    );
                } else {
                    tracing::info!("{:.1} MB downloaded  {}/s", downloaded as f64 / (1024.0 * 1024.0), speed_str);
                }
                last_report = std::time::Instant::now();
                last_downloaded = downloaded;
            }
        }
        tracing::info!("Download complete: {:.0} MB", downloaded as f64 / (1024.0 * 1024.0));

        // Promote .part → final name only after successful download
        let _ = fs::remove_file(&archive_path);
        fs::rename(&part_path, &archive_path)
            .with_context(|| "Failed to rename downloaded file")?;
    } else {
        let size = fs::metadata(&archive_path)?.len();
        tracing::info!("Using cached archive: {:.0} MB", size as f64 / (1024.0 * 1024.0));
    }

    tracing::info!("Extracting libraries ...");
    let extract_dir = temp_dir.join("extracted");
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir)?;

    let status = std::process::Command::new("tar")
        .args(["-xjf", &archive_path.to_string_lossy(), "-C", &extract_dir.to_string_lossy()])
        .output()
        .context("Failed to run tar command")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        let _ = fs::remove_file(&archive_path);
        let _ = fs::remove_file(&part_path);
        bail!("tar extraction failed: {}", stderr.trim());
    }

    let lib_dir = find_lib_dir(&extract_dir)?;
    copy_libs_from(&lib_dir, exe_dir)?;

    let _ = fs::remove_dir_all(&extract_dir);

    Ok(())
}

#[cfg(target_os = "windows")]
fn find_lib_dir(extract_dir: &Path) -> Result<PathBuf> {
    let primary = extract_dir.join(SHERPA_RELEASE_NAME).join("lib");
    if primary.is_dir() {
        return Ok(primary);
    }

    let secondary = extract_dir.join("lib");
    if secondary.is_dir() {
        return Ok(secondary);
    }

    if let Ok(entries) = fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let candidate = entry.path().join("lib");
                if candidate.is_dir() && dir_has_any_target_lib(&candidate) {
                    return Ok(candidate);
                }
                if let Ok(sub_entries) = fs::read_dir(&entry.path()) {
                    for sub in sub_entries.flatten() {
                        if sub.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            let sub_lib = sub.path().join("lib");
                            if sub_lib.is_dir() && dir_has_any_target_lib(&sub_lib) {
                                return Ok(sub_lib);
                            }
                        }
                    }
                }
            }
        }
    }

    bail!(
        "Could not find lib/ directory in extracted archive at {}",
        extract_dir.display()
    )
}

#[cfg(target_os = "windows")]
fn dir_has_any_target_lib(dir: &Path) -> bool {
    REQUIRED_LIBS.iter().any(|lib| dir.join(lib).exists())
}

#[cfg(target_os = "windows")]
fn copy_libs_from(src_dir: &Path, dst_dir: &Path) -> Result<()> {
    for lib in REQUIRED_LIBS {
        let src = src_dir.join(lib);
        let dst = dst_dir.join(lib);
        if src.exists() {
            fs::copy(&src, &dst)
                .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;
            let size = fs::metadata(&dst)?.len();
            tracing::info!("{} ({:.1} MB)", lib, size as f64 / (1024.0 * 1024.0));
        } else {
            bail!("Required library not found in archive: {}", lib);
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn exe_dir() -> Result<PathBuf> {
    std::env::current_exe()
        .context("Failed to get exe path")?
        .parent()
        .map(|p| p.to_path_buf())
        .context("Failed to get exe directory")
}
