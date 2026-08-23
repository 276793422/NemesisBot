//! Tests for U15 credentials.yaml (`yaml:<alias>` references + import).

use super::*;
use crate::{Config, ModelConfig};
use std::path::PathBuf;

/// Hold the crate global-state lock while the credentials GLOBAL path is set,
/// and clear it on drop (env-test discipline; see lib.rs GLOBAL_STATE_LOCK).
struct GlobalPathGuard;

impl GlobalPathGuard {
    fn set(path: impl AsRef<Path>) -> Self {
        set_global_credentials_path(path.as_ref().to_path_buf());
        GlobalPathGuard
    }
}

impl Drop for GlobalPathGuard {
    fn drop(&mut self) {
        clear_global_credentials_path();
    }
}

fn model_config_with_key(api_key: &str) -> Config {
    Config {
        model_list: vec![ModelConfig {
            model_name: "ref".to_string(),
            model: "openai/gpt-4".to_string(),
            api_key: api_key.to_string(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Reference resolution (env > yaml > literal)
// ---------------------------------------------------------------------------

#[test]
fn test_yaml_reference_resolves() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cred_path = dir.path().join("credentials.yaml");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("main".to_string(), "secret-from-yaml".to_string())].into(),
        },
    )
    .unwrap();
    let _g = GlobalPathGuard::set(cred_path);

    let cfg = model_config_with_key("yaml:main");
    let res = crate::resolve_model_config(&cfg, "ref").unwrap();
    assert_eq!(res.api_key, "secret-from-yaml");
}

#[test]
fn test_yaml_reference_reads_file_per_resolve() {
    // Editing credentials.yaml takes effect on the NEXT resolve (no caching).
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cred_path = dir.path().join("credentials.yaml");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("main".to_string(), "first".to_string())].into(),
        },
    )
    .unwrap();
    let _g = GlobalPathGuard::set(&cred_path);

    let cfg = model_config_with_key("yaml:main");
    assert_eq!(
        crate::resolve_model_config(&cfg, "ref").unwrap().api_key,
        "first"
    );
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("main".to_string(), "second".to_string())].into(),
        },
    )
    .unwrap();
    assert_eq!(
        crate::resolve_model_config(&cfg, "ref").unwrap().api_key,
        "second"
    );
}

#[test]
fn test_yaml_alias_missing_fails_loud_with_remedy() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cred_path = dir.path().join("credentials.yaml");
    save_credentials_file(&cred_path, &CredentialsFile::default()).unwrap();
    let _g = GlobalPathGuard::set(cred_path);

    let cfg = model_config_with_key("yaml:nope");
    let err = format!("{:?}", crate::resolve_model_config(&cfg, "ref").unwrap_err());
    assert!(err.contains("nope"), "error names the alias: {err}");
    assert!(err.contains("credentials import"), "error carries remedy: {err}");
}

#[test]
fn test_yaml_file_missing_fails_loud() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _g = GlobalPathGuard::set(dir.path().join("absent.yaml"));

    let cfg = model_config_with_key("yaml:main");
    let err = format!("{:?}", crate::resolve_model_config(&cfg, "ref").unwrap_err());
    assert!(err.contains("absent.yaml"), "error names the file: {err}");
    assert!(err.contains("credentials import"), "error carries remedy: {err}");
}

#[test]
fn test_yaml_path_unset_fails_loud() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    clear_global_credentials_path();
    let cfg = model_config_with_key("yaml:main");
    let err = format!("{:?}", crate::resolve_model_config(&cfg, "ref").unwrap_err());
    assert!(
        err.contains("credentials.yaml location"),
        "error explains the unset global: {err}"
    );
}

#[test]
fn test_yaml_empty_alias_fails_loud() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _g = GlobalPathGuard::set(dir.path().join("credentials.yaml"));

    let cfg = model_config_with_key("yaml:");
    assert!(crate::resolve_model_config(&cfg, "ref").is_err());
}

#[test]
fn test_env_reference_wins_when_prefixed_env() {
    // Precedence is structural: an `env:`-prefixed value resolves via the
    // environment even when the same-named alias exists in credentials.yaml.
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cred_path = dir.path().join("credentials.yaml");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("MAIN".to_string(), "from-yaml".to_string())].into(),
        },
    )
    .unwrap();
    let _g = GlobalPathGuard::set(cred_path);

    let var = format!("NEMESIS_TEST_KEY_YAML_PREC_{}", std::process::id());
    // SAFETY: GLOBAL_STATE_LOCK held, unique var name.
    unsafe { std::env::set_var(&var, "from-env") };
    let cfg = model_config_with_key(&format!("env:{}", var));
    assert_eq!(
        crate::resolve_model_config(&cfg, "ref").unwrap().api_key,
        "from-env"
    );
    // SAFETY: same lock held, unique var.
    unsafe { std::env::remove_var(&var) };
}

// ---------------------------------------------------------------------------
// File IO
// ---------------------------------------------------------------------------

#[test]
fn test_credentials_save_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.yaml");
    let file = CredentialsFile {
        keys: [
            ("openai-main".to_string(), "sk-aaa".to_string()),
            ("zhipu_free".to_string(), "bbb.ccc".to_string()),
        ]
        .into(),
    };
    save_credentials_file(&path, &file).unwrap();
    let loaded = load_credentials_file(&path).unwrap();
    assert_eq!(loaded.keys.get("openai-main").unwrap(), "sk-aaa");
    assert_eq!(loaded.keys.get("zhipu_free").unwrap(), "bbb.ccc");

    // Shape: top-level `keys:` map.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("keys:"), "yaml shape `keys:` first: {text}");
}

#[test]
fn test_credentials_file_mode_0600() {
    // POSIX perms only exist on Unix; on Windows the file inherits the
    // user-profile ACL (see restrict_permissions no-op twin).
    #[cfg(unix)]
    {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.yaml");
        save_credentials_file(&path, &CredentialsFile::default()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials.yaml must be 0600");
    }
}

#[test]
fn test_load_credentials_missing_file_is_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = load_credentials_file(&dir.path().join("nope.yaml"));
    assert!(err.is_err());
}

// ---------------------------------------------------------------------------
// `credentials import`
// ---------------------------------------------------------------------------

/// Build a temp home with a config.json whose model_list has the given api_keys.
fn temp_home_with_keys(keys: &[&str]) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    let mut cfg = Config::default();
    for (i, k) in keys.iter().enumerate() {
        cfg.model_list.push(ModelConfig {
            model_name: format!("m{}", i),
            model: format!("openai/gpt-{}", i),
            api_key: k.to_string(),
            ..Default::default()
        });
    }
    let config_path = home.join("config.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    let cred_path = credentials_path_for_home(&home);
    (dir, config_path, cred_path)
}

#[test]
fn test_import_migrates_plaintext_keys() {
    let (_dir, config_path, cred_path) = temp_home_with_keys(&[
        "sk-plaintext-one",
        "env:SOME_VAR",
        "",
        "sk-plaintext-two",
    ]);

    let report = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(report.migrated.len(), 2, "two plaintext keys migrated");
    assert_eq!(report.skipped_reference, 1);
    assert_eq!(report.skipped_empty, 1);

    // credentials.yaml holds the keys.
    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(creds.keys.get("m0").unwrap(), "sk-plaintext-one");
    assert_eq!(creds.keys.get("m3").unwrap(), "sk-plaintext-two");

    // config.json: references in place of literals, and the raw file text no
    // longer contains ANY plaintext key (grep-style acceptance).
    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(!raw.contains("sk-plaintext-one"), "config.json has no plaintext: {raw}");
    assert!(!raw.contains("sk-plaintext-two"), "config.json has no plaintext: {raw}");
    assert!(raw.contains("yaml:m0"));
    assert!(raw.contains("env:SOME_VAR"), "env: entries untouched");
}

#[test]
fn test_import_is_idempotent() {
    let (_dir, config_path, cred_path) = temp_home_with_keys(&["sk-secret-xyz"]);

    let first = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(first.migrated.len(), 1);
    let raw_after_first = std::fs::read_to_string(&config_path).unwrap();

    let second = run_import(&config_path, &cred_path).unwrap();
    assert!(second.is_noop(), "second run migrates nothing: {:?}", second);
    assert_eq!(second.skipped_reference, 1);

    // Config unchanged by the second run.
    let raw_after_second = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(raw_after_first, raw_after_second);

    // And the alias still resolves to the original value.
    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(creds.keys.get("m0").unwrap(), "sk-secret-xyz");
}

#[test]
fn test_import_alias_conflict_uses_suffix_never_overwrites() {
    let (dir, config_path, cred_path) = temp_home_with_keys(&["sk-new-value"]);
    // Pre-existing yaml alias "m0" with a DIFFERENT value.
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("m0".to_string(), "sk-old-value".to_string())].into(),
        },
    )
    .unwrap();

    let report = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0], ("m0".to_string(), "m0__2".to_string()));

    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(creds.keys.get("m0").unwrap(), "sk-old-value", "existing alias NOT overwritten");
    assert_eq!(creds.keys.get("m0__2").unwrap(), "sk-new-value");

    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(raw.contains("yaml:m0__2"));
    assert!(!raw.contains("sk-new-value"));
    drop(dir);
}

#[test]
fn test_import_missing_config_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let cred_path = dir.path().join("credentials.yaml");
    let report = run_import(&config_path, &cred_path).unwrap();
    assert!(report.is_noop());
    // No side-effect file creation.
    assert!(!config_path.exists());
    assert!(!cred_path.exists());
}

#[test]
fn test_sanitize_alias() {
    assert_eq!(sanitize_alias("openai/gpt-4"), "openai_gpt-4");
    assert_eq!(sanitize_alias("my model"), "my_model");
    assert_eq!(sanitize_alias(""), "model");
}
