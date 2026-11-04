//! [`BoardAutonomy`] 自治模式总开关测试（AOR 批三 5.1）：serde 缺省 /
//! 大小写不敏感 / 非法值 loud 拒绝 / 序列化恒小写 / 矛盾态（autonomous 且
//! unlimited=false）加载期诚实降级为 assisted。

use crate::{BoardAutonomy, BoardFlagConfig, Config};

#[test]
fn missing_key_defaults_to_assisted() {
    let cfg: BoardFlagConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.autonomy, BoardAutonomy::Assisted);
    assert_eq!(BoardAutonomy::default(), BoardAutonomy::Assisted);
}

#[test]
fn string_modes_case_insensitive_and_trimmed() {
    for (s, want) in [
        ("assisted", BoardAutonomy::Assisted),
        ("autonomous", BoardAutonomy::Autonomous),
        (" AUTONOMOUS ", BoardAutonomy::Autonomous),
        ("Assisted", BoardAutonomy::Assisted),
    ] {
        let cfg: BoardFlagConfig =
            serde_json::from_str(&format!(r#"{{"autonomy": "{s}"}}"#)).unwrap();
        assert_eq!(cfg.autonomy, want, "input {s:?}");
    }
}

#[test]
fn invalid_value_is_loud_rejection() {
    assert!(serde_json::from_str::<BoardFlagConfig>(r#"{"autonomy": "full"}"#).is_err());
    // 数字/布尔形态不属于任何兼容面，拒绝（新键无旧形态要兼容）。
    assert!(serde_json::from_str::<BoardFlagConfig>(r#"{"autonomy": 1}"#).is_err());
    assert!(serde_json::from_str::<BoardFlagConfig>(r#"{"autonomy": true}"#).is_err());
}

#[test]
fn serializes_as_lowercase_string() {
    for (mode, want) in [
        (BoardAutonomy::Assisted, "assisted"),
        (BoardAutonomy::Autonomous, "autonomous"),
    ] {
        let cfg = BoardFlagConfig {
            autonomy: mode,
            ..Default::default()
        };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(v["autonomy"], serde_json::json!(want), "{mode:?}");
    }
}

#[test]
fn autonomy_roundtrip_preserves_value() {
    let cfg: BoardFlagConfig = serde_json::from_str(r#"{"autonomy": "autonomous"}"#).unwrap();
    let re: BoardFlagConfig = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(re.autonomy, BoardAutonomy::Autonomous);
}

#[test]
fn contradictory_state_downgrades_to_assisted_on_load() {
    // autonomous + unlimited=false = 矛盾态：post_process_for_compatibility
    // 诚实降级为 assisted（加载期安全方向回退；面板保存路径硬拒在 nemesis-web）。
    let mut config: Config =
        serde_json::from_str(r#"{"board": {"autonomy": "autonomous", "unlimited_mode": false}}"#)
            .unwrap();
    config.post_process_for_compatibility();
    let board = config.board.as_ref().unwrap();
    assert_eq!(
        board.autonomy,
        BoardAutonomy::Assisted,
        "矛盾态必须降级为 assisted"
    );
}

#[test]
fn autonomous_with_unlimited_enabled_is_preserved() {
    let mut config: Config =
        serde_json::from_str(r#"{"board": {"autonomy": "autonomous", "unlimited_mode": true}}"#)
            .unwrap();
    config.post_process_for_compatibility();
    let board = config.board.as_ref().unwrap();
    assert_eq!(
        board.autonomy,
        BoardAutonomy::Autonomous,
        "合法组合不得被降级"
    );
}

#[test]
fn assisted_without_unlimited_is_untouched() {
    let mut config: Config =
        serde_json::from_str(r#"{"board": {"autonomy": "assisted", "unlimited_mode": false}}"#)
            .unwrap();
    config.post_process_for_compatibility();
    let board = config.board.as_ref().unwrap();
    assert_eq!(board.autonomy, BoardAutonomy::Assisted);
}

#[test]
fn absent_board_section_stays_none() {
    let mut config: Config = serde_json::from_str("{}").unwrap();
    config.post_process_for_compatibility();
    assert!(config.board.is_none(), "无 board 段不得凭空造出");
}
