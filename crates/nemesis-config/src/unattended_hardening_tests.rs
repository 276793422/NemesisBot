//! 无人值守加固（批一①/批二/批三）配置面测试：
//! - 批一① `unlimited_without_budget_fuse` 组合谓词矩阵；
//! - 批二 `board.review.transient_retry_*` 两键默认与解析；
//! - 批三 `board.notify` 段默认（关 + 空目标）与解析。
//!
//! 全部默认值 = 现网行为不变（零配置零迁移）。

use crate::board::BoardFlagConfig;
use crate::load_config;

// ---------- 批一①：unlimited + 预算三维全关组合谓词 ----------

#[test]
fn unlimited_with_all_budget_fuses_off_is_detected() {
    let cfg = BoardFlagConfig {
        unlimited_mode: true,
        ..Default::default()
    };
    assert!(
        cfg.unlimited_without_budget_fuse(),
        "unlimited + 三维全 0（默认预算）= 无资源上限组合，必须命中"
    );
}

#[test]
fn unlimited_with_any_budget_fuse_set_is_not_detected() {
    for (name, patch) in [
        (
            "max_total_redispatch",
            serde_json::json!({"max_total_redispatch": 5}),
        ),
        (
            "wall_clock_budget_secs",
            serde_json::json!({"wall_clock_budget_secs": 3600}),
        ),
        (
            "max_tokens_per_parent",
            serde_json::json!({"max_tokens_per_parent": 1000}),
        ),
    ] {
        let raw = format!(
            r#"{{"board": {{"unlimited_mode": true, "budget": {}}}}}"#,
            patch
        );
        let cfg = serde_json::from_str::<crate::Config>(&raw).expect("解析");
        let board = cfg.board.as_ref().expect("board 段");
        assert!(
            !board.unlimited_without_budget_fuse(),
            "unlimited + {name} 非零 = 至少一维保险丝在场，不命中"
        );
    }
}

#[test]
fn unlimited_off_is_never_detected() {
    let cfg = BoardFlagConfig::default();
    assert!(
        !cfg.unlimited_without_budget_fuse(),
        "unlimited 关（默认）永不命中（默认预算三维本来就是 0）"
    );
}

// ---------- 批二：transient_retry_* 两键 ----------

#[test]
fn transient_retry_defaults_are_on_with_cap_6_delay_60() {
    let cfg = BoardFlagConfig::default();
    assert_eq!(cfg.review.transient_retry_max, 6, "默认上限 6");
    assert_eq!(cfg.review.transient_retry_delay_secs, 60, "默认退避 60s");
}

#[test]
fn transient_retry_keys_parse_and_zero_disables() {
    let cfg = serde_json::from_str::<crate::Config>(
        r#"{"board": {"review": {"transient_retry_max": 2, "transient_retry_delay_secs": 0}}}"#,
    )
    .expect("解析");
    let board = cfg.board.as_ref().expect("board 段");
    assert_eq!(board.review.transient_retry_max, 2);
    assert_eq!(
        board.review.transient_retry_delay_secs, 0,
        "0 = 测试注入不等"
    );
}

// ---------- 批三：board.notify 段 ----------

#[test]
fn notify_defaults_are_disabled_noop() {
    let cfg = BoardFlagConfig::default();
    assert!(!cfg.notify.enabled, "默认关");
    assert!(
        cfg.notify.channel.is_empty() && cfg.notify.chat_id.is_empty(),
        "默认空目标"
    );
}

#[test]
fn notify_section_parses_when_configured() {
    let cfg = serde_json::from_str::<crate::Config>(
        r#"{"board": {"notify": {"enabled": true, "channel": "telegram", "chat_id": "42"}}}"#,
    )
    .expect("解析");
    let board = cfg.board.as_ref().expect("board 段");
    assert!(board.notify.enabled);
    assert_eq!(board.notify.channel, "telegram");
    assert_eq!(board.notify.chat_id, "42");
}

#[test]
fn notify_missing_section_defaults_via_load_config() {
    // 旧配置（无 notify 键）走完整 load_config 路径零迁移。
    let dir = std::env::temp_dir().join(format!("nemesis-cfg-unattended-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
    std::fs::write(&path, r#"{"board": {"auto_review": true}}"#).unwrap();
    let cfg = load_config(&path).expect("旧配置可加载");
    let board = cfg.board.expect("board 段");
    assert!(!board.notify.enabled, "缺 notify 段 = Default 关");
    let _ = std::fs::remove_dir_all(&dir);
}
