//! W5d batch tests: bin-target logic — scaffold() merge semantics, the run_*
//! command paths against a temp project root, and path helpers.
//!
//! Structurally exempt (goal §9.4): `run_tui` (launches the real full-screen
//! TUI), `run_has_config`'s missing-file path and `run_check`'s out-of-sync
//! path (both call `std::process::exit` — untestable in-process).

use super::*;
use std::fs;

const SCAN_CARGO: &str = r#"
[package]
name = "nemesisbot"

[features]
default = ["channels-web", "migrate"]
channels-web = ["nemesis-channels/web"]
channels-rpc = ["nemesis-channels/rpc"]
migrate = ["dep:nemesis-migrate"]
sandbox = ["dep:nemesis-sandbox"]
"#;

fn scan() -> cargo_scan::ScanResult {
    cargo_scan::scan_text(SCAN_CARGO).unwrap()
}

fn write(root: &std::path::Path, rel: &str, text: &str) -> std::path::PathBuf {
    let p = root.join(rel);
    if let Some(d) = p.parent() { fs::create_dir_all(d).unwrap() }
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn w5d_category_for_matrix() {
    assert_eq!(category_for("channels-web"), "channels");
    assert_eq!(category_for("channels-"), "channels");
    assert_eq!(category_for("build-profile"), "build");
    assert_eq!(category_for("migrate"), "subsystems");
    assert_eq!(category_for("forge"), "subsystems");
}

#[test]
fn w5d_scaffold_from_scratch_uses_scan_reality() {
    let s = scan();
    let m = scaffold(&s, None);
    // every non-default cargo feature appears, plus the build-profile enum
    let ids: Vec<&str> = m.features.iter().map(|f| f.id.as_str()).collect();
    for expected in ["channels-web", "channels-rpc", "migrate", "sandbox", "build-profile"] {
        assert!(ids.contains(&expected), "missing {expected} in {ids:?}");
    }
    assert_eq!(m.features.len(), 5);
    // defaults refreshed from the cargo default list
    let web = m.features.iter().find(|f| f.id == "channels-web").unwrap();
    assert_eq!(web.default.as_bool(), Some(true));
    assert_eq!(web.category, "channels");
    assert_eq!(web.label, "channels-web");
    let rpc = m.features.iter().find(|f| f.id == "channels-rpc").unwrap();
    assert_eq!(rpc.default.as_bool(), Some(false));
    assert_eq!(rpc.category, "channels");
}

#[test]
fn w5d_scaffold_appends_builtin_build_profile_enum() {
    let m = scaffold(&scan(), None);
    let bp = m.features.iter().find(|f| f.id == "build-profile").unwrap();
    assert!(bp.is_enum());
    assert_eq!(bp.default.as_str(), Some("release"));
    assert_eq!(bp.category, "build");
    assert_eq!(bp.options, vec!["release".to_string(), "iotsmall".to_string()]);
}

#[test]
fn w5d_scaffold_keeps_curated_metadata_but_refreshes_default() {
    let existing = FeatureManifest::parse(
        r#"
[[feature]]
id = "sandbox"
label = "沙盒隔离"
desc = "Sandboxie 集成"
category = "subsystems"
default = true
depends = ["security"]
"#,
    )
    .unwrap();
    let m = scaffold(&scan(), Some(&existing));
    let sb = m.features.iter().find(|f| f.id == "sandbox").unwrap();
    // curated fields survive a refresh...
    assert_eq!(sb.label, "沙盒隔离");
    assert_eq!(sb.desc, "Sandboxie 集成");
    assert_eq!(sb.category, "subsystems");
    assert_eq!(sb.depends, vec!["security".to_string()]);
    // ...but the default is re-derived from the Cargo.toml scan (sandbox is
    // NOT in default = [...] => false, overriding the stale manifest default)
    assert_eq!(sb.default.as_bool(), Some(false));
}

#[test]
fn w5d_scaffold_drops_stale_manifest_features() {
    // a manifest entry that no longer exists in Cargo.toml disappears
    let existing = FeatureManifest::parse(
        r#"
[[feature]]
id = "removed-long-ago"
label = "ghost"
default = false
"#,
    )
    .unwrap();
    let m = scaffold(&scan(), Some(&existing));
    assert!(
        !m.features.iter().any(|f| f.id == "removed-long-ago"),
        "stale feature must not survive re-init"
    );
}

#[test]
fn w5d_scaffold_preserves_curated_build_profile_when_scan_lacks_it() {
    let existing = FeatureManifest::parse(
        r#"
[[feature]]
id = "build-profile"
label = "定制 profile"
category = "build"
type = "enum"
default = "iotsmall"
options = ["release", "iotsmall"]
"#,
    )
    .unwrap();
    let m = scaffold(&scan(), Some(&existing));
    // ensure-block finds it missing from scan names and re-adds the curated one
    let bps: Vec<&FeatureSpec> = m.features.iter().filter(|f| f.id == "build-profile").collect();
    assert_eq!(bps.len(), 1, "build-profile must appear exactly once");
    assert_eq!(bps[0].label, "定制 profile");
    assert_eq!(bps[0].default.as_str(), Some("iotsmall"));
}

#[test]
fn w5d_path_helpers_join_under_root() {
    let root = std::path::Path::new("/proj");
    assert_eq!(manifest_path(root), root.join("scripts/customize/features.toml"));
    assert_eq!(config_path(root), root.join("scripts/customize/.config"));
    assert_eq!(profiles_dir(root), root.join("scripts/customize/profiles"));
    assert_eq!(nemesisbot_cargo(root), root.join("nemesisbot/Cargo.toml"));
}

#[test]
fn w5d_load_config_or_default_prefers_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "scripts/customize/.config",
        "[features]\nchannels-rpc = true\n\n[enums]\nbuild-profile = \"iotsmall\"\n",
    );
    let manifest = FeatureManifest::parse(
        "[[feature]]\nid = \"channels-rpc\"\ndefault = false\n",
    )
    .unwrap();
    let cfg = load_config_or_default(dir.path(), &manifest).unwrap();
    // file value wins over the manifest default (false)
    assert_eq!(cfg.get_bool("channels-rpc"), Some(true));
    assert_eq!(cfg.get_enum("build-profile"), Some("iotsmall"));
}

#[test]
fn w5d_load_config_or_default_falls_back_to_manifest_defaults() {
    let dir = tempfile::tempdir().unwrap();
    // no .config anywhere
    let manifest = FeatureManifest::parse(
        "[[feature]]\nid = \"channels-rpc\"\ndefault = true\n",
    )
    .unwrap();
    let cfg = load_config_or_default(dir.path(), &manifest).unwrap();
    assert_eq!(cfg.get_bool("channels-rpc"), Some(true));
}

#[test]
fn w5d_run_load_copies_preset_and_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "scripts/customize/profiles/minimal-iot.config",
        "[features]\nmemory = false\n",
    );
    run_load(dir.path(), "minimal-iot").unwrap();
    let copied = fs::read_to_string(config_path(dir.path())).unwrap();
    assert_eq!(copied, "[features]\nmemory = false\n");
}

#[test]
fn w5d_run_load_unknown_preset_bails_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = run_load(dir.path(), "does-not-exist").unwrap_err().to_string();
    assert!(err.contains("preset not found"), "err was: {err}");
    assert!(err.contains("does-not-exist.config"), "err was: {err}");
}

#[test]
fn w5d_run_init_scaffolds_manifest_from_cargo_toml() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    run_init(dir.path()).unwrap();
    let mpath = manifest_path(dir.path());
    assert!(mpath.exists(), "manifest must be written");
    let m = FeatureManifest::load(&mpath).unwrap();
    let ids: Vec<&str> = m.features.iter().map(|f| f.id.as_str()).collect();
    for expected in ["channels-web", "channels-rpc", "migrate", "sandbox", "build-profile"] {
        assert!(ids.contains(&expected), "missing {expected}");
    }
}

#[test]
fn w5d_run_init_merges_existing_curated_manifest() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    write(
        dir.path(),
        "scripts/customize/features.toml",
        "[[feature]]\nid = \"sandbox\"\nlabel = \"沙盒\"\ncategory = \"subsystems\"\ndefault = true\n",
    );
    run_init(dir.path()).unwrap();
    let m = FeatureManifest::load(&manifest_path(dir.path())).unwrap();
    let sb = m.features.iter().find(|f| f.id == "sandbox").unwrap();
    assert_eq!(sb.label, "沙盒", "curated label must survive re-init");
    assert_eq!(sb.default.as_bool(), Some(false), "default refreshed to scan truth");
}

#[test]
fn w5d_run_check_in_sync_ok() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    // scaffold the exact matching manifest (what init produces)
    write(
        dir.path(),
        "scripts/customize/features.toml",
        &scaffold(&scan(), None).to_string().unwrap(),
    );
    run_check(dir.path()).unwrap();
}

#[test]
fn w5d_run_export_bails_without_config() {
    let dir = tempfile::tempdir().unwrap();
    let err = run_export(dir.path(), true, false, false, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("no .config"), "err was: {err}");
}

#[test]
fn w5d_run_export_flags_and_frontend_env_ok() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    write(
        dir.path(),
        "scripts/customize/features.toml",
        &scaffold(&scan(), None).to_string().unwrap(),
    );
    write(
        dir.path(),
        "scripts/customize/.config",
        "[features]\nchannels-web = true\n\n[enums]\nbuild-profile = \"iotsmall\"\n",
    );
    let root = dir.path();
    run_export(root, true, false, false, false).unwrap();
    run_export(root, false, true, false, false).unwrap();
    run_export(root, false, false, true, false).unwrap();
    run_export(root, false, false, false, true).unwrap();
    run_export(root, false, false, false, false).unwrap();
}

#[test]
fn w5d_run_list_ok_with_and_without_problems() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    write(
        dir.path(),
        "scripts/customize/features.toml",
        &scaffold(&scan(), None).to_string().unwrap(),
    );
    // clean selection
    write(
        dir.path(),
        "scripts/customize/.config",
        "[features]\nchannels-web = true\n",
    );
    run_list(dir.path()).unwrap();
    // selection with a validation problem (unknown feature) still just
    // reports — list is informational, not a gate
    write(
        dir.path(),
        "scripts/customize/.config",
        "[features]\nghost = true\n",
    );
    run_list(dir.path()).unwrap();
    // and with no .config at all (defaults path)
    fs::remove_file(config_path(dir.path())).unwrap();
    run_list(dir.path()).unwrap();
}

#[test]
fn w5d_run_has_config_ok_when_present() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "scripts/customize/.config", "[features]\n");
    run_has_config(dir.path()).unwrap();
}

#[test]
fn w5d_run_check_reports_via_result_on_missing_pieces() {
    // missing manifest => Err (not exit) — the ? propagation path
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    assert!(run_check(dir.path()).is_err());
}

#[test]
fn w5d_run_list_missing_manifest_is_err() {
    let dir = tempfile::tempdir().unwrap();
    assert!(run_list(dir.path()).is_err());
}

#[test]
fn w5d_run_init_missing_cargo_toml_is_err_with_context() {
    let dir = tempfile::tempdir().unwrap();
    let err = run_init(dir.path()).unwrap_err().to_string();
    assert!(
        err.contains("scanning nemesisbot/Cargo.toml"),
        "err was: {err}"
    );
}
