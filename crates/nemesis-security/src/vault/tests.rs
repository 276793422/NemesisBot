//! vault 模块测试：roundtrip、错误口令、损坏文件、AAD 绑定、list 防泄漏。
//! 平台：argon2 全平台；DPAPI 段挂 `#[cfg(windows)]`。

use super::*;
use std::path::PathBuf;

fn tmp_vault_path(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesisbot-vault-test-{}-{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("vault.enc")
}

fn cleanup(path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// argon2 模式全流程：create → set → reopen（锁定）→ list 可见但 get 锁定 →
/// 错误口令拒绝 → 正确口令解锁 → get 还原。
#[test]
fn argon2_roundtrip_lock_and_unlock() {
    let path = tmp_vault_path("argon2-roundtrip");
    {
        let mut store =
            VaultStore::create(&path, VaultMode::Argon2id, Some("correct horse")).unwrap();
        assert!(store.is_unlocked());
        store
            .set("openai-main", "sk-SUPERSECRET123", "openai", "主力模型 key")
            .unwrap();
        store.save().unwrap();
    }
    // 重开：锁定态。
    let store = VaultStore::open(&path).unwrap();
    assert_eq!(store.mode(), VaultMode::Argon2id);
    assert!(!store.is_unlocked());
    // list 未解锁可用，只见元数据。
    let listing = store.list();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].alias, "openai-main");
    assert_eq!(listing[0].meta.domain, "openai");
    // get 在锁定态 → Locked。
    assert!(matches!(store.get("openai-main"), Err(VaultError::Locked)));
    // 错误口令 → WrongPassphrase。
    let mut store = store;
    assert!(matches!(
        store.unlock("wrong password"),
        Err(VaultError::WrongPassphrase)
    ));
    // 正确口令 → 还原。
    store.unlock("correct horse").unwrap();
    assert_eq!(store.get("openai-main").unwrap(), "sk-SUPERSECRET123");
    cleanup(&path);
}

/// 创建后未 save（create 内部已 save）重开即丢——不，create 落盘；
/// 这里验证 create 后立刻 reopen 可读。
#[test]
fn create_persists_immediately() {
    let path = tmp_vault_path("create-persist");
    VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    let store = VaultStore::open(&path).unwrap();
    assert!(store.list().is_empty());
    cleanup(&path);
}

/// 已存在文件 create → FileExists。
#[test]
fn create_rejects_existing_file() {
    let path = tmp_vault_path("create-exists");
    VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    assert!(matches!(
        VaultStore::create(&path, VaultMode::Argon2id, Some("pw")),
        Err(VaultError::FileExists(_))
    ));
    cleanup(&path);
}

/// 文件不存在 open → NotFound。
#[test]
fn open_missing_file_is_not_found() {
    let path = tmp_vault_path("open-missing");
    assert!(matches!(
        VaultStore::open(&path),
        Err(VaultError::NotFound(_))
    ));
    cleanup(&path);
}

/// 损坏文件诚实报错（非 panic、非静默重建）。
#[test]
fn corrupted_file_honest_error() {
    let path = tmp_vault_path("corrupted");
    std::fs::write(&path, b"\x00\x01not json at all").unwrap();
    assert!(matches!(
        VaultStore::open(&path),
        Err(VaultError::Corrupted(_))
    ));
    // 结构损坏：合法 JSON 但缺 mode。
    std::fs::write(&path, br#"{"version":1,"entries":{}}"#).unwrap();
    assert!(matches!(
        VaultStore::open(&path),
        Err(VaultError::Corrupted(_))
    ));
    // 版本不识别。
    std::fs::write(&path, br#"{"version":999,"mode":"argon2id","entries":{}}"#).unwrap();
    assert!(matches!(
        VaultStore::open(&path),
        Err(VaultError::Corrupted(_))
    ));
    cleanup(&path);
}

/// 覆盖写保留 created_at、更新 rotated_at。
#[test]
fn set_overwrite_tracks_rotation() {
    let path = tmp_vault_path("rotate");
    let mut store = VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    store.set("a", "v1", "d", "first").unwrap();
    let created = store.list()[0].meta.created_at.clone();
    store.set("a", "v2", "d", "rotated").unwrap();
    let meta = &store.list()[0].meta;
    assert_eq!(meta.created_at, created);
    assert!(meta.rotated_at.is_some());
    assert_eq!(store.get("a").unwrap(), "v2");
    cleanup(&path);
}

/// remove 后 get → UnknownAlias；remove 不存在的返回 false。
#[test]
fn remove_semantics() {
    let path = tmp_vault_path("remove");
    let mut store = VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    store.set("a", "v", "", "").unwrap();
    assert!(store.remove("a").unwrap());
    assert!(!store.remove("a").unwrap());
    assert!(matches!(store.get("a"), Err(VaultError::UnknownAlias(_))));
    cleanup(&path);
}

/// 非法别名拒绝：空串、首尾空白、控制字符。
#[test]
fn invalid_alias_rejected() {
    let path = tmp_vault_path("bad-alias");
    let mut store = VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    assert!(matches!(
        store.set("", "v", "", ""),
        Err(VaultError::InvalidAlias(_))
    ));
    assert!(matches!(
        store.set(" space ", "v", "", ""),
        Err(VaultError::InvalidAlias(_))
    ));
    assert!(matches!(
        store.set("a\nb", "v", "", ""),
        Err(VaultError::InvalidAlias(_))
    ));
    cleanup(&path);
}

/// AAD 绑定：把条目密文搬到另一别名下必须解密失败（防置换）。
#[test]
fn ciphertext_bound_to_alias() {
    let path = tmp_vault_path("aad");
    {
        let mut store = VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
        store.set("real", "secret-value", "", "").unwrap();
        store.save().unwrap();
    }
    // 手工把 real 的密文挪到 decoy 名下。
    let raw = std::fs::read(&path).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let entry = json["entries"]["real"].clone();
    json["entries"]["decoy"] = entry;
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    // 解锁后 decoy 解密必须失败，real 不受影响。
    let mut store = VaultStore::open(&path).unwrap();
    store.unlock("pw").unwrap();
    assert!(matches!(store.get("decoy"), Err(VaultError::Corrupted(_))));
    assert_eq!(store.get("real").unwrap(), "secret-value");
    cleanup(&path);
}

/// 防泄漏：密文与元数据层面都不得出现明文 secret。
#[test]
fn plaintext_never_on_disk_or_in_listings() {
    let path = tmp_vault_path("no-leak");
    let secret = "sk-LEAKCHECK-9f8e7d6c";
    let mut store = VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    store.set("k", secret, "dom", "desc").unwrap();
    store.save().unwrap();
    // 落盘文件不含明文。
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains(secret), "vault 文件泄漏了明文 secret");
    // listing 序列化不含明文。
    let listings_json = serde_json::to_string(&store.list()).unwrap();
    assert!(!listings_json.contains(secret), "list 泄漏了明文 secret");
    cleanup(&path);
}

/// argon2 缺口令 create → 报错。
#[test]
fn argon2_requires_passphrase() {
    let path = tmp_vault_path("argon2-nopw");
    assert!(matches!(
        VaultStore::create(&path, VaultMode::Argon2id, None),
        Err(VaultError::Crypto(_))
    ));
    cleanup(&path);
}

/// Windows DPAPI：create → set → reopen 自动解锁 → roundtrip。
#[cfg(windows)]
#[test]
fn dpapi_roundtrip_same_user() {
    let path = tmp_vault_path("dpapi-roundtrip");
    {
        let mut store = VaultStore::create(&path, VaultMode::Dpapi, None).unwrap();
        assert!(store.is_unlocked(), "dpapi 模式 create 即解锁");
        store
            .set("tg-token", "123456:ABC-DPAPI-SECRET", "telegram", "")
            .unwrap();
        store.save().unwrap();
    }
    let store = VaultStore::open(&path).unwrap();
    assert!(store.is_unlocked(), "dpapi 模式同用户 reopen 即解锁");
    assert_eq!(store.get("tg-token").unwrap(), "123456:ABC-DPAPI-SECRET");
    cleanup(&path);
}

/// Windows DPAPI：密文层手工翻转一个比特 → 解包失败诚实报错。
#[cfg(windows)]
#[test]
fn dpapi_corrupted_blob_honest_error() {
    use base64::Engine as _;
    let path = tmp_vault_path("dpapi-corrupt");
    VaultStore::create(&path, VaultMode::Dpapi, None).unwrap();
    let raw = std::fs::read(&path).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    let blob_b64 = json["dpapi_blob"].as_str().unwrap().to_string();
    let mut blob = base64::engine::general_purpose::STANDARD
        .decode(&blob_b64)
        .unwrap();
    let mid = blob.len() / 2;
    blob[mid] ^= 0xFF;
    json["dpapi_blob"] =
        serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(&blob));
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let err = VaultStore::open(&path).unwrap_err();
    assert!(
        matches!(err, VaultError::Platform(_)),
        "期望 Platform 错误，实际 {err:?}"
    );
    cleanup(&path);
}

// ---------------------------------------------------------------------------
// P6-R2 agent 迁移：export_entries / import_entries。
// ---------------------------------------------------------------------------

/// 导出→导入 roundtrip：值还原、元数据保留、错误口令拒绝、
/// 导出 blob 是 argon2id 形态（可跨机器）。
#[test]
fn export_import_roundtrip_preserves_values_and_meta() {
    let src_path = tmp_vault_path("xfer-src");
    let dst_path = tmp_vault_path("xfer-dst");
    let mut src = VaultStore::create(&src_path, VaultMode::Argon2id, Some("src-pw")).unwrap();
    src.set("agent/a1/k1", "secret-one", "openai", "主 key")
        .unwrap();
    src.set("agent/a1/k2", "secret-two", "anthropic", "")
        .unwrap();
    src.set("unrelated", "other", "x", "").unwrap();

    let blob = src
        .export_entries(
            &["agent/a1/k1".into(), "agent/a1/k2".into()],
            "one-time-pass",
        )
        .unwrap();
    // blob 全程密文：不能含明文。
    assert!(!String::from_utf8_lossy(&blob).contains("secret-one"));

    // 错误口令拒绝。
    let mut dst = VaultStore::create(&dst_path, VaultMode::Argon2id, Some("dst-pw")).unwrap();
    assert!(matches!(
        dst.import_entries(&blob, "wrong-pass"),
        Err(VaultError::WrongPassphrase)
    ));

    let imported = dst.import_entries(&blob, "one-time-pass").unwrap();
    assert_eq!(imported, vec!["agent/a1/k1", "agent/a1/k2"]);
    assert_eq!(dst.get("agent/a1/k1").unwrap(), "secret-one");
    assert_eq!(dst.get("agent/a1/k2").unwrap(), "secret-two");
    assert!(matches!(
        dst.get("unrelated"),
        Err(VaultError::UnknownAlias(_))
    ));
    // 导出无关条目不进 blob。
    let meta = dst.list();
    let k1 = meta.iter().find(|l| l.alias == "agent/a1/k1").unwrap();
    assert_eq!(k1.meta.domain, "openai");
    assert_eq!(k1.meta.description, "主 key");
    assert!(!k1.meta.created_at.is_empty());

    // 重复导入 = 覆盖（set 轮换语义），值照常还原。
    dst.import_entries(&blob, "one-time-pass").unwrap();
    assert_eq!(dst.get("agent/a1/k1").unwrap(), "secret-one");

    cleanup(&src_path);
    cleanup(&dst_path);
}

/// 缺别名 loud 拒绝 + 未解锁拒绝导出。
#[test]
fn export_rejects_missing_alias_and_locked_store() {
    let path = tmp_vault_path("xfer-errs");
    let mut store = VaultStore::create(&path, VaultMode::Argon2id, Some("pw")).unwrap();
    store.set("only", "v", "d", "").unwrap();
    store.save().unwrap();
    assert!(matches!(
        store.export_entries(&["only".into(), "ghost".into()], "p"),
        Err(VaultError::UnknownAlias(_))
    ));
    // 锁定后拒绝导出。
    drop(store);
    let locked = VaultStore::open(&path).unwrap();
    assert!(matches!(
        locked.export_entries(&["only".into()], "p"),
        Err(VaultError::Locked)
    ));
    // 锁定后拒绝导入（先造合法 blob）。
    let mut unlocked = VaultStore::open(&path).unwrap();
    unlocked.unlock("pw").unwrap();
    let blob = unlocked.export_entries(&["only".into()], "p2").unwrap();
    let dst_path = tmp_vault_path("xfer-errs-dst");
    let dst_locked = VaultStore::create(&dst_path, VaultMode::Argon2id, Some("x")).unwrap();
    drop(unlocked);
    // dst create 后已解锁；重开成锁定态再试导入。
    drop(dst_locked);
    let mut dst = VaultStore::open(&dst_path).unwrap();
    assert!(matches!(
        dst.import_entries(&blob, "p2"),
        Err(VaultError::Locked)
    ));

    cleanup(&path);
    cleanup(&dst_path);
}

// ---------------------------------------------------------------------------
// 模型 key 迁移核心（2026-10-27 自 nemesisbot/commands/vault.rs 下沉）：
// migrate_model_keys_to_vault + vault_alias_for 直测（别名裁决四臂随函数
// 一起搬家——原 nemesisbot 侧 alias_arbiter 直测已由本节承接）。
// ---------------------------------------------------------------------------

/// tempdir + 独立已解锁 vault store（create 即解锁；argon2 平台带口令）。
fn migrate_store(tag: &str) -> (PathBuf, VaultStore) {
    let dir = std::env::temp_dir().join(format!(
        "nemesisbot-vault-migrate-{}-{}",
        tag,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let vp = dir.join("vault.enc");
    let mode = VaultStore::default_mode();
    let pw = if mode == VaultMode::Argon2id {
        Some("migrate-test-pw")
    } else {
        None
    };
    let store = VaultStore::create(&vp, mode, pw).unwrap();
    (dir, store)
}

#[test]
fn migrate_inline_yaml_env_vault_and_empty_arms() {
    let (dir, mut store) = migrate_store("arms");
    let mut config = serde_json::json!({
        "model_list": [
            { "model_name": "gpt-main", "model": "openai/gpt", "api_key": "sk-plain-123" },
            { "model_name": "claude-main", "model": "anthropic/claude", "api_key": "yaml:sk-claude" },
            { "model_name": "env-entry", "model": "openai/env", "api_key": "env:MY_KEY" },
            { "model_name": "vault-entry", "model": "openai/v", "api_key": "vault:existing" },
            { "model_name": "empty-entry", "model": "openai/e", "api_key": "" }
        ]
    });
    let creds = nemesis_config::credentials::CredentialsFile {
        keys: [("sk-claude".to_string(), "sk-ant-real".to_string())]
            .into_iter()
            .collect(),
    };
    let mut creds = Some(creds);

    let report = migrate_model_keys_to_vault(&mut config, &mut store, creds.as_mut()).unwrap();

    assert_eq!(report.migrated.len(), 2, "inline + yaml 各迁移一条");
    assert_eq!(store.get("gpt-main").unwrap(), "sk-plain-123");
    assert_eq!(
        store.get("sk-claude").unwrap(),
        "sk-ant-real",
        "yaml 升格别名不变"
    );
    assert_eq!(report.skipped_env, 1);
    assert_eq!(report.already_vault, 1);
    assert!(report.creds_changed, "yaml 条目被移除");
    let creds = creds.as_ref().unwrap();
    assert!(!creds.keys.contains_key("sk-claude"), "明文彻底离盘");
    // 配置回写形态。
    let list = config
        .get_mut("model_list")
        .and_then(|v| v.as_array_mut())
        .unwrap();
    assert_eq!(list[0]["api_key"], "vault:gpt-main");
    assert_eq!(list[1]["api_key"], "vault:sk-claude");
    assert_eq!(list[2]["api_key"], "env:MY_KEY", "env 引用原样");
    assert_eq!(list[3]["api_key"], "vault:existing");
    assert_eq!(list[4]["api_key"], "");
    assert!(report.config_changed());
    cleanup(&dir);
}

#[test]
fn migrate_yaml_broken_leaves_config_untouched() {
    let (dir, mut store) = migrate_store("broken");
    let mut config = serde_json::json!({
        "model_list": [
            { "model_name": "b1", "model": "openai/b1", "api_key": "yaml:missing-alias" }
        ]
    });

    let report = migrate_model_keys_to_vault(&mut config, &mut store, None).unwrap();

    assert_eq!(report.yaml_broken.len(), 1);
    assert_eq!(
        report.yaml_broken[0],
        ("b1".to_string(), "missing-alias".to_string())
    );
    assert!(!report.config_changed(), "断引用不改配置");
    let list = config
        .get_mut("model_list")
        .and_then(|v| v.as_array_mut())
        .unwrap();
    assert_eq!(list[0]["api_key"], "yaml:missing-alias", "配置原样");
    cleanup(&dir);
}

#[test]
fn migrate_no_model_list_is_default_report() {
    let (dir, mut store) = migrate_store("nolist");
    let mut config = serde_json::json!({ "session": {} });
    let report = migrate_model_keys_to_vault(&mut config, &mut store, None).unwrap();
    assert!(report.is_noop());
    assert!(!report.config_changed());
    cleanup(&dir);
}

// --- vault_alias_for 裁决四臂（原 nemesisbot alias_arbiter 直测承接） ---

#[test]
fn arbiter_free_base_alias_is_returned_as_is() {
    let (dir, store) = migrate_store("arb1");
    let mut report = VaultMigrateReport::default();
    let alias = vault_alias_for(&store, "openai", "sk-lit-1", &mut report);
    assert_eq!(alias, "openai");
    assert_eq!(report.reused, 0);
    assert!(report.conflicts.is_empty(), "空闲别名不得记冲突");
    cleanup(&dir);
}

#[test]
fn arbiter_same_literal_under_taken_alias_is_reused() {
    let (dir, mut store) = migrate_store("arb2");
    store.set("openai", "sk-lit-1", "model", "").unwrap();
    let mut report = VaultMigrateReport::default();
    let alias = vault_alias_for(&store, "openai", "sk-lit-1", &mut report);
    assert_eq!(alias, "openai", "同值复用现别名");
    assert_eq!(report.reused, 1);
    assert!(report.conflicts.is_empty());
    cleanup(&dir);
}

#[test]
fn arbiter_different_literal_gets_numbered_suffix() {
    let (dir, mut store) = migrate_store("arb3");
    store.set("openai", "sk-other", "model", "").unwrap();
    let mut report = VaultMigrateReport::default();
    let alias = vault_alias_for(&store, "openai", "sk-lit-2", &mut report);
    assert_eq!(alias, "openai__2", "异值走 __2 后缀");
    assert_eq!(report.reused, 0);
    assert_eq!(
        report.conflicts,
        vec![("openai".to_string(), "openai__2".to_string())]
    );
    cleanup(&dir);
}

#[test]
fn arbiter_suffix_chain_reuses_matching_value_and_skips_taken() {
    let (dir, mut store) = migrate_store("arb4");
    store.set("openai", "v-base", "model", "").unwrap();
    store.set("openai__2", "v-two", "model", "").unwrap();
    store.set("openai__3", "v-target", "model", "").unwrap();
    let mut report = VaultMigrateReport::default();
    // 目标字面量与 __3 相同：__2 被异值占位跳过，__3 同值复用。
    let alias = vault_alias_for(&store, "openai", "v-target", &mut report);
    assert_eq!(alias, "openai__3");
    assert_eq!(report.reused, 1, "命中同值后缀记复用");
    assert!(report.conflicts.is_empty(), "复用不记冲突");
    cleanup(&dir);
}

// --- 冲突裁决经核心全链（CLI 侧 migrate_conflict_suffix 同款语义回归） ---

#[test]
fn migrate_conflict_suffix_never_overwrites() {
    let (dir, mut store) = migrate_store("conflict");
    store.set("dup", "sk-old-value", "model", "").unwrap();
    let mut config = serde_json::json!({
        "model_list": [
            { "model_name": "dup", "model": "openai/dup", "api_key": "sk-new-value" }
        ]
    });
    let report = migrate_model_keys_to_vault(&mut config, &mut store, None).unwrap();
    assert_eq!(report.migrated[0].1, "dup__2");
    assert_eq!(store.get("dup").unwrap(), "sk-old-value", "原值未被覆盖");
    assert_eq!(store.get("dup__2").unwrap(), "sk-new-value");
    cleanup(&dir);
}
