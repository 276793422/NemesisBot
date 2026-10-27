//! B1 真机臂：宿主统一入口 QEMU 安装链实跑（释放模板→解析矩阵→分派
//! archive/packages 链→落位→探针）。
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
//! 跨平台统一入口：Windows x64 命中 archive 臂（weilnetz NSIS 直解），
//! 其余平台命中 packages 臂（apt/dnf/pacman/Termux；半自动分支 = 诚实
//! 报错并给出精确命令，正是不伪装的实证面）。

use std::path::PathBuf;

use nemesis_sandbox::vm::install::{QemuInstallDirs, install_qemu_for_host_workspace};

fn main() {
    let clean = std::env::args().any(|a| a == "--clean");
    let base = PathBuf::from("target/vm-arm");
    if clean {
        let _ = std::fs::remove_dir_all(&base);
        println!("[arm] 已清 {base:?}（全新环境模拟）");
    }
    let runtime = base.join("runtime");
    // workspace config 指向 target/vm-arm/config（--clean 连配置模板一起重放
    // 释放语义；生产面 = <workspace>/config/）。
    let ws_config = base.join("config");
    let dirs = QemuInstallDirs {
        install_dir: runtime.join("qemu"),
        work_dir: runtime.join("install-work"),
        seven_zip_dir: base.join("sandboxie-runtime"),
        parallel: 12,
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let (report, meta) = rt
        .block_on(install_qemu_for_host_workspace(&ws_config, &dirs))
        .unwrap_or_else(|e| {
            eprintln!("[arm] 安装链失败: {e:#}");
            std::process::exit(1);
        });

    println!("[arm] platform          = {}", meta.platform_key);
    println!("[arm] kind              = {}", meta.kind.as_str());
    println!("[arm] matrix_source     = {:?}", meta.source);
    println!("[arm] released          = {}", meta.released);
    if !meta.note.is_empty() {
        println!("[arm] note              = {}", meta.note);
    }
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
