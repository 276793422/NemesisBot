//! W5d batch tests: filesystem round-trips, save/load error paths and
//! DefaultVal edge cases for FeatureManifest.

use super::*;
use std::fs;

#[test]
fn w5d_load_reads_real_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("features.toml");
    fs::write(
        &p,
        "[[feature]]\nid = \"cluster\"\ndefault = false\nlabel = \"集群\"\n",
    )
    .unwrap();
    let m = FeatureManifest::load(&p).unwrap();
    assert_eq!(m.features.len(), 1);
    assert_eq!(m.features[0].id, "cluster");
    assert_eq!(m.features[0].label, "集群");
}

#[test]
fn w5d_load_missing_file_is_err_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("missing.toml");
    let err = FeatureManifest::load(&p).unwrap_err().to_string();
    assert!(err.contains("read manifest"), "err was: {err}");
    assert!(err.contains("missing.toml"), "err was: {err}");
}

#[test]
fn w5d_load_malformed_file_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("features.toml");
    fs::write(&p, "[[feature]\nid = broken").unwrap();
    let err = FeatureManifest::load(&p).unwrap_err().to_string();
    assert!(err.contains("parse manifest"), "err was: {err}");
}

#[test]
fn w5d_save_writes_header_and_roundtrips_curated_fields() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("features.toml");
    let m = FeatureManifest::parse(
        r#"
[[feature]]
id = "cluster"
label = "集群编排"
desc = "P2P 集群"
category = "subsystems"
default = false
depends = ["channels-rpc"]
conflicts = ["forge"]

[[feature]]
id = "build-profile"
label = "构建 profile"
category = "build"
type = "enum"
default = "iotsmall"
options = ["release", "iotsmall"]
"#,
    )
    .unwrap();
    m.save(&p).unwrap();
    let text = fs::read_to_string(&p).unwrap();
    assert!(
        text.starts_with("# NemesisBot feature manifest"),
        "text was: {text}"
    );
    let back = FeatureManifest::load(&p).unwrap();
    assert_eq!(back.features.len(), 2);
    let cluster = back.features.iter().find(|f| f.id == "cluster").unwrap();
    assert_eq!(cluster.label, "集群编排");
    assert_eq!(cluster.desc, "P2P 集群");
    assert_eq!(cluster.category, "subsystems");
    assert_eq!(cluster.depends, vec!["channels-rpc".to_string()]);
    assert_eq!(cluster.conflicts, vec!["forge".to_string()]);
    assert_eq!(cluster.default.as_bool(), Some(false));
    let bp = back
        .features
        .iter()
        .find(|f| f.id == "build-profile")
        .unwrap();
    assert!(bp.is_enum());
    assert_eq!(
        bp.options,
        vec!["release".to_string(), "iotsmall".to_string()]
    );
    assert_eq!(bp.default.as_str(), Some("iotsmall"));
}

#[test]
fn w5d_save_to_unwritable_path_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, "file").unwrap();
    let target = blocker.join("features.toml");
    let err = FeatureManifest::default()
        .save(&target)
        .unwrap_err()
        .to_string();
    assert!(err.contains("write manifest"), "err was: {err}");
}

#[test]
fn w5d_all_ids_includes_conflicts_and_dedups() {
    let m = FeatureManifest::parse(
        r#"
[[feature]]
id = "a"
depends = ["shared", "b"]
conflicts = ["c"]
[[feature]]
id = "b"
depends = ["shared"]
"#,
    )
    .unwrap();
    let ids = m.all_ids();
    for expected in ["a", "b", "c", "shared"] {
        assert!(ids.contains(expected), "missing {expected} in {ids:?}");
    }
    // BTreeSet dedup: "shared" referenced twice appears once
    assert_eq!(ids.len(), 4);
}

#[test]
fn w5d_default_val_variant_accessors() {
    let b = DefaultVal::Bool(true);
    assert_eq!(b.as_bool(), Some(true));
    assert_eq!(b.as_str(), None);
    let s = DefaultVal::Str("release".to_string());
    assert_eq!(s.as_bool(), None);
    assert_eq!(s.as_str(), Some("release"));
    // Default::default() is Bool(false) — relied on by serde field default
    let d = DefaultVal::default();
    assert_eq!(d.as_bool(), Some(false));
}

#[test]
fn w5d_parse_empty_manifest() {
    let m = FeatureManifest::parse("").unwrap();
    assert!(m.features.is_empty());
    assert!(m.all_ids().is_empty());
}

#[test]
fn w5d_untagged_default_accepts_bool_and_string_forms() {
    let m = FeatureManifest::parse(
        r#"
[[feature]]
id = "bool-form"
default = true
[[feature]]
id = "str-form"
type = "enum"
default = "release"
options = ["release"]
"#,
    )
    .unwrap();
    assert_eq!(m.features[0].default.as_bool(), Some(true));
    assert_eq!(m.features[1].default.as_str(), Some("release"));
}

#[test]
fn w5d_optional_fields_default_empty() {
    let m = FeatureManifest::parse("[[feature]]\nid = \"bare\"\n").unwrap();
    let f = &m.features[0];
    assert_eq!(f.label, "");
    assert_eq!(f.desc, "");
    assert_eq!(f.category, "");
    assert!(!f.is_enum());
    assert!(f.options.is_empty());
    assert!(f.depends.is_empty());
    assert!(f.conflicts.is_empty());
}

#[test]
fn w5d_serialize_then_parse_preserves_feature_count_and_ids() {
    let mut m = FeatureManifest::default();
    for id in ["z-last", "a-first", "m-middle"] {
        m.features.push(FeatureSpec {
            id: id.to_string(),
            label: id.to_string(),
            desc: String::new(),
            category: "subsystems".to_string(),
            feature_type: None,
            default: DefaultVal::Bool(false),
            options: Vec::new(),
            depends: Vec::new(),
            conflicts: Vec::new(),
        });
    }
    let text = m.to_string().unwrap();
    let back = FeatureManifest::parse(&text).unwrap();
    assert_eq!(back.features.len(), 3);
    let ids: Vec<&str> = back.features.iter().map(|f| f.id.as_str()).collect();
    // declaration order preserved through TOML array-of-tables
    assert_eq!(ids, vec!["z-last", "a-first", "m-middle"]);
}
