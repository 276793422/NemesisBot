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
            extra: Default::default(),
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
    let err = format!(
        "{:?}",
        crate::resolve_model_config(&cfg, "ref").unwrap_err()
    );
    assert!(err.contains("nope"), "error names the alias: {err}");
    assert!(
        err.contains("credentials import"),
        "error carries remedy: {err}"
    );
}

#[test]
fn test_yaml_file_missing_fails_loud() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _g = GlobalPathGuard::set(dir.path().join("absent.yaml"));

    let cfg = model_config_with_key("yaml:main");
    let err = format!(
        "{:?}",
        crate::resolve_model_config(&cfg, "ref").unwrap_err()
    );
    assert!(err.contains("absent.yaml"), "error names the file: {err}");
    assert!(
        err.contains("credentials import"),
        "error carries remedy: {err}"
    );
}

#[test]
fn test_yaml_path_unset_fails_loud() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    clear_global_credentials_path();
    let cfg = model_config_with_key("yaml:main");
    let err = format!(
        "{:?}",
        crate::resolve_model_config(&cfg, "ref").unwrap_err()
    );
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
    assert!(
        text.starts_with("keys:"),
        "yaml shape `keys:` first: {text}"
    );
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
            extra: Default::default(),
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
    let (_dir, config_path, cred_path) =
        temp_home_with_keys(&["sk-plaintext-one", "env:SOME_VAR", "", "sk-plaintext-two"]);

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
    assert!(
        !raw.contains("sk-plaintext-one"),
        "config.json has no plaintext: {raw}"
    );
    assert!(
        !raw.contains("sk-plaintext-two"),
        "config.json has no plaintext: {raw}"
    );
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
    assert!(
        second.is_noop(),
        "second run migrates nothing: {:?}",
        second
    );
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
    assert_eq!(
        creds.keys.get("m0").unwrap(),
        "sk-old-value",
        "existing alias NOT overwritten"
    );
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

// ---------------------------------------------------------------------------
// Coverage batch 2026-08-25: file-IO error branches + import corner cases.
// ---------------------------------------------------------------------------

#[test]
fn test_load_credentials_blank_file_is_default() {
    // A whitespace-only file parses to an EMPTY store (not an error).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.yaml");
    std::fs::write(&path, "  \n\t\n").unwrap();
    let file = load_credentials_file(&path).unwrap();
    assert!(file.keys.is_empty(), "blank file → default store");
}

#[test]
fn test_load_credentials_invalid_yaml_fails_loud() {
    // `keys:` must be a map — a sequence is invalid YAML FOR THIS SHAPE and
    // fails with a remedy message (never silently treated as empty).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.yaml");
    std::fs::write(&path, "keys: [not, a, map]").unwrap();
    let err = load_credentials_file(&path).unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("not valid YAML"),
        "error names the shape problem: {msg}"
    );
    assert!(
        msg.contains("keys"),
        "error carries the expected shape: {msg}"
    );
}

#[test]
fn test_save_credentials_parent_is_file_fails() {
    // Parent path exists as a regular FILE → create_dir_all fails.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker.txt");
    std::fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("credentials.yaml");

    let err = save_credentials_file(&path, &CredentialsFile::default()).unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("cannot create credentials dir"),
        "create-dir error surfaces: {msg}"
    );
}

#[test]
fn test_save_credentials_target_is_directory_fails() {
    // Target path is an existing DIRECTORY → fs::write fails (dir creation
    // and serialization both succeed first, isolating the write-error branch).
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("iamdir");
    std::fs::create_dir_all(&target).unwrap();

    let err = save_credentials_file(&target, &CredentialsFile::default()).unwrap_err();
    let msg = format!("{:?}", err);
    assert!(
        msg.contains("cannot write credentials file"),
        "write error surfaces: {msg}"
    );
}

#[test]
fn test_yaml_alias_set_but_empty_fails_loud() {
    // Alias EXISTS in the file but maps to an empty value → loud error with
    // the alias and the remedy (never a silent empty key).
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cred_path = dir.path().join("credentials.yaml");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("e".to_string(), String::new())].into(),
        },
    )
    .unwrap();
    let _g = GlobalPathGuard::set(&cred_path);

    let cfg = model_config_with_key("yaml:e");
    let err = format!(
        "{:?}",
        crate::resolve_model_config(&cfg, "ref").unwrap_err()
    );
    assert!(
        err.contains("set but empty"),
        "error names the empty value: {err}"
    );
    assert!(err.contains("'e'"), "error names the alias: {err}");
}

/// Write a config.json with one model entry built from explicit parts.
fn temp_home_with_entry(
    model_name: &str,
    model: &str,
    api_key: &str,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    let mut cfg = Config::default();
    cfg.model_list.push(ModelConfig {
        extra: Default::default(),
        model_name: model_name.to_string(),
        model: model.to_string(),
        api_key: api_key.to_string(),
        ..Default::default()
    });
    let config_path = home.join("config.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    let cred_path = credentials_path_for_home(&home);
    (dir, config_path, cred_path)
}

#[test]
fn test_import_reuses_same_value_alias() {
    // Pre-existing alias with the SAME value → reused (file untouched),
    // config still rewritten to the `yaml:` reference.
    let (_dir, config_path, cred_path) = temp_home_with_entry("m0", "openai/gpt-0", "sk-same");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [("m0".to_string(), "sk-same".to_string())].into(),
        },
    )
    .unwrap();

    let report = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(report.reused, 1, "same-value alias reused: {report:?}");
    assert_eq!(report.migrated.len(), 1);
    assert_eq!(report.migrated[0], ("m0".to_string(), "m0".to_string()));

    // No new alias inserted — the file keeps exactly one key.
    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(creds.keys.len(), 1);
    assert_eq!(creds.keys.get("m0").unwrap(), "sk-same");

    // config.json rewritten to the reference.
    let raw = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        raw.contains("yaml:m0"),
        "config references the alias: {raw}"
    );
    assert!(!raw.contains("sk-same"), "no plaintext left: {raw}");
}

#[test]
fn test_import_conflict_reuses_existing_suffixed_alias_with_same_value() {
    // m0 exists with a different value AND m0__2 already holds EXACTLY the
    // literal being imported → the suffix scan stops at m0__2 (break-on-same
    // arm) and no new alias is inserted.
    let (_dir, config_path, cred_path) = temp_home_with_entry("m0", "openai/gpt-0", "sk-new");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [
                ("m0".to_string(), "sk-old".to_string()),
                ("m0__2".to_string(), "sk-new".to_string()),
            ]
            .into(),
        },
    )
    .unwrap();

    let report = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(
        report.conflicts,
        vec![("m0".to_string(), "m0__2".to_string())]
    );
    assert_eq!(report.migrated[0], ("m0".to_string(), "m0__2".to_string()));

    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(
        creds.keys.len(),
        2,
        "no extra alias created: {:?}",
        creds.keys
    );
    assert_eq!(creds.keys.get("m0__2").unwrap(), "sk-new");
}

#[test]
fn test_import_conflict_advances_past_taken_suffixes() {
    // m0, m0__2, m0__3 all taken with DIFFERENT values → the scan advances
    // (n += 1 arm, twice) and lands on m0__4.
    let (_dir, config_path, cred_path) = temp_home_with_entry("m0", "openai/gpt-0", "sk-new");
    save_credentials_file(
        &cred_path,
        &CredentialsFile {
            keys: [
                ("m0".to_string(), "v-old".to_string()),
                ("m0__2".to_string(), "v-2".to_string()),
                ("m0__3".to_string(), "v-3".to_string()),
            ]
            .into(),
        },
    )
    .unwrap();

    let report = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(
        report.conflicts,
        vec![("m0".to_string(), "m0__4".to_string())]
    );
    assert_eq!(report.migrated[0], ("m0".to_string(), "m0__4".to_string()));

    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(creds.keys.get("m0__4").unwrap(), "sk-new");
    // Nothing pre-existing was overwritten.
    assert_eq!(creds.keys.get("m0").unwrap(), "v-old");
    assert_eq!(creds.keys.get("m0__3").unwrap(), "v-3");
}

#[test]
fn test_import_empty_model_name_uses_model_string() {
    // Empty model_name → both the ALIAS base and the REPORT display name come
    // from the `model` string (sanitized for the alias).
    let (_dir, config_path, cred_path) = temp_home_with_entry("", "openai/gpt-9", "sk-nine");

    let report = run_import(&config_path, &cred_path).unwrap();
    assert_eq!(
        report.migrated,
        vec![("openai/gpt-9".to_string(), "openai_gpt-9".to_string())],
        "display name = model, alias = sanitized model: {report:?}"
    );

    let creds = load_credentials_file(&cred_path).unwrap();
    assert_eq!(creds.keys.get("openai_gpt-9").unwrap(), "sk-nine");
}

// ---------------------------------------------------------------------------
// G4 (U15 dashboard badge): classify_key_source 四分支 + 零明文
// ---------------------------------------------------------------------------

#[test]
fn classify_key_source_covers_all_four_kinds() {
    let env = classify_key_source("env:ZHIPU_API_KEY");
    assert_eq!(env.kind, "env");
    assert_eq!(env.reference, "ZHIPU_API_KEY");

    let yaml = classify_key_source("yaml:openai-main");
    assert_eq!(yaml.kind, "yaml");
    assert_eq!(yaml.reference, "openai-main");

    let inline = classify_key_source("sk-plaintext-value");
    assert_eq!(inline.kind, "inline");
    assert_eq!(inline.reference, "", "inline 只回标记，绝不回值");

    let none = classify_key_source("");
    assert_eq!(none.kind, "none");
    assert_eq!(none.reference, "");
}

#[test]
fn classify_key_source_serializes_without_plaintext() {
    // 序列化形态即 WSAPI 响应形态：{kind, ref}，键名是 "ref"。
    let v = serde_json::to_value(classify_key_source("yaml:main")).unwrap();
    assert_eq!(v["kind"], "yaml");
    assert_eq!(v["ref"], "main");
    assert!(v.get("reference").is_none(), "serde rename 生效");
    assert_eq!(
        serde_json::to_string(&classify_key_source("sk-secret"))
            .unwrap()
            .find("sk-secret"),
        None,
        "内联值绝不进序列化输出"
    );
}
