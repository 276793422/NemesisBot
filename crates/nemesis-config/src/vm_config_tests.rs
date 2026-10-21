//! P5（桌面级 VM）：`executor.vm` 配置面测试。
//!
//! - serde 缺省：executor 段无 vm 键 = 铁律默认（enabled=false，全部子段缺省）；
//! - typed roundtrip：显式值序列化/反序列化不丢（2026-09-03 typed save
//!   回归锁同款纪律）；
//! - `validate()`：枚举型字符串未知值 loud 拒绝 + max_instances=0 拒绝；
//!   合法值域与 nemesis-sandbox vm::guest_arch 同步钉住。

use super::*;

// ---------------------------------------------------------------------------
// serde 缺省（铁律：默认关）
// ---------------------------------------------------------------------------

#[test]
fn vm_serde_default_when_key_absent() {
    let cfg: Config = serde_json::from_str("{}").expect("empty config parses");
    // executor 段整段缺席 = None（消费面 unwrap_or_default 落全缺省——
    // 与 ExecutorSeparationConfig::default() 的 vm 字段同源断言）。
    assert!(cfg.executor.is_none(), "缺 executor 段 = None");
    let vm = &ExecutorSeparationConfig::default().vm;
    assert!(!vm.enabled, "缺键 = VM 零激活（铁律默认关）");
    assert_eq!(vm.arch, "x86_64");
    assert_eq!(vm.hypervisor, "auto");
    assert_eq!(vm.desktop, "minimal");
    assert_eq!(vm.max_instances, 2);
    assert_eq!(vm.idle_sleep_secs, 300);
    assert_eq!(vm.ready_budget_secs, 240);
    assert_eq!(vm.resources.vcpu, 2);
    assert_eq!(vm.resources.ram_mb, 2048);
    assert_eq!(vm.resources.disk_gb, 20);
    assert_eq!(vm.gc.max_total_disk_gb, 50);
    assert_eq!(vm.gc.over_limit, "warn");
    // 镜像来源有缺省（Debian genericcloud），sha256 空 = sidecar 查表。
    assert!(vm.image.source.starts_with("https://"));
    assert!(vm.image.sha256.is_empty());
    // Default impl 与 serde 缺省同源（手写 Default 不走 serde，两处一致）。
    assert_eq!(*vm, ExecutorVmConfig::default());
}

/// 嵌套双路径陷阱：vm 键存在但只写部分字段 → 字段级 serde default 生效。
#[test]
fn vm_partial_section_field_defaults_apply() {
    let src = serde_json::json!({
        "executor": {"vm": {"enabled": true}}
    });
    let cfg: Config = serde_json::from_value(src).expect("partial vm section parses");
    let vm = &cfg.executor.as_ref().expect("executor 段在场").vm;
    assert!(vm.enabled);
    assert_eq!(vm.arch, "x86_64", "未写字段 = 字段级缺省");
    assert_eq!(vm.hypervisor, "auto");
    assert_eq!(vm.resources.vcpu, 2);
    assert_eq!(vm.gc.over_limit, "warn");
}

// ---------------------------------------------------------------------------
// typed roundtrip
// ---------------------------------------------------------------------------

#[test]
fn vm_typed_roundtrip_preserves_explicit_values() {
    let src = serde_json::json!({
        "executor": {
            "vm": {
                "enabled": true,
                "arch": "aarch64",
                "hypervisor": "kvm",
                "desktop": "full",
                "image": {
                    "source": "https://example.com/img.qcow2",
                    "sha256": "abcd1234"
                },
                "resources": {"vcpu": 4, "ram_mb": 4096, "disk_gb": 40},
                "max_instances": 3,
                "idle_sleep_secs": 0,
                "ready_budget_secs": 480,
                "gc": {"max_total_disk_gb": 0, "over_limit": "stop"}
            }
        }
    });
    let cfg: Config = serde_json::from_value(src).expect("full vm section parses");
    let vm = &cfg.executor.as_ref().expect("executor 段在场").vm;
    assert!(vm.enabled);
    assert_eq!(vm.arch, "aarch64");
    assert_eq!(vm.hypervisor, "kvm");
    assert_eq!(vm.desktop, "full");
    assert_eq!(vm.image.source, "https://example.com/img.qcow2");
    assert_eq!(vm.image.sha256, "abcd1234");
    assert_eq!(vm.resources.vcpu, 4);
    assert_eq!(vm.resources.ram_mb, 4096);
    assert_eq!(vm.resources.disk_gb, 40);
    assert_eq!(vm.max_instances, 3);
    assert_eq!(vm.idle_sleep_secs, 0, "0 = 不自动睡");
    assert_eq!(vm.ready_budget_secs, 480, "显式值保真");
    assert_eq!(vm.gc.max_total_disk_gb, 0, "0 = 不设预算");
    assert_eq!(vm.gc.over_limit, "stop");

    // 序列化回 JSON 再反序列化，全字段不丢（typed save 面）。
    let bytes = serde_json::to_vec(&cfg).expect("serialize");
    let reparsed: Config = serde_json::from_slice(&bytes).expect("reparse");
    assert_eq!(
        reparsed
            .executor
            .as_ref()
            .expect("roundtrip 后 executor 段在场")
            .vm,
        *vm,
        "roundtrip 字段全保真"
    );
}

// ---------------------------------------------------------------------------
// validate() loud 拒绝
// ---------------------------------------------------------------------------

#[test]
fn vm_validate_accepts_defaults() {
    ExecutorVmConfig::default()
        .validate()
        .expect("全缺省配置必须通过校验");
}

#[test]
fn vm_validate_loud_rejects_unknown_arch() {
    let mut vm = ExecutorVmConfig::default();
    vm.arch = "riscv".into();
    let err = vm.validate().unwrap_err().to_string();
    assert!(err.contains("x86_64") && err.contains("aarch64"), "{err}");
    // 值域与 nemesis-sandbox vm::guest_arch 同步（crate 分层不依赖，测试钉）。
    for legal in ["x86_64", "aarch64"] {
        vm.arch = legal.into();
        vm.validate().unwrap_or_else(|e| panic!("{legal}: {e}"));
    }
}

#[test]
fn vm_validate_loud_rejects_unknown_hypervisor() {
    let mut vm = ExecutorVmConfig::default();
    vm.hypervisor = "vbox".into();
    let err = vm.validate().unwrap_err().to_string();
    assert!(
        err.contains("auto") && err.contains("kvm") && err.contains("whpx"),
        "{err}"
    );
    for legal in ["auto", "kvm", "whpx"] {
        vm.hypervisor = legal.into();
        vm.validate().unwrap_or_else(|e| panic!("{legal}: {e}"));
    }
}

#[test]
fn vm_validate_loud_rejects_unknown_desktop() {
    let mut vm = ExecutorVmConfig::default();
    vm.desktop = "gnome".into();
    let err = vm.validate().unwrap_err().to_string();
    assert!(err.contains("minimal") && err.contains("full"), "{err}");
    for legal in ["minimal", "full"] {
        vm.desktop = legal.into();
        vm.validate().unwrap_or_else(|e| panic!("{legal}: {e}"));
    }
}

#[test]
fn vm_validate_loud_rejects_unknown_gc_over_limit() {
    let mut vm = ExecutorVmConfig::default();
    vm.gc.over_limit = "delete".into();
    let err = vm.validate().unwrap_err().to_string();
    assert!(err.contains("warn") && err.contains("stop"), "{err}");
    for legal in ["warn", "stop"] {
        vm.gc.over_limit = legal.into();
        vm.validate().unwrap_or_else(|e| panic!("{legal}: {e}"));
    }
}

#[test]
fn vm_validate_rejects_zero_max_instances() {
    let mut vm = ExecutorVmConfig::default();
    vm.max_instances = 0;
    let err = vm.validate().unwrap_err().to_string();
    assert!(err.contains("enabled=false"), "错误必须指路关闭开关: {err}");
    vm.max_instances = 1;
    vm.validate().unwrap_or_else(|e| panic!("1 合法: {e}"));
}

// ---------------------------------------------------------------------------
// ready_budget_secs（P5-R4 配置化：原 vm_runtime 编译期常量 240s 退役）
// ---------------------------------------------------------------------------

/// 缺省 240 = 原编译期常量行为字节不变（回归锁：双路径同源——serde 字段级
/// 缺省与手写 Default 都落 240）。
#[test]
fn vm_ready_budget_default_is_240_both_paths() {
    assert_eq!(
        ExecutorVmConfig::default().ready_budget_secs,
        240,
        "手写 Default 路径"
    );
    let cfg: Config = serde_json::from_str(r#"{"executor":{"vm":{"enabled":true}}}"#)
        .expect("partial vm section parses");
    let vm = &cfg.executor.as_ref().expect("executor 段在场").vm;
    assert_eq!(vm.ready_budget_secs, 240, "serde 字段级缺省路径");
}

#[test]
fn vm_validate_rejects_zero_ready_budget() {
    let mut vm = ExecutorVmConfig::default();
    vm.ready_budget_secs = 0;
    let err = vm.validate().unwrap_err().to_string();
    assert!(
        err.contains("ready_budget_secs") && err.contains("≥ 1"),
        "0 = 无限挂起不是合法形态: {err}"
    );
    vm.ready_budget_secs = 1;
    vm.validate().unwrap_or_else(|e| panic!("1 合法: {e}"));
}

// ---------------------------------------------------------------------------
// display / display_resolution（P5-R2 带界面 VM）
// ---------------------------------------------------------------------------

/// 缺省 none = 原无头形态字节不变（回归锁：双路径同源——serde 字段级缺省
/// 与手写 Default 都落 "none"；分辨率缺省 1280x720）。
#[test]
fn vm_display_default_is_none_both_paths() {
    assert_eq!(
        ExecutorVmConfig::default().display,
        "none",
        "手写 Default 路径"
    );
    assert_eq!(
        ExecutorVmConfig::default().display_resolution,
        "1280x720",
        "分辨率缺省"
    );
    let cfg: Config = serde_json::from_str(r#"{"executor":{"vm":{"enabled":true}}}"#)
        .expect("partial vm section parses");
    let vm = &cfg.executor.as_ref().expect("executor 段在场").vm;
    assert_eq!(vm.display, "none", "serde 字段级缺省路径");
    assert_eq!(vm.display_resolution, "1280x720");
}

#[test]
fn vm_display_typed_roundtrip_preserves_vnc() {
    let src = serde_json::json!({
        "executor": {"vm": {
            "enabled": true,
            "display": "vnc",
            "display_resolution": "1920x1080"
        }}
    });
    let cfg: Config = serde_json::from_value(src).expect("display 段 parses");
    let vm = &cfg.executor.as_ref().expect("executor 段在场").vm;
    assert_eq!(vm.display, "vnc");
    assert_eq!(vm.display_resolution, "1920x1080");
    let bytes = serde_json::to_vec(&cfg).expect("serialize");
    let reparsed: Config = serde_json::from_slice(&bytes).expect("reparse");
    assert_eq!(
        reparsed
            .executor
            .as_ref()
            .expect("roundtrip 后 executor 段在场")
            .vm,
        *vm,
        "roundtrip 字段全保真"
    );
}

#[test]
fn vm_validate_loud_rejects_unknown_display() {
    let mut vm = ExecutorVmConfig::default();
    vm.display = "spice".into();
    let err = vm.validate().unwrap_err().to_string();
    assert!(err.contains("none") && err.contains("vnc"), "{err}");
    for legal in ["none", "vnc"] {
        vm.display = legal.into();
        vm.validate().unwrap_or_else(|e| panic!("{legal}: {e}"));
    }
}

#[test]
fn vm_validate_loud_rejects_bad_display_resolution() {
    let mut vm = ExecutorVmConfig::default();
    for bad in ["1280", "1280X720", "axb", "0x720", "1280x0", "-1280x720"] {
        vm.display_resolution = bad.into();
        let err = vm.validate().unwrap_err().to_string();
        assert!(
            err.contains("display_resolution") && err.contains("宽x高"),
            "{bad}: {err}"
        );
    }
    vm.display_resolution = "1920x1080".into();
    vm.validate()
        .unwrap_or_else(|e| panic!("1920x1080 合法: {e}"));
}
