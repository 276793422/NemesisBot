//! VM 环境自举 B2 真机臂：probe 状态机在真实宿主上的直证（双端）。
//!
//! 每步打印状态与判定，全部只读（不安装、不提权、不改系统态）：
//! - QEMU 在场分类（install-dir=None → 落位目录略过，走 PATH/常规目录）；
//! - 加速器事实采集 + 引导态分类（Linux：/dev/kvm + /proc；Windows：WHPX DLL）；
//! - Linux：`install_qemu_linux()` 半自动链直证（已装 → 包管理器 no-op 或
//!   无免密 sudo → 诚实报错；两态都是合法产出，打印即证）；
//! - Windows：`enable_accel_windows()` 早退直证（WHPX 已启用 → Ok 不提权）。
//!
//! 运行：`cargo run --release -p nemesis-sandbox --features vm --example vm_bootstrap_arm`

use nemesis_sandbox::vm::guest_arch::GuestArch;
use nemesis_sandbox::vm::install;
use nemesis_sandbox::vm::probe;

#[tokio::main]
async fn main() {
    println!("== B2 bootstrap arm: {}", std::env::consts::OS);

    let presence = probe::classify_qemu_presence(GuestArch::X86_64, None);
    println!("qemu presence: {presence:?}");
    if let probe::QemuPresence::Ready { source } = &presence {
        let (sys, _) = install::qemu_required_binaries();
        let dir = which_dir(sys).expect("ready 但找不到 qemu-system 目录");
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
    match install::install_qemu_linux().await {
        Ok(rep) => println!(
            "install_qemu_linux: OK dir={} qemu={}",
            rep.install_dir.display(),
            rep.qemu_system.display()
        ),
        Err(e) => println!("install_qemu_linux: 诚实拒绝/失败（半自动分支直证）：{e:#}"),
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

fn which_dir(binary: &str) -> Option<std::path::PathBuf> {
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(binary);
            if p.is_file() {
                return Some(dir);
            }
        }
    }
    None
}
