//! vault CLI 测试：口令来源优先级、别名存在性判断等纯逻辑部分。

use super::*;

/// argon2 模式：环境变量存在 → 直接使用（不触发交互输入）。
#[test]
fn passphrase_from_env_var() {
    // 本测试进程内设置环境变量，验证取用路径（临时键避免污染）。
    unsafe {
        std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "env-passphrase-test");
    }
    let got = passphrase_for_mode(VaultMode::Argon2id).unwrap();
    unsafe {
        std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE");
    }
    assert_eq!(got.as_deref(), Some("env-passphrase-test"));
}

/// dpapi 模式：永远不需要口令。
#[cfg(windows)]
#[test]
fn dpapi_needs_no_passphrase() {
    assert_eq!(passphrase_for_mode(VaultMode::Dpapi).unwrap(), None);
}

/// CLI 侧 vault 路径解析与 nemesis-path 单一拼接点一致。
#[test]
fn vault_path_matches_path_helper() {
    let home = std::env::temp_dir().join("nemesisbot-vault-cli-path-test");
    let ws = common::workspace_path(&home);
    assert_eq!(
        nemesis_path::resolve_vault_path_in_workspace(&ws),
        ws.join("config").join("vault.enc")
    );
}

// ---------------------------------------------------------------------------
// P0 B2：vault migrate（模型 key 迁移）
// ---------------------------------------------------------------------------

use std::path::Path;

/// migrate 测试经 test_store 触碰 env 口令（argon2 平台）——互斥防踩。
static MIGRATE_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 写一个最小 config.json + credentials.yaml 到临时 home。
fn seed_config(home: &Path, model_entries: &str) -> std::path::PathBuf {
    std::fs::create_dir_all(home).unwrap();
    let cfg = format!(r#"{{"model_list":[{model_entries}],"session":{{}},"gateway":{{}}}}"#);
    let p = home.join("config.json");
    std::fs::write(&p, cfg).unwrap();
    p
}

/// 明文 key → vault:<alias>，值入 vault，配置回写（typed round-trip）。
#[test]
fn migrate_inline_plaintext_key() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"gpt-main","model":"openai/gpt-4o","api_key":"sk-plain-123"}"#,
    );
    let cred_path = home.join("config").join("credentials.yaml");
    std::fs::create_dir_all(cred_path.parent().unwrap()).unwrap();

    let mut store = test_store(home);
    let report = super::migrate_model_keys(&config_path, &cred_path, &mut store).unwrap();

    assert_eq!(report.migrated.len(), 1);
    assert_eq!(report.migrated[0].1, "gpt-main");
    // 配置回写为引用，明文不在 config.json。
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("vault:gpt-main"), "got: {cfg_text}");
    assert!(!cfg_text.contains("sk-plain-123"));
    // 值在 vault 里（内存 store 可读），不在磁盘明文。
    assert_eq!(store.get("gpt-main").unwrap(), "sk-plain-123");
    let vp = nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(home));
    assert!(
        !vp.exists()
            || !std::fs::read_to_string(&vp)
                .unwrap()
                .contains("sk-plain-123")
    );
}

/// yaml: 引用：值搬进 vault（别名不变），配置回写 vault:，credentials.yaml
/// 条目移除——明文彻底离盘。
#[test]
fn migrate_yaml_reference_moves_value_out_of_yaml() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"claude-main","model":"anthropic/claude","api_key":"yaml:sk-claude"}"#,
    );
    let cred_path = home.join("config").join("credentials.yaml");
    std::fs::create_dir_all(cred_path.parent().unwrap()).unwrap();
    std::fs::write(&cred_path, "keys:\n  sk-claude: \"sk-ant-real\"\n").unwrap();

    let mut store = test_store(home);
    let report = super::migrate_model_keys(&config_path, &cred_path, &mut store).unwrap();

    assert_eq!(report.migrated.len(), 1);
    assert_eq!(report.migrated[0].1, "sk-claude");
    assert_eq!(store.get("sk-claude").unwrap(), "sk-ant-real");
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("vault:sk-claude"));
    let creds: nemesis_config::credentials::CredentialsFile =
        nemesis_config::credentials::load_credentials_file(&cred_path).unwrap();
    assert!(!creds.keys.contains_key("sk-claude"), "yaml 条目应已移除");
}

/// 幂等：第二次运行不再动已迁移条目；env: 引用保持原样。
#[test]
fn migrate_is_idempotent_and_keeps_env_refs() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"a","model":"openai/a","api_key":"env:MY_KEY"},
{"model_name":"b","model":"openai/b","api_key":"vault:existing"}"#,
    );

    let mut store = test_store(home);
    let report = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store,
    )
    .unwrap();
    assert!(report.is_noop());
    assert_eq!(report.skipped_env, 1);
    assert_eq!(report.already_vault, 1);
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("env:MY_KEY"));
}

/// 别名冲突：同名异值加后缀，绝不覆盖；同值复用。
#[test]
fn migrate_conflict_suffix_never_overwrites() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"dup","model":"openai/dup","api_key":"sk-new-value"}"#,
    );

    let mut store = test_store(home);
    store.set("dup", "sk-old-value", "model", "").unwrap();
    let report = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store,
    )
    .unwrap();
    assert_eq!(report.migrated[0].1, "dup__2");
    assert_eq!(store.get("dup").unwrap(), "sk-old-value", "原值未被覆盖");
    assert_eq!(store.get("dup__2").unwrap(), "sk-new-value");

    // 同值复用：再跑一次同值条目 → reused。
    store.set("same", "sk-same", "model", "").unwrap();
    let config_path = seed_config(
        home,
        r#"{"model_name":"same","model":"openai/same","api_key":"sk-same"}"#,
    );
    let mut store2 = store;
    let rep = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store2,
    )
    .unwrap();
    assert_eq!(rep.reused, 1);
    assert_eq!(rep.migrated[0].1, "same");
}

/// 断引 yaml: 引用：不迁移不丢引用，报告里指名。
#[test]
fn migrate_reports_broken_yaml_reference() {
    let _g = MIGRATE_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let config_path = seed_config(
        home,
        r#"{"model_name":"ghost","model":"openai/ghost","api_key":"yaml:no-such-alias"}"#,
    );
    std::fs::create_dir_all(home.join("config")).unwrap();
    let mut store = test_store(home);
    let report = super::migrate_model_keys(
        &config_path,
        &home.join("config").join("credentials.yaml"),
        &mut store,
    )
    .unwrap();
    assert!(report.migrated.is_empty());
    assert_eq!(report.yaml_broken.len(), 1);
    // 配置未被改动（引用保留，等用户处理）。
    let cfg_text = std::fs::read_to_string(&config_path).unwrap();
    assert!(cfg_text.contains("yaml:no-such-alias"));
}

/// 测试用 vault store（默认模式；argon2 走固定口令）。
fn test_store(home: &Path) -> VaultStore {
    let vp = nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(home));
    std::fs::create_dir_all(vp.parent().unwrap()).unwrap();
    let mode = VaultStore::default_mode();
    let pw = if mode == VaultMode::Argon2id {
        Some("migrate-test-pw")
    } else {
        None
    };
    VaultStore::create(&vp, mode, pw).unwrap()
}
