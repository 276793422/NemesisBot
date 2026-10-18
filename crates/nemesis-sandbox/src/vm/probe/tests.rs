//! [`probe`] 单测：结构性质断言（不依赖宿主真实状态——CI 无加速是常态）。

use super::*;

#[test]
fn kvm_probe_reason_always_present_when_unavailable() {
    let probe = probe_kvm();
    if probe.available {
        assert_eq!(probe.reason, "");
        assert_eq!(probe.layer, "static-dev");
    } else {
        assert!(!probe.reason.is_empty(), "缺席必须带人读原因");
        assert_eq!(probe.layer, "none");
    }
}

#[test]
fn whpx_probe_reason_always_present_when_unavailable() {
    let probe = probe_whpx();
    if probe.available {
        assert_eq!(probe.reason, "");
        assert_eq!(probe.layer, "static-dll");
    } else {
        assert!(!probe.reason.is_empty(), "缺席必须带人读原因");
        assert_eq!(probe.layer, "none");
    }
}

/// 非 Linux 宿主 KVM 恒不可用（诚实：不探测、不猜）。
#[cfg(not(target_os = "linux"))]
#[test]
fn kvm_unavailable_off_linux() {
    let probe = probe_kvm();
    assert!(!probe.available);
    assert_eq!(probe.layer, "none");
}

/// 非 Windows 宿主 WHPX 恒不可用（诚实：不探测、不猜）。
#[cfg(not(target_os = "windows"))]
#[test]
fn whpx_unavailable_off_windows() {
    let probe = probe_whpx();
    assert!(!probe.available);
    assert_eq!(probe.layer, "none");
}

/// QEMU 实跑探针的错误面：不存在的二进制 = 诚实错误（不挂死、不吞错）。
#[tokio::test]
async fn qemu_version_missing_binary_errors_honestly() {
    let err = qemu_version_ok(Path::new("/nonexistent/qemu-system-x86_64"))
        .await
        .unwrap_err();
    assert!(err.contains("不可执行"), "{err}");
}

// ---------------------------------------------------------------------------
// B2：加速器引导态分类器（纯函数表驱动——任何宿主全测，不依赖真实设备）
// ---------------------------------------------------------------------------

use crate::vm::install::{CpuVendor, DismFeatureState};

fn facts(os: &'static str) -> HostAccelFacts {
    HostAccelFacts {
        os,
        ..Default::default()
    }
}

#[test]
fn classify_windows_table() {
    use AccelBootstrap as B;
    // DLL 在场 = Ok（DISM 态无关——DLL 是直接证据）。
    let mut f = facts("windows");
    f.whpx_dll_present = true;
    assert_eq!(classify_accel_bootstrap(&f), B::Ok);
    // DLL 缺席 × DISM 四态。
    let mut f = facts("windows");
    f.dism_feature_state = Some(DismFeatureState::Enabled);
    assert_eq!(classify_accel_bootstrap(&f), B::NeedsBios);
    let mut f = facts("windows");
    f.dism_feature_state = Some(DismFeatureState::EnablePending);
    assert_eq!(classify_accel_bootstrap(&f), B::NeedsReboot);
    let mut f = facts("windows");
    f.dism_feature_state = Some(DismFeatureState::Disabled);
    assert_eq!(classify_accel_bootstrap(&f), B::NeedsEnableWindows);
    // 查询不到（非提权/本地化）= 默认按未启用引导（DISM 一键无副作用）。
    let f = facts("windows");
    assert_eq!(classify_accel_bootstrap(&f), B::NeedsEnableWindows);
}

#[test]
fn classify_linux_table() {
    use AccelBootstrap as B;
    // 设备在场可写 = Ok。
    let mut f = facts("linux");
    f.kvm_dev_exists = true;
    f.kvm_writable = true;
    assert_eq!(classify_accel_bootstrap(&f), B::Ok);
    // 设备在场不可写 = kvm 组引导。
    let mut f = facts("linux");
    f.kvm_dev_exists = true;
    f.kvm_writable = false;
    assert_eq!(classify_accel_bootstrap(&f), B::NeedsGroup);
    // 设备缺席 × 模块未载 × vendor 已知 = modprobe 引导（vendor 映射）。
    let mut f = facts("linux");
    f.cpu_vendor = Some(CpuVendor::Intel);
    assert_eq!(
        classify_accel_bootstrap(&f),
        B::NeedsKmod {
            module: "kvm_intel"
        }
    );
    let mut f = facts("linux");
    f.cpu_vendor = Some(CpuVendor::Amd);
    assert_eq!(
        classify_accel_bootstrap(&f),
        B::NeedsKmod { module: "kvm_amd" }
    );
    // 模块载了设备却没有 = 矛盾态诚实未知（带模块名原因）。
    let mut f = facts("linux");
    f.kvm_module_loaded = Some("kvm_amd");
    match classify_accel_bootstrap(&f) {
        B::Unknown { reason } => assert!(reason.contains("kvm_amd"), "{reason}"),
        other => panic!("期望 Unknown，得到 {other:?}"),
    }
    // vendor 也未知 = 诚实未知。
    let f = facts("linux");
    assert!(matches!(classify_accel_bootstrap(&f), B::Unknown { .. }));
}

#[test]
fn classify_other_platform_unsupported() {
    use AccelBootstrap as B;
    assert_eq!(classify_accel_bootstrap(&facts("macos")), B::Unsupported);
    assert_eq!(classify_accel_bootstrap(&facts("freebsd")), B::Unsupported);
    assert_eq!(classify_accel_bootstrap(&facts("")), B::Unsupported);
}

/// 事实采集（本机）与分类器闭环：产出即合法输入，分类结果非矛盾
/// （Linux 上若 /dev/kvm 存在则可写性已实证；Windows 上 DLL 在场必 Ok）。
#[test]
fn collect_then_classify_no_contradiction() {
    let f = collect_accel_facts();
    let b = classify_accel_bootstrap(&f);
    match f.os {
        "linux" => {
            if f.kvm_dev_exists && f.kvm_writable {
                assert_eq!(b, AccelBootstrap::Ok);
            }
        }
        "windows" => {
            if f.whpx_dll_present {
                assert_eq!(b, AccelBootstrap::Ok);
            }
        }
        _ => assert_eq!(b, AccelBootstrap::Unsupported),
    }
}

// ---------------------------------------------------------------------------
// B2：QEMU 在场分类（install-dir 语义用临时目录实证；PATH 分支依赖真实
// 宿主 PATH 环境噪声大，仅 install-dir 优先级在此钉死）
// ---------------------------------------------------------------------------

use crate::vm::guest_arch::GuestArch;
use crate::vm::install::qemu_required_binaries;

#[test]
fn qemu_presence_install_dir_ready_and_partial() {
    let (sys_name, img_name) = qemu_required_binaries();
    let tmp = std::env::temp_dir().join(format!(
        "nmb-probe-presence-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    // 双二进制在场 = Ready{install-dir}（install-dir 优先于任何真实 PATH）。
    std::fs::write(tmp.join(sys_name), b"mz").unwrap();
    std::fs::write(tmp.join(img_name), b"mz").unwrap();
    assert_eq!(
        classify_qemu_presence(GuestArch::X86_64, Some(&tmp)),
        QemuPresence::Ready {
            source: "install-dir"
        }
    );
    // 缺 qemu-img = Partial（半装态诚实列出缺谁）。
    std::fs::remove_file(tmp.join(img_name)).unwrap();
    assert_eq!(
        classify_qemu_presence(GuestArch::X86_64, Some(&tmp)),
        QemuPresence::Partial {
            missing: vec![img_name.to_string()]
        }
    );
    // 目录里没 qemu-system = 落到 PATH 分支（PATH 语义依赖真实宿主，不在此断言）。
    std::fs::remove_file(tmp.join(sys_name)).unwrap();
    let _ = std::fs::remove_dir(&tmp);
}
