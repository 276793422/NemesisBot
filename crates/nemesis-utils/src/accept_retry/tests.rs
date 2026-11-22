//! accept_retry 单元测试（独立测试文件，无内联测试）。

use super::*;
use std::io::Error;

#[test]
fn transient_kinds_get_short_backoff() {
    // 连接级瞬态（Windows WSAECONNRESET 形态 = ConnectionReset）。
    for kind in [
        std::io::ErrorKind::ConnectionAborted,
        std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::Interrupted,
    ] {
        let e = Error::new(kind, "test");
        assert!(transient_accept_error(&e), "kind {kind:?} 应判瞬态");
        assert_eq!(accept_error_backoff(&e), ACCEPT_BACKOFF_TRANSIENT);
    }
}

#[cfg(unix)]
#[test]
fn exhausted_kinds_get_long_backoff() {
    // fd 耗尽族按 raw OS 错误码判定（EMFILE/ENFILE/ENOMEM/ENOBUFS）。
    for code in [24, 23, 12, 105] {
        let e = Error::from_raw_os_error(code);
        assert!(resource_exhausted_accept(&e), "raw {code} 应判耗尽");
        assert!(transient_accept_error(&e));
        assert_eq!(accept_error_backoff(&e), ACCEPT_BACKOFF_EXHAUSTED);
    }
}

#[cfg(not(unix))]
#[test]
fn non_unix_has_no_exhausted_family() {
    // Windows 无 EMFILE 族（fd 模型不同），raw 码不参与判定。
    let e = Error::from_raw_os_error(24);
    assert!(!resource_exhausted_accept(&e));
}

#[test]
fn unknown_kinds_get_spin_guard_backoff() {
    // listener 本体损坏/未知类别 → 防自旋退避（不热自旋也不退出）。
    let e = Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    assert!(!transient_accept_error(&e));
    assert_eq!(accept_error_backoff(&e), ACCEPT_BACKOFF_UNKNOWN);

    let e2 = Error::other("weird");
    assert_eq!(accept_error_backoff(&e2), ACCEPT_BACKOFF_UNKNOWN);
}
