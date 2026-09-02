//! Tests for the embedded SDK artifacts (P2-2).
//!
//! Structural round-trip via `zip` (dev-dep): open the embedded bytes as a
//! real archive and assert content shape — this is what guarantees a user's
//! `unzip`/`pip install` works, not just that non-empty bytes exist.

use super::*;
use std::io::Read;
use std::path::Path;

fn open(data: &[u8]) -> zip::ZipArchive<std::io::Cursor<&[u8]>> {
    zip::ZipArchive::new(std::io::Cursor::new(data)).expect("embedded bytes must be a valid zip")
}

fn entry_names(ar: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> Vec<String> {
    (0..ar.len())
        .map(|i| ar.by_index(i).expect("entry readable").name().to_string())
        .collect()
}

#[test]
fn export_zip_contains_source_tree_at_root() {
    let mut ar = open(SDK_EXPORT_ZIP);
    let names = entry_names(&mut ar);
    assert!(
        names.iter().any(|n| n == "pyproject.toml"),
        "pyproject.toml at root, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "nemesisbot/client.py"),
        "package file present, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "README.md"),
        "README present, got: {names:?}"
    );
}

#[test]
fn export_zip_excludes_build_junk() {
    let mut ar = open(SDK_EXPORT_ZIP);
    let names = entry_names(&mut ar);
    for n in &names {
        let lower = n.to_ascii_lowercase();
        assert!(!lower.contains("build/"), "build/ leaked: {n}");
        assert!(!lower.contains(".egg-info"), "egg-info leaked: {n}");
        assert!(!lower.contains("__pycache__"), "pycache leaked: {n}");
        assert!(!lower.ends_with(".pyc"), "pyc leaked: {n}");
    }
}

#[test]
fn export_zip_file_bytes_roundtrip() {
    // The pyproject content must be byte-identical to the source file so the
    // pip build uses the real packaging config (not a stale copy).
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(manifest.join("../../test-tools/python/sdk/pyproject.toml"))
        .expect("source pyproject.toml");
    let mut ar = open(SDK_EXPORT_ZIP);
    let mut embedded = String::new();
    ar.by_name("pyproject.toml")
        .expect("pyproject.toml entry")
        .read_to_string(&mut embedded)
        .expect("decompress entry");
    assert_eq!(embedded, src, "embedded pyproject must match source");
}

#[test]
fn sdist_zip_has_single_top_level_dir_with_version() {
    let mut ar = open(SDK_SDIST_ZIP);
    let expected_prefix = format!("nemesisbot-{SDK_VERSION}/");
    let mut saw_pyproject = false;
    for i in 0..ar.len() {
        let name = ar.by_index(i).unwrap().name().to_string();
        assert!(
            name.starts_with(&expected_prefix),
            "entry outside {expected_prefix}: {name}"
        );
        if name == format!("{expected_prefix}pyproject.toml") {
            saw_pyproject = true;
        }
    }
    assert!(saw_pyproject, "pyproject.toml inside sdist top dir");
}

#[test]
fn sdk_version_is_populated() {
    // "0.0.0" is the parse fallback — a real build must have found a version.
    assert_ne!(SDK_VERSION, "0.0.0", "version parsed from pyproject.toml");
    assert!(
        SDK_VERSION
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.'),
        "version is x.y.z shaped: {SDK_VERSION}"
    );
}
