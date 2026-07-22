//! NemesisBot utility library.
//!
//! Common utilities: string helpers, media detection, file operations,
//! message splitting, HTTP retry, skill parsing, zip handling.

pub mod file;
pub mod http_retry;
pub mod media;
pub mod message;
pub mod platform;
pub mod skills;
pub mod string_utils;
pub mod zip_util;

// Re-export the most commonly used functions
pub use file::{ensure_dir, read_file_string, write_file_atomic};
pub use http_retry::{
    RetryConfig, RetryableResponse, do_request_with_retry_reqwest, do_request_with_retry_simple,
    should_retry, sleep_with_cancel,
};
pub use media::{
    DownloadOptions, detect_media_type, download_file, download_file_simple,
    download_file_with_opts, is_audio_file, sanitize_filename,
};
pub use message::{format_message, sanitize_for_log, split_message};
pub use platform::{
    find_plugin_library, find_plugin_library_in, plugin_library_filename, plugin_library_label,
};
pub use skills::{extract_slug, normalize_skill_name};
pub use string_utils::{
    deref_str, format_datetime_compact, format_timestamp, is_blank, json_get_bool, json_get_f64,
    json_get_i64, json_get_str, parse_json, pretty_json, random_id, random_short_id, truncate,
};
pub use zip_util::{create_zip, extract_zip, is_path_within_dir};
