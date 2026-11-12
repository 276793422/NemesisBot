use super::*;
use std::collections::BTreeMap;

fn overrides(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

// -- 收紧表 -------------------------------------------------------------

#[test]
fn tighten_with_accepts_valid_tightening() {
    let base = PluginLimits::default();
    let (out, ignored) = base.tighten_with(&overrides(&[
        ("fuel", 500),
        ("timeout-ms", 1_000),
        ("host-call-budget", 10),
        ("memory-bytes", 1024),
    ]));
    assert_eq!(out.fuel, 500);
    assert_eq!(out.timeout_ms, 1_000);
    assert_eq!(out.host_call_budget, 10);
    assert_eq!(out.memory_bytes, 1024);
    assert!(ignored.is_empty());
    // 基表不变（clone 语义）。
    assert_eq!(base.fuel, 1_000_000_000);
}

#[test]
fn tighten_with_ignores_relaxation_and_invalid() {
    let base = PluginLimits::default();
    // 0 值与放宽请求各走一轮（同键重复声明会被 BTreeMap 去重，分轮验证）。
    let (out, ignored) = base.tighten_with(&overrides(&[
        ("fuel", 0),
        ("timeout-ms", 0),
        ("host-call-budget", 0),
        ("memory-bytes", 0),
        ("bogus-key", 1),
    ]));
    assert_eq!(out.fuel, base.fuel);
    assert_eq!(out.timeout_ms, base.timeout_ms);
    assert_eq!(out.host_call_budget, base.host_call_budget);
    assert_eq!(out.memory_bytes, base.memory_bytes);
    assert_eq!(
        ignored,
        vec![
            "bogus-key",
            "fuel",
            "host-call-budget",
            "memory-bytes",
            "timeout-ms"
        ]
    );
    let (out2, ignored2) = base.tighten_with(&overrides(&[
        ("fuel", base.fuel + 1),
        ("timeout-ms", base.timeout_ms + 1),
        ("host-call-budget", 5_000),
        ("memory-bytes", u64::from(u32::MAX) + 1),
    ]));
    assert_eq!(out2.fuel, base.fuel);
    assert_eq!(out2.timeout_ms, base.timeout_ms);
    assert_eq!(out2.host_call_budget, base.host_call_budget);
    assert_eq!(out2.memory_bytes, base.memory_bytes);
    assert_eq!(
        ignored2,
        vec!["fuel", "host-call-budget", "memory-bytes", "timeout-ms"]
    );
}

#[test]
fn tighten_with_u64_overflow_guard() {
    let base = PluginLimits::default();
    // host-call-budget 超过 u32 上限的比较走 u64 侧（as u32 截断不发生）。
    let (out, ignored) =
        base.tighten_with(&overrides(&[("host-call-budget", u64::from(u32::MAX))]));
    assert_eq!(out.host_call_budget, base.host_call_budget);
    assert_eq!(ignored.len(), 1);
}

// -- 策略点 -------------------------------------------------------------

#[test]
fn policy_point_export_names_and_timeouts() {
    assert_eq!(PolicyPoint::Validation.export_name(), "validate-args");
    assert_eq!(PolicyPoint::Compact.export_name(), "compact-check");
    assert_eq!(PolicyPoint::FinalJudge.export_name(), "judge-final");
    assert_eq!(PolicyPoint::Validation.default_timeout_ms(), 15_000);
    assert_eq!(PolicyPoint::Compact.default_timeout_ms(), 660_000);
    assert_eq!(PolicyPoint::FinalJudge.default_timeout_ms(), 660_000);
}

#[test]
fn policy_call_timeout_matrix() {
    let base = PluginLimits::default();
    // validation：基表与点缺省取较紧者（30s vs 15s → 15s）。
    assert_eq!(base.policy_call_timeout_ms(PolicyPoint::Validation), 15_000);
    // manifest 收紧到 5s → 5s。
    let (tight, _) = base.tighten_with(&overrides(&[("timeout-ms", 5_000)]));
    assert_eq!(tight.policy_call_timeout_ms(PolicyPoint::Validation), 5_000);
    // compact/judge 固定点缺省（基表收紧不影响）。
    assert_eq!(tight.policy_call_timeout_ms(PolicyPoint::Compact), 660_000);
    assert_eq!(
        tight.policy_call_timeout_ms(PolicyPoint::FinalJudge),
        660_000
    );
}

// -- 并发实例闸 -----------------------------------------------------------

#[test]
fn instance_gate_acquires_and_reports_busy() {
    let gate = InstanceGate::new(2);
    let _a = gate.try_acquire().expect("first");
    let _b = gate.try_acquire().expect("second");
    let err = gate.try_acquire().err().unwrap_or_else(|| panic!("full"));
    assert!(matches!(err, crate::error::PluginError::Busy(2)));
    drop(_a);
    let _slot1 = gate.try_acquire().expect("released slot");
    drop(_b);
    let _slot2 = gate.try_acquire().expect("all released");
}

#[test]
fn instance_gate_zero_capacity_always_busy() {
    let gate = InstanceGate::new(0);
    let err = gate
        .try_acquire()
        .err()
        .unwrap_or_else(|| panic!("no capacity"));
    assert!(matches!(err, crate::error::PluginError::Busy(0)));
}
