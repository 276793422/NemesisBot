//! [`assembly`] 单测（纯函数装配断言——G2 门「QEMU 参数装配 / hostfwd
//! 127.0.0.1 断言」的 CI 落点）。

use std::path::PathBuf;

use super::*;

fn spec(accel: QemuAccel, shared: Option<PathBuf>) -> VmLaunchSpec {
    VmLaunchSpec {
        qemu_bin: PathBuf::from("qemu-system-x86_64"),
        pidfile: PathBuf::from("/run/vm/vm-1.pid"),
        accel,
        arch: GuestArch::X86_64,
        vcpu: 2,
        ram_mb: 2048,
        overlay_path: PathBuf::from("/vm/overlays/agent-1.qcow2"),
        seed_iso: None,
        qmp_port: 45123,
        host_forward_port: 45124,
        token: "tok-abc123".into(),
        shared_host_path: shared,
        loadvm_tag: None,
        incoming_file: None,
    }
}

fn args_of(accel: QemuAccel, shared: Option<PathBuf>) -> Vec<String> {
    build_qemu_args(&spec(accel, shared))
}

/// G2 宿主零暴露面：hostfwd 恒绑 127.0.0.1（装配层钉死——外部接口不可达
/// 由绑定语义保证）。
#[test]
fn hostfwd_binds_loopback_only() {
    let args = args_of(QemuAccel::Kvm, None);
    let nic = args
        .iter()
        .find(|a| a.starts_with("user,"))
        .expect("-nic user 装配在位");
    assert!(
        nic.contains("hostfwd=tcp:127.0.0.1:45124-:48500"),
        "hostfwd 必须绑 127.0.0.1 且指向 guest agent 端口: {nic}"
    );
    assert!(
        !args.iter().any(|a| a.contains("hostfwd=tcp:0.0.0.0")),
        "禁止全零绑定"
    );
}

/// G2 宿主零暴露面（对称）：QMP 也恒绑 127.0.0.1（QMP 无鉴权）。
#[test]
fn qmp_binds_loopback_only() {
    let args = args_of(QemuAccel::Whpx, None);
    let qmp = args
        .iter()
        .find(|a| a.starts_with("tcp:"))
        .expect("-qmp 装配在位");
    assert!(
        qmp.contains("tcp:127.0.0.1:45123,server=on,wait=off"),
        "QMP 必须绑 127.0.0.1 + 非阻塞 server: {qmp}"
    );
}

/// headless 固定：-display none（GPU/音频恢复点的当前形态锚定）。
#[test]
fn display_none_present() {
    let args = args_of(QemuAccel::Kvm, None);
    let i = args
        .iter()
        .position(|a| a == "-display")
        .expect("-display 在位");
    assert_eq!(args[i + 1], "none");
}

/// accel → 参数映射 + 机型/CPU 来自 machine_spec（arch × accel 全组合）。
#[test]
fn accel_and_machine_mapping() {
    for &accel in QemuAccel::ALL {
        for &arch in GuestArch::ALL {
            let mut s = spec(accel, None);
            s.arch = arch;
            let args = build_qemu_args(&s);
            let ms = machine_spec(arch, accel);
            let mi = args.iter().position(|a| a == "-machine").unwrap();
            assert_eq!(args[mi + 1], ms.machine, "{arch:?}/{accel:?}");
            let ai = args.iter().position(|a| a == "-accel").unwrap();
            assert_eq!(args[ai + 1], accel.as_str(), "{arch:?}/{accel:?}");
            let ci = args.iter().position(|a| a == "-cpu").unwrap();
            assert_eq!(args[ci + 1], ms.cpu, "{arch:?}/{accel:?}");
        }
    }
}

/// 资源配额透传（vcpu/ram）。
#[test]
fn vcpu_and_ram_passed_through() {
    let args = args_of(QemuAccel::Kvm, None);
    let si = args.iter().position(|a| a == "-smp").unwrap();
    assert_eq!(args[si + 1], "2");
    let mi = args.iter().position(|a| a == "-m").unwrap();
    assert_eq!(args[mi + 1], "2048");
}

/// 首帧鉴权 token 经 SMBIOS serial 传入（per-instance token 通道锚定）。
#[test]
fn token_flows_via_smbios_serial() {
    let args = args_of(QemuAccel::Kvm, None);
    let i = args.iter().position(|a| a == "-smbios").unwrap();
    assert_eq!(args[i + 1], "type=1,serial=tok-abc123");
}

/// 9p 共享子树：Some = 装配 mount_tag；None = 不出现 -virtfs（Windows
/// 宿主诚实边界）。
#[test]
fn shared_mount_optional() {
    let with = args_of(QemuAccel::Kvm, Some(PathBuf::from("/host/ws")));
    let i = with
        .iter()
        .position(|a| a == "-virtfs")
        .expect("virtfs 在位");
    assert!(
        with[i + 1].starts_with("local,path=/host/ws,mount_tag=workspace,"),
        "mount_tag 约定锚定: {}",
        with[i + 1]
    );
    let without = args_of(QemuAccel::Kvm, None);
    assert!(
        !without.iter().any(|a| a == "-virtfs"),
        "无共享子树时不装配 -virtfs"
    );
}

/// overlay 挂为 virtio-blk 主盘。
#[test]
fn overlay_is_virtio_primary_disk() {
    let args = args_of(QemuAccel::Whpx, None);
    let i = args.iter().position(|a| a == "-drive").unwrap();
    assert_eq!(
        args[i + 1],
        "file=/vm/overlays/agent-1.qcow2,format=qcow2,if=virtio"
    );
}

/// cloud-init seed：Some = cdrom 只读挂载（fs label cidata 由 cloud-init
/// 发现）；None = 无 cdrom（已装配持久 overlay 复用启动形态）。
#[test]
fn seed_iso_optional_cdrom() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.seed_iso = Some(PathBuf::from("/vm/images/seed.iso"));
    let args = build_qemu_args(&s);
    let drives: Vec<&String> = args.iter().filter(|a| a.starts_with("file=")).collect();
    let cd = args
        .iter()
        .find(|a| a.starts_with("file=") && a.contains("format=raw,media=cdrom,readonly=on"))
        .expect("seed cdrom 在位");
    assert_eq!(
        *cd,
        "file=/vm/images/seed.iso,format=raw,media=cdrom,readonly=on"
    );
    assert_eq!(drives.len(), 2, "主盘 + seed 两块: {drives:?}");

    let without = args_of(QemuAccel::Kvm, None);
    assert!(
        !without.iter().any(|a| a.contains("media=cdrom")),
        "无 seed 时不装配 cdrom"
    );
}

/// pidfile 在位（孤儿 sweep 通道锚定）。
#[test]
fn pidfile_present() {
    let args = args_of(QemuAccel::Kvm, None);
    let i = args.iter().position(|a| a == "-pidfile").unwrap();
    assert_eq!(args[i + 1], "/run/vm/vm-1.pid");
}

/// wake 线 A：loadvm tag → `-loadvm <tag>`；缺省无该 flag（冷启形态
/// 锚定——重复 flag 在 QEMU 侧是静默后者覆盖，缺席必须显式）。
#[test]
fn loadvm_tag_optional_flag() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.loadvm_tag = Some("p5sleep".into());
    let args = build_qemu_args(&s);
    let i = args
        .iter()
        .position(|a| a == "-loadvm")
        .expect("loadvm 在位");
    assert_eq!(args[i + 1], "p5sleep");

    let cold = args_of(QemuAccel::Kvm, None);
    assert!(!cold.iter().any(|a| a == "-loadvm"), "冷启不装配 -loadvm");
}

/// wake 线 B：incoming 状态文件 → `-incoming file:<path>`；Windows 反斜杠
/// 路径统一正斜杠（QEMU file: URI 解析坑，装配层钉死）；缺省无该 flag。
#[test]
fn incoming_file_optional_flag() {
    let mut s = spec(QemuAccel::Whpx, None);
    s.incoming_file = Some(PathBuf::from(r"C:\vm\run\a1\vm.state"));
    let args = build_qemu_args(&s);
    let i = args
        .iter()
        .position(|a| a == "-incoming")
        .expect("incoming 在位");
    assert_eq!(args[i + 1], "file:C:/vm/run/a1/vm.state");

    let cold = args_of(QemuAccel::Kvm, None);
    assert!(
        !cold.iter().any(|a| a == "-incoming"),
        "冷启不装配 -incoming"
    );
}

/// 参数无重复 flag（装配函数回归锚——重复 flag 在 QEMU 侧是静默后者覆盖）。
#[test]
fn no_duplicate_flags() {
    let args = args_of(QemuAccel::Kvm, Some(PathBuf::from("/host/ws")));
    let flags: Vec<_> = args
        .iter()
        .filter(|a| a.starts_with('-'))
        .cloned()
        .collect();
    let mut sorted = flags.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(flags.len(), sorted.len(), "重复 flag: {flags:?}");
}

/// 二进制解析：显式目录命中优先；目录未命中落回 PATH 名；无目录 = PATH 名。
#[cfg(target_os = "windows")]
#[test]
fn resolve_qemu_binary_windows_shape() {
    let r = resolve_qemu_binary(
        GuestArch::X86_64,
        Some(std::path::Path::new("Z:/no/such/dir")),
    );
    assert_eq!(r, PathBuf::from("qemu-system-x86_64"));
}
