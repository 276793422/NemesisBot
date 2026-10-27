//! models.migrate_keys（明文 key 一步迁移，2026-10-27）测试——yaml/vault
//! 双臂端到端 + 原子性 + 断引用 + 幂等 + target 校验。
//!
//! 与 copy_rename_tests 同形态：声明在 models.rs 内、直接构私有方法（不触
//! WS 路由）；vault 臂经 make_ctx 转发（workspace 覆写为真实拓扑
//! home/workspace，vault_path 与 credentials_path_for_home 各走各的单一
//! 真相源）。口令一律显式传参（不碰进程 env，跨平台确定：DPAPI 忽略口令、
//! argon2 用之）。

use super::*;

fn make_ctx(dir: &tempfile::TempDir) -> crate::ws_router::RequestContext {
    super::tests::make_ctx(dir)
}

fn home_str(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

/// 带 raw extras 的种子配置：m1 明文（带 tier/real_name/custom_note）、
/// m2 env 引用、m3 yaml 引用、m4 vault 引用——四形态一次盖全。
const SEED: &str = r#"{
  "agents": { "defaults": { "llm": "m1" } },
  "model_list": [
    {
      "model_name": "m1",
      "model": "qwen/qwen3-30b",
      "api_key": "sk-secret-1",
      "model_tier": "auto",
      "real_name": "Qwen3-30B",
      "custom_note": "keep-me"
    },
    { "model_name": "m2", "model": "test/testai-1.1", "api_key": "env:KEEP_ME" },
    { "model_name": "m3", "model": "test/testai-1.2", "api_key": "yaml:legacy" },
    { "model_name": "m4", "model": "test/testai-1.3", "api_key": "vault:already" }
  ]
}"#;

fn write_config(home: &std::path::Path, body: &str) {
    std::fs::write(home.join("config.json"), body).unwrap();
}

fn read_config_raw(home: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
}

fn read_config_text(home: &std::path::Path) -> String {
    std::fs::read_to_string(home.join("config.json")).unwrap()
}

fn cred_path(home: &std::path::Path) -> std::path::PathBuf {
    nemesis_config::credentials::credentials_path_for_home(home)
}

// ---------------------------------------------------------------------------
// target=yaml
// ---------------------------------------------------------------------------

#[test]
fn yaml_end_to_end_migrates_inline_and_preserves_extras() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_str(&dir);
    write_config(dir.path(), SEED);

    let out = ModelsHandler::migrate_keys_to_yaml(&home).unwrap().unwrap();

    assert_eq!(out["target"], "yaml");
    let migrated = out["migrated"].as_array().unwrap();
    assert_eq!(migrated.len(), 1, "只有 m1 明文被迁移");
    assert_eq!(migrated[0]["name"], "m1");
    assert_eq!(migrated[0]["alias"], "m1");
    assert_eq!(out["skipped_reference"], 3, "env/yaml/vault 三形态全跳过");

    // config.json 落盘：引用形态 + extras 保真 + 明文离盘。
    let cfg = read_config_raw(dir.path());
    let m1 = &cfg["model_list"][0];
    assert_eq!(m1["api_key"], "yaml:m1");
    assert_eq!(m1["custom_note"], "keep-me", "未类型化 extras 保真");
    assert_eq!(m1["real_name"], "Qwen3-30B");
    assert_eq!(cfg["model_list"][1]["api_key"], "env:KEEP_ME");
    assert!(!read_config_text(dir.path()).contains("sk-secret-1"));

    // credentials.yaml 落盘：别名条目在，值正确。
    let creds: nemesis_config::credentials::CredentialsFile =
        nemesis_config::credentials::load_credentials_file(&cred_path(dir.path())).unwrap();
    assert_eq!(
        creds.keys.get("m1").map(String::as_str),
        Some("sk-secret-1")
    );
}

#[test]
fn yaml_is_idempotent_second_run_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_str(&dir);
    write_config(dir.path(), SEED);

    let first = ModelsHandler::migrate_keys_to_yaml(&home).unwrap().unwrap();
    assert_eq!(first["migrated"].as_array().unwrap().len(), 1);
    let snap = read_config_text(dir.path());

    let second = ModelsHandler::migrate_keys_to_yaml(&home).unwrap().unwrap();
    assert_eq!(
        second["migrated"].as_array().unwrap().len(),
        0,
        "二次运行零迁移"
    );
    assert_eq!(
        second["skipped_reference"], 4,
        "yaml:m1 也算引用（绝不降级回明文）"
    );
    assert_eq!(read_config_text(dir.path()), snap, "幂等：配置逐字节不变");
}

#[test]
fn yaml_conflict_uses_suffix_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_str(&dir);
    // credentials.yaml 预置同别名异值。
    std::fs::create_dir_all(cred_path(dir.path()).parent().unwrap()).unwrap();
    nemesis_config::credentials::save_credentials_file(
        &cred_path(dir.path()),
        &nemesis_config::credentials::CredentialsFile {
            keys: [("m1".to_string(), "sk-OTHER-value".to_string())]
                .into_iter()
                .collect(),
        },
    )
    .unwrap();
    write_config(
        dir.path(),
        r#"{"model_list":[{"model_name":"m1","model":"openai/x","api_key":"sk-new-value"}]}"#,
    );

    let out = ModelsHandler::migrate_keys_to_yaml(&home).unwrap().unwrap();
    assert_eq!(out["conflicts"][0]["wanted"], "m1");
    assert_eq!(out["conflicts"][0]["used"], "m1__2");
    let creds: nemesis_config::credentials::CredentialsFile =
        nemesis_config::credentials::load_credentials_file(&cred_path(dir.path())).unwrap();
    assert_eq!(
        creds.keys.get("m1").map(String::as_str),
        Some("sk-OTHER-value"),
        "原值未被覆盖"
    );
    assert_eq!(
        creds.keys.get("m1__2").map(String::as_str),
        Some("sk-new-value")
    );
    let cfg = read_config_raw(dir.path());
    assert_eq!(cfg["model_list"][0]["api_key"], "yaml:m1__2");
}

#[test]
fn yaml_missing_model_list_is_clean_noop() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_str(&dir);
    write_config(dir.path(), r#"{"session":{}}"#);
    let out = ModelsHandler::migrate_keys_to_yaml(&home).unwrap().unwrap();
    assert_eq!(out["migrated"].as_array().unwrap().len(), 0);
    assert!(
        !cred_path(dir.path()).exists(),
        "noop 不造 credentials.yaml"
    );
}

#[test]
fn target_validation_rejects_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = ModelsHandler::migrate_keys(
        &home_str(&dir),
        &ctx,
        &serde_json::json!({ "target": "sqlite" }),
    )
    .unwrap_err();
    assert!(
        err.contains("yaml") && err.contains("vault"),
        "报错列出合法值: {err}"
    );
}

// ---------------------------------------------------------------------------
// target=vault（security feature 闸内）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
mod vault_arm {
    use super::*;

    /// 真实拓扑 ctx：home=X，workspace=X/workspace（make_ctx 默认同路径，
    /// 这里覆写 workspace/home 让 vault_path / credentials_path_for_home
    /// 各走各的单一真相源）。
    fn make_home_ctx(
        dir: &tempfile::TempDir,
    ) -> (std::path::PathBuf, crate::ws_router::RequestContext) {
        let home = dir.path().join("nb-home");
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let mut ctx = make_ctx(dir);
        ctx.workspace = Some(home.join("workspace").to_string_lossy().to_string());
        ctx.home = Some(home.to_string_lossy().to_string());
        (home, ctx)
    }

    fn vault_file(ws: &std::path::Path) -> std::path::PathBuf {
        nemesis_path::resolve_vault_path_in_workspace(ws)
    }

    fn call(
        home: &std::path::Path,
        ctx: &crate::ws_router::RequestContext,
        pw: &str,
    ) -> serde_json::Value {
        ModelsHandler::migrate_keys(
            &home.to_string_lossy(),
            ctx,
            &serde_json::json!({ "target": "vault", "passphrase": pw }),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn vault_end_to_end_upgrades_inline_and_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let (home, ctx) = make_home_ctx(&dir);
        write_config(&home, SEED);
        std::fs::create_dir_all(cred_path(&home).parent().unwrap()).unwrap();
        nemesis_config::credentials::save_credentials_file(
            &cred_path(&home),
            &nemesis_config::credentials::CredentialsFile {
                keys: [("legacy".to_string(), "sk-legacy-real".to_string())]
                    .into_iter()
                    .collect(),
            },
        )
        .unwrap();

        let out = call(&home, &ctx, "migrate-test-pw");

        assert_eq!(out["target"], "vault");
        let migrated = out["migrated"].as_array().unwrap();
        assert_eq!(migrated.len(), 2, "inline m1 + yaml m3 各一条");
        assert_eq!(out["yaml_aliases_removed"], serde_json::json!(["legacy"]));

        // config 落盘：vault 引用 + 明文彻底离盘。
        let cfg = read_config_raw(&home);
        assert_eq!(cfg["model_list"][0]["api_key"], "vault:m1");
        assert_eq!(cfg["model_list"][2]["api_key"], "vault:legacy");
        let text = read_config_text(&home);
        assert!(!text.contains("sk-secret-1"));
        assert!(!text.contains("sk-legacy-real"));

        // vault 落盘：重开可读（跨进程一致性）。
        let mut store =
            nemesis_security::vault::VaultStore::open(&vault_file(&home.join("workspace")))
                .unwrap();
        if store.mode() == nemesis_security::vault::VaultMode::Argon2id {
            store.unlock("migrate-test-pw").unwrap();
        }
        assert_eq!(store.get("m1").unwrap(), "sk-secret-1");
        assert_eq!(store.get("legacy").unwrap(), "sk-legacy-real");

        // credentials.yaml：legacy 条目已移除。
        let creds: nemesis_config::credentials::CredentialsFile =
            nemesis_config::credentials::load_credentials_file(&cred_path(&home)).unwrap();
        assert!(!creds.keys.contains_key("legacy"));
    }

    #[test]
    fn vault_wrong_passphrase_aborts_with_config_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let (home, ctx) = make_home_ctx(&dir);
        write_config(&home, SEED);
        // 预置 argon2 锁定 vault（跨平台确定形态），口令 "right-pw"。
        let vf = vault_file(&home.join("workspace"));
        nemesis_security::vault::VaultStore::create(
            &vf,
            nemesis_security::vault::VaultMode::Argon2id,
            Some("right-pw"),
        )
        .unwrap();
        let before = read_config_text(&home);

        let err = ModelsHandler::migrate_keys(
            &home.to_string_lossy(),
            &ctx,
            &serde_json::json!({ "target": "vault", "passphrase": "wrong-pw" }),
        )
        .unwrap_err();
        assert!(!err.is_empty(), "解锁失败必须诚实报错");

        // 原子性：config.json 磁盘字节不变（明文原地）。
        assert_eq!(read_config_text(&home), before, "失败迁移不得动配置");
    }

    #[test]
    fn vault_idempotent_second_run_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let (home, ctx) = make_home_ctx(&dir);
        write_config(&home, SEED);
        // m3 的 yaml:legacy 引用需要 credentials.yaml 在场（否则算断引用）。
        std::fs::create_dir_all(cred_path(&home).parent().unwrap()).unwrap();
        nemesis_config::credentials::save_credentials_file(
            &cred_path(&home),
            &nemesis_config::credentials::CredentialsFile {
                keys: [("legacy".to_string(), "sk-legacy-real".to_string())]
                    .into_iter()
                    .collect(),
            },
        )
        .unwrap();

        let first = call(&home, &ctx, "migrate-test-pw");
        assert_eq!(first["migrated"].as_array().unwrap().len(), 2);
        let snap = read_config_text(&home);

        let second = call(&home, &ctx, "migrate-test-pw");
        assert_eq!(second["migrated"].as_array().unwrap().len(), 0);
        assert_eq!(second["already_vault"], 3, "m1/m3/m4 已是 vault: 引用");
        assert_eq!(second["skipped_env"], 1, "m2 env 引用照常跳过");
        assert_eq!(read_config_text(&home), snap, "幂等：配置逐字节不变");
    }
}
