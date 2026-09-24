//! verify_policy 单测：§1 矩阵逐行钉死（消费版 / 锁定版两种 feature 形态
//! 分别编译验证；锚存在性依赖构建环境，断言按 `ROOT_ANCHOR` 实际值分支）。

use super::*;

/// 未知值 loud 拒绝——两种形态、有无锚都一致。
#[test]
fn unknown_value_loud_reject() {
    for bad in ["yes", "OFF", "enforcement", "0"] {
        let err = resolve_mode(Some(bad)).expect_err("未知值必须 Err");
        assert!(err.contains("非法值"), "报错应指认非法值: {err}");
    }
}

/// 消费版 off → Off，永不降级、永不升级（有无锚均然）；锁定版忽略 config，
/// off 同样被钉成 Enforce（与 locked_ignores_config_values 一致）。
#[test]
fn off_stays_off() {
    let r = resolve_mode(Some("off")).expect("off 合法");
    if VERIFY_ENFORCE_LOCKED {
        assert_eq!(r.mode, VerifyMode::Enforce);
    } else {
        assert_eq!(r.mode, VerifyMode::Off);
        assert!(!r.degraded);
    }
}

/// 缺省（None / 空串）= warn 请求。
#[test]
fn absent_value_means_warn_request() {
    for v in [None, Some("")] {
        let r = resolve_mode(v).expect("缺省合法");
        if VERIFY_ENFORCE_LOCKED {
            assert_eq!(r.mode, VerifyMode::Enforce);
        } else if ROOT_ANCHOR.is_some() {
            assert_eq!(r.mode, VerifyMode::Warn);
        } else {
            assert_eq!(r.mode, VerifyMode::Off);
            assert!(r.degraded);
        }
    }
}

/// enforce 请求：有锚兑现 / 无锚降级 off（响亮声明由 self_check 负责）。
#[test]
fn enforce_depending_on_anchor() {
    let r = resolve_mode(Some("enforce")).expect("enforce 合法");
    if VERIFY_ENFORCE_LOCKED {
        assert_eq!(r.mode, VerifyMode::Enforce);
        assert!(r.locked);
    } else if ROOT_ANCHOR.is_some() {
        assert_eq!(r.mode, VerifyMode::Enforce);
        assert!(!r.locked);
    } else {
        assert_eq!(r.mode, VerifyMode::Off);
        assert!(r.degraded);
        assert!(!r.locked);
    }
}

/// warn 请求 + 无锚（消费版）→ 降级 off；有锚 → Warn。
#[test]
fn warn_depending_on_anchor() {
    let r = resolve_mode(Some("warn")).expect("warn 合法");
    if VERIFY_ENFORCE_LOCKED {
        assert_eq!(r.mode, VerifyMode::Enforce);
    } else if ROOT_ANCHOR.is_some() {
        assert_eq!(r.mode, VerifyMode::Warn);
        assert!(!r.degraded);
    } else {
        assert_eq!(r.mode, VerifyMode::Off);
        assert!(r.degraded);
    }
}

/// 退出码契约（计划决策 6）。
#[test]
fn exit_code_is_86() {
    assert_eq!(EXIT_ENFORCE_REJECTED, 86);
}

/// 锁定版形态专属：任意合法 config 值被忽略，恒 Enforce；无锚时
/// degraded=true（self_check 侧拒启 86——exit 不进单测）。
#[cfg(feature = "verify-enforce-lock")]
#[test]
fn locked_ignores_config_values() {
    for v in [None, Some(""), Some("off"), Some("warn"), Some("enforce")] {
        let r = resolve_mode(v).expect("合法值");
        assert!(r.locked, "锁定形态 locked=true");
        assert_eq!(r.mode, VerifyMode::Enforce, "锁定形态忽略 config");
        assert_eq!(r.degraded, ROOT_ANCHOR.is_none());
    }
}

/// 消费版形态专属：locked 恒 false。
#[cfg(not(feature = "verify-enforce-lock"))]
#[test]
fn consumer_form_never_locked() {
    for v in [None, Some("off"), Some("warn"), Some("enforce")] {
        let r = resolve_mode(v).expect("合法值");
        assert!(!r.locked);
    }
}
