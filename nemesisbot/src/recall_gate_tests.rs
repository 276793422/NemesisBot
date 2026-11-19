//! 召回质量闸 gateway 侧装配（`build_recall_gate`）单测：模型链三臂
//! （显式别名 / small_model / 诚实跳过）+ 默认关直通 + provider 构造。
//! 全程无网络——create_provider 只构造客户端不拨号。

use crate::recall_gate::build_recall_gate;
use nemesis_config::Config;

/// 造一个带 model_list 条目的最小 Config（JSON 反序列化形态，与
/// config.json 磁盘契约同源）。
fn config_with(json_patch: &str) -> Config {
    let raw = format!(
        r#"{{
            "default_model": "test/main",
            "model_list": [
                {{"model_name": "test/main", "model": "main", "api_base": "http://127.0.0.1:9/v1", "api_key": "k"}},
                {{"model_name": "test/small", "model": "small", "api_base": "http://127.0.0.1:9/v1", "api_key": "k"}}
            ],
            "agents": {{"small_model": "test/small"}},
            "memory": {json_patch}
        }}"#
    );
    serde_json::from_str(&raw).expect("parse config")
}

/// recall_gate 缺席 = None（静默直通）。
#[test]
fn absent_section_returns_none() {
    let cfg = config_with(r#"{"enabled": true}"#);
    assert!(build_recall_gate(&cfg).is_none());
}

/// enabled=false（默认形态）= None（静默直通，零 WARN 噪音路径）。
#[test]
fn disabled_returns_none() {
    let cfg = config_with(r#"{"enabled": false, "recall_gate": {"enabled": false}}"#);
    assert!(build_recall_gate(&cfg).is_none());
}

/// 模型链诚实跳过臂：enabled=true 但 model 空 + small_model 未配 → None。
/// （本测试 config 的 agents.small_model 显式置空。）
#[test]
fn no_model_anywhere_returns_none() {
    let raw = r#"{
        "default_model": "test/main",
        "model_list": [
            {"model_name": "test/main", "model": "main", "api_base": "http://127.0.0.1:9/v1", "api_key": "k"}
        ],
        "agents": {"small_model": null},
        "memory": {"enabled": true, "recall_gate": {"enabled": true}}
    }"#;
    let cfg: Config = serde_json::from_str(raw).expect("parse");
    assert!(build_recall_gate(&cfg).is_none());
}

/// small_model 回落臂：model 空 → 走 agents.small_model → Some。
#[test]
fn small_model_fallback_builds_gate() {
    let cfg = config_with(r#"{"enabled": true, "recall_gate": {"enabled": true}}"#);
    let gate = build_recall_gate(&cfg).expect("gate via small_model");
    let s = gate.stats();
    // 新闸统计全零（只验可达性）。
    assert_eq!((s.recalled, s.judge_calls), (0, 0));
}

/// 显式别名臂优先：recall_gate.model 直接命中 model_list 条目。
#[test]
fn explicit_model_alias_builds_gate() {
    let cfg = config_with(
        r#"{"enabled": true, "recall_gate": {"enabled": true, "model": "test/main", "max_items": 2}}"#,
    );
    assert!(build_recall_gate(&cfg).is_some());
}

/// 显式别名指向不存在条目 = 解析失败 → None（诚实跳过，不 panic）。
#[test]
fn unknown_alias_returns_none() {
    let cfg = config_with(
        r#"{"enabled": true, "recall_gate": {"enabled": true, "model": "no/such-model"}}"#,
    );
    assert!(build_recall_gate(&cfg).is_none());
}
