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

    // ------------------------------------------------------------------
    // 签名验证接入（verify_policy 锚注入，三级优先级）：
    //   1. env NEMESIS_BUILD_ROOT_ANCHOR（CI 的 Inject root anchor step 注入）
    //   2. 仓库 certs/root_cert.der 现算 SHA-256（本地开发者：证书入仓后
    //      无需手动设 env，锁定版本地构建直接可跑）
    //   3. 皆缺 → 不 emit（verify_policy::ROOT_ANCHOR = None，消费版降级
    //      off / 锁定版拒启）
    // rerun 指令防 stale：env 变化或根证书文件变化都会重跑本脚本重烧常量。
    // ------------------------------------------------------------------
    println!("cargo:rerun-if-env-changed=NEMESIS_BUILD_ROOT_ANCHOR");
    // rerun-if-changed 路径相对**包根**（nemesisbot/）而非仓库根——证书在
    // 仓库根 certs/ 下，必须带 ../ 前缀；否则密钥仪式落盘证书后不会触发
    // 重烧锚（无锚旧产物 stale 存活）。
    println!("cargo:rerun-if-changed=../certs/root_cert.der");
    let repo_root = Path::new(&manifest_dir).join("..");
    let anchor = match std::env::var("NEMESIS_BUILD_ROOT_ANCHOR") {
        Ok(v) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => {
            let cert = repo_root.join("certs").join("root_cert.der");
            match std::fs::read(&cert) {
                Ok(bytes) => {
                    use sha2::{Digest, Sha256};
                    let fp: String = Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect();
                    Some(fp)
                }
                Err(_) => None,
            }
        }
    };
    match anchor {
        Some(a) => println!("cargo:rustc-env=NEMESIS_ROOT_ANCHOR={}", a),
        // 显式 emit 空值：压掉编译环境里可能存在的同名 ambient 变量，
        // 保证「无锚」裁决只由本脚本的三级优先级决定（option_env! 侧
        // 把空串当 None）。
        None => println!("cargo:rustc-env=NEMESIS_ROOT_ANCHOR="),
    }

    // Re-run build script if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");

    // Embed icon on Windows
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("recourse/nemesisbot_multi.ico");
        res.compile().expect("failed to embed icon");
    }
}
