//! VM 环境自举 B2 真机臂：probe 状态机在真实宿主上的直证（双端）。
//!
//! 每步打印状态与判定（除 Linux 安装链直证外全部只读）：
//! - QEMU 在场分类（矩阵宿主条目 binaries；install-dir=None → 落位目录略过，
//!   走 msys2/PATH/常规目录）；
//! - 加速器事实采集 + 引导态分类（Linux：/dev/kvm + /proc；Windows：WHPX DLL）；
//! - Linux：统一安装入口直证（已装 → 包管理器 no-op，或无免密 sudo → 诚实
//!   报错；两态都是合法产出，打印即证）；
//! - Windows：`enable_accel_windows()` 早退直证（WHPX 已启用 → Ok 不提权）。
//!
//! 运行：`cargo run --release -p nemesis-sandbox --features vm --example vm_bootstrap_arm`

// Windows 臂专用（Linux 下 cfg 掉使用点，导入随之标注防 unused warning）。
#[cfg(target_os = "windows")]
use nemesis_sandbox::vm::install;
#[cfg(target_os = "linux")]
use nemesis_sandbox::vm::install::{QemuInstallDirs, install_qemu_for_host_workspace};
use nemesis_sandbox::vm::probe;
use nemesis_sandbox::vm::qemu_config;

#[tokio::main]
async fn main() {
    println!("== B2 bootstrap arm: {}", std::env::consts::OS);

    // 在场分类走矩阵宿主条目（宿主键缺格 = 矩阵补全欠账，诚实报错即证）。
    let matrix = qemu_config::embedded_matrix();
    let entry = match qemu_config::host_entry(matrix) {
        Some(e) => e,
        None => {
            eprintln!(
                "FAIL: 宿主键 {} 不在默认矩阵（先补 config.vm_qemu.default.json 再上构建）",
                qemu_config::host_key()
            );
            std::process::exit(1);
        }
    };
    let presence = probe::classify_qemu_presence(entry, None);
    println!("qemu presence: {presence:?}");
    if let probe::QemuPresence::Ready { source } = &presence {
        let (sys, _) = qemu_config::required_binaries_for(entry).expect("binaries 成对");
        let dir = probe::find_binary_dir_on_path(sys).expect("ready 但找不到 qemu-system 目录");
        match probe::qemu_version_ok(&dir.join(sys)).await {
            Ok(v) => println!("  version probe: {v}"),
            Err(e) => {
                eprintln!("FAIL: 在场但不可执行（真裁决在实跑）: {e}");
                std::process::exit(1);
            }
        }
        println!("  ready source: {source}");
    }

    let facts = probe::collect_accel_facts();
    println!("accel facts: {facts:?}");
    let boot = probe::classify_accel_bootstrap(&facts);
    println!("accel bootstrap: {boot:?}");

    #[cfg(target_os = "linux")]
    {
        // 统一入口直证：落位/暂存全隔离进 target/vm-arm/（不碰真 workspace）。
        let base = std::path::PathBuf::from("target/vm-arm");
        let ws_config = base.join("config");
        let dirs = QemuInstallDirs {
            install_dir: base.join("runtime/qemu"),
            work_dir: base.join("runtime/install-work"),
            seven_zip_dir: base.join("sandboxie-runtime"),
            parallel: 12,
        };
        match install_qemu_for_host_workspace(&ws_config, &dirs).await {
            Ok((rep, meta)) => println!(
                "install: OK kind={} dir={} qemu={}",
                meta.kind.as_str(),
                rep.install_dir.display(),
                rep.qemu_system.display()
            ),
            Err(e) => println!("install: 诚实拒绝/失败（半自动分支直证）：{e:#}"),
        }
    }
    #[cfg(target_os = "windows")]
    match install::enable_accel_windows() {
        Ok(msg) => println!("enable_accel_windows: {msg}"),
        Err(e) => println!("enable_accel_windows: 诚实拒绝/失败：{e:#}"),
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    println!("(平台 {} 不参与安装链直证)", std::env::consts::OS);
    println!("== done");
}
