//! P2-2 (2026-08-24 UI entry gap): compile-time embedded Python SDK artifacts.
//!
//! `build.rs` zips the filtered SDK tree (`test-tools/python/sdk/`, build
//! junk excluded — see build.rs for why not `include_dir!`) into two layouts
//! that the 「二次开发」 page serves over HTTP:
//!
//! - [`SDK_EXPORT_ZIP`] — tree at zip root (导出 SDK 目录).
//! - [`SDK_SDIST_ZIP`] — sdist layout under `nemesisbot-<version>/` (pip 包;
//!   `pip install ./<file>.zip` descends into the single top-level dir).
//!
//! Both are immutable build artifacts: serving them is zero-IO and the exe
//! is self-contained (only-the-exe machines get the full SDK).

/// Zip with the SDK source tree at the archive root.
pub const SDK_EXPORT_ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sdk_export.zip"));

/// Zip with the SDK source tree under `nemesisbot-<version>/` (sdist layout).
pub const SDK_SDIST_ZIP: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sdk_sdist.zip"));

/// SDK version parsed from `pyproject.toml` at build time (for filenames).
pub const SDK_VERSION: &str = env!("NEMESIS_SDK_VERSION");

#[cfg(test)]
mod tests;
