//! W1 安全图谱地基批（2026-11-19）：#26/#45/#46/#25/#30 新配置键测试。
//!
//! 覆盖三面（tool_result_cap_tests 同款纪律）：
//! - serde 缺省：键/段缺席 = 缺省值（typed 结构体手写 Default 与字段级
//!   serde 缺省同向，BUG-20261031-226 纪律）；
//! - typed roundtrip：显式值（含「关」形态）序列化/反序列化不丢；
//! - raw-JSON 解析（`resolve_chat_scan` / `resolve_untrusted_wrap` /
//!   `resolve_result_scan` / `resolve_rate_limit` / `resolve_usage_budget`）：
//!   AgentLoop / web 网关消费点的 fresh-read 走这条路，键路径/缺省/非法值
//!   必须与 typed 字段单源。

use super::*;

// ---------------------------------------------------------------------------
// #26 chat_scan
// ---------------------------------------------------------------------------

#[test]
fn w1_chat_scan_serde_default_log_redact() {
    // 顶层 {}：security 段缺席 → Option None；段在、chat_scan 键缺席 →
    // 字段级缺省（log/redact）。
    let sec: SecurityFlagConfig = serde_json::from_str("{}").expect("empty section parses");
    assert!(
        sec.enabled,
        "段级缺省 enabled=true（BUG-20261109-035 同向）"
    );
    assert_eq!(sec.chat_scan.inbound, DEFAULT_CHAT_SCAN_INBOUND);
    assert_eq!(sec.chat_scan.outbound, DEFAULT_CHAT_SCAN_OUTBOUND);
    // SecurityFlagConfig 手写 Default 同向。
    let d = SecurityFlagConfig::default();
    assert_eq!(d.chat_scan.inbound, "log");
    assert_eq!(d.chat_scan.outbound, "redact");
    assert!(d.untrusted_wrap, "#46 缺省开");
    assert_eq!(d.result_scan, "note", "#45 缺省 note");
    assert!(!d.rate_limit.enabled, "#25 缺省关");
}

#[test]
fn w1_chat_scan_typed_roundtrip_preserves_explicit() {
    let src = serde_json::json!({
        "security": {
            "enabled": true,
            "chat_scan": {"inbound": "off", "outbound": "log"},
            "untrusted_wrap": false,
            "result_scan": "off",
            "rate_limit": {"enabled": true, "qps": 5, "burst": 10, "max_concurrent_streams": 2}
        }
    });
    let cfg: Config = serde_json::from_value(src).expect("parses");
    let sec = cfg.security.as_ref().expect("security present");
    assert_eq!(sec.chat_scan.inbound, "off");
    assert_eq!(sec.chat_scan.outbound, "log");
    assert!(!sec.untrusted_wrap);
    assert_eq!(sec.result_scan, "off");
    assert!(sec.rate_limit.enabled);
    assert_eq!(sec.rate_limit.qps, 5);
    let out = serde_json::to_value(&cfg).expect("serializes");
    let sec_out = &out["security"];
    assert_eq!(
        sec_out["chat_scan"]["outbound"], "log",
        "roundtrip 保留显式值"
    );
    assert_eq!(
        sec_out["untrusted_wrap"], false,
        "roundtrip 保留 false（关形态）"
    );
    assert_eq!(sec_out["result_scan"], "off");
    assert_eq!(sec_out["rate_limit"]["qps"], 5);
}

#[test]
fn w1_chat_scan_resolver_defaults_and_values() {
    // 缺 config / 缺 security 段 / 缺 chat_scan 键 → 缺省 (Log, Redact)。
    assert_eq!(
        resolve_chat_scan(None),
        (ChatScanMode::Log, ChatScanMode::Redact)
    );
    assert_eq!(
        resolve_chat_scan(Some(&serde_json::json!({}))),
        (ChatScanMode::Log, ChatScanMode::Redact)
    );
    assert_eq!(
        resolve_chat_scan(Some(&serde_json::json!({"security": {}}))),
        (ChatScanMode::Log, ChatScanMode::Redact)
    );
    // 显式值透传。
    assert_eq!(
        resolve_chat_scan(Some(&serde_json::json!({
            "security": {"chat_scan": {"inbound": "off", "outbound": "log"}}
        }))),
        (ChatScanMode::Off, ChatScanMode::Log)
    );
    assert_eq!(
        resolve_chat_scan(Some(&serde_json::json!({
            "security": {"chat_scan": {"outbound": "off"}}
        }))),
        (ChatScanMode::Log, ChatScanMode::Off)
    );
    // 未知字符串值 = 回落该方向缺省（不静默纠偏到 off）。
    assert_eq!(
        resolve_chat_scan(Some(&serde_json::json!({
            "security": {"chat_scan": {"inbound": "block", "outbound": "zap"}}
        }))),
        (ChatScanMode::Log, ChatScanMode::Redact)
    );
}

// ---------------------------------------------------------------------------
// #46 untrusted_wrap / #45 result_scan
// ---------------------------------------------------------------------------

#[test]
fn w1_untrusted_wrap_resolver_defaults_on() {
    assert!(resolve_untrusted_wrap(None));
    assert!(resolve_untrusted_wrap(Some(&serde_json::json!({}))));
    assert!(resolve_untrusted_wrap(Some(
        &serde_json::json!({"security": {}})
    )));
    assert!(!resolve_untrusted_wrap(Some(
        &serde_json::json!({"security": {"untrusted_wrap": false}})
    )));
}

#[test]
fn w1_result_scan_resolver_defaults_and_values() {
    assert_eq!(resolve_result_scan(None), ResultScanMode::Note);
    assert_eq!(
        resolve_result_scan(Some(&serde_json::json!({}))),
        ResultScanMode::Note
    );
    assert_eq!(
        resolve_result_scan(Some(
            &serde_json::json!({"security": {"result_scan": "off"}})
        )),
        ResultScanMode::Off
    );
    // 未知值回落缺省 note。
    assert_eq!(
        resolve_result_scan(Some(
            &serde_json::json!({"security": {"result_scan": "block"}})
        )),
        ResultScanMode::Note
    );
}

// ---------------------------------------------------------------------------
// #25 rate_limit
// ---------------------------------------------------------------------------

#[test]
fn w1_rate_limit_resolver_defaults_off() {
    let s = resolve_rate_limit(None);
    assert!(!s.enabled);
    assert_eq!(s.qps, DEFAULT_RATE_LIMIT_QPS);
    assert_eq!(s.burst, DEFAULT_RATE_LIMIT_BURST);
    assert_eq!(
        s.max_concurrent_streams,
        DEFAULT_RATE_LIMIT_MAX_CONCURRENT_STREAMS
    );
}

#[test]
fn w1_rate_limit_resolver_clamps_zero() {
    // qps/burst 0 无语义 → 钳 1（防误配把网关整体关死）；并发 0 = 不限
    // 并发只限 QPS（有语义，保留 0）。
    let s = resolve_rate_limit(Some(&serde_json::json!({
        "security": {"rate_limit": {"enabled": true, "qps": 0, "burst": 0, "max_concurrent_streams": 0}}
    })));
    assert!(s.enabled);
    assert_eq!(s.qps, 1);
    assert_eq!(s.burst, 1);
    assert_eq!(s.max_concurrent_streams, 0);
}

// ---------------------------------------------------------------------------
// #30 usage.budget
// ---------------------------------------------------------------------------

#[test]
fn w1_usage_budget_serde_default_all_closed() {
    let usage: UsageConfig = serde_json::from_str("{}").expect("parses");
    assert_eq!(usage.retention_days, 30, "既有字段缺省不变");
    assert_eq!(usage.max_rows, 500_000);
    assert_eq!(usage.budget.daily_token_limit, 0, "配额缺省全关");
    assert_eq!(usage.budget.monthly_token_limit, 0);
    assert_eq!(usage.budget.daily_cost_limit_usd, 0.0);
    assert_eq!(usage.budget.mode, "reject");
    // usage 段在场、budget 键缺席（老配置）→ 字段级缺省。
    let usage2: UsageConfig = serde_json::from_str(r#"{"retention_days": 7}"#).expect("parses");
    assert_eq!(usage2.retention_days, 7);
    assert_eq!(usage2.budget.daily_token_limit, 0);
}

#[test]
fn w1_usage_budget_typed_roundtrip_preserves_explicit() {
    let src = serde_json::json!({
        "usage": {"budget": {
            "daily_token_limit": 100_000,
            "monthly_cost_limit_usd": 12.5,
            "mode": "warn"
        }}
    });
    let cfg: Config = serde_json::from_value(src).expect("parses");
    let b = &cfg.usage.as_ref().expect("usage present").budget;
    assert_eq!(b.daily_token_limit, 100_000);
    assert_eq!(b.monthly_cost_limit_usd, 12.5);
    assert_eq!(b.mode, "warn");
    let out = serde_json::to_value(&cfg).expect("serializes");
    assert_eq!(out["usage"]["budget"]["daily_token_limit"], 100_000);
    assert_eq!(out["usage"]["budget"]["mode"], "warn");
}

#[test]
fn w1_usage_budget_resolver_defaults_and_values() {
    let d = resolve_usage_budget(None);
    assert_eq!(d.daily_token_limit, 0);
    assert_eq!(d.monthly_token_limit, 0);
    assert_eq!(d.daily_cost_limit_usd, 0.0);
    assert_eq!(d.monthly_cost_limit_usd, 0.0);
    assert!(d.reject, "缺省 reject");
    let v = resolve_usage_budget(Some(&serde_json::json!({
        "usage": {"budget": {
            "daily_token_limit": 50_000,
            "monthly_token_limit": 1_000_000,
            "daily_cost_limit_usd": 3.0,
            "monthly_cost_limit_usd": 40.0,
            "mode": "warn"
        }}
    })));
    assert_eq!(v.daily_token_limit, 50_000);
    assert_eq!(v.monthly_token_limit, 1_000_000);
    assert_eq!(v.daily_cost_limit_usd, 3.0);
    assert_eq!(v.monthly_cost_limit_usd, 40.0);
    assert!(!v.reject);
    // 负费用钳 0（= 关）。
    let neg = resolve_usage_budget(Some(&serde_json::json!({
        "usage": {"budget": {"daily_cost_limit_usd": -5.0}}
    })));
    assert_eq!(neg.daily_cost_limit_usd, 0.0);
}

// ---------------------------------------------------------------------------
// #11+#6–10 assets（inventory 快照周期刷新间隔；typed 字段化防 set_field
// round-trip 丢键——plugins/browser 段同款教训）
// ---------------------------------------------------------------------------

#[test]
fn w1_assets_config_serde_default_and_roundtrip() {
    // 顶层 {}：assets 段缺席 → Option None（None = 全默认 300s）。
    let none: Config = serde_json::from_str("{}").expect("empty parses");
    assert!(none.assets.is_none(), "段缺席 = None");
    // 段在、键缺席 → 字段级缺省 300。
    let empty: Config = serde_json::from_str(r#"{"assets": {}}"#).expect("empty section parses");
    assert_eq!(
        empty
            .assets
            .as_ref()
            .expect("assets present")
            .inventory_refresh_secs,
        300,
        "键缺席 = 缺省 300s"
    );
    // 显式值 roundtrip 不丢（0 = 只启动刷一次的合法形态）。
    let src = serde_json::json!({"assets": {"inventory_refresh_secs": 0}});
    let cfg: Config = serde_json::from_value(src.clone()).expect("parses");
    assert_eq!(cfg.assets.as_ref().unwrap().inventory_refresh_secs, 0);
    let round: Config =
        serde_json::from_value(serde_json::to_value(&cfg).expect("ser")).expect("rt");
    assert_eq!(round.assets.as_ref().unwrap().inventory_refresh_secs, 0);
    // 消费侧缺省同源：None → unwrap_or(300) 语义在 gateway（services.rs），
    // 这里锁 serde 缺省函数与 Default impl 同向。
    assert_eq!(AssetsConfig::default().inventory_refresh_secs, 300);
}

// ---------------------------------------------------------------------------
// W2 #2 三档基线：security.level 档位表 + 显式键优先
// ---------------------------------------------------------------------------

#[test]
fn w2_security_level_serde_default_basic() {
    // typed 缺省：SecurityFlagConfig.level 缺席 = "basic"（与「键缺席 →
    // Basic」同语义，upgrade 零行为突变）。
    let sec: SecurityFlagConfig = serde_json::from_str("{}").expect("empty parses");
    assert_eq!(sec.level, "basic");
    let sec2: SecurityFlagConfig = serde_json::from_str(r#"{"level": "STRICT"}"#).expect("parses");
    assert_eq!(sec2.level, "STRICT", "原样保留（归一在 parse 侧）");
    // roundtrip 不丢。
    let out = serde_json::to_value(&sec2).expect("ser");
    assert_eq!(out["level"], "STRICT");
}

#[test]
fn w2_resolve_security_level_checked_tuple() {
    // 键缺席 → (Basic, None)。
    assert_eq!(
        resolve_security_level_checked(None),
        (SecurityLevel::Basic, None)
    );
    assert_eq!(
        resolve_security_level_checked(Some(&serde_json::json!({}))),
        (SecurityLevel::Basic, None)
    );
    // 合法值（大小写不敏感）→ 对应档位 + 无注记。
    assert_eq!(
        resolve_security_level_checked(Some(&serde_json::json!(
            {"security": {"level": "STRICT"}}
        ))),
        (SecurityLevel::Strict, None)
    );
    assert_eq!(
        resolve_security_level_checked(Some(&serde_json::json!(
            {"security": {"level": " Standard "}}
        ))),
        (SecurityLevel::Standard, None)
    );
    // 键在场但值非法 → (Basic, Some(原文))——静默降级必须在装配面响出来。
    let (lvl, note) = resolve_security_level_checked(Some(&serde_json::json!(
        {"security": {"level": "paranoid"}}
    )));
    assert_eq!(lvl, SecurityLevel::Basic);
    assert_eq!(note.as_deref(), Some("paranoid"), "非法值必须带原文注记");
    // 便捷形态丢注记。
    assert_eq!(
        resolve_security_level(Some(&serde_json::json!(
            {"security": {"level": "paranoid"}}
        ))),
        SecurityLevel::Basic
    );
}

#[test]
fn w2_level_defaults_rate_limit_budget_executor() {
    let std_cfg = serde_json::json!({"security": {"level": "standard"}});
    let strict_cfg = serde_json::json!({"security": {"level": "strict"}});
    let basic_cfg = serde_json::json!({"security": {"level": "basic"}});

    // 流控：basic 关（现形态），standard+ 开（qps/burst 仍走结构缺省）。
    assert!(!resolve_rate_limit(Some(&basic_cfg)).enabled);
    assert!(resolve_rate_limit(Some(&std_cfg)).enabled);
    assert!(resolve_rate_limit(Some(&strict_cfg)).enabled);
    assert_eq!(
        resolve_rate_limit(Some(&std_cfg)).qps,
        DEFAULT_RATE_LIMIT_QPS
    );

    // token 配额：basic 0（关），standard+ 2M/20M；费用维恒 0（不设档位缺省）。
    let b = resolve_usage_budget(Some(&basic_cfg));
    assert_eq!(b.daily_token_limit, 0);
    assert_eq!(b.monthly_token_limit, 0);
    let s = resolve_usage_budget(Some(&std_cfg));
    assert_eq!(s.daily_token_limit, 2_000_000);
    assert_eq!(s.monthly_token_limit, 20_000_000);
    assert_eq!(s.daily_cost_limit_usd, 0.0, "费用维不设档位缺省");
    assert!(s.reject, "mode 缺席仍 reject");

    // 执行体隔离：只 strict 开。
    assert!(!resolve_executor_enabled(Some(&basic_cfg)));
    assert!(!resolve_executor_enabled(Some(&std_cfg)));
    assert!(resolve_executor_enabled(Some(&strict_cfg)));
    // 段缺席（None config）→ Basic 档 → 关（升级零突变）。
    assert!(!resolve_executor_enabled(None));
}

#[test]
fn w2_explicit_key_wins_over_level_table() {
    // 档位是起点不是锁死：显式键永远优先。
    // strict 档 + 显式流控关 → 关。
    let off = resolve_rate_limit(Some(&serde_json::json!({
        "security": {"level": "strict", "rate_limit": {"enabled": false}}
    })));
    assert!(!off.enabled, "strict 档 + 显式关 = 关（显式键优先）");

    // basic 档 + 显式流控开 → 开。
    let on = resolve_rate_limit(Some(&serde_json::json!({
        "security": {"level": "basic", "rate_limit": {"enabled": true}}
    })));
    assert!(on.enabled, "basic 档 + 显式开 = 开");

    // strict 档 + 显式执行体隔离关 → 关（显式 false 永远生效）。
    assert!(!resolve_executor_enabled(Some(&serde_json::json!({
        "security": {"level": "strict"},
        "executor": {"enabled": false}
    }))));
    // basic 档 + 显式执行体隔离开 → 开。
    assert!(resolve_executor_enabled(Some(&serde_json::json!({
        "security": {"level": "basic"},
        "executor": {"enabled": true}
    }))));

    // standard 档 + 显式配额覆盖档位表数字。
    let b = resolve_usage_budget(Some(&serde_json::json!({
        "security": {"level": "standard"},
        "usage": {"budget": {"daily_token_limit": 500}}
    })));
    assert_eq!(b.daily_token_limit, 500);
    assert_eq!(b.monthly_token_limit, 20_000_000, "未覆盖的维仍走档位表");
}

#[test]
fn w2_security_level_from_flag_matrix() {
    assert_eq!(security_level_from_flag(None), SecurityLevel::Basic);
    let f = |level: &str| SecurityFlagConfig {
        level: level.to_string(),
        ..SecurityFlagConfig::default()
    };
    assert_eq!(
        security_level_from_flag(Some(&f("strict"))),
        SecurityLevel::Strict
    );
    assert_eq!(
        security_level_from_flag(Some(&f("Standard"))),
        SecurityLevel::Standard
    );
    assert_eq!(
        security_level_from_flag(Some(&f("junk"))),
        SecurityLevel::Basic,
        "非法值 typed 读回落 Basic（响铃职责在装配面 checked 形态）"
    );
}

#[test]
fn w2_level_behavior_table() {
    // 三档 × 四行为位：chat_mode / executor / preinstall_scan / charter。
    for (lvl, mode, exec, scan, charter) in [
        (SecurityLevel::Basic, "build", false, false, false),
        (SecurityLevel::Standard, "build", false, false, true),
        (SecurityLevel::Strict, "plan", true, true, true),
    ] {
        assert_eq!(lvl.default_chat_mode(), mode);
        assert_eq!(lvl.executor_enabled_default(), exec);
        assert_eq!(lvl.preinstall_scan_required(), scan);
        assert_eq!(lvl.charter_required(), charter);
    }
}

#[test]
fn w2_level_as_str_roundtrip_with_parse() {
    // as_str 是规范小写形态的单一真相源（wire 宣告 / 审计盖章 / 显示投影
    // 用）——parse(as_str(l)) 恒等于 l，防各处手写小写字面量漂移。
    for lvl in [
        SecurityLevel::Basic,
        SecurityLevel::Standard,
        SecurityLevel::Strict,
    ] {
        assert_eq!(SecurityLevel::parse(lvl.as_str()), Some(lvl));
    }
    assert_eq!(SecurityLevel::Basic.as_str(), "basic");
    assert_eq!(SecurityLevel::Standard.as_str(), "standard");
    assert_eq!(SecurityLevel::Strict.as_str(), "strict");
}

// ---------------------------------------------------------------------------
// W2 批三：#16 prompt_eval / #14 preinstall_scan 解析
// ---------------------------------------------------------------------------

#[test]
fn w2b3_resolve_prompt_eval_defaults_and_overrides() {
    // 键缺席 → 常态化缺省：on_onboard=true（默认开），on_persona_change=
    // false（编辑高频路径默认不跑）。
    let d = resolve_prompt_eval(None);
    assert!(d.on_onboard, "onboard 后自动评估缺省开（常态化语义）");
    assert!(!d.on_persona_change, "人格编辑触发缺省关");
    assert_eq!(d, resolve_prompt_eval(Some(&serde_json::json!({}))));
    assert_eq!(
        d,
        resolve_prompt_eval(Some(&serde_json::json!({"security": {}})))
    );
    // 显式键照读（档位是起点不是锁死）。
    let both_on = resolve_prompt_eval(Some(&serde_json::json!(
        {"security": {"prompt_eval": {"on_onboard": true, "on_persona_change": true}}}
    )));
    assert!(both_on.on_onboard && both_on.on_persona_change);
    let both_off = resolve_prompt_eval(Some(&serde_json::json!(
        {"security": {"prompt_eval": {"on_onboard": false}}}
    )));
    assert!(!both_off.on_onboard, "显式关压过缺省开");
    assert!(!both_off.on_persona_change);
    // 非布尔值（写坏）→ 按缺省，不 panic。
    let garbage = resolve_prompt_eval(Some(&serde_json::json!(
        {"security": {"prompt_eval": {"on_onboard": "yes"}}}
    )));
    assert!(garbage.on_onboard);
    assert!(!garbage.on_persona_change);
}

#[test]
fn w2b3_resolve_preinstall_scan_mode() {
    use super::PreinstallScanMode;
    // 缺省（basic、键缺席）= Off = 现形态零突变。
    assert_eq!(resolve_preinstall_scan(None), PreinstallScanMode::Off);
    assert_eq!(
        resolve_preinstall_scan(Some(&serde_json::json!({}))),
        PreinstallScanMode::Off
    );
    assert_eq!(
        resolve_preinstall_scan(Some(&serde_json::json!(
            {"security": {"level": "basic"}}
        ))),
        PreinstallScanMode::Off
    );
    // 显式 enabled=true（非 strict）→ On。
    assert_eq!(
        resolve_preinstall_scan(Some(&serde_json::json!(
            {"security": {"preinstall_scan": {"enabled": true}}}
        ))),
        PreinstallScanMode::On
    );
    // 显式 enabled=false / 非布尔 → Off。
    assert_eq!(
        resolve_preinstall_scan(Some(&serde_json::json!(
            {"security": {"preinstall_scan": {"enabled": false}}}
        ))),
        PreinstallScanMode::Off
    );
    assert_eq!(
        resolve_preinstall_scan(Some(&serde_json::json!(
            {"security": {"preinstall_scan": {"enabled": "on"}}}
        ))),
        PreinstallScanMode::Off
    );
    // strict 档 → Enforced（显式键压不过档位强制）。
    let strict = serde_json::json!({"security": {"level": "strict"}});
    assert_eq!(
        resolve_preinstall_scan(Some(&strict)),
        PreinstallScanMode::Enforced
    );
    let strict_off = serde_json::json!(
        {"security": {"level": "strict", "preinstall_scan": {"enabled": false}}}
    );
    assert_eq!(
        resolve_preinstall_scan(Some(&strict_off)),
        PreinstallScanMode::Enforced,
        "强制不可关"
    );
}
