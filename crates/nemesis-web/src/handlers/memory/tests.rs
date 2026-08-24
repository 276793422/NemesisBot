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
    assert!(target.is_file(), "target {} must exist after migration", target.display());
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
    assert!(!target.exists(), "noop must not create {}", target.display());
}
