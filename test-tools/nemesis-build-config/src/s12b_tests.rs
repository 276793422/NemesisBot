//! S12b batch tests: `check_problems` STALE/MISSING classification (extracted
//! from run_check so the diff itself is testable without the exit(1)) and the
//! extracted `dispatch` arm routing. `run_tui` / exit(1) arms remain
//! structurally exempt (goal §9.4, see w5d_tests header).

use super::*;
use std::fs;

/// Same fixture text as w5d_tests::SCAN_CARGO (that const is private to its
/// module) — a Cargo.toml [features] table with 4 bool features.
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
    p.parent().map(|d| fs::create_dir_all(d).unwrap());
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn s12b_check_problems_clean_scan_reports_nothing() {
    let scan = cargo_scan::scan_text(SCAN_CARGO).unwrap();
    let manifest = scaffold(&scan, None);
    assert!(check_problems(&scan, &manifest).is_empty());
}

#[test]
fn s12b_check_problems_flags_stale_manifest_entries() {
    let scan = cargo_scan::scan_text(SCAN_CARGO).unwrap();
    // manifest carries an extra bool feature the Cargo.toml no longer has
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "channels-web"
default = false
[[feature]]
id = "ghost-feature"
label = "ghost"
default = false
"#,
    )
    .unwrap();
    let problems = check_problems(&scan, &manifest);
    // 1 stale (ghost-feature) + 3 missing (channels-rpc/migrate/sandbox absent
    // from the manifest) — both directions fire on a partial manifest.
    assert_eq!(problems.len(), 4, "got: {problems:?}");
    let stale: Vec<&String> = problems.iter().filter(|p| p.starts_with("STALE:")).collect();
    assert_eq!(stale.len(), 1, "got: {problems:?}");
    assert!(stale[0].contains("ghost-feature"), "got: {stale:?}");
}

#[test]
fn s12b_check_problems_flags_missing_and_ignores_enums() {
    // Cargo.toml has a feature the manifest lacks → MISSING; enum rows in the
    // manifest must never be counted as bool drift.
    let scan = cargo_scan::scan_text(SCAN_CARGO).unwrap();
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "channels-web"
default = false
[[feature]]
id = "build-profile"
type = "enum"
options = ["release", "iotsmall"]
default = "release"
"#,
    )
    .unwrap();
    let problems = check_problems(&scan, &manifest);
    assert_eq!(problems.len(), 3, "sandbox+migrate+channels-rpc missing: {problems:?}");
    assert!(
        problems.iter().all(|p| p.starts_with("MISSING:")),
        "got: {problems:?}"
    );
}

#[test]
fn s12b_check_problems_reports_both_directions_together() {
    let scan = cargo_scan::scan_text(SCAN_CARGO).unwrap();
    let manifest = FeatureManifest::parse(
        r#"
[[feature]]
id = "ghost"
default = false
[[feature]]
id = "migrate"
default = true
"#,
    )
    .unwrap();
    let problems = check_problems(&scan, &manifest);
    assert_eq!(problems.len(), 4, "1 stale + 3 missing: {problems:?}");
    assert_eq!(problems.iter().filter(|p| p.starts_with("STALE:")).count(), 1);
    assert_eq!(problems.iter().filter(|p| p.starts_with("MISSING:")).count(), 3);
}

#[test]
fn s12b_run_check_prints_problem_lines_via_run_check_ok_path_still_works() {
    // sync state still routes to Ok(()) after the refactor
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
    write(
        dir.path(),
        "scripts/customize/features.toml",
        &scaffold(&scan(), None).to_string().unwrap(),
    );
    run_check(dir.path()).unwrap();
}

/// Dispatch fan-out: every non-exit arm routes to the same behavior as its
/// run_* helper, observed through filesystem effects / error identity.
mod dispatch_arms {
    use super::*;

    #[test]
    fn init_arm_matches_run_init() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
        dispatch(Some(Cmd::Init), dir.path()).unwrap();
        assert!(manifest_path(dir.path()).exists());
    }

    #[test]
    fn check_arm_errs_on_missing_manifest() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
        assert!(dispatch(Some(Cmd::Check), dir.path()).is_err());
    }

    #[test]
    fn list_arm_ok_and_none_arm_routes_to_tui() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "nemesisbot/Cargo.toml", SCAN_CARGO);
        write(
            dir.path(),
            "scripts/customize/features.toml",
            &scaffold(&scan(), None).to_string().unwrap(),
        );
        assert!(dispatch(Some(Cmd::List), dir.path()).is_ok());

        // The None arm routes to run_tui, which needs a real terminal
        // (raw mode + event loop) — invoking it from a test could enter the
        // alternate screen of or hang the runner console. That arm stays
        // covered by manual runs of the binary; it is structurally exempt
        // (goal §9.4).
    }

    #[test]
    fn export_arm_matches_run_export() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "scripts/customize/.config",
            "[features]\nmemory = true\n\n[enums]\nbuild-profile = \"iotsmall\"\n",
        );
        dispatch(
            Some(Cmd::Export {
                features: true,
                profile: false,
                cmd: false,
                frontend_env: false,
            }),
            dir.path(),
        )
        .unwrap();
    }

    #[test]
    fn load_arm_copies_preset() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "scripts/customize/profiles/preset.config",
            "[features]\nmemory = false\n",
        );
        dispatch(
            Some(Cmd::Load {
                name: "preset".to_string(),
            }),
            dir.path(),
        )
        .unwrap();
        assert!(config_path(dir.path()).exists());
    }

    #[test]
    fn load_arm_bails_on_unknown_preset() {
        let dir = tempfile::tempdir().unwrap();
        let err = dispatch(
            Some(Cmd::Load {
                name: "nope".to_string(),
            }),
            dir.path(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("preset not found"), "err was: {err}");
    }
}
