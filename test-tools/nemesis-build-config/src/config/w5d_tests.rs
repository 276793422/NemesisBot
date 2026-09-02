//! W5d batch tests: filesystem round-trips and error paths for BuildConfig.

use super::*;
use std::fs;

fn write_temp(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".config");
    fs::write(&p, text).unwrap();
    (dir, p)
}

#[test]
fn w5d_load_reads_real_file() {
    let (_dir, p) =
        write_temp("[features]\nchannels-web = true\n\n[enums]\nbuild-profile = \"iotsmall\"\n");
    let cfg = BuildConfig::load(&p).unwrap();
    assert_eq!(cfg.get_bool("channels-web"), Some(true));
    assert_eq!(cfg.get_enum("build-profile"), Some("iotsmall"));
}

#[test]
fn w5d_load_missing_file_is_err_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("nope.config");
    let err = BuildConfig::load(&p).unwrap_err().to_string();
    assert!(err.contains("read config"), "err was: {err}");
    assert!(err.contains("nope.config"), "err was: {err}");
}

#[test]
fn w5d_load_malformed_file_is_parse_err() {
    let (_dir, p) = write_temp("this is [ not toml");
    let err = BuildConfig::load(&p).unwrap_err().to_string();
    assert!(err.contains("parse config"), "err was: {err}");
}

#[test]
fn w5d_save_writes_generated_header_comment() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".config");
    BuildConfig::default().save(&p).unwrap();
    let text = fs::read_to_string(&p).unwrap();
    assert!(text.starts_with("# NemesisBot build configuration"));
    assert!(
        text.contains("# Remove this file to return to the full default build."),
        "text was: {text}"
    );
}

#[test]
fn w5d_save_then_load_roundtrip_through_disk() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".config");
    let mut cfg = BuildConfig::default();
    cfg.set_bool("channels-web", true);
    cfg.set_bool("migrate", false);
    cfg.set_enum("build-profile", "iotsmall");
    cfg.save(&p).unwrap();
    let back = BuildConfig::load(&p).unwrap();
    assert_eq!(back.get_bool("channels-web"), Some(true));
    assert_eq!(back.get_bool("migrate"), Some(false));
    assert_eq!(back.get_enum("build-profile"), Some("iotsmall"));
    // off feature persists too (not just the enabled set)
    assert!(back.features.contains_key("migrate"));
}

#[test]
fn w5d_save_overwrites_previous_content() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(".config");
    let mut cfg = BuildConfig::default();
    cfg.set_bool("a", true);
    cfg.save(&p).unwrap();
    let mut cfg2 = BuildConfig::default();
    cfg2.set_bool("b", true);
    cfg2.save(&p).unwrap();
    let back = BuildConfig::load(&p).unwrap();
    assert_eq!(back.get_bool("b"), Some(true));
    assert_eq!(back.get_bool("a"), None, "stale entry must be overwritten");
}

#[test]
fn w5d_save_to_unwritable_path_is_err() {
    // parent "directory" is actually a regular file -> create/write fails
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "i am a file").unwrap();
    let target = blocker.join(".config");
    let err = BuildConfig::default()
        .save(&target)
        .unwrap_err()
        .to_string();
    assert!(err.contains("write config"), "err was: {err}");
}

#[test]
fn w5d_parse_empty_text_yields_default() {
    let cfg = BuildConfig::parse("").unwrap();
    assert!(cfg.features.is_empty());
    assert!(cfg.enums.is_empty());
    assert_eq!(cfg.get_bool("anything"), None);
    assert_eq!(cfg.get_enum("anything"), None);
}

#[test]
fn w5d_setters_overwrite_previous_value() {
    let mut cfg = BuildConfig::default();
    cfg.set_bool("x", true);
    cfg.set_bool("x", false);
    assert_eq!(cfg.get_bool("x"), Some(false));
    cfg.set_enum("build-profile", "release");
    cfg.set_enum("build-profile", "iotsmall");
    assert_eq!(cfg.get_enum("build-profile"), Some("iotsmall"));
}

#[test]
fn w5d_from_defaults_enum_with_bool_default_sets_nothing() {
    // an enum feature whose default is a bool (authoring mistake / edge):
    // neither an enum selection nor a bool toggle is derived from it
    let m = crate::manifest::FeatureManifest::parse(
        r#"
[[feature]]
id = "build-profile"
type = "enum"
default = true
options = ["release", "iotsmall"]
"#,
    )
    .unwrap();
    let cfg = BuildConfig::from_defaults(&m);
    assert_eq!(cfg.get_enum("build-profile"), None);
    assert_eq!(cfg.get_bool("build-profile"), None);
}

#[test]
fn w5d_from_defaults_missing_default_implies_false() {
    // serde default for DefaultVal is Bool(false)
    let m = crate::manifest::FeatureManifest::parse("[[feature]]\nid = \"x\"\n").unwrap();
    let cfg = BuildConfig::from_defaults(&m);
    assert_eq!(cfg.get_bool("x"), Some(false));
}

#[test]
fn w5d_from_defaults_covers_only_manifest_features() {
    let m = crate::manifest::FeatureManifest::parse(
        r#"
[[feature]]
id = "a"
default = true
"#,
    )
    .unwrap();
    let cfg = BuildConfig::from_defaults(&m);
    assert_eq!(cfg.get_bool("a"), Some(true));
    assert_eq!(cfg.get_bool("b"), None, "no entry for undeclared features");
    assert_eq!(cfg.features.len(), 1);
}
