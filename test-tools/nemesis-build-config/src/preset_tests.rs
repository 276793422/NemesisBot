//! Preset drift-guard tests: every preset under scripts/customize/profiles/
//! must explicitly cover every boolean feature in features.toml, and the
//! `desktop` preset must enable every default-on feature.
//!
//! Background (S1, 2026-09-23): desktop.config was written before
//! sandbox/eval/board/terminal/usage joined features.toml (all default=true)
//! and was never backported — a desktop build silently shipped without those
//! subsystems. These tests pin the invariant so a new manifest entry that
//! skips the presets fails here instead of silently missing from
//! preset-built binaries (omission = cargo feature off, not "inherit default").

use super::*;
use std::fs;

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .canonicalize()
        .expect("project root exists")
}

fn load_manifest() -> FeatureManifest {
    FeatureManifest::load(&project_root().join("scripts/customize/features.toml"))
        .expect("features.toml loads")
}

fn preset_names() -> Vec<String> {
    let dir = project_root().join("scripts/customize/profiles");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .expect("profiles dir exists")
        .map(|e| e.expect("dir entry readable").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("config"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Every preset must explicitly state every boolean feature — omission is a
/// silent "off" in preset-built binaries, so a new manifest entry must be
/// backported to all presets or this fails.
#[test]
fn presets_cover_every_bool_feature() {
    let manifest = load_manifest();
    let bool_ids: Vec<&str> = manifest
        .features
        .iter()
        .filter(|f| !f.is_enum())
        .map(|f| f.id.as_str())
        .collect();
    assert!(!bool_ids.is_empty(), "manifest must list bool features");

    for name in preset_names() {
        let path = project_root()
            .join("scripts/customize/profiles")
            .join(format!("{name}.config"));
        let cfg = BuildConfig::load(&path).unwrap_or_else(|e| panic!("preset {name} parses: {e}"));
        for id in &bool_ids {
            assert!(
                cfg.features.contains_key(*id),
                "preset `{name}` omits bool feature `{id}` — presets must be \
                 fully explicit (omission = silently off in the built binary)"
            );
        }
    }
}

/// `desktop` is the full-featured preset: every default-on bool feature must
/// be not just listed but enabled (S1: sandbox/eval/board/terminal/usage).
#[test]
fn desktop_preset_enables_every_default_on_feature() {
    let manifest = load_manifest();
    let path = project_root().join("scripts/customize/profiles/desktop.config");
    let cfg = BuildConfig::load(&path).expect("desktop preset parses");

    let missing: Vec<&str> = manifest
        .features
        .iter()
        .filter(|f| !f.is_enum())
        .filter(|f| f.default.as_bool() == Some(true))
        .map(|f| f.id.as_str())
        .filter(|id| cfg.get_bool(id) != Some(true))
        .collect();
    assert!(
        missing.is_empty(),
        "desktop preset must enable every default-on feature; missing/off: {missing:?}"
    );
}
