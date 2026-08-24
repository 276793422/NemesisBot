use super::*;

#[test]
fn parse_and_access() {
    let text = r#"
[features]
channels-web = true
migrate = false

[enums]
build-profile = "iotsmall"
"#;
    let cfg = BuildConfig::parse(text).unwrap();
    assert_eq!(cfg.get_bool("channels-web"), Some(true));
    assert_eq!(cfg.get_bool("migrate"), Some(false));
    assert_eq!(cfg.get_enum("build-profile"), Some("iotsmall"));
    assert_eq!(cfg.get_bool("nonexistent"), None);
}

#[test]
fn toggle_and_serialize_roundtrip() {
    let mut cfg = BuildConfig::default();
    cfg.set_bool("channels-web", true);
    cfg.set_bool("channels-rpc", true);
    cfg.set_enum("build-profile", "release");
    let text = toml::to_string_pretty(&cfg).unwrap();
    let cfg2 = BuildConfig::parse(&text).unwrap();
    assert_eq!(cfg2.get_bool("channels-web"), Some(true));
    assert_eq!(cfg2.get_bool("channels-rpc"), Some(true));
    assert_eq!(cfg2.get_enum("build-profile"), Some("release"));
}

#[test]
fn from_defaults_uses_manifest_defaults() {
    let m = crate::manifest::FeatureManifest::parse(
        r#"
[[feature]]
id = "a"
default = true
[[feature]]
id = "b"
default = false
[[feature]]
id = "build-profile"
type = "enum"
default = "iotsmall"
"#,
    )
    .unwrap();
    let cfg = BuildConfig::from_defaults(&m);
    assert_eq!(cfg.get_bool("a"), Some(true));
    assert_eq!(cfg.get_bool("b"), Some(false));
    assert_eq!(cfg.get_enum("build-profile"), Some("iotsmall"));
}
