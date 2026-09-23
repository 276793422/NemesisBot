//! Build script for nemesisbot binary.
//!
//! Injects version information via environment variables that are read at compile time:
//! - `NEMESISBOT_GIT_COMMIT`: Short git commit hash
//! - `NEMESISBOT_BUILD_TIME`: RFC 3339 build timestamp
//! - `NEMESISBOT_RUSTC_VERSION`: Rust compiler version string

use std::path::Path;
use std::process::Command;

fn main() {
    // BUILD-001（2026-09-22 审查）：静态前端产物缺失时给出可读报错。
    // `embedded.rs` 的 `include_dir!` 编译期硬要求 crates/nemesis-web/static/
    //（gitignored 构建产物，干净 clone 必然缺失），proc-macro 的 panic 信息
    // 不指向根因——在这里先挡下并直接告知修复命令。CI 与一键构建脚本总是
    // 先执行 npm build，不受影响。
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let static_dir = Path::new(&manifest_dir).join("../crates/nemesis-web/static");
    if !static_dir.is_dir() {
        println!(
            "cargo:error=nemesisbot 编译需要前端构建产物 crates/nemesis-web/static/（当前缺失）"
        );
        println!(
            "cargo:error=请先构建前端: npm --prefix web install && npm --prefix web run build"
        );
        println!("cargo:error=或使用一键脚本: scripts/build-windows.bat / scripts/build-linux.sh");
        std::process::exit(1);
    }

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

    // Embed icon on Windows
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("recourse/nemesisbot_multi.ico");
        res.compile().expect("failed to embed icon");
    }
}
