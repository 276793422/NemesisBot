//! Tests for the private migration helper in `handlers/memory.rs`
//! (4b layer-1 gap fill — `migrate_legacy_vector_store` /
//! `vector_store_jsonl_path` had zero coverage; sibling
//! `memory_extra_tests.rs` covers the public commands only).
//!
//! Why a dedicated module: `memory_extra_tests` hangs off `handlers`
//! (sibling module), which cannot see `handlers::memory`'s private fns;
//! this file IS a child of `handlers::memory`, so `super::` reaches them.

use super::{migrate_legacy_vector_store, vector_store_jsonl_path};

fn legacy_path(ws: &std::path::Path) -> std::path::PathBuf {
    ws.join("memory").join("vector").join("vector_store.jsonl")
}

#[test]
fn migrates_legacy_file_to_manager_path_with_same_bytes() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(legacy_path(ws.path()).parent().unwrap()).unwrap();
    std::fs::write(legacy_path(ws.path()), "line1\nline2\n").unwrap();

    migrate_legacy_vector_store(ws.path().to_str().unwrap());

    let target = vector_store_jsonl_path(ws.path().to_str().unwrap());
    assert!(
        target.is_file(),
        "target {} must exist after migration",
        target.display()
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "line1\nline2\n",
        "copy must be byte-identical"
    );
    assert!(
        legacy_path(ws.path()).is_file(),
        "legacy copy is kept (one-way copy, not a move)"
    );
}

#[test]
fn does_not_overwrite_existing_target() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(legacy_path(ws.path()).parent().unwrap()).unwrap();
    std::fs::write(legacy_path(ws.path()), "LEGACY-BYTES").unwrap();

    let target = vector_store_jsonl_path(ws.path().to_str().unwrap());
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "CURRENT-BYTES").unwrap();

    migrate_legacy_vector_store(ws.path().to_str().unwrap());

    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "CURRENT-BYTES",
        "an existing target must win — migration never overwrites newer data"
    );
}

#[test]
fn missing_legacy_file_is_noop() {
    let ws = tempfile::tempdir().unwrap();
    // No legacy file at all: nothing happens, no target created.
    migrate_legacy_vector_store(ws.path().to_str().unwrap());
    let target = vector_store_jsonl_path(ws.path().to_str().unwrap());
    assert!(
        !target.exists(),
        "noop must not create {}",
        target.display()
    );
}

// ============================================================
// Phase 3 coverage (2026-08-25): main-switch read/write, status,
// documents / document.get / document.save, vector.status direct-path testing.
// Private methods callable directly (this file is a child of the memory module).
// ============================================================

use super::{MemoryHandler, read_main_switch, set_main_switch};

#[test]
fn read_main_switch_no_config_returns_false() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!read_main_switch(dir.path().to_str().unwrap()));
}

#[test]
fn read_main_switch_parses_flag_and_garbage_is_false() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.json");
    std::fs::write(&cfg, r#"{"memory": {"enabled": true}}"#).unwrap();
    assert!(read_main_switch(dir.path().to_str().unwrap()));

    // Garbage JSON / non-boolean values uniformly fall back to false.
    std::fs::write(&cfg, "not json").unwrap();
    assert!(!read_main_switch(dir.path().to_str().unwrap()));
    std::fs::write(&cfg, r#"{"memory": {"enabled": "yes"}}"#).unwrap();
    assert!(!read_main_switch(dir.path().to_str().unwrap()));
}

#[test]
fn set_main_switch_no_config_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = set_main_switch(dir.path().to_str().unwrap(), true).unwrap_err();
    assert!(err.contains("config.json not found"), "{err}");
}

#[test]
fn set_main_switch_creates_memory_node_and_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), r#"{"other": 1}"#).unwrap();
    set_main_switch(dir.path().to_str().unwrap(), true).unwrap();
    assert!(read_main_switch(dir.path().to_str().unwrap()));
    // The existing top-level field must not be lost.
    let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["other"], 1);

    // Flipping to false also takes effect.
    set_main_switch(dir.path().to_str().unwrap(), false).unwrap();
    assert!(!read_main_switch(dir.path().to_str().unwrap()));
}

#[test]
fn status_counts_documents_and_switches() {
    let h = MemoryHandler;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();

    // Empty workspace: all zeros/false, but memory dir missing does not count as error.
    let out = h.status(ws, ws).unwrap().unwrap();
    assert_eq!(out["document_memory"]["document_count"], 0);
    assert_eq!(out["document_memory"]["directory_exists"], false);
    assert_eq!(out["vector_memory"]["enabled"], false);
    assert_eq!(out["vector_memory"]["main_enabled"], false);

    // Documents present + enhanced config enabled=true + main switch on.
    std::fs::create_dir_all(dir.path().join("memory/sub")).unwrap();
    std::fs::write(dir.path().join("memory/a.md"), "x").unwrap();
    std::fs::write(dir.path().join("memory/sub/b.txt"), "y").unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.enhanced_memory.json"),
        r#"{"enabled": true}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"memory": {"enabled": true}}"#,
    )
    .unwrap();
    let out = h.status(ws, ws).unwrap().unwrap();
    assert_eq!(out["document_memory"]["document_count"], 2);
    assert_eq!(out["document_memory"]["directory_exists"], true);
    assert_eq!(out["vector_memory"]["enabled"], true);
    assert_eq!(out["vector_memory"]["main_enabled"], true);
}

#[test]
fn documents_empty_and_nested_listing() {
    let h = MemoryHandler;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();

    // No memory directory → empty list.
    let out = h.documents(ws).unwrap().unwrap();
    assert_eq!(out["documents"].as_array().unwrap().len(), 0);

    // Nested directories must recurse into the listing (relative path carries the subdirectory).
    std::fs::create_dir_all(dir.path().join("memory/sub")).unwrap();
    std::fs::write(dir.path().join("memory/top.md"), "top").unwrap();
    std::fs::write(dir.path().join("memory/sub/deep.md"), "deep").unwrap();
    let out = h.documents(ws).unwrap().unwrap();
    let docs = out["documents"].as_array().unwrap();
    assert_eq!(docs.len(), 2);
    let paths: Vec<&str> = docs.iter().map(|d| d["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"memory/top.md"));
    assert!(paths.contains(&"memory/sub/deep.md"));
}

#[test]
fn document_get_save_roundtrip_and_missing_file() {
    let h = MemoryHandler;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();

    let out = h
        .document_save(ws, "memory/notes.md", "hello")
        .unwrap()
        .unwrap();
    assert_eq!(out["saved"], true);
    assert_eq!(out["path"], "memory/notes.md");

    let got = h.document_get(ws, "memory/notes.md").unwrap().unwrap();
    assert_eq!(got["content"], "hello");

    // Non-existent file → Err (propagated from read_workspace_file).
    assert!(h.document_get(ws, "memory/nope.md").is_err());
}

#[test]
fn vector_status_reflects_embedding_config() {
    let h = MemoryHandler;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();

    // config missing → load default (enabled=false).
    let out = h.vector_status(ws).unwrap().unwrap();
    assert_eq!(out["enabled"], false);

    // enabled=true must be passed through as-is.
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.enhanced_memory.json"),
        r#"{"enabled": true}"#,
    )
    .unwrap();
    let out = h.vector_status(ws).unwrap().unwrap();
    assert_eq!(out["enabled"], true);
}

#[test]
fn env_check_local_model_paths_mark_ready() {
    // local_model_path / local_tokenizer_path pointing to actually-existing files
    // must set model_ready/tokenizer_ready to true (the local path takes priority
    // over the embedding data directory's model.onnx probe). In the test environment
    // the plugin DLL is always missing → overall is at most degraded, but per-model
    // readiness is completely determined by file existence.
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{"memory": {"enabled": true}}"#,
    )
    .unwrap();

    let model_file = dir.path().join("fake.onnx");
    let tok_file = dir.path().join("fake-tok.json");
    std::fs::write(&model_file, "m").unwrap();
    std::fs::write(&tok_file, "t").unwrap();
    let cfg = serde_json::json!({
        "enabled": true,
        "active": "small",
        "models": {
            "small": {
                "name": "mini",
                "dimension": 384,
                "local_model_path": model_file.to_string_lossy(),
                "local_tokenizer_path": tok_file.to_string_lossy()
            }
        }
    });
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config/config.enhanced_memory.json"),
        cfg.to_string(),
    )
    .unwrap();

    let h = MemoryHandler;
    let out = h
        .env_check(&std::path::PathBuf::from(ws).join("config"), ws)
        .unwrap()
        .unwrap();
    let small = &out["models"]["small"];
    assert_eq!(
        small["model_ready"], true,
        "local_model_path exists → ready"
    );
    assert_eq!(small["tokenizer_ready"], true);
    assert_eq!(small["name"], "mini");
    assert_eq!(small["dimension"], 384);
    assert_eq!(out["active_tier"], "small");
    assert_eq!(out["main_switch"], true);
    assert_eq!(out["sub_switch"], true);
    // No plugin DLL in the test environment → degraded (the ready arm structurally
    // depends on the plugin artifact existing on disk; see §9.4).
    assert_eq!(out["overall"], "degraded");

    // Absent local path and no model.onnx under the data directory → not ready.
    let cfg2 = serde_json::json!({
        "enabled": true,
        "active": "small",
        "models": { "small": { "name": "mini2", "dimension": 384 } }
    });
    std::fs::write(
        dir.path().join("config/config.enhanced_memory.json"),
        cfg2.to_string(),
    )
    .unwrap();
    let out = h
        .env_check(&std::path::PathBuf::from(ws).join("config"), ws)
        .unwrap()
        .unwrap();
    assert_eq!(out["models"]["small"]["model_ready"], false);
    assert_eq!(out["models"]["small"]["tokenizer_ready"], false);
    // active model not ready → still degraded.
    assert_eq!(out["overall"], "degraded");
}
