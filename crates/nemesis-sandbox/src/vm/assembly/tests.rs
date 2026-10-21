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
        qmp_port: 45123,
        host_forward_port: 45124,
        token: "tok-abc123".into(),
        shared_host_path: shared,
        loadvm_tag: None,
        incoming_file: None,
        display_vnc_port: None,
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

/// P5-R2 vnc 态：`-vnc 127.0.0.1:<display>` 回环装配 + `-display` 不出现
/// （两形态互斥）；display = port-5900 反向换算（QEMU 冒号后数字是显示器
/// 编号而非端口——2026-10-21 真机探针：`127.0.0.1:51206` 绑 57106 的根因
/// 回归锁）；默认 None 路径字节不变（display_none_present 回归锁的对称面）。
#[test]
fn display_vnc_loopback_assembly() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.display_vnc_port = Some(51206);
    let args = build_qemu_args(&s);
    let i = args.iter().position(|a| a == "-vnc").expect("-vnc 在位");
    // 51206 - 5900 = 45306：QEMU 绑 5900+45306 = 51206（真实端口契约）
    assert_eq!(
        args[i + 1],
        "127.0.0.1:45306",
        "VNC 必须绑 127.0.0.1 且按 display 编号换算"
    );
    assert!(
        !args.iter().any(|a| a == "-display"),
        "vnc 态不装 -display（两形态互斥）"
    );
    assert!(
        !args
            .iter()
            .any(|a| a.contains("-:590") || a.contains("0.0.0.0:51206")),
        "禁止非回环绑定"
    );
}

/// 守卫：display_vnc_port < 5900 无合法 display 编号形态——诚实 panic
/// （错绑 5900 偏移端口 = registry 记录连不上 = VNC 永久哑）。
#[test]
#[should_panic(expected = "display_vnc_port")]
fn display_vnc_port_below_base_panics() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.display_vnc_port = Some(4599);
    let _ = build_qemu_args(&s);
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

/// 绝对指点设备：显式 xhci 控制器 + usb-tablet 挂显式总线（q35 无默认
/// pointer 硬件也无默认 USB 总线——裸 `-device usb-tablet` 实测
/// `No 'usb-bus' bus found` 提前退出；缺它则 VNC 操作档的键鼠事件到
/// QEMU 后 guest 无设备可收，鼠标「完全不动」。2026-10-21 演示实录回归锁）。
#[test]
fn usb_tablet_always_present() {
    for &accel in QemuAccel::ALL {
        let args = args_of(accel, None);
        let i = args
            .iter()
            .position(|a| a == "-device")
            .unwrap_or_else(|| panic!("-device 在位（{accel:?}）"));
        assert_eq!(args[i + 1], "qemu-xhci,id=vm-usb", "{accel:?}");
        assert_eq!(args[i + 2], "-device", "{accel:?}");
        assert_eq!(args[i + 3], "usb-tablet,bus=vm-usb.0", "{accel:?}");
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
/// 例外：`-device` 刻意出现两枚（xhci 控制器 + usb-tablet，见
/// usb_tablet_always_present），排除后查重。
#[test]
fn no_duplicate_flags() {
    let args = args_of(QemuAccel::Kvm, Some(PathBuf::from("/host/ws")));
    let flags: Vec<_> = args
        .iter()
        .filter(|a| a.starts_with('-') && a.as_str() != "-device")
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

// ---------------------------------------------------------------------------
// validate_launch_spec：逗号闸 + wake 载体互斥（spawn 前最后一道闸）
// ---------------------------------------------------------------------------

/// 干净 spec = Ok（基线不误伤）。
#[test]
fn validate_clean_spec_ok() {
    validate_launch_spec(&spec(QemuAccel::Kvm, Some(PathBuf::from("/host/ws"))))
        .expect("干净 spec 应通过");
}

/// 逗号闸逐字段：overlay_path / shared_host_path / incoming_file 任一含
/// 逗号 = Err 且报错点名该字段（L6++ 项目路径用户任意，可含逗号；
/// 逗号在 QEMU 单 argv 子选项串里是分隔符）。
#[test]
fn validate_rejects_comma_in_each_path_field() {
    let cases: Vec<(&str, Box<dyn Fn(&mut VmLaunchSpec)>)> = vec![
        (
            "overlay_path",
            Box::new(|s: &mut VmLaunchSpec| {
                s.overlay_path = PathBuf::from("/vm/a,b/o.qcow2");
            }),
        ),
        (
            "shared_host_path",
            Box::new(|s: &mut VmLaunchSpec| {
                s.shared_host_path = Some(PathBuf::from("/host/ws,2"));
            }),
        ),
        (
            "incoming_file",
            Box::new(|s: &mut VmLaunchSpec| {
                s.incoming_file = Some(PathBuf::from("/vm/run/a,1/vm.state"));
            }),
        ),
    ];
    for (field, apply) in cases {
        let mut s = spec(QemuAccel::Kvm, None);
        apply(&mut s);
        let err = validate_launch_spec(&s).expect_err(&format!("{field} 含逗号必须拒绝"));
        assert!(err.contains(field), "报错要点名字段 {field}: {err}");
        assert!(err.contains("逗号"), "报错要讲原因: {err}");
    }
}

/// token 逗号（-smbios serial 子选项面）= Err。
#[test]
fn validate_rejects_comma_in_token() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.token = "to,k,en".into();
    let err = validate_launch_spec(&s).expect_err("token 含逗号必须拒绝");
    assert!(err.contains("token"), "{err}");
}

/// `-pidfile` / `-loadvm` 的值是独立 argv、无子选项解析——逗号无害不闸
/// （校验面贴真实风险，不过度）。
#[test]
fn validate_allows_comma_in_pidfile_and_loadvm_tag() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.pidfile = PathBuf::from("/run/vm,a/qemu.pid");
    s.loadvm_tag = Some("p5,sleep".into());
    validate_launch_spec(&s).expect("pidfile/loadvm_tag 逗号无害");
}

/// wake 载体互斥：loadvm_tag 与 incoming_file 同 Some = Err；单 Some = Ok。
#[test]
fn validate_rejects_loadvm_and_incoming_together() {
    let mut s = spec(QemuAccel::Kvm, None);
    s.loadvm_tag = Some("p5sleep".into());
    s.incoming_file = Some(PathBuf::from("/vm/run/vm.state"));
    let err = validate_launch_spec(&s).expect_err("双 wake 载体必须拒绝");
    assert!(err.contains("互斥"), "{err}");

    let mut a = spec(QemuAccel::Kvm, None);
    a.loadvm_tag = Some("p5sleep".into());
    validate_launch_spec(&a).expect("仅 loadvm = Ok");
    let mut b = spec(QemuAccel::Whpx, None);
    b.incoming_file = Some(PathBuf::from("/vm/run/vm.state"));
    validate_launch_spec(&b).expect("仅 incoming = Ok");
}
