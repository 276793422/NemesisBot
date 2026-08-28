//! U15: model API-key credential store (`workspace/config/credentials.yaml`).
//!
//! A `config.json` model entry's `api_key` may be a REFERENCE instead of an
//! inline literal:
//! - `env:VAR_NAME` — resolved from the process environment (H2, earlier batch)
//! - `yaml:<alias>` — resolved from this file's `keys:` map (this module)
//!
//! Resolution order (`provider_resolver::resolve_api_key_value`):
//! `env:VAR` > `yaml:<alias>` > inline literal (legacy, unchanged).
//! References fail LOUD with the alias/var name and a remedy — never silently
//! degrade to an empty key (which would surface later as a confusing 401).
//!
//! The file is read PER RESOLUTION (no caching), mirroring the `env:` design:
//! editing credentials.yaml takes effect on the next resolve without restart.
//!
//! File shape:
//! ```yaml
//! keys:
//!   openai-main: sk-...
//!   zhipu-free: "..."
//! ```
//! Written with 0600 permissions on Unix (POSIX perms don't exist on Windows —
//! see `save_credentials_file`). Loading a file whose mode is looser than 0600
//! warns but does not fail.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::{ConfigError, Result};

/// Parsed contents of `credentials.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialsFile {
    /// alias -> API key value.
    pub keys: BTreeMap<String, String>,
}

/// Result of `run_import` — what the migration did, for CLI output and tests.
#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    /// Inline plaintext keys migrated into credentials.yaml:
    /// (model_name, alias used).
    pub migrated: Vec<(String, String)>,
    /// Entries skipped because api_key was already `env:`/`yaml:` reference.
    pub skipped_reference: usize,
    /// Entries skipped because api_key was empty.
    pub skipped_empty: usize,
    /// Aliases that already existed in credentials.yaml with the SAME value
    /// (config rewritten to reference them; file untouched — idempotent).
    pub reused: usize,
    /// Aliases that already existed with a DIFFERENT value; a `__N`-suffixed
    /// alias was used instead: (requested, used).
    pub conflicts: Vec<(String, String)>,
}

impl ImportReport {
    /// True when the import changed nothing (second run = idempotent no-op).
    pub fn is_noop(&self) -> bool {
        self.migrated.is_empty() && self.conflicts.is_empty()
    }
}

/// G4 (U15 dashboard badge): where a model entry's `api_key` resolves from —
/// the same three-layer prefix contract `provider_resolver::resolve_api_key_value`
/// applies (`env:` > `yaml:` > inline literal), but carrying ONLY the
/// reference name, never the key value: dashboards badge the source with
/// zero plaintext leakage.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KeySource {
    /// `"env"` | `"yaml"` | `"inline"` | `"none"`.
    pub kind: String,
    /// `env:` → the VAR name; `yaml:` → the alias; `inline`/`none` → empty.
    #[serde(rename = "ref")]
    pub reference: String,
}

/// Classify one raw `api_key` string into its source badge. Pure prefix
/// logic — mirrors the resolution order without touching any store.
pub fn classify_key_source(api_key: &str) -> KeySource {
    if let Some(var) = api_key.strip_prefix("env:") {
        KeySource { kind: "env".to_string(), reference: var.to_string() }
    } else if let Some(alias) = api_key.strip_prefix("yaml:") {
        KeySource { kind: "yaml".to_string(), reference: alias.to_string() }
    } else if api_key.is_empty() {
        KeySource { kind: "none".to_string(), reference: String::new() }
    } else {
        KeySource { kind: "inline".to_string(), reference: String::new() }
    }
}

/// Canonical credentials.yaml path for a NemesisBot home dir:
/// `<home>/workspace/config/credentials.yaml` (sits next to auth.json).
pub fn credentials_path_for_home(home: &Path) -> PathBuf {
    home.join("workspace").join("config").join("credentials.yaml")
}

// ----------------------------------------------------------------------------
// Global path (process-wide; set by the CLI/gateway entry points)
// ----------------------------------------------------------------------------

/// Process-global credentials.yaml location. Set once at process start by the
/// binary (`run_command`) and by `eval_worker`; library code never mutates it,
/// so tests that set it explicitly cannot race with unrelated config loads.
static GLOBAL_CREDENTIALS_PATH: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Point `yaml:<alias>` resolution at a credentials.yaml file.
pub fn set_global_credentials_path(path: PathBuf) {
    *GLOBAL_CREDENTIALS_PATH.write() = Some(path);
}

/// Reset the global path (test hygiene).
pub fn clear_global_credentials_path() {
    *GLOBAL_CREDENTIALS_PATH.write() = None;
}

/// Current global path, if set.
pub fn global_credentials_path() -> Option<PathBuf> {
    GLOBAL_CREDENTIALS_PATH.read().clone()
}

// ----------------------------------------------------------------------------
// File IO
// ----------------------------------------------------------------------------

/// Read and parse credentials.yaml.
///
/// Missing file is an ERROR (callers resolve a `yaml:` reference and must fail
/// loud with a remedy, not treat the store as empty). Non-0600 mode warns on
/// Unix but does not fail.
pub fn load_credentials_file(path: &Path) -> Result<CredentialsFile> {
    warn_if_permissions_loose(path);
    let content = std::fs::read_to_string(path).map_err(|e| {
        ConfigError::Validation(format!(
            "credentials file '{}' cannot be read ({}). If api_key references use 'yaml:<alias>', \
             create the file (shape: keys: {{ alias: value }}) or run `nemesisbot credentials import`",
            path.display(),
            e
        ))
    })?;
    if content.trim().is_empty() {
        return Ok(CredentialsFile::default());
    }
    serde_yaml::from_str(&content).map_err(|e| {
        ConfigError::Validation(format!(
            "credentials file '{}' is not valid YAML ({}) — expected shape: keys: {{ alias: value }}",
            path.display(),
            e
        ))
    })
}

/// Serialize credentials.yaml with 0600 permissions (Unix).
pub fn save_credentials_file(path: &Path, file: &CredentialsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            ConfigError::Validation(format!(
                "cannot create credentials dir '{}': {}",
                parent.display(),
                e
            ))
        })?;
    }
    let body = serde_yaml::to_string(file).map_err(|e| {
        ConfigError::Validation(format!("failed to serialize credentials.yaml: {}", e))
    })?;
    std::fs::write(path, body).map_err(|e| {
        ConfigError::Validation(format!(
            "cannot write credentials file '{}': {}",
            path.display(),
            e
        ))
    })?;
    restrict_permissions(path);
    Ok(())
}

/// Warn (not fail) when the file is readable beyond the owner — Unix only.
#[cfg(unix)]
fn warn_if_permissions_loose(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mode = meta.permissions().mode() & 0o777;
        if mode != 0o600 {
            tracing::warn!(
                "[Credentials] '{}' has mode {:o} (expected 0600) — tighten it: chmod 600 {}",
                path.display(),
                mode,
                path.display()
            );
        }
    }
}

/// POSIX permissions don't exist on Windows; the file inherits the directory
/// ACL (user profile is private by default). No-op there.
#[cfg(not(unix))]
fn warn_if_permissions_loose(_path: &Path) {}

/// Set owner-only permissions (0600) — Unix only.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        if let Err(e) = std::fs::set_permissions(path, perms) {
            tracing::warn!(
                "[Credentials] failed to chmod 600 '{}': {}",
                path.display(),
                e
            );
        }
    }
}

/// See the Unix twin above — no-op on Windows.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

// ----------------------------------------------------------------------------
// Reference resolution
// ----------------------------------------------------------------------------

/// Resolve a `yaml:<alias>` reference against the global credentials path.
/// Called from `provider_resolver::resolve_api_key_value` — every failure is
/// loud with the alias and a remedy.
pub(crate) fn resolve_yaml_reference(alias: &str, model_for_error: &str) -> Result<String> {
    if alias.is_empty() {
        return Err(ConfigError::Validation(format!(
            "model '{}': api_key uses an empty yaml: reference — name an alias (e.g. \"yaml:my_provider\") \
             or use a literal key / env:VAR in config.json",
            model_for_error
        )));
    }
    let Some(path) = global_credentials_path() else {
        return Err(ConfigError::Validation(format!(
            "model '{}': api_key uses 'yaml:{}' but no credentials.yaml location is configured in \
             this process — run through the nemesisbot CLI/gateway, or call \
             nemesis_config::credentials::set_global_credentials_path first",
            model_for_error, alias
        )));
    };
    let file = load_credentials_file(&path)?;
    match file.keys.get(alias) {
        Some(v) if !v.is_empty() => Ok(v.clone()),
        Some(_) => Err(ConfigError::Validation(format!(
            "model '{}': alias '{}' in credentials file '{}' is set but empty — set it to the key value",
            model_for_error, alias, path.display()
        ))),
        None => Err(ConfigError::Validation(format!(
            "model '{}': alias '{}' not found in credentials file '{}' — add it under `keys:` \
             or run `nemesisbot credentials import`",
            model_for_error, alias, path.display()
        ))),
    }
}

// ----------------------------------------------------------------------------
// `nemesisbot credentials import` core (library-level, testable)
// ----------------------------------------------------------------------------

/// Migrate inline plaintext api_keys from config.json's model_list into
/// credentials.yaml, rewriting them as `yaml:<alias>` references.
///
/// Idempotent: aliases already present in credentials.yaml with the same value
/// are reused (file untouched); with a DIFFERENT value the new key lands under
/// an `__N`-suffixed alias (nothing is ever overwritten). Entries already
/// referencing `env:`/`yaml:` and empty keys are skipped. When nothing needs
/// migrating, neither file is written (a missing config.json is a no-op, not
/// an accidental creation).
pub fn run_import(config_path: &Path, credentials_path: &Path) -> Result<ImportReport> {
    if !config_path.exists() {
        // Nothing to migrate; do NOT create a config.json as a side effect.
        return Ok(ImportReport::default());
    }
    let mut config = crate::load_config(config_path)?;

    let mut creds = if credentials_path.exists() {
        load_credentials_file(credentials_path)?
    } else {
        CredentialsFile::default()
    };

    let mut report = ImportReport::default();
    let mut config_changed = false;

    for mc in config.model_list.iter_mut() {
        if mc.api_key.is_empty() {
            report.skipped_empty += 1;
            continue;
        }
        if mc.api_key.starts_with("env:") || mc.api_key.starts_with("yaml:") {
            report.skipped_reference += 1;
            continue;
        }
        let base_alias = sanitize_alias(if mc.model_name.is_empty() {
            mc.model.as_str()
        } else {
            mc.model_name.as_str()
        });
        let literal = mc.api_key.clone();

        // Existing alias with same value → reuse. Different value → suffix.
        let alias = if let Some(existing) = creds.keys.get(&base_alias) {
            if existing == &literal {
                report.reused += 1;
                base_alias
            } else {
                let mut n = 2;
                let mut candidate = format!("{}__{}", base_alias, n);
                while let Some(v) = creds.keys.get(&candidate) {
                    if v == &literal {
                        break;
                    }
                    n += 1;
                    candidate = format!("{}__{}", base_alias, n);
                }
                if !creds.keys.contains_key(&candidate) {
                    creds.keys.insert(candidate.clone(), literal);
                }
                report.conflicts.push((base_alias.clone(), candidate.clone()));
                candidate
            }
        } else {
            creds.keys.insert(base_alias.clone(), literal);
            base_alias
        };

        mc.api_key = format!("yaml:{}", alias);
        let display_name = if mc.model_name.is_empty() {
            mc.model.clone()
        } else {
            mc.model_name.clone()
        };
        report.migrated.push((display_name, alias));
        config_changed = true;
    }

    if !report.is_noop() || config_changed {
        save_credentials_file(credentials_path, &creds)?;
    }
    if config_changed {
        crate::save_config(config_path, &mut config)?;
    }
    Ok(report)
}

/// Turn a model name/model string into a yaml-safe alias
/// (no `/`, `\`, `:`, whitespace).
fn sanitize_alias(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | ' ' | '\t' | '\n' | '\r' => '_',
            _ => c,
        })
        .collect();
    if cleaned.is_empty() {
        "model".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests;
