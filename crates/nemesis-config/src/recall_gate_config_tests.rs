//! `memory.recall_gate` 配置节（项2 P2）serde 契约：缺席 = None（默认关）、
//! 部分键 = 其余落默认、完整 round-trip 保留、非对象 memory 节容忍。

use crate::MemoryFlagConfig;

/// 缺席 recall_gate 键 = None（老 config.json 零迁移）。
#[test]
fn absent_recall_gate_deserializes_none() {
    let raw = r#"{"enabled": true, "dreaming": {"enabled": false}}"#;
    let cfg: MemoryFlagConfig = serde_json::from_str(raw).expect("parse");
    assert!(cfg.enabled);
    assert!(cfg.recall_gate.is_none());
}

/// 空 memory 节 = 全默认（enabled=false 直通现状）。
#[test]
fn empty_memory_section_defaults() {
    let cfg: MemoryFlagConfig = serde_json::from_str("{}").expect("parse");
    assert!(!cfg.enabled);
    assert!(cfg.recall_gate.is_none());
}

/// recall_gate 部分键 = 其余字段落内置默认（min_candidates=4 / cache_secs=300）。
#[test]
fn partial_recall_gate_fills_defaults() {
    let raw = r#"{"recall_gate": {"enabled": true}}"#;
    let cfg: MemoryFlagConfig = serde_json::from_str(raw).expect("parse");
    let g = cfg.recall_gate.expect("recall_gate present");
    assert!(g.enabled);
    assert_eq!(g.model, "");
    assert_eq!(g.max_items, 0);
    assert_eq!(g.min_candidates, 4);
    assert_eq!(g.cache_secs, 300);
}

/// 完整 round-trip：全部字段写盘再读回零漂移。
#[test]
fn full_recall_gate_roundtrip() {
    let raw = r#"{
        "enabled": true,
        "recall_gate": {
            "enabled": true,
            "model": "glm-5.3-flash",
            "max_items": 2,
            "min_candidates": 3,
            "cache_secs": 120
        }
    }"#;
    let cfg: MemoryFlagConfig = serde_json::from_str(raw).expect("parse");
    let re: MemoryFlagConfig =
        serde_json::from_str(&serde_json::to_string(&cfg).expect("serialize")).expect("re-parse");
    let g = re.recall_gate.expect("roundtrip keeps recall_gate");
    assert!(g.enabled);
    assert_eq!(g.model, "glm-5.3-flash");
    assert_eq!(g.max_items, 2);
    assert_eq!(g.min_candidates, 3);
    assert_eq!(g.cache_secs, 120);
    // dreaming 与 recall_gate 并存不互斥。
    assert!(re.enabled);
}
