//! Model download and path management
//!
//! Reads model sources from config.toml, downloads from configured mirror on first use.
//! Models stored in {data_dir}/{category}/{model_name}/

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::AppConfig;

// ---------------------------------------------------------------------------
// Progress reporting — thread-local callback for UI progress updates
// ---------------------------------------------------------------------------

thread_local! {
    static PROGRESS_CB: RefCell<Option<Box<dyn Fn(&str) + Send + 'static>>> = RefCell::new(None);
}

/// Set a progress callback for the current thread's download operations.
/// Called with messages like "model.onnx ... 70% (217.4/310.5 MB, 7066 KB/s)".
pub fn set_progress(cb: Option<Box<dyn Fn(&str) + Send + 'static>>) {
    PROGRESS_CB.with(|p| *p.borrow_mut() = cb);
}

fn report_progress(msg: &str) {
    PROGRESS_CB.with(|p| {
        if let Some(ref cb) = *p.borrow() {
            cb(msg);
        }
    });
}

/// Check if a model directory has at least the expected files.
fn check_model_files(dir: &Path, expected_files: &[(&str, &str)]) -> bool {
    if !dir.exists() {
        return false;
    }
    expected_files
        .iter()
        .all(|(local, _)| dir.join(local).exists())
}

/// Build download URL from mirror base + repo + filename
fn build_url(mirror_base: &str, repo: &str, filename: &str) -> String {
    format!(
        "{}/{}/resolve/main/{}",
        mirror_base.trim_end_matches('/'),
        repo,
        filename
    )
}

/// Ensure STT model is ready. Returns the model directory path.
pub fn ensure_stt_model(cfg: &AppConfig) -> Result<PathBuf> {
    let model_name = &cfg.stt.model_name;
    let dir = cfg.model_dir().join("stt").join(model_name);

    let source = cfg.find_model_source(model_name);
    let files: Vec<(&str, &str)> = source
        .map(|s| {
            s.files
                .iter()
                .map(|f| (f.local.as_str(), f.remote.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !files.is_empty() && check_model_files(&dir, &files) {
        tracing::info!("[STT] Model '{}' found at {}", model_name, dir.display());
        return Ok(dir);
    }

    if !cfg.models.auto_download {
        anyhow::bail!(
            "STT model '{}' not found at {} and auto_download is disabled",
            model_name,
            dir.display()
        );
    }

    let source = source.context(format!(
        "STT model '{}' not found in config [models.sources]. Add it to config.toml.",
        model_name
    ))?;

    download_model_files(
        &cfg.models.mirror.base,
        &source.name,
        &source.repo,
        &source.files,
        &dir,
        &cfg.models.proxy.url,
    )?;

    Ok(dir)
}

/// Ensure VAD model is ready. Returns the model file path (not directory).
pub fn ensure_vad_model(cfg: &AppConfig) -> Result<PathBuf> {
    let model_name = &cfg.vad.model_name;
    let dir = cfg.model_dir().join("vad").join(model_name);

    let source = cfg.find_model_source(model_name);
    let files: Vec<(&str, &str)> = source
        .map(|s| {
            s.files
                .iter()
                .map(|f| (f.local.as_str(), f.remote.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !files.is_empty() && check_model_files(&dir, &files) {
        let model_file = dir.join(files[0].0);
        tracing::info!(
            "[VAD] Model '{}' found at {}",
            model_name,
            model_file.display()
        );
        return Ok(model_file);
    }

    if !cfg.models.auto_download {
        anyhow::bail!(
            "VAD model '{}' not found and auto_download is disabled",
            model_name
        );
    }

    let source = source.context(format!(
        "VAD model '{}' not found in config [models.sources]. Add it to config.toml.",
        model_name
    ))?;

    download_model_files(
        &cfg.models.mirror.base,
        &source.name,
        &source.repo,
        &source.files,
        &dir,
        &cfg.models.proxy.url,
    )?;

    // Return the first file (the model itself)
    Ok(dir.join(&source.files[0].local))
}

/// Ensure TTS model is ready. Returns the model directory path.
pub fn ensure_tts_model(cfg: &AppConfig) -> Result<PathBuf> {
    let model_name = &cfg.tts.model_name;
    let dir = cfg.model_dir().join("tts").join(model_name);

    let source = cfg.find_model_source(model_name);
    let files: Vec<(&str, &str)> = source
        .map(|s| {
            s.files
                .iter()
                .map(|f| (f.local.as_str(), f.remote.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !files.is_empty() && check_model_files(&dir, &files) {
        tracing::info!("[TTS] Model '{}' found at {}", model_name, dir.display());
        return Ok(dir);
    }

    if !cfg.models.auto_download {
        anyhow::bail!(
            "TTS model '{}' not found and auto_download is disabled",
            model_name
        );
    }

    let source = source.context(format!(
        "TTS model '{}' not found in config [models.sources]. Add it to config.toml.",
        model_name
    ))?;

    download_model_files(
        &cfg.models.mirror.base,
        &source.name,
        &source.repo,
        &source.files,
        &dir,
        &cfg.models.proxy.url,
    )?;

    Ok(dir)
}

/// Ensure punctuation model is ready. Returns the model directory path.
pub fn ensure_punct_model(cfg: &AppConfig) -> Result<PathBuf> {
    let model_name = &cfg.punct.model_name;
    let dir = cfg.model_dir().join("punct").join(model_name);

    let source = cfg.find_model_source(model_name);
    let files: Vec<(&str, &str)> = source
        .map(|s| {
            s.files
                .iter()
                .map(|f| (f.local.as_str(), f.remote.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !files.is_empty() && check_model_files(&dir, &files) {
        tracing::info!("[Punct] Model '{}' found at {}", model_name, dir.display());
        return Ok(dir);
    }

    if !cfg.models.auto_download {
        anyhow::bail!(
            "Punctuation model '{}' not found and auto_download is disabled",
            model_name
        );
    }

    let source = source.context(format!(
        "Punctuation model '{}' not found in config [models.sources]. Add it to config.toml.",
        model_name
    ))?;

    download_model_files(
        &cfg.models.mirror.base,
        &source.name,
        &source.repo,
        &source.files,
        &dir,
        &cfg.models.proxy.url,
    )?;

    Ok(dir)
}

/// Ensure speaker embedding model is ready. Returns the model directory path.
pub fn ensure_speaker_model(cfg: &AppConfig) -> Result<PathBuf> {
    let model_name = &cfg.speaker.model_name;
    let dir = cfg.model_dir().join("speaker").join(model_name);

    let source = cfg.find_model_source(model_name);
    let files: Vec<(&str, &str)> = source
        .map(|s| {
            s.files
                .iter()
                .map(|f| (f.local.as_str(), f.remote.as_str()))
                .collect()
        })
        .unwrap_or_default();

    if !files.is_empty() && check_model_files(&dir, &files) {
        tracing::info!(
            "[Speaker] Model '{}' found at {}",
            model_name,
            dir.display()
        );
        return Ok(dir);
    }

    if !cfg.models.auto_download {
        anyhow::bail!(
            "Speaker model '{}' not found and auto_download is disabled",
            model_name
        );
    }

    let source = source.context(format!(
        "Speaker model '{}' not found in config [models.sources]. Add it to config.toml.",
        model_name
    ))?;

    download_model_files(
        &cfg.models.mirror.base,
        &source.name,
        &source.repo,
        &source.files,
        &dir,
        &cfg.models.proxy.url,
    )?;

    Ok(dir)
}

/// Download all files for a model from the configured mirror with resume support.
/// Downloads to `.part` temp files, renames to final name on completion.
#[cfg(feature = "download")]
fn download_model_files(
    mirror_base: &str,
    model_name: &str,
    repo: &str,
    files: &[crate::config::ModelFile],
    target_dir: &Path,
    proxy_url: &str,
) -> Result<()> {
    fs::create_dir_all(target_dir)
        .with_context(|| format!("Failed to create directory: {}", target_dir.display()))?;

    let source = if repo.is_empty() { "direct URL" } else { repo };
    tracing::info!("[{}] Downloading from {} ...", model_name, source);
    report_progress(&format!("开始下载 {} ...", model_name));

    let mut client_builder = reqwest::blocking::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36 Edg/148.0.0.0")
        .timeout(std::time::Duration::from_secs(600))
        .connect_timeout(std::time::Duration::from_secs(30));

    if !proxy_url.trim().is_empty() {
        let proxy = reqwest::Proxy::all(proxy_url)
            .with_context(|| format!("Invalid proxy URL: {}", proxy_url))?;
        tracing::info!("Using proxy: {}", proxy_url);
        client_builder = client_builder.proxy(proxy);
    }

    let client = client_builder
        .build()
        .context("Failed to create HTTP client")?;

    for file in files {
        let target_file = target_dir.join(&file.local);
        // Create parent directories for files in subdirectories (e.g., dict/file)
        if let Some(parent) = target_file.parent() {
            let _ = fs::create_dir_all(parent);
        }

        // Final file exists = fully downloaded before, skip
        if target_file.exists() {
            let size = fs::metadata(&target_file)?.len();
            if size > 0 {
                let msg = format!(
                    "{} (已存在, {:.1} MB)",
                    file.local,
                    size as f64 / (1024.0 * 1024.0)
                );
                tracing::info!("{}", msg);
                report_progress(&msg);
                continue;
            }
        }

        let url = if !file.url.is_empty() {
            file.url.clone()
        } else {
            build_url(mirror_base, repo, &file.remote)
        };

        let part_file = {
            let mut p = target_file.as_os_str().to_owned();
            p.push(".part");
            PathBuf::from(p)
        };

        download_file_resume(&client, &url, &target_file, &part_file, &file.local)?;
    }

    tracing::info!("[{}] Download complete.", model_name);
    report_progress(&format!("{} 下载完成", model_name));
    Ok(())
}

/// Get remote file size via HEAD request. Returns None if not available.
#[cfg(feature = "download")]
fn get_remote_size(client: &reqwest::blocking::Client, url: &str) -> Option<u64> {
    let resp = client.head(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
}

/// Download a single file with resume support.
/// Downloads to `part_file`, renames to `target_file` on completion.
/// If `part_file` exists, resumes from its current size using HTTP Range.
#[cfg(feature = "download")]
fn download_file_resume(
    client: &reqwest::blocking::Client,
    url: &str,
    target_file: &Path,
    part_file: &Path,
    display_name: &str,
) -> Result<()> {
    let existing_size = if part_file.exists() {
        fs::metadata(part_file).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let total_size = get_remote_size(client, url);

    // Try resuming if we have a partial file
    if existing_size > 0 {
        // Check if already complete
        if total_size.map_or(false, |t| existing_size >= t) {
            fs::rename(part_file, target_file)
                .with_context(|| format!("Failed to rename {}", display_name))?;
            let msg = format!(
                "{} (完整, {:.1} MB)",
                display_name,
                existing_size as f64 / (1024.0 * 1024.0)
            );
            tracing::info!("{}", msg);
            report_progress(&msg);
            return Ok(());
        }

        let range_header = format!("bytes={}-", existing_size);
        let resp = client
            .get(url)
            .header(reqwest::header::RANGE, &range_header)
            .send()
            .with_context(|| format!("Failed to download: {}", url))?;

        if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let remaining = total_size.map_or("?".to_string(), |t| {
                format!("{:.1} MB", (t - existing_size) as f64 / (1024.0 * 1024.0))
            });
            let msg = format!(
                "{} (续传 {:.1} MB, 剩余 {})",
                display_name,
                existing_size as f64 / (1024.0 * 1024.0),
                remaining
            );
            tracing::info!(
                "{} <- {} (resuming from {:.1} MB, {} remaining)",
                display_name,
                url,
                existing_size as f64 / (1024.0 * 1024.0),
                remaining
            );
            report_progress(&msg);

            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(part_file)
                .with_context(|| format!("Failed to open for append: {}", part_file.display()))?;

            stream_to_file(&mut file, resp, existing_size, total_size, display_name)?;

            fs::rename(part_file, target_file)
                .with_context(|| format!("Failed to rename {}", display_name))?;
            return Ok(());
        }
        // Server didn't support Range — fall through to full download, overwrite .part
    }

    // Full download
    tracing::info!("{} <- {}", display_name, url);
    report_progress(&format!("{} 开始下载...", display_name));

    let resp = client
        .get(url)
        .send()
        .with_context(|| format!("Failed to download: {}", url))?;

    if !resp.status().is_success() {
        anyhow::bail!("Download failed (HTTP {}): {}", resp.status(), url);
    }

    let mut file = fs::File::create(part_file)
        .with_context(|| format!("Failed to create: {}", part_file.display()))?;

    stream_to_file(&mut file, resp, 0, total_size, display_name)?;

    fs::rename(part_file, target_file)
        .with_context(|| format!("Failed to rename {}", display_name))?;

    Ok(())
}

/// Stream HTTP response to file with periodic flush and progress display.
#[cfg(feature = "download")]
fn stream_to_file(
    file: &mut fs::File,
    mut response: reqwest::blocking::Response,
    start_offset: u64,
    total_size: Option<u64>,
    display_name: &str,
) -> Result<()> {
    let start = Instant::now();
    let mut written: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    let mut last_flush: u64 = 0;
    const FLUSH_INTERVAL: u64 = 4 * 1024 * 1024; // flush every 4 MB
    let mut last_print = Instant::now();

    loop {
        match response.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                file.write_all(&buf[..n])
                    .with_context(|| format!("Write failed for {}", display_name))?;
                written += n as u64;

                // Periodic flush to disk so data survives a crash
                if written - last_flush >= FLUSH_INTERVAL {
                    let _ = file.sync_data();
                    last_flush = written;
                }

                let now = Instant::now();
                if now.duration_since(last_print).as_secs() >= 2 {
                    let elapsed = now.duration_since(start).as_secs_f64();
                    let downloaded = start_offset + written;
                    let speed_kbs = written as f64 / elapsed / 1024.0;
                    if let Some(total) = total_size {
                        let pct = downloaded as f64 / total as f64 * 100.0;
                        let msg = format!(
                            "{} ... {:.0}% ({:.1}/{:.1} MB, {:.0} KB/s)",
                            display_name,
                            pct,
                            downloaded as f64 / (1024.0 * 1024.0),
                            total as f64 / (1024.0 * 1024.0),
                            speed_kbs
                        );
                        tracing::info!("{}", msg);
                        report_progress(&msg);
                    } else {
                        let msg = format!(
                            "{} ... {:.1} MB downloaded ({:.0} KB/s)",
                            display_name,
                            downloaded as f64 / (1024.0 * 1024.0),
                            speed_kbs
                        );
                        tracing::info!("{}", msg);
                        report_progress(&msg);
                    }
                    last_print = now;
                }
            }
            Err(e) => {
                let _ = file.sync_data();
                anyhow::bail!(
                    "Download interrupted for {}: {}. Re-run to resume.",
                    display_name,
                    e
                );
            }
        }
    }

    let _ = file.sync_data();

    let elapsed = start.elapsed().as_secs_f64();
    let total_downloaded = start_offset + written;
    let speed_kbs = if elapsed > 0.0 {
        written as f64 / elapsed / 1024.0
    } else {
        0.0
    };
    let msg = format!(
        "{} 下载完成 ({:.1} MB, {:.0} KB/s)",
        display_name,
        total_downloaded as f64 / (1024.0 * 1024.0),
        speed_kbs
    );
    tracing::info!("{}", msg);
    report_progress(&msg);

    Ok(())
}

#[cfg(not(feature = "download"))]
fn download_model_files(
    _mirror_base: &str,
    _model_name: &str,
    _repo: &str,
    _files: &[crate::config::ModelFile],
    _target_dir: &Path,
    _proxy_url: &str,
) -> Result<()> {
    anyhow::bail!("Model download requires 'download' feature (enabled by default)");
}
