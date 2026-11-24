//! （持续性预算，2026-11-23 拍板二次修订）：`agents.turn_end_continues` 配置面测试。
//!
//! - serde 缺省：缺键 = 2（= 修复前硬编码常量 `MAX_TURN_END_CONTINUES`
//!   的行为，**兼容老版本/老配置零漂移**）；
//! - typed roundtrip：显式值（**含 0=不限 opt-in 哨兵**）序列化/反序列化
//!   不丢——0 是「不限」哨兵，绝不能被缺省覆盖；
//! - raw-JSON 解析（`resolve_turn_end_continues`）：AgentLoop hook Continue
//!   判定点的 fresh-read 走这条路，键路径/缺省/垃圾输入语义必须与 typed
//!   字段单源（垃圾输入落缺省 2，**不得被晋升成 0=不限**）。

use super::*;

#[test]
fn turn_end_continues_serde_default_matches_legacy_constant() {
    // 顶层 `{}`：agents 键整体缺席 → AgentsConfig 手写 Default
    // （turn_end_continues = 2 = 旧常量行为，2026-11-23 二次拍板：兼容老版本）。
    let cfg: Config = serde_json::from_str("{}").expect("empty config parses");
    assert_eq!(
        cfg.agents.turn_end_continues, DEFAULT_TURN_END_CONTINUES,
        "缺键 = 2 = 旧常量行为（兼容老版本）"
    );
    // agents 节在场、键缺席 → 字段级 serde 缺省路径，同一单源。
    let cfg2: Config = serde_json::from_str(r#"{"agents": {}}"#).expect("parses");
    assert_eq!(cfg2.agents.turn_end_continues, 2, "键缺席同样 = 旧常量行为");
}

#[test]
fn turn_end_continues_typed_roundtrip_preserves_explicit_values() {
    // 0 = 不限哨兵必须与有限帽一起在 roundtrip 后原样保留。
    for v in [0u64, 1, 2, 50, 100_000] {
        let src = serde_json::json!({
            "agents": {"turn_end_continues": v}
        });
        let cfg: Config = serde_json::from_value(src).expect("parses");
        assert_eq!(cfg.agents.turn_end_continues, v as u32);
        // round-trip：显式值（尤其 0=不限）不得被缺省覆盖。
        let out = serde_json::to_value(&cfg).expect("serializes");
        assert_eq!(
            out["agents"]["turn_end_continues"]
                .as_u64()
                .expect("typed u32 survives roundtrip"),
            v,
            "显式 {v} 必须在 save roundtrip 后保留"
        );
    }
}

#[test]
fn turn_end_continues_raw_json_resolver_matches_typed_surface() {
    // 缺 config / 缺 agents / 缺键 → 缺省 2（旧常量行为，兼容老版本）。
    assert_eq!(resolve_turn_end_continues(None), 2);
    assert_eq!(resolve_turn_end_continues(Some(&serde_json::json!({}))), 2);
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {}
        }))),
        2
    );
    // 显式 0 = 不限（opt-in 新特性）——缺省路径绝不能吞掉这个哨兵。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": 0}
        }))),
        0
    );
    // 显式有限帽透传（loop 侧语义：拦 N 次后放行）；2 与缺省同值但路径不同。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": 2}
        }))),
        2
    );
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": 50}
        }))),
        50
    );
    // 负数无语义 → 缺省 2（**不落 0**——0 在本键是「不限」哨兵，垃圾输入
    // 不得被晋升成不限；同族先例 tool_result_cap 钳 0=关，此处语义相反）。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": -3}
        }))),
        2
    );
    // BUG-20261124-015：越域值（> u32::MAX，i64 正数域）按「未设」落缺省
    // 2——裸 `as u32` 会把 2^32 截成 0 恰好命中「不限」哨兵（注释明文
    // 「垃圾输入不得被晋升成不限」；BUG-20261117-085 同族惯例）。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": 4_294_967_296i64}
        }))),
        2,
        "2^32 截断不得晋升成 0=不限"
    );
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": 9_000_000_000i64}
        }))),
        2,
        "越域大值按未设落缺省"
    );
    // u32::MAX 边界内是合法值（透传，有限帽语义）。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": 4_294_967_295i64}
        }))),
        4_294_967_295u32,
        "边界内 u32::MAX 透传"
    );
    // 类型怪（字符串）→ 缺省（2 = 旧常量行为），不炸。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"turn_end_continues": "many"}
        }))),
        2
    );
    // defaults 层同名字段不是本键（键路径唯一：agents 直下）。
    assert_eq!(
        resolve_turn_end_continues(Some(&serde_json::json!({
            "agents": {"defaults": {"turn_end_continues": 123}}
        }))),
        2
    );
}
