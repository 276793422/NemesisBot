//! [`BoardAttendanceConfig`] 人工触点考勤配置测试：serde 缺省铁律
//! （缺键不炸全按默认）/ mode 归一（大小写不敏感 / 空串=unattended /
//! 未知值 loud 拒绝不静默纠偏）/ kind 等待窗映射 / roundtrip 保留。

use crate::{AttendanceMode, BoardAttendanceConfig, BoardFlagConfig};

#[test]
fn missing_section_defaults() {
    let cfg: BoardFlagConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.attendance, BoardAttendanceConfig::default());
    assert_eq!(cfg.attendance.mode, "unattended");
    assert_eq!(cfg.attendance.spec_timeout_secs, 43_200);
    assert_eq!(cfg.attendance.prototype_timeout_secs, 14_400);
    assert_eq!(cfg.attendance.veto_max_rounds, 2);
    assert_eq!(cfg.attendance.blocking_cap_secs, 259_200);
}

#[test]
fn explicit_values_roundtrip() {
    let cfg: BoardFlagConfig = serde_json::from_str(
        r#"{"attendance":{"mode":"checkpoint","spec_timeout_secs":60,
            "prototype_timeout_secs":30,"veto_max_rounds":5,"blocking_cap_secs":0}}"#,
    )
    .unwrap();
    assert_eq!(cfg.attendance.mode, "checkpoint");
    assert_eq!(cfg.attendance.spec_timeout_secs, 60);
    assert_eq!(cfg.attendance.prototype_timeout_secs, 30);
    assert_eq!(cfg.attendance.veto_max_rounds, 5);
    assert_eq!(cfg.attendance.blocking_cap_secs, 0);
    // roundtrip：typed 保存不丢键。
    let back: BoardAttendanceConfig =
        serde_json::from_str(&serde_json::to_string(&cfg.attendance).unwrap()).unwrap();
    assert_eq!(back, cfg.attendance);
}

#[test]
fn mode_parse_normalization() {
    assert_eq!(
        AttendanceMode::parse("unattended"),
        Ok(AttendanceMode::Unattended)
    );
    assert_eq!(
        AttendanceMode::parse("Checkpoint"),
        Ok(AttendanceMode::Checkpoint)
    );
    // 空串 = 未配置 = 默认态无人值守。
    assert_eq!(AttendanceMode::parse(""), Ok(AttendanceMode::Unattended));
    assert_eq!(AttendanceMode::parse("  "), Ok(AttendanceMode::Unattended));
    // 未知值 loud 拒绝。
    let err = AttendanceMode::parse("strict").unwrap_err();
    assert!(err.contains("unattended | checkpoint"), "{err}");
    assert_eq!(
        serde_json::from_str::<BoardAttendanceConfig>(r#"{"mode":"strict"}"#)
            .unwrap()
            .normalized_mode()
            .unwrap_err(),
        err
    );
}

#[test]
fn timeout_secs_by_kind() {
    let cfg = BoardAttendanceConfig::default();
    assert_eq!(cfg.timeout_secs_for("spec"), 43_200);
    assert_eq!(cfg.timeout_secs_for("prototype"), 14_400);
    // 未知 kind 回落 spec 窗（值域在类型层保证，此处是纵深防御）。
    assert_eq!(cfg.timeout_secs_for(""), 43_200);
}
