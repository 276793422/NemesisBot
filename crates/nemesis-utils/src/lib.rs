//! NemesisBot utility library.
//!
//! Common utilities: string helpers, media detection, file operations,
//! message splitting, HTTP retry, skill parsing, zip handling.

pub mod file;
pub mod http_retry;
pub mod media;
pub mod message;
pub mod skills;
pub mod string_utils;
pub mod zip_util;

// Re-export the most commonly used functions
pub use string_utils::{
    truncate, random_id, random_short_id,
    format_timestamp, format_datetime_compact,
    parse_json, pretty_json,
    json_get_str, json_get_i64, json_get_f64, json_get_bool,
    is_blank, deref_str,
};
pub use media::{detect_media_type, is_audio_file, sanitize_filename, download_file, DownloadOptions, download_file_with_opts, download_file_simple};
pub use file::{write_file_atomic, ensure_dir, read_file_string};
pub use message::{split_message, format_message, sanitize_for_log};
pub use http_retry::{RetryConfig, should_retry, do_request_with_retry_simple, RetryableResponse, sleep_with_cancel, do_request_with_retry_reqwest};
pub use zip_util::{extract_zip, create_zip, is_path_within_dir};
pub use skills::{extract_slug, normalize_skill_name};
