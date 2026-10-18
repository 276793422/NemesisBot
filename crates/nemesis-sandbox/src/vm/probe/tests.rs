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
