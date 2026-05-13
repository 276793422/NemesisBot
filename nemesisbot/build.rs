//! Build script for nemesisbot binary.
//!
//! Injects version information via environment variables that are read at compile time:
//! - `NEMESISBOT_GIT_COMMIT`: Short git commit hash
//! - `NEMESISBOT_BUILD_TIME`: RFC 3339 build timestamp
//! - `NEMESISBOT_RUSTC_VERSION`: Rust compiler version string

use std::process::Command;

fn main() {
    // Git commit hash (short)
    let git_commit = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Build time (RFC 3339)
    let build_time = chrono::Local::now().to_rfc3339();

    // Rustc version
    let rust_version = Command::new("rustc")
        .args(["--version"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_default();

    // Set env vars for compile-time reading
    println!("cargo:rustc-env=NEMESISBOT_GIT_COMMIT={}", git_commit);
    println!("cargo:rustc-env=NEMESISBOT_BUILD_TIME={}", build_time);
    println!("cargo:rustc-env=NEMESISBOT_RUSTC_VERSION={}", rust_version);

    // Re-run build script if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
}
