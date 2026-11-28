//! Media type detection, audio checks, filename sanitization, and file download utilities.
// http 门控：模块核心是 reqwest 下载（download_file*）——未启用
// `http` feature 的消费方（nemesis-guest musl 交叉构建）整个模块不编译。
// detect_media_type 等纯函数当前零外部消费者，随模门控；将来有消费者再拆。
#![cfg(feature = "http")]

use std::collections::HashMap;
use std::time::Duration;

/// 媒体下载响应体上限（BUG-20261128-030）：BUG-20261101-273 /
/// BUG-20261107-005 下载加固横扫的漏网面——本模块 download 家族此前
/// `resp.bytes()` 无帽全量读入，恶意/失控源可把任意大响应灌进内存。
/// 300MB 对齐 selfupdate 资产档（二进制资产族：skills ZIP 50MB <
/// selfupdate 300MB < 模型 2GiB）；IM 媒体附件合法上限远低于此
/// （Telegram 下载侧 20MB 级），帽只防内存放大不伤合法面。
const MAX_MEDIA_DOWNLOAD_BYTES: u64 = 300 * 1024 * 1024;

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
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".mp3") {
        "audio/mpeg"
    } else if lower.ends_with(".wav") {
        "audio/wav"
    } else if lower.ends_with(".ogg") {
        "audio/ogg"
    } else if lower.ends_with(".m4a") {
        "audio/mp4"
    } else if lower.ends_with(".flac") {
        "audio/flac"
    } else if lower.ends_with(".aac") {
        "audio/aac"
    } else if lower.ends_with(".wma") {
        "audio/x-ms-wma"
    } else if lower.ends_with(".mp4") {
        "video/mp4"
    } else if lower.ends_with(".webm") {
        "video/webm"
    } else if lower.ends_with(".pdf") {
        "application/pdf"
    } else if lower.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
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
    base.replace("..", "").replace('/', "_")
}

/// Download a file from a URL to a local temp directory.
/// Returns the local file path or an error.
pub async fn download_file(
    url: &str,
    filename: &str,
    timeout: std::time::Duration,
) -> Result<String, String> {
    download_file_inner(
        url,
        filename,
        timeout,
        &HashMap::new(),
        MAX_MEDIA_DOWNLOAD_BYTES,
    )
    .await
}

/// 下载内核（BUG-20261128-030 单一实现）：带帽读体（[`crate::http_cap`]
/// 读入中途超限即断，替代旧 `resp.bytes()` 无帽全量）+ 不跨主机跟随
/// 重定向（[`crate::http_redirect`] 单一真相源——extra_headers 可含
/// Authorization 类凭据头，reqwest 默认策略跨主机只剥标准四头，自定义
/// 凭据头会随 302 流向第三方主机；同源跟随保留）。`max_bytes` 显式
/// 传入（测试缝：小帽实跑帽闸；公开入口恒 [`MAX_MEDIA_DOWNLOAD_BYTES`]）。
async fn download_file_inner(
    url: &str,
    filename: &str,
    timeout: Duration,
    extra_headers: &HashMap<String, String>,
    max_bytes: u64,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(crate::http_redirect::no_cross_host_redirect_policy())
        .build()
        .map_err(|e| format!("create client: {}", e))?;

    let mut request = client.get(url);
    for (key, value) in extra_headers {
        request = request.header(key.as_str(), value.as_str());
    }

    let resp = request
        .send()
        .await
        .map_err(|e| format!("download: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let bytes = crate::http_cap::read_body_capped(resp, max_bytes)
        .await
        .map_err(|e| format!("read body: {e}"))?;

    let media_dir = std::env::temp_dir().join("nemesisbot_media");
    std::fs::create_dir_all(&media_dir).map_err(|e| format!("mkdir: {}", e))?;

    let safe_name = sanitize_filename(filename);
    let id = crate::string_utils::random_short_id();
    let local_path = media_dir.join(format!("{}_{}", id, safe_name));

    std::fs::write(&local_path, &bytes).map_err(|e| format!("write: {}", e))?;

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

    match download_file_inner(
        url,
        filename,
        timeout,
        &opts.extra_headers,
        MAX_MEDIA_DOWNLOAD_BYTES,
    )
    .await
    {
        Ok(path) => {
            tracing::debug!(prefix = logger_prefix, path = %path, "File downloaded successfully");
            path
        }
        Err(stage) => {
            tracing::error!(prefix = logger_prefix, url = url, stage = %stage, "Failed to download file");
            String::new()
        }
    }
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
mod cov_tests;
#[cfg(test)]
mod tests;
