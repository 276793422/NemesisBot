//! capabilities 表的防漂移与良构性测试（从生产文件迁出，遵循内联测试纪律）。

use super::*;
use crate::WorkflowContext;
use crate::nodes::NodeExecutor;
use crate::types::{NodeDef, NodeResult};
use std::collections::HashMap;
use std::sync::Arc;

/// No-op executor used only to occupy registry slots in the anti-drift
/// test (engine-registered types need a provider/runner we don't have in
/// unit tests — the executor identity is irrelevant here, only the key
/// set matters).
struct DummyExecutor;

#[async_trait::async_trait]
impl NodeExecutor for DummyExecutor {
    async fn execute(
        &self,
        _node: &NodeDef,
        _context: &HashMap<String, serde_json::Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        Err("dummy".to_string())
    }
}

fn table_node_types() -> Vec<String> {
    let mut v: Vec<String> = capabilities()
        .node_types
        .into_iter()
        .map(|n| n.node_type)
        .collect();
    v.sort();
    v
}

/// Anti-drift: the capability table must exactly equal the production
/// executor registry key set. Production = `NodeExecutorRegistry::new()`
/// (8 builtin + 3 composite stubs) + engine.rs additional registrations
/// (question_classifier / parameter_extractor / agent). If someone adds
/// or removes an executor without updating this table, this fails.
#[test]
fn capabilities_match_production_executor_registry() {
    let reg = crate::nodes::NodeExecutorRegistry::new();
    // Mirror WorkflowEngine::new()'s extra registrations (engine.rs 593-653).
    for extra in ["question_classifier", "parameter_extractor", "agent"] {
        reg.register(extra, Arc::new(DummyExecutor));
    }
    let mut registry_keys = reg.node_types();
    registry_keys.sort();
    assert_eq!(
        table_node_types(),
        registry_keys,
        "capabilities.rs node table drifted from NodeExecutorRegistry — \
         update capabilities() when executors change"
    );
}

/// Trigger whitelist must match parser::validate's accepted set and
/// driver_status's known set — three surfaces, one list.
#[test]
fn trigger_types_match_parser_and_driver_status() {
    let caps = capabilities();
    let table: Vec<&str> = caps
        .trigger_types
        .iter()
        .map(|t| t.trigger_type.as_str())
        .collect();
    assert_eq!(table, crate::driver_status::all_known_trigger_types());
}

/// Every node type in the table must carry a non-empty summary, and
/// required/optional config must not overlap (a key can't be both).
#[test]
fn table_rows_are_well_formed() {
    for n in capabilities().node_types {
        assert!(
            !n.summary.is_empty(),
            "node {} missing summary",
            n.node_type
        );
        for r in &n.required_config {
            assert!(
                !n.optional_config.contains(r),
                "node {} lists {} as both required and optional",
                n.node_type,
                r
            );
        }
    }
}

/// The prompt renderer must mention every node type and trigger type —
/// the LLM can only generate what it can see.
#[test]
fn prompt_renderer_covers_every_type() {
    let rendered = render_for_prompt();
    for n in capabilities().node_types {
        assert!(
            rendered.contains(&n.node_type),
            "render_for_prompt missing node type {}",
            n.node_type
        );
    }
    for t in capabilities().trigger_types {
        assert!(
            rendered.contains(&t.trigger_type),
            "render_for_prompt missing trigger type {}",
            t.trigger_type
        );
    }
}
