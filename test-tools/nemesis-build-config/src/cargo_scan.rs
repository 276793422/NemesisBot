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
mod tests;
