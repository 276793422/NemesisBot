//! P5（能力扩展 WS2 compaction）：`agents.defaults.compact_keep_recent_tokens`
//! 配置面测试。
//!
//! - serde 缺省：缺键 = 20000（对齐 业界 keepRecentTokens）；
//! - typed roundtrip：显式值（含 0=旧按条数回退）序列化/反序列化不丢；
//! - raw-JSON 解析（`resolve_compact_keep_recent_tokens`）：AgentLoop compact
//!   域的 fresh-read 走这条路，键路径/缺省/负数钳 0 必须与 typed 字段单源。

use super::*;

#[test]
fn ws2_serde_default_is_20000_when_key_absent() {
    let cfg: Config = serde_json::from_str("{}").expect("empty config parses");
    assert_eq!(
        cfg.agents.defaults.compact_keep_recent_tokens, DEFAULT_COMPACT_KEEP_RECENT_TOKENS,
        "缺键 = 缺省 20000（对齐 pi keepRecentTokens 口径）"
    );
    // Default impl 同源（手写 Default 不走 serde，两处必须一致）。
    assert_eq!(
        AgentDefaults::default().compact_keep_recent_tokens,
        DEFAULT_COMPACT_KEEP_RECENT_TOKENS
    );
}

#[test]
fn ws2_typed_roundtrip_preserves_explicit_values() {
    for v in [0i64, 1, 20000, 100_000] {
        let src = serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": v}}
        });
        let cfg: Config = serde_json::from_value(src).expect("parses");
        assert_eq!(cfg.agents.defaults.compact_keep_recent_tokens, v);
        // round-trip：显式值（尤其 0=旧按条数回退）不得被缺省覆盖。
        let out = serde_json::to_value(&cfg).expect("serializes");
        assert_eq!(
            out["agents"]["defaults"]["compact_keep_recent_tokens"]
                .as_i64()
                .expect("typed i64 survives roundtrip"),
            v,
            "显式 {v} 必须在 save roundtrip 后保留"
        );
    }
}

#[test]
fn ws2_raw_json_resolver_matches_typed_surface() {
    // 缺 config / 缺 agents / 缺 defaults / 缺键 → 缺省。
    assert_eq!(resolve_compact_keep_recent_tokens(None), 20000);
    assert_eq!(
        resolve_compact_keep_recent_tokens(Some(&serde_json::json!({}))),
        20000
    );
    assert_eq!(
        resolve_compact_keep_recent_tokens(Some(&serde_json::json!({
            "agents": {}
        }))),
        20000
    );
    // 显式值透传；0 = 旧按条数回退路径（调用方语义）。
    assert_eq!(
        resolve_compact_keep_recent_tokens(Some(&serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": 0}}
        }))),
        0
    );
    assert_eq!(
        resolve_compact_keep_recent_tokens(Some(&serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": 5000}}
        }))),
        5000
    );
    // 负数无语义 → 钳 0（回退旧路径，不 panic 不取缺省）。
    assert_eq!(
        resolve_compact_keep_recent_tokens(Some(&serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": -7}}
        }))),
        0
    );
    // 类型怪（字符串）→ 缺省，不炸。
    assert_eq!(
        resolve_compact_keep_recent_tokens(Some(&serde_json::json!({
            "agents": {"defaults": {"compact_keep_recent_tokens": "big"}}
        }))),
        20000
    );
}
