use super::*;

const CARGO: &str = r#"
[package]
name = "nemesisbot"

[features]
default = ["channels-web", "channels-webhook", "channels-rpc", "migrate"]
channels-web = ["nemesis-channels/web"]
channels-rpc = ["nemesis-channels/rpc"]
channels-telegram = ["nemesis-channels/telegram"]
migrate = ["dep:nemesis-migrate"]
"#;

#[test]
fn extracts_feature_names() {
    let s = scan_text(CARGO).unwrap();
    let names = s.names();
    assert!(names.contains(&"channels-web".to_string()));
    assert!(names.contains(&"channels-telegram".to_string()));
    assert!(names.contains(&"migrate".to_string()));
    // "default" is excluded from names
    assert!(!names.contains(&"default".to_string()));
}

#[test]
fn detects_defaults() {
    let s = scan_text(CARGO).unwrap();
    assert!(s.is_default("channels-web"));
    assert!(s.is_default("migrate"));
    assert!(!s.is_default("channels-telegram"));
}

#[test]
fn handles_no_features_table() {
    let s = scan_text("[package]\nname = \"x\"\n").unwrap();
    assert!(s.names().is_empty());
    assert!(s.default_enabled().is_empty());
}
