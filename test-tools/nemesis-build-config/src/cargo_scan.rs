//! Read the real `[features]` table out of a Cargo.toml. Used by `init` (to
//! scaffold a manifest that matches reality) and `check` (to detect drift).

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// Minimal Cargo.toml projection — only what we need: the `[features]` table.
#[derive(Debug, Deserialize)]
struct CargoManifest {
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
}

/// Scan result: every declared feature name, plus the set the `default` feature
/// enables (i.e. what an uncustomized build turns on).
#[derive(Debug)]
pub struct ScanResult {
    pub features: BTreeMap<String, Vec<String>>,
}

impl ScanResult {
    /// All declared feature names except `default` itself.
    pub fn names(&self) -> Vec<String> {
        self.features
            .keys()
            .filter(|k| k.as_str() != "default")
            .cloned()
            .collect()
    }

    /// Names that `default = [...]` enables. Empty if there is no default.
    pub fn default_enabled(&self) -> Vec<String> {
        self.features.get("default").cloned().unwrap_or_default()
    }

    /// Is `name` turned on by default?
    pub fn is_default(&self, name: &str) -> bool {
        self.default_enabled().iter().any(|d| d == name)
    }
}

/// Parse a Cargo.toml's text and extract its `[features]`.
pub fn scan_text(text: &str) -> Result<ScanResult, toml::de::Error> {
    let m: CargoManifest = toml::from_str(text)?;
    Ok(ScanResult {
        features: m.features,
    })
}

/// Read a Cargo.toml file and extract its `[features]`.
pub fn scan_file(path: &Path) -> anyhow::Result<ScanResult> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("read {}: {}", path.display(), e))?;
    scan_text(&text).map_err(|e| anyhow::anyhow!("parse {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
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
}
