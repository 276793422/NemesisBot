//! lint 规则（L1-L6）单测——自生产文件内联测试外置（2026-09-23，
//! check-inline-tests 纪律；原内联模块随 faed9364 引入，L6 用例随
//! e16a4702 增补，外置时一并纳入）。

use super::*;
use crate::types::{NodeDef, TriggerConfig};
use std::collections::HashMap;

fn wf(nodes: Vec<NodeDef>, triggers: Vec<TriggerConfig>) -> Workflow {
    Workflow {
        name: "t".into(),
        description: String::new(),
        version: "1.0.0".into(),
        triggers,
        nodes,
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

fn node(id: &str, node_type: &str, config: serde_json::Value) -> NodeDef {
    NodeDef {
        id: id.into(),
        node_type: node_type.into(),
        config: serde_json::from_value(config).unwrap(),
        depends_on: Vec::new(),
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }
}

#[test]
fn l1_small_llm_max_tokens_warns() {
    let w = wf(
        vec![node(
            "n1",
            "llm",
            serde_json::json!({"prompt": "p", "max_tokens": 200}),
        )],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    let warnings = lint(&w);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("n1") && warnings[0].contains("max_tokens=200"));
}

#[test]
fn l1_adequate_max_tokens_and_missing_key_stay_silent() {
    let w = wf(
        vec![
            node(
                "n1",
                "llm",
                serde_json::json!({"prompt": "p", "max_tokens": 2000}),
            ),
            node("n2", "llm", serde_json::json!({"prompt": "p"})),
        ],
        vec![],
    );
    let warnings = lint(&w);
    // 只剩 triggers 为空这一条
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("触发器"));
}

#[test]
fn l2_empty_triggers_warn_once() {
    let w = wf(
        vec![node("n1", "http", serde_json::json!({"url": "https://x"}))],
        vec![],
    );
    let warnings = lint(&w);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("run_now"));
}

#[test]
fn l3_embedded_credential_reference_warns() {
    let w = wf(
        vec![node(
            "h",
            "http",
            serde_json::json!({
                "url": "https://api.example.com?token=env:TOKEN",
                "headers": {"Authorization": "Bearer vault:tok", "X-Ok": "vault:whole"}
            }),
        )],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    let warnings = lint(&w);
    // url 内嵌 env: + Authorization 内嵌 vault: 各一条；X-Ok 整值合法
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("url")));
    assert!(warnings.iter().any(|w| w.contains("headers.Authorization")));
}

#[test]
fn l4_tier_name_as_model_warns_real_model_stays_silent() {
    let w = wf(
        vec![
            node(
                "n1",
                "llm",
                serde_json::json!({"prompt": "p", "model": "small"}),
            ),
            node(
                "n2",
                "llm",
                serde_json::json!({"prompt": "p", "model": "zhipu/glm-5.3-flash"}),
            ),
            node(
                "n3",
                "llm",
                serde_json::json!({"prompt": "p", "model": "gemini-2.5-pro"}),
            ),
        ],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    let warnings = lint(&w);
    // 只有 n1 的档位名触发；真名（含档位词的完整模型名）不误报
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("n1") && warnings[0].contains("档位名"),
        "{warnings:?}"
    );
}

#[test]
fn l5_output_placeholder_typo_warns_with_field_guidance() {
    let w = wf(
        vec![
            node(
                "s",
                "llm",
                serde_json::json!({"prompt": "汇总：{{fetch_todo.output}}"}),
            ),
            node(
                "h",
                "http",
                serde_json::json!({"url": "https://x?p={{a.output}}"}),
            ),
        ],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    let warnings = lint(&w);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for w in &warnings {
        assert!(
            w.contains("output 不是输出字段名") && w.contains("{{节点id."),
            "{w}"
        );
    }
    assert!(warnings[0].contains("s") && warnings[0].contains("prompt"));
    assert!(warnings[1].contains("h") && warnings[1].contains("url"));
}

#[test]
fn l5_correct_field_references_stay_silent() {
    let w = wf(
        vec![node(
            "s",
            "llm",
            serde_json::json!({
                "prompt": "汇总：{{fetch_todo.body}} / {{fetch_todo}} / {{user_name}}"
            }),
        )],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    assert!(lint(&w).is_empty(), "{:?}", lint(&w));
}

/// L6 回归（E 级复核实测）：条件边引用 condition 节点幻觉字段
/// `{{check.passed}}`（实际输出字段是 `condition_result`）当轮预警。
#[test]
fn l6_condition_edge_hallucinated_field_warns() {
    let mut w = wf(
        vec![
            node("check", "condition", serde_json::json!({})),
            node(
                "ok",
                "transform",
                serde_json::json!({"expression": "identity", "input": "x"}),
            ),
        ],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    w.edges = vec![crate::types::Edge {
        from_node: "check".into(),
        to_node: "ok".into(),
        condition: Some("!{{check.passed}}".into()),
    }];
    let warnings = lint(&w);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("check→ok")
            && warnings[0].contains("check.passed")
            && warnings[0].contains("{{check.condition_result}}"),
        "{warnings:?}"
    );
}

/// L6 不误伤：真实字段、取反、非 condition 节点的字段、变量引用。
#[test]
fn l6_correct_condition_refs_stay_silent() {
    let mut w = wf(
        vec![
            node("check", "condition", serde_json::json!({})),
            node("fetch", "http", serde_json::json!({"url": "https://x"})),
        ],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    w.edges = vec![
        crate::types::Edge {
            from_node: "check".into(),
            to_node: "fetch".into(),
            condition: Some("{{check.condition_result}}".into()),
        },
        crate::types::Edge {
            from_node: "fetch".into(),
            to_node: "check".into(),
            condition: Some("{{fetch.status_code}} == 200 && !{{skip}}".into()),
        },
        crate::types::Edge {
            from_node: "payload".into(),
            to_node: "fetch".into(),
            condition: Some("{{payload.value}}".into()),
        },
    ];
    assert!(lint(&w).is_empty(), "{:?}", lint(&w));
}

#[test]
fn no_warnings_for_clean_workflow() {
    let w = wf(
        vec![node(
            "h",
            "http",
            serde_json::json!({
                "url": "https://api.example.com",
                "headers": {"Authorization": "vault:tok"}
            }),
        )],
        vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: HashMap::new(),
        }],
    );
    assert!(lint(&w).is_empty(), "{:?}", lint(&w));
}
