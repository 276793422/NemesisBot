//! `DispatchFallbackMode` 三档 serde 兼容测试（2026-09-29 三档化）：
//! 旧布尔 false/true 保真（→Off/Full）、字符串档位（大小写不敏感/带空白）、
//! 缺省 = Role、非法值 loud 拒绝、序列化恒小写字符串。

use crate::{BoardFlagConfig, DispatchFallbackMode};

#[test]
fn legacy_bool_false_maps_to_off() {
    let cfg: BoardFlagConfig = serde_json::from_str(r#"{"dispatch_fallback": false}"#).unwrap();
    assert_eq!(cfg.dispatch_fallback, DispatchFallbackMode::Off);
}

#[test]
fn legacy_bool_true_maps_to_full() {
    let cfg: BoardFlagConfig = serde_json::from_str(r#"{"dispatch_fallback": true}"#).unwrap();
    assert_eq!(cfg.dispatch_fallback, DispatchFallbackMode::Full);
}

#[test]
fn missing_key_defaults_to_role() {
    let cfg: BoardFlagConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.dispatch_fallback, DispatchFallbackMode::Role);
}

#[test]
fn string_modes_case_insensitive_and_trimmed() {
    for (s, want) in [
        ("off", DispatchFallbackMode::Off),
        ("role", DispatchFallbackMode::Role),
        ("full", DispatchFallbackMode::Full),
        (" FULL ", DispatchFallbackMode::Full),
        ("Role", DispatchFallbackMode::Role),
    ] {
        let cfg: BoardFlagConfig =
            serde_json::from_str(&format!(r#"{{"dispatch_fallback": "{s}"}}"#)).unwrap();
        assert_eq!(cfg.dispatch_fallback, want, "input {s:?}");
    }
}

#[test]
fn invalid_value_is_loud_rejection() {
    assert!(serde_json::from_str::<BoardFlagConfig>(r#"{"dispatch_fallback": "wrong"}"#).is_err());
    // 数字形态不属于任何兼容面，拒绝。
    assert!(serde_json::from_str::<BoardFlagConfig>(r#"{"dispatch_fallback": 1}"#).is_err());
}

#[test]
fn serializes_as_lowercase_string() {
    for (mode, want) in [
        (DispatchFallbackMode::Off, "off"),
        (DispatchFallbackMode::Role, "role"),
        (DispatchFallbackMode::Full, "full"),
    ] {
        let cfg = BoardFlagConfig {
            dispatch_fallback: mode,
            ..Default::default()
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["dispatch_fallback"], serde_json::json!(want), "{mode:?}");
    }
}

#[test]
fn roundtrip_via_typed_save_preserves_mode() {
    let cfg: BoardFlagConfig = serde_json::from_str(r#"{"dispatch_fallback": "role"}"#).unwrap();
    let json = serde_json::to_string(&cfg).unwrap();
    let cfg2: BoardFlagConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg, cfg2);
}
