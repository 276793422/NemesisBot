//! B1 真机臂：Windows 全新环境 QEMU 安装链实跑（下载→校验→7z 直解→落位→探针）。
//!
//! 全新环境模拟 = 全部落位/暂存/7z 缓存指向 `target/vm-arm/`（不碰既有
//! workspace 的 sandboxie 7z 缓存与 PATH 既有 QEMU——`seven_zip_dir` 指向
//! 空目录时 resolve_seven_zip 走系统 7-Zip 或现场下载 7z.zip）。
//!
//! 用法：
//! ```text
//! cargo run -p nemesis-sandbox --features vm --example vm_install_arm [--clean]
//! ```
//! `--clean` 先清 `target/vm-arm/`（真·全新环境）；缺省 = 幂等复跑
//! （已安装 → already_installed 早退验证）。
//!
//! 非.Windows = 诚实 SKIP（安装链是 Windows 面；Linux 走 B2 包管理链）。

fn main() {
    #[cfg(target_os = "windows")]
    windows::run();
    #[cfg(not(target_os = "windows"))]
    println!("SKIP: vm_install_arm 仅 Windows 真机臂（Linux 安装链归 B2 包管理路径）");
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::PathBuf;

    use nemesis_sandbox::vm::install::{
        QEMU_WINDOWS_SHA512, QEMU_WINDOWS_URL, QemuInstallOptions, install_qemu_windows,
    };

    pub fn run() {
        let clean = std::env::args().any(|a| a == "--clean");
        let base = PathBuf::from("target/vm-arm");
        if clean {
            let _ = std::fs::remove_dir_all(&base);
            println!("[arm] 已清 {base:?}（全新环境模拟）");
        }
        let runtime = base.join("runtime");
        let opts = QemuInstallOptions {
            url: QEMU_WINDOWS_URL.to_string(),
            expected_sha512: Some(QEMU_WINDOWS_SHA512.to_string()),
            install_dir: runtime.join("qemu"),
            work_dir: runtime.join("install-work"),
            seven_zip_dir: base.join("sandboxie-runtime"),
            parallel: 12,
        };

        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let report = rt
            .block_on(install_qemu_windows(&opts))
            .unwrap_or_else(|e| {
                eprintln!("[arm] 安装链失败: {e:#}");
                std::process::exit(1);
            });

        println!("[arm] already_installed = {}", report.already_installed);
        println!("[arm] install_dir       = {}", report.install_dir.display());
        println!("[arm] qemu_system       = {}", report.qemu_system.display());
        println!("[arm] qemu_img          = {}", report.qemu_img.display());
        println!("[arm] version           = {}", report.version);
        if !report.installer_sha256.is_empty() {
            println!(
                "[arm] installer_sha256  = {} (钉 manifest)",
                report.installer_sha256
            );
        }
        assert!(report.qemu_img.exists(), "qemu-img 未落位");
        assert!(
            report.version.contains("version"),
            "版本探针输出异常: {}",
            report.version
        );
        println!("[arm] PASS：安装链端到端可用（产物可执行）");
    }
}
