//! DraftStore 的行为测试（从生产文件迁出，遵循内联测试纪律）。

use super::*;
use crate::types::{NodeDef, TriggerConfig};

fn sample_workflow(name: &str) -> Workflow {
    Workflow {
        name: name.to_string(),
        description: "generated".into(),
        version: "1.0.0".into(),
        triggers: vec![TriggerConfig {
            trigger_type: "cron".into(),
            config: Default::default(),
        }],
        nodes: vec![NodeDef {
            id: "start".into(),
            node_type: "llm".into(),
            config: {
                let mut m = std::collections::HashMap::new();
                m.insert("prompt".to_string(), serde_json::json!("hello"));
                m
            },
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
            is_terminal: true,
        }],
        edges: vec![],
        variables: Default::default(),
        metadata: Default::default(),
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nmb_wf_drafts_{}_{}_{}",
        tag,
        std::process::id(),
        chrono::Local::now().timestamp_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn save_list_get_roundtrip() {
    let root = temp_dir("roundtrip");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    store.save(&sample_workflow("daily-report")).unwrap();

    let list = store.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "daily-report");
    assert!(list[0].valid, "errors: {:?}", list[0].validation_errors);
    assert_eq!(list[0].node_count, 1);
    assert_eq!(list[0].trigger_types, vec!["cron"]);

    let detail = store.get("daily-report").unwrap();
    assert_eq!(detail.workflow.name, "daily-report");
    assert!(detail.yaml.contains("daily-report"));
    assert!(detail.summary.valid);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn invalid_draft_still_saves_and_lists_with_errors() {
    let root = temp_dir("invalid");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs);

    let mut wf = sample_workflow("broken");
    wf.nodes.clear(); // validate() rejects empty node list
    let summary = store.save(&wf).unwrap();
    assert!(!summary.valid);
    assert!(!summary.validation_errors.is_empty());

    // apply refuses
    let engine = crate::engine::WorkflowEngine::new();
    let err = store.apply(&engine, "broken").unwrap_err();
    assert!(err.contains("failed validation"), "err: {}", err);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_registers_and_deletes_draft() {
    let root = temp_dir("apply");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    store.save(&sample_workflow("new-flow")).unwrap();
    let applied = store.apply(&engine, "new-flow").unwrap();
    assert!(!applied.replaced_existing);
    assert!(applied.backup_file.is_none());

    assert!(engine.get_workflow("new-flow").is_some());
    assert!(store.list().is_empty(), "draft must be consumed");
    assert!(defs.join("new-flow.yaml").exists(), "definition written");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn apply_over_existing_backs_up_to_history() {
    let root = temp_dir("backup");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    // Seed an existing definition through the normal path.
    engine
        .persist_workflow(sample_workflow("existing-flow"))
        .unwrap();

    // Draft a modified version (different node count so we can tell them apart).
    let mut wf = sample_workflow("existing-flow");
    wf.nodes.push(NodeDef {
        id: "second".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(1));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf).unwrap();

    let applied = store.apply(&engine, "existing-flow").unwrap();
    assert!(applied.replaced_existing);
    let backup = applied.backup_file.expect("backup recorded");

    let hist_dir = defs.join(".history");
    let backup_path = hist_dir.join(&backup);
    assert!(backup_path.exists(), "backup file on disk");
    let old: Workflow =
        serde_yaml::from_str(&std::fs::read_to_string(&backup_path).unwrap()).unwrap();
    assert_eq!(old.nodes.len(), 1, "backup holds the OLD definition");
    assert_eq!(engine.get_workflow("existing-flow").unwrap().nodes.len(), 2);

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rapid_reapplies_keep_distinct_history_backups() {
    // Two applies inside the same second must not overwrite each other's
    // .history/ backup (fs::copy replaces the destination silently) — an
    // agent iterating create→apply without runs produces rapid consecutive
    // applies. The backup timestamp carries milliseconds, so each apply
    // keeps its own backup and the OLDER one survives verbatim.
    let root = temp_dir("backup-ms");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    let engine = crate::engine::WorkflowEngine::new();
    engine.set_workflow_defs_dir(defs.clone());

    // v1 (1 node) registered through the normal path.
    engine.persist_workflow(sample_workflow("hot")).unwrap();

    // v2 (2 nodes) and v3 (3 nodes) applied back-to-back.
    //
    // The 5ms sleep is part of the fixture, not a delay tactic: it
    // guarantees the two backup timestamps land in distinct milliseconds
    // (a sub-millisecond gap could coincidentally straddle zero ms
    // distance and fail the fixed code), while staying far inside the
    // same second — which is exactly the window the old second-resolution
    // timestamp collided in.
    let mut wf2 = sample_workflow("hot");
    wf2.nodes.push(NodeDef {
        id: "n2".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(1));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf2).unwrap();
    let a1 = store.apply(&engine, "hot").unwrap();
    assert!(a1.replaced_existing);

    std::thread::sleep(std::time::Duration::from_millis(5));

    let mut wf3 = wf2.clone();
    wf3.nodes.push(NodeDef {
        id: "n3".into(),
        node_type: "delay".into(),
        config: {
            let mut m = std::collections::HashMap::new();
            m.insert("seconds".to_string(), serde_json::json!(2));
            m
        },
        depends_on: vec![],
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    });
    store.save(&wf3).unwrap();
    let a2 = store.apply(&engine, "hot").unwrap();
    assert!(a2.replaced_existing);

    // Both backups survive with distinct names AND distinct contents:
    // a1's backup holds v1 (1 node), a2's holds v2 (2 nodes).
    assert_ne!(
        a1.backup_file.as_ref(),
        a2.backup_file.as_ref(),
        "distinct backup names"
    );
    let hist = defs.join(".history");
    let names: Vec<std::path::PathBuf> = std::fs::read_dir(&hist)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert_eq!(names.len(), 2, "both backups must survive: {:?}", names);
    let parse = |name: &str| -> Workflow {
        serde_yaml::from_str(&std::fs::read_to_string(hist.join(name)).unwrap()).unwrap()
    };
    assert_eq!(
        parse(a1.backup_file.as_deref().unwrap()).nodes.len(),
        1,
        "a1's backup must hold v1"
    );
    assert_eq!(
        parse(a2.backup_file.as_deref().unwrap()).nodes.len(),
        2,
        "a2's backup must hold v2"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn discard_is_idempotent() {
    let root = temp_dir("discard");
    let store = DraftStore::new(root.join("drafts"), root.join("definitions"));
    store.save(&sample_workflow("doomed")).unwrap();
    store.discard("doomed").unwrap();
    store.discard("doomed").unwrap(); // second call: Ok
    assert!(store.list().is_empty());
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn drafts_dir_is_sibling_of_definitions() {
    let defs = Path::new("/ws/workflow/definitions");
    assert_eq!(drafts_dir_from_defs(defs), Path::new("/ws/workflow/drafts"));
}

#[test]
fn sanitizer_matches_engine_behavior() {
    assert_eq!(sanitize_workflow_filename("a b/c.d"), "a_b_c_d");
    assert_eq!(sanitize_workflow_filename(""), "wf_unnamed");
    // '.' is replaced like any other unsafe char (the engine's wf_ prefix
    // branch is unreachable for the same reason) — the copy here must stay
    // byte-identical to engine.rs's so draft stems == definition stems.
    assert_eq!(sanitize_workflow_filename(".hidden"), "_hidden");
}

#[test]
fn save_surfaces_lint_warnings_in_summary() {
    let root = temp_dir("warnings");
    let defs = root.join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    let store = DraftStore::new(root.join("drafts"), defs.clone());

    // llm max_tokens 过小 + 无触发器 → L1 + L2 两条 warning
    let mut wf = sample_workflow("risky");
    wf.triggers.clear();
    wf.nodes[0]
        .config
        .insert("max_tokens".to_string(), serde_json::json!(200));
    store.save(&wf).unwrap();

    let summary = &store.list()[0];
    assert!(summary.valid, "lint 是 warning 通道，不影响 valid");
    assert_eq!(summary.warnings.len(), 2, "{:?}", summary.warnings);
    assert!(summary.warnings[0].contains("max_tokens=200"));
    assert!(summary.warnings[1].contains("run_now"));

    std::fs::remove_dir_all(&root).ok();
}
