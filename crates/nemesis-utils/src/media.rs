//! Media type detection, audio checks, filename sanitization, and file download utilities.

use std::collections::HashMap;
use std::time::Duration;

/// Options for downloading files.
///
/// Mirrors the Go `DownloadOptions` struct from `module/utils/media.go`.
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Request timeout. Defaults to 60 seconds if zero.
    pub timeout: Duration,
    /// Extra HTTP headers (e.g., Authorization for Slack).
    pub extra_headers: HashMap<String, String>,
    /// Prefix for log messages. Defaults to "utils" if empty.
    pub logger_prefix: String,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            extra_headers: HashMap::new(),
            logger_prefix: "utils".to_string(),
        }
    }
}

/// Detect media type from file extension.
pub fn detect_media_type(filename: &str) -> &str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".png") { "image/png" }
    else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") { "image/jpeg" }
    else if lower.ends_with(".gif") { "image/gif" }
    else if lower.ends_with(".webp") { "image/webp" }
    else if lower.ends_with(".svg") { "image/svg+xml" }
    else if lower.ends_with(".mp3") { "audio/mpeg" }
    else if lower.ends_with(".wav") { "audio/wav" }
    else if lower.ends_with(".ogg") { "audio/ogg" }
    else if lower.ends_with(".m4a") { "audio/mp4" }
    else if lower.ends_with(".flac") { "audio/flac" }
    else if lower.ends_with(".aac") { "audio/aac" }
    else if lower.ends_with(".wma") { "audio/x-ms-wma" }
    else if lower.ends_with(".mp4") { "video/mp4" }
    else if lower.ends_with(".webm") { "video/webm" }
    else if lower.ends_with(".pdf") { "application/pdf" }
    else if lower.ends_with(".json") { "application/json" }
    else { "application/octet-stream" }
}

/// Check if a media type is an image.
pub fn is_image(media_type: &str) -> bool {
    media_type.starts_with("image/")
}

/// Check if a media type is audio.
pub fn is_audio(media_type: &str) -> bool {
    media_type.starts_with("audio/")
}

/// Check if a media type is video.
pub fn is_video(media_type: &str) -> bool {
    media_type.starts_with("video/")
}

const AUDIO_EXTENSIONS: &[&str] = &[".mp3", ".wav", ".ogg", ".m4a", ".flac", ".aac", ".wma"];
const AUDIO_CONTENT_PREFIXES: &[&str] = &["audio/", "application/ogg", "application/x-ogg"];

/// Check if a file is an audio file based on its filename extension and content type.
pub fn is_audio_file(filename: &str, content_type: &str) -> bool {
    let lower_filename = filename.to_lowercase();
    for ext in AUDIO_EXTENSIONS {
        if lower_filename.ends_with(ext) {
            return true;
        }
    }
    let lower_content = content_type.to_lowercase();
    for prefix in AUDIO_CONTENT_PREFIXES {
        if lower_content.starts_with(prefix) {
            return true;
        }
    }
    false
}

/// Sanitize a filename by removing dangerous characters and path traversal attempts.
pub fn sanitize_filename(filename: &str) -> String {
    // Normalize backslashes to forward slashes so that Path::file_name()
    // extracts the last segment correctly on all platforms.
    let normalized = filename.replace('\\', "/");
    let base = std::path::Path::new(&normalized)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    base.replace("..", "")
        .replace('/', "_")
}

/// Download a file from a URL to a local temp directory.
/// Returns the local file path or an error.
pub async fn download_file(url: &str, filename: &str, timeout: std::time::Duration) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("create client: {}", e))?;

    let resp = client.get(url)
        .send()
        .await
        .map_err(|e| format!("download: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let bytes = resp.bytes()
        .await
        .map_err(|e| format!("read body: {}", e))?;

    let media_dir = std::env::temp_dir().join("nemesisbot_media");
    std::fs::create_dir_all(&media_dir)
        .map_err(|e| format!("mkdir: {}", e))?;

    let safe_name = sanitize_filename(filename);
    let id = crate::string_utils::random_short_id();
    let local_path = media_dir.join(format!("{}_{}", id, safe_name));

    std::fs::write(&local_path, &bytes)
        .map_err(|e| format!("write: {}", e))?;

    Ok(local_path.to_string_lossy().to_string())
}

/// Download a file from a URL to a local temp directory with full options.
///
/// Mirrors the Go `DownloadFile(url, filename, opts)` function. Downloads the
/// file with configurable timeout, extra headers, and logging prefix. The file
/// is saved with a UUID prefix to prevent name conflicts.
///
/// # Arguments
/// * `url` - The URL to download from
/// * `filename` - The target filename (will be sanitized)
/// * `opts` - Download options (timeout, headers, log prefix)
///
/// # Returns
/// The local file path on success, or an empty string on error.
pub async fn download_file_with_opts(url: &str, filename: &str, opts: DownloadOptions) -> String {
    let timeout = if opts.timeout.is_zero() {
        Duration::from_secs(60)
    } else {
        opts.timeout
    };
    let logger_prefix = if opts.logger_prefix.is_empty() {
        "utils"
    } else {
        &opts.logger_prefix
    };

    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(prefix = logger_prefix, error = %e, "Failed to create download client");
            return String::new();
        }
    };

    let mut request = client.get(url);
    for (key, value) in &opts.extra_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let resp = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(prefix = logger_prefix, error = %e, url = url, "Failed to download file");
            return String::new();
        }
    };

    if !resp.status().is_success() {
        tracing::error!(
            prefix = logger_prefix,
            status = resp.status().as_u16(),
            url = url,
            "File download returned non-200 status"
        );
        return String::new();
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(prefix = logger_prefix, error = %e, "Failed to read response body");
            return String::new();
        }
    };

    let media_dir = std::env::temp_dir().join("nemesisbot_media");
    if let Err(e) = std::fs::create_dir_all(&media_dir) {
        tracing::error!(prefix = logger_prefix, error = %e, "Failed to create media directory");
        return String::new();
    }

    let safe_name = sanitize_filename(filename);
    let id = crate::string_utils::random_short_id();
    let local_path = media_dir.join(format!("{}_{}", id, safe_name));

    if let Err(e) = std::fs::write(&local_path, &bytes) {
        tracing::error!(prefix = logger_prefix, error = %e, "Failed to write file");
        return String::new();
    }

    tracing::debug!(prefix = logger_prefix, path = %local_path.display(), "File downloaded successfully");

    local_path.to_string_lossy().to_string()
}

/// Download a file from a URL with default options.
///
/// Convenience wrapper that mirrors the Go `DownloadFileSimple(url, filename)` function.
pub async fn download_file_simple(url: &str, filename: &str) -> String {
    download_file_with_opts(
        url,
        filename,
        DownloadOptions {
            logger_prefix: "media".to_string(),
            ..Default::default()
        },
    )
    .await
}

#[cfg(test)]
mod tests;
