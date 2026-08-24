use super::*;

const SAMPLE: &str = r#"
[[feature]]
id = "channels-web"
label = "Web 通道"
desc = "内置 Web 界面"
category = "channels"
default = true

[[feature]]
id = "migrate"
label = "OpenClaw 迁移"
category = "subsystems"
default = true

[[feature]]
id = "build-profile"
label = "构建 profile"
category = "build"
type = "enum"
default = "release"
options = ["release", "iotsmall"]
"#;

#[test]
fn parses_sample_manifest() {
    let m = FeatureManifest::parse(SAMPLE).unwrap();
    assert_eq!(m.features.len(), 3);
    assert_eq!(m.features[0].id, "channels-web");
    assert_eq!(m.features[0].default.as_bool(), Some(true));
    assert!(!m.features[0].is_enum());
}

#[test]
fn enum_feature_detected() {
    let m = FeatureManifest::parse(SAMPLE).unwrap();
    let bf = m.features.iter().find(|f| f.id == "build-profile").unwrap();
    assert!(bf.is_enum());
    assert_eq!(bf.default.as_str(), Some("release"));
    assert_eq!(
        bf.options,
        vec!["release".to_string(), "iotsmall".to_string()]
    );
}

#[test]
fn roundtrip_preserves_features() {
    let m = FeatureManifest::parse(SAMPLE).unwrap();
    let text = m.to_string().unwrap();
    let m2 = FeatureManifest::parse(&text).unwrap();
    assert_eq!(m.features.len(), m2.features.len());
    assert_eq!(m2.features[0].id, "channels-web");
}

#[test]
fn all_ids_collects_dependencies() {
    let text = r#"
[[feature]]
id = "a"
default = true
depends = ["b"]
[[feature]]
id = "b"
default = false
"#;
    let m = FeatureManifest::parse(text).unwrap();
    let ids = m.all_ids();
    assert!(ids.contains("a"));
    assert!(ids.contains("b"));
}
