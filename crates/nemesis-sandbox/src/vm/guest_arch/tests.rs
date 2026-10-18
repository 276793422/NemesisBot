//! [`guest_arch`] 单测：解析（round-trip / loud 拒绝）、机型映射表全组合
//! 存在性（G0 门：aarch64 条目编译期存在性断言）、产出闸语义。

use super::*;

// ---------------------------------------------------------------------------
// GuestArch 解析
// ---------------------------------------------------------------------------

#[test]
fn guest_arch_parse_roundtrip() {
    for arch in GuestArch::ALL {
        assert_eq!(GuestArch::parse(arch.as_str()).unwrap(), *arch);
    }
}

#[test]
fn guest_arch_unknown_value_loud_reject() {
    let err = GuestArch::parse("riscv").unwrap_err();
    assert!(err.contains("x86_64"), "错误信息必须带合法值域: {err}");
    assert!(err.contains("aarch64"), "{err}");
    // 空串同样拒绝（缺键走 serde default，空串是显式坏值）。
    assert!(GuestArch::parse("").is_err());
}

#[test]
fn guest_arch_production_gate() {
    assert!(GuestArch::X86_64.is_produced());
    assert!(!GuestArch::AArch64.is_produced(), "aarch64 = 接口预留档");
}

#[test]
fn guest_arch_qemu_binary_names() {
    assert_eq!(GuestArch::X86_64.qemu_system_binary(), "qemu-system-x86_64");
    assert_eq!(
        GuestArch::AArch64.qemu_system_binary(),
        "qemu-system-aarch64"
    );
}

// ---------------------------------------------------------------------------
// QemuAccel 解析
// ---------------------------------------------------------------------------

#[test]
fn hypervisor_parse_values() {
    assert_eq!(QemuAccel::parse_hypervisor("auto").unwrap(), None);
    assert_eq!(
        QemuAccel::parse_hypervisor("kvm").unwrap(),
        Some(QemuAccel::Kvm)
    );
    assert_eq!(
        QemuAccel::parse_hypervisor("whpx").unwrap(),
        Some(QemuAccel::Whpx)
    );
}

#[test]
fn hypervisor_unknown_value_loud_reject() {
    let err = QemuAccel::parse_hypervisor("vbox").unwrap_err();
    assert!(err.contains("auto"), "{err}");
    assert!(err.contains("kvm"), "{err}");
    assert!(err.contains("whpx"), "{err}");
}

// ---------------------------------------------------------------------------
// 机型映射表（全组合在位 + aarch64 预留标注）
// ---------------------------------------------------------------------------

#[test]
fn machine_table_covers_all_arch_accel_combinations() {
    for arch in GuestArch::ALL {
        for accel in QemuAccel::ALL {
            let spec = machine_spec(*arch, *accel);
            assert!(!spec.machine.is_empty(), "{arch:?}x{accel:?} 机型缺失");
            assert!(!spec.cpu.is_empty(), "{arch:?}x{accel:?} cpu 缺失");
        }
    }
}

/// G0 门：aarch64 两条目（KVM on ARM64 Linux / WHPX on Windows ARM64）
/// 编译期存在 + 明确标注 Reserved（接口在位、当前不产出）。
#[test]
fn aarch64_entries_present_and_reserved() {
    let kvm = machine_spec(GuestArch::AArch64, QemuAccel::Kvm);
    let whpx = machine_spec(GuestArch::AArch64, QemuAccel::Whpx);
    assert_eq!(kvm.machine, "virt");
    assert_eq!(kvm.production, Production::Reserved);
    assert_eq!(whpx.machine, "virt");
    assert_eq!(whpx.production, Production::Reserved);
}

#[test]
fn x86_64_entries_produced() {
    for accel in QemuAccel::ALL {
        let spec = machine_spec(GuestArch::X86_64, *accel);
        assert_eq!(spec.machine, "q35");
        assert_eq!(spec.cpu, "host");
        assert_eq!(spec.production, Production::Produced);
    }
}

// ---------------------------------------------------------------------------
// display/audio 装配（headless 固定形态 + 扩展点）
// ---------------------------------------------------------------------------

#[test]
fn display_args_are_headless() {
    let args = display_args();
    assert_eq!(args, &["-display", "none"]);
}
