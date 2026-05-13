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
    let base = std::path::Path::new(filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    base.replace("..", "")
        .replace('/', "_")
        .replace('\\', "_")
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
mod tests {
    use super::*;

    #[test]
    fn test_detect_media_types() {
        assert_eq!(detect_media_type("photo.png"), "image/png");
        assert_eq!(detect_media_type("photo.jpg"), "image/jpeg");
        assert_eq!(detect_media_type("photo.JPEG"), "image/jpeg");
        assert_eq!(detect_media_type("audio.mp3"), "audio/mpeg");
        assert_eq!(detect_media_type("audio.wav"), "audio/wav");
        assert_eq!(detect_media_type("video.mp4"), "video/mp4");
        assert_eq!(detect_media_type("data.json"), "application/json");
        assert_eq!(detect_media_type("unknown.xyz"), "application/octet-stream");
    }

    #[test]
    fn test_type_checks() {
        assert!(is_image("image/png"));
        assert!(is_audio("audio/mpeg"));
        assert!(is_video("video/mp4"));
        assert!(!is_image("audio/mpeg"));
    }

    #[test]
    fn test_is_audio_file() {
        assert!(is_audio_file("song.mp3", ""));
        assert!(is_audio_file("", "audio/mpeg"));
        assert!(is_audio_file("rec.ogg", "application/ogg"));
        assert!(!is_audio_file("photo.png", ""));
    }

    #[test]
    fn test_sanitize_filename() {
        // Path traversal is stripped by file_name() extraction
        assert_eq!(sanitize_filename("../../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
        assert_eq!(sanitize_filename("path/to/file.txt"), "file.txt");
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_detect_media_types_all() {
        assert_eq!(detect_media_type("photo.png"), "image/png");
        assert_eq!(detect_media_type("photo.jpg"), "image/jpeg");
        assert_eq!(detect_media_type("photo.jpeg"), "image/jpeg");
        assert_eq!(detect_media_type("photo.gif"), "image/gif");
        assert_eq!(detect_media_type("photo.webp"), "image/webp");
        assert_eq!(detect_media_type("icon.svg"), "image/svg+xml");
        assert_eq!(detect_media_type("song.mp3"), "audio/mpeg");
        assert_eq!(detect_media_type("song.wav"), "audio/wav");
        assert_eq!(detect_media_type("song.ogg"), "audio/ogg");
        assert_eq!(detect_media_type("song.m4a"), "audio/mp4");
        assert_eq!(detect_media_type("song.flac"), "audio/flac");
        assert_eq!(detect_media_type("song.aac"), "audio/aac");
        assert_eq!(detect_media_type("song.wma"), "audio/x-ms-wma");
        assert_eq!(detect_media_type("video.mp4"), "video/mp4");
        assert_eq!(detect_media_type("video.webm"), "video/webm");
        assert_eq!(detect_media_type("doc.pdf"), "application/pdf");
        assert_eq!(detect_media_type("data.json"), "application/json");
        assert_eq!(detect_media_type("unknown.xyz"), "application/octet-stream");
    }

    #[test]
    fn test_detect_media_type_case_insensitive() {
        assert_eq!(detect_media_type("photo.PNG"), "image/png");
        assert_eq!(detect_media_type("photo.JPG"), "image/jpeg");
        assert_eq!(detect_media_type("photo.Jpeg"), "image/jpeg");
        assert_eq!(detect_media_type("song.MP3"), "audio/mpeg");
    }

    #[test]
    fn test_is_image_various() {
        assert!(is_image("image/png"));
        assert!(is_image("image/jpeg"));
        assert!(is_image("image/gif"));
        assert!(is_image("image/webp"));
        assert!(!is_image("audio/mpeg"));
        assert!(!is_image("video/mp4"));
        assert!(!is_image("application/json"));
    }

    #[test]
    fn test_is_audio_various() {
        assert!(is_audio("audio/mpeg"));
        assert!(is_audio("audio/wav"));
        assert!(is_audio("audio/ogg"));
        assert!(!is_audio("image/png"));
        assert!(!is_audio("video/mp4"));
    }

    #[test]
    fn test_is_video_various() {
        assert!(is_video("video/mp4"));
        assert!(is_video("video/webm"));
        assert!(!is_video("image/png"));
        assert!(!is_video("audio/mpeg"));
    }

    #[test]
    fn test_is_audio_file_by_extension() {
        assert!(is_audio_file("song.mp3", ""));
        assert!(is_audio_file("song.wav", ""));
        assert!(is_audio_file("song.ogg", ""));
        assert!(is_audio_file("song.m4a", ""));
        assert!(is_audio_file("song.flac", ""));
        assert!(is_audio_file("song.aac", ""));
        assert!(is_audio_file("song.wma", ""));
        assert!(!is_audio_file("photo.png", ""));
        assert!(!is_audio_file("video.mp4", ""));
        assert!(!is_audio_file("doc.pdf", ""));
    }

    #[test]
    fn test_is_audio_file_by_content_type() {
        assert!(is_audio_file("", "audio/mpeg"));
        assert!(is_audio_file("", "audio/wav"));
        assert!(is_audio_file("", "application/ogg"));
        assert!(is_audio_file("", "application/x-ogg"));
        assert!(!is_audio_file("", "image/png"));
        assert!(!is_audio_file("", "video/mp4"));
    }

    #[test]
    fn test_is_audio_file_case_insensitive() {
        assert!(is_audio_file("song.MP3", ""));
        assert!(is_audio_file("", "Audio/MPEG"));
    }

    #[test]
    fn test_sanitize_filename_removes_backslash() {
        assert_eq!(sanitize_filename("path\\to\\file.txt"), "file.txt");
    }

    #[test]
    fn test_sanitize_filename_removes_double_dot() {
        assert_eq!(sanitize_filename("file..name.txt"), "filename.txt");
    }

    #[test]
    fn test_sanitize_filename_no_extension() {
        assert_eq!(sanitize_filename("noextension"), "noextension");
    }

    #[test]
    fn test_sanitize_filename_simple() {
        assert_eq!(sanitize_filename("simple.txt"), "simple.txt");
    }

    #[test]
    fn test_detect_media_type_no_extension() {
        assert_eq!(detect_media_type("noextension"), "application/octet-stream");
    }

    #[test]
    fn test_detect_media_type_empty() {
        assert_eq!(detect_media_type(""), "application/octet-stream");
    }

    #[test]
    fn test_download_options_default() {
        let opts = DownloadOptions::default();
        assert_eq!(opts.timeout, Duration::from_secs(60));
        assert!(opts.extra_headers.is_empty());
        assert_eq!(opts.logger_prefix, "utils");
    }

    #[test]
    fn test_download_options_custom() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token".to_string());
        let opts = DownloadOptions {
            timeout: Duration::from_secs(120),
            extra_headers: headers,
            logger_prefix: "test".to_string(),
        };
        assert_eq!(opts.timeout, Duration::from_secs(120));
        assert_eq!(opts.extra_headers.len(), 1);
        assert_eq!(opts.logger_prefix, "test");
    }

    #[test]
    fn test_sanitize_filename_with_slashes() {
        assert_eq!(sanitize_filename("dir/subdir/file.txt"), "file.txt");
        assert_eq!(sanitize_filename("a/b"), "b");
    }

    #[test]
    fn test_sanitize_filename_with_dots() {
        // Double dots are removed after file_name extraction
        assert_eq!(sanitize_filename("file..txt"), "filetxt");
    }

    #[test]
    fn test_sanitize_filename_complex() {
        assert_eq!(sanitize_filename("../../../shell.php"), "shell.php");
        assert_eq!(sanitize_filename("normal-file_v2.txt"), "normal-file_v2.txt");
    }

    #[test]
    fn test_detect_media_type_various_unknowns() {
        assert_eq!(detect_media_type("file.tar.gz"), "application/octet-stream");
        assert_eq!(detect_media_type("file.zip"), "application/octet-stream");
        assert_eq!(detect_media_type("file.doc"), "application/octet-stream");
    }

    #[test]
    fn test_is_audio_file_both_extension_and_content() {
        // Both extension and content type indicate audio
        assert!(is_audio_file("song.mp3", "audio/mpeg"));
        // Extension is audio, content type is not
        assert!(is_audio_file("song.mp3", "image/png"));
        // Extension is not audio, content type is audio
        assert!(is_audio_file("file.bin", "audio/wav"));
        // Neither is audio
        assert!(!is_audio_file("file.bin", "image/png"));
    }

    #[test]
    fn test_is_audio_file_mixed_case_extension() {
        assert!(is_audio_file("song.Mp3", ""));
        assert!(is_audio_file("song.WAV", ""));
        assert!(is_audio_file("song.Ogg", ""));
    }

    #[test]
    fn test_is_audio_file_mixed_case_content_type() {
        assert!(is_audio_file("", "Audio/MPEG"));
        assert!(is_audio_file("", "AUDIO/WAV"));
        assert!(is_audio_file("", "Application/Ogg"));
        assert!(is_audio_file("", "application/X-Ogg"));
    }

    #[test]
    fn test_sanitize_filename_only_dots() {
        // file_name() on ".." returns "..", then replace("..", "") -> ""
        assert_eq!(sanitize_filename(".."), "");
    }

    #[test]
    fn test_sanitize_filename_empty() {
        assert_eq!(sanitize_filename(""), "");
    }

    #[test]
    fn test_sanitize_filename_with_special_chars() {
        // Characters that are safe but worth testing
        assert_eq!(sanitize_filename("file (1).txt"), "file (1).txt");
        assert_eq!(sanitize_filename("file@#$.txt"), "file@#$.txt");
    }

    #[test]
    fn test_sanitize_filename_windows_path() {
        assert_eq!(sanitize_filename("C:\\Users\\test\\file.txt"), "file.txt");
    }

    #[test]
    fn test_sanitize_filename_unix_path() {
        assert_eq!(sanitize_filename("/home/user/file.txt"), "file.txt");
    }

    #[test]
    fn test_download_options_zero_timeout() {
        // Test that zero timeout is handled in download_file_with_opts
        // (we just verify the struct can be created with zero timeout)
        let opts = DownloadOptions {
            timeout: Duration::from_secs(0),
            extra_headers: HashMap::new(),
            logger_prefix: String::new(),
        };
        assert!(opts.timeout.is_zero());
        assert!(opts.logger_prefix.is_empty());
    }

    #[test]
    fn test_download_options_with_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token123".to_string());
        headers.insert("X-Custom".to_string(), "value".to_string());
        let opts = DownloadOptions {
            timeout: Duration::from_secs(30),
            extra_headers: headers,
            logger_prefix: "media".to_string(),
        };
        assert_eq!(opts.extra_headers.len(), 2);
        assert_eq!(opts.extra_headers.get("Authorization").unwrap(), "Bearer token123");
    }

    #[test]
    fn test_detect_media_type_dot_in_name() {
        assert_eq!(detect_media_type("my.file.name.jpg"), "image/jpeg");
        assert_eq!(detect_media_type("archive.tar.gz"), "application/octet-stream");
    }

    #[test]
    fn test_all_image_types() {
        assert!(is_image("image/png"));
        assert!(is_image("image/jpeg"));
        assert!(is_image("image/gif"));
        assert!(is_image("image/webp"));
        assert!(is_image("image/svg+xml"));
        assert!(is_image("image/bmp"));
        // Empty string
        assert!(!is_image(""));
        // Just prefix is enough
        assert!(is_image("image/"));
    }

    #[test]
    fn test_all_audio_types() {
        assert!(is_audio("audio/mpeg"));
        assert!(is_audio("audio/wav"));
        assert!(is_audio("audio/ogg"));
        assert!(is_audio("audio/mp4"));
        assert!(is_audio("audio/flac"));
        assert!(is_audio("audio/"));
        assert!(!is_audio(""));
    }

    #[test]
    fn test_all_video_types() {
        assert!(is_video("video/mp4"));
        assert!(is_video("video/webm"));
        assert!(is_video("video/"));
        assert!(!is_video(""));
    }

    #[tokio::test]
    async fn test_download_file_bad_url() {
        let result = download_file("http://127.0.0.1:1/nonexistent.txt", "test.txt", Duration::from_millis(100)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_file_with_opts_bad_url() {
        let result = download_file_with_opts(
            "http://127.0.0.1:1/file.txt",
            "test.txt",
            DownloadOptions::default(),
        ).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_download_file_simple_bad_url() {
        let result = download_file_simple("http://127.0.0.1:1/file.txt", "test.txt").await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_download_file_with_opts_zero_timeout() {
        let opts = DownloadOptions {
            timeout: Duration::from_secs(0),
            extra_headers: HashMap::new(),
            logger_prefix: "test".to_string(),
        };
        // Zero timeout should be replaced with 60s default, but bad port will fail
        let result = download_file_with_opts(
            "http://127.0.0.1:1/file.txt",
            "test.txt",
            opts,
        ).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_download_file_with_opts_empty_logger_prefix() {
        let opts = DownloadOptions {
            timeout: Duration::from_millis(100),
            extra_headers: HashMap::new(),
            logger_prefix: String::new(),
        };
        let result = download_file_with_opts(
            "http://127.0.0.1:1/file.txt",
            "test.txt",
            opts,
        ).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_download_file_with_opts_extra_headers() {
        let mut headers = HashMap::new();
        headers.insert("X-Test".to_string(), "value".to_string());
        let opts = DownloadOptions {
            timeout: Duration::from_millis(100),
            extra_headers: headers,
            logger_prefix: "test".to_string(),
        };
        let result = download_file_with_opts(
            "http://127.0.0.1:1/file.txt",
            "test.txt",
            opts,
        ).await;
        assert!(result.is_empty());
    }

    #[test]
    fn test_sanitize_filename_null_char() {
        // Null bytes in filename
        assert_eq!(sanitize_filename("file\x00name.txt"), "file\x00name.txt");
    }

    #[test]
    fn test_detect_media_type_bmp() {
        assert_eq!(detect_media_type("icon.bmp"), "application/octet-stream");
    }

    #[test]
    fn test_download_options_clone() {
        let opts = DownloadOptions::default();
        let cloned = opts.clone();
        assert_eq!(opts.timeout, cloned.timeout);
        assert_eq!(opts.logger_prefix, cloned.logger_prefix);
    }

    // --- Additional coverage tests ---

    #[test]
    fn test_detect_media_type_empty_string() {
        assert_eq!(detect_media_type(""), "application/octet-stream");
    }

    #[test]
    fn test_is_image_edge_cases() {
        assert!(is_image("image/anything"));
        assert!(!is_image("application/json"));
        assert!(!is_image("text/plain"));
    }

    #[test]
    fn test_is_audio_edge_cases() {
        assert!(is_audio("audio/anything"));
        assert!(!is_audio("image/png"));
        assert!(!is_audio("video/mp4"));
    }

    #[test]
    fn test_is_video_edge_cases() {
        assert!(is_video("video/anything"));
        assert!(!is_video("audio/mpeg"));
        assert!(!is_video("image/png"));
    }

    #[test]
    fn test_is_audio_file_aac_extension() {
        assert!(is_audio_file("song.aac", ""));
    }

    #[test]
    fn test_is_audio_file_wma_extension() {
        assert!(is_audio_file("song.wma", ""));
    }

    #[test]
    fn test_is_audio_file_m4a_extension() {
        assert!(is_audio_file("song.m4a", ""));
    }

    #[test]
    fn test_is_audio_file_flac_extension() {
        assert!(is_audio_file("song.flac", ""));
    }

    #[test]
    fn test_is_audio_file_content_type_x_ogg() {
        assert!(is_audio_file("", "application/x-ogg"));
    }

    #[test]
    fn test_sanitize_filename_forward_slash() {
        assert_eq!(sanitize_filename("path/to/file.txt"), "file.txt");
    }

    #[test]
    fn test_sanitize_filename_backward_slash() {
        assert_eq!(sanitize_filename("path\\to\\file.txt"), "file.txt");
    }

    #[test]
    fn test_sanitize_filename_double_dot() {
        assert_eq!(sanitize_filename("file..name.txt"), "filename.txt");
    }

    #[tokio::test]
    async fn test_download_file_with_connection_refused() {
        let result = download_file(
            "http://127.0.0.1:1/nonexistent.txt",
            "test.txt",
            Duration::from_millis(100),
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_file_with_opts_default_timeout() {
        // Test with default options but unreachable URL
        let opts = DownloadOptions::default();
        let result = download_file_with_opts(
            "http://127.0.0.1:1/file.txt",
            "test.txt",
            opts,
        )
        .await;
        assert!(result.is_empty());
    }

    #[test]
    fn test_download_options_default_values() {
        let opts = DownloadOptions::default();
        assert_eq!(opts.timeout, Duration::from_secs(60));
        assert!(opts.extra_headers.is_empty());
        assert_eq!(opts.logger_prefix, "utils");
    }

    #[test]
    fn test_download_options_debug_format() {
        let opts = DownloadOptions::default();
        let debug_str = format!("{:?}", opts);
        assert!(debug_str.contains("DownloadOptions"));
        assert!(debug_str.contains("utils"));
    }

    #[test]
    fn test_detect_media_type_all_image_formats() {
        assert_eq!(detect_media_type("a.png"), "image/png");
        assert_eq!(detect_media_type("a.jpg"), "image/jpeg");
        assert_eq!(detect_media_type("a.jpeg"), "image/jpeg");
        assert_eq!(detect_media_type("a.gif"), "image/gif");
        assert_eq!(detect_media_type("a.webp"), "image/webp");
        assert_eq!(detect_media_type("a.svg"), "image/svg+xml");
    }

    #[test]
    fn test_detect_media_type_all_audio_formats() {
        assert_eq!(detect_media_type("a.mp3"), "audio/mpeg");
        assert_eq!(detect_media_type("a.wav"), "audio/wav");
        assert_eq!(detect_media_type("a.ogg"), "audio/ogg");
        assert_eq!(detect_media_type("a.m4a"), "audio/mp4");
        assert_eq!(detect_media_type("a.flac"), "audio/flac");
        assert_eq!(detect_media_type("a.aac"), "audio/aac");
        assert_eq!(detect_media_type("a.wma"), "audio/x-ms-wma");
    }

    #[test]
    fn test_detect_media_type_video_formats() {
        assert_eq!(detect_media_type("a.mp4"), "video/mp4");
        assert_eq!(detect_media_type("a.webm"), "video/webm");
    }

    #[test]
    fn test_detect_media_type_document_formats() {
        assert_eq!(detect_media_type("a.pdf"), "application/pdf");
        assert_eq!(detect_media_type("a.json"), "application/json");
    }

    #[test]
    fn test_is_audio_file_by_extension_all() {
        assert!(is_audio_file("a.mp3", ""));
        assert!(is_audio_file("a.wav", ""));
        assert!(is_audio_file("a.ogg", ""));
        assert!(is_audio_file("a.m4a", ""));
        assert!(is_audio_file("a.flac", ""));
        assert!(is_audio_file("a.aac", ""));
        assert!(is_audio_file("a.wma", ""));
    }

    #[test]
    fn test_is_audio_file_by_content_type_all() {
        assert!(is_audio_file("", "audio/mpeg"));
        assert!(is_audio_file("", "audio/wav"));
        assert!(is_audio_file("", "audio/ogg"));
        assert!(is_audio_file("", "application/ogg"));
        assert!(is_audio_file("", "application/x-ogg"));
    }

    #[test]
    fn test_sanitize_filename_various_paths() {
        assert_eq!(sanitize_filename("/tmp/test.txt"), "test.txt");
        assert_eq!(sanitize_filename("C:\\Users\\test.txt"), "test.txt");
        assert_eq!(sanitize_filename("../secret.txt"), "secret.txt");
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
    }

    #[tokio::test]
    async fn test_download_file_invalid_url() {
        let result = download_file("not_a_url", "test.txt", Duration::from_secs(5)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_file_simple_invalid_url() {
        let result = download_file_simple("http://127.0.0.1:1/nonexistent", "test.txt").await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_download_file_with_opts_zero_timeout_custom_prefix() {
        let opts = DownloadOptions {
            timeout: Duration::ZERO,
            extra_headers: HashMap::new(),
            logger_prefix: "test".to_string(),
        };
        // Should use default 60s timeout
        let result = download_file_with_opts("http://127.0.0.1:1/file", "test.txt", opts).await;
        assert!(result.is_empty());
    }

    #[test]
    fn test_download_options_with_custom_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer test".to_string());
        headers.insert("X-Custom".to_string(), "value".to_string());
        let opts = DownloadOptions {
            timeout: Duration::from_secs(30),
            extra_headers: headers,
            logger_prefix: "custom".to_string(),
        };
        assert_eq!(opts.extra_headers.len(), 2);
        assert_eq!(opts.logger_prefix, "custom");
    }
}
