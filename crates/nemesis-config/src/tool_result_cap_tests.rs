//! B30（2026-11-06 三方对比整改）：`agents.tool_result_cap` 配置面测试。
//!
//! - serde 缺省：缺键 = 8000（对齐业界通行 8K 字符封顶）——含顶层缺
//!   `agents` 键的整段缺席路径（`AgentsConfig` 手写 Default，同
//!   AgentDefaults/FsWatcherConfig 先例；derive 零值会把全部
//!   `default = "default_*"` 字段塌成 0/false）；
//! - typed roundtrip：显式值（含 0=关）序列化/反序列化不丢；
//! - raw-JSON 解析（`resolve_tool_result_cap`）：AgentLoop dispatch 后整形点
//!   的 fresh-read 走这条路，键路径/缺省/负数钳 0 必须与 typed 字段单源。

use super::*;

#[test]
fn b30_serde_default_is_8000_when_key_absent() {
    // 顶层 `{}`：agents 键整体缺席 → Config.agents 字段级 #[serde(default)]
    // → AgentsConfig 手写 Default（tool_result_cap = 8000）。若回退 derive，
    // 本断言红（u64 0 = 关，语义反转）——即手写 impl 的回归锁。
    let cfg: Config = serde_json::from_str("{}").expect("empty config parses");
    assert_eq!(
        cfg.agents.tool_result_cap, DEFAULT_TOOL_RESULT_CAP as u64,
        "缺键 = 缺省 8000（对齐业界 8K 字符封顶口径）"
    );
    // 同坑兄弟字段顺手钉住（手写 impl 的其余收益）：
    assert!(cfg.agents.turn_wal, "{{}} 缺席路径 turn_wal 不得塌成 false");
    assert!(
        cfg.agents.image_downscale,
        "{{}} 缺席路径 image_downscale 不得塌成 false"
    );
    assert!(
        cfg.agents.executor_tool_timeout_secs > 0,
        "{{}} 缺席路径 executor_tool_timeout_secs 不得塌成 0（0=关）"
    );
    // agents 节在场、键缺席 → 字段级 serde 缺省函数路径。
    let cfg2: Config = serde_json::from_str(r#"{"agents": {}}"#).expect("parses");
    assert_eq!(cfg2.agents.tool_result_cap, DEFAULT_TOOL_RESULT_CAP as u64);
}

#[test]
fn b30_typed_roundtrip_preserves_explicit_values() {
    for v in [0u64, 1, 8000, 100_000] {
        let src = serde_json::json!({
            "agents": {"tool_result_cap": v}
        });
        let cfg: Config = serde_json::from_value(src).expect("parses");
        assert_eq!(cfg.agents.tool_result_cap, v);
        // round-trip：显式值（尤其 0=关）不得被缺省覆盖。
        let out = serde_json::to_value(&cfg).expect("serializes");
        assert_eq!(
            out["agents"]["tool_result_cap"]
                .as_u64()
                .expect("typed u64 survives roundtrip"),
            v,
            "显式 {v} 必须在 save roundtrip 后保留"
        );
    }
}

#[test]
fn b30_raw_json_resolver_matches_typed_surface() {
    // 缺 config / 缺 agents / 缺键 → 缺省。
    assert_eq!(resolve_tool_result_cap(None), 8000);
    assert_eq!(resolve_tool_result_cap(Some(&serde_json::json!({}))), 8000);
    assert_eq!(
        resolve_tool_result_cap(Some(&serde_json::json!({
            "agents": {}
        }))),
        8000
    );
    // 显式值透传；0 = 关（调用方语义：跨工具兜底封顶关闭）。
    assert_eq!(
        resolve_tool_result_cap(Some(&serde_json::json!({
            "agents": {"tool_result_cap": 0}
        }))),
        0
    );
    assert_eq!(
        resolve_tool_result_cap(Some(&serde_json::json!({
            "agents": {"tool_result_cap": 4000}
        }))),
        4000
    );
    // 负数无语义 → 钳 0（关闭，不 panic 不取缺省）。
    assert_eq!(
        resolve_tool_result_cap(Some(&serde_json::json!({
            "agents": {"tool_result_cap": -3}
        }))),
        0
    );
    // 类型怪（字符串）→ 缺省，不炸。
    assert_eq!(
        resolve_tool_result_cap(Some(&serde_json::json!({
            "agents": {"tool_result_cap": "huge"}
        }))),
        8000
    );
    // defaults 层同名字段不是本键（键路径唯一：agents 直下）。
    assert_eq!(
        resolve_tool_result_cap(Some(&serde_json::json!({
            "agents": {"defaults": {"tool_result_cap": 123}}
        }))),
        8000
    );
}
