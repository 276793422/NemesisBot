//! vault 模块测试：roundtrip、错误口令、损坏文件、AAD 绑定、list 防泄漏。
//! 平台：argon2 全平台；DPAPI 段挂 `#[cfg(windows)]`。

use super::*;
use std::path::{Path, PathBuf};

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

/// `VaultStore::create` 带瞬态重试：CI runner 上的外力（AV 扫描/临时区保留
/// 清理）会在 create 的 seal → save 原子写窗口内删掉 temp 目录。实录两臂：
/// rename 报 os error 3（cov_tests 同族：2026-11-06 shard7 首录，2026-11-09
/// shard9 create_persists_immediately 再实录）；atomic write 建临时文件时
/// 父目录已被删报 `create temp: No such file or directory`（2026-11-11
/// ET 42950419158 ciphertext_bound_to_alias 实录）。helper 契约
/// （nemesis-utils/src/file.rs）明示 rename 遇外部占用"由调用方决定重试或
/// 上报"，测试侧选重试。create 自带 create_dir_all 臂（父目录被外力删掉后
/// 同路径重试即恢复现场，且瞬态臂下 vault.enc 尚未落盘、无残留），换目录
/// 重试反而令调用侧缓存的第一手 path 失效。只吞上述两瞬态臂，业务错误
/// 照常上抛——断言力不变。期望失败的 create（FileExists/Crypto 臂）原子写
/// 根本不发生，不走此 helper。
fn create_retrying(path: &Path, mode: VaultMode, passphrase: Option<&str>) -> VaultStore {
    let mut last_err = String::new();
    for _ in 0..3 {
        match VaultStore::create(path, mode, passphrase) {
            Ok(store) => return store,
            Err(VaultError::Io(e))
                if e.to_string().contains("rename:")
                    || e.to_string()
                        .contains("create temp: No such file or directory") =>
            {
                last_err = e.to_string();
            }
            Err(err) => panic!("create 失败（非瞬态外力噪声）: {err}"),
        }
    }
    panic!("create 连续 3 次遇瞬态失败: {last_err}");
}

/// 外力瞬态第二臂的 body 级重跑（2026-11-09 coverage 42558777444 实录）：
/// vault.enc 在 create 成功、save 落盘之后、open 之前被外力删掉（Linux
/// coverage runner，`NotFound("/tmp/...")`）——create_retrying 管不了
/// create 返回之后的窗口，且文件已灭时逐点重试无意义，只能整段重来。
/// `f` 必须自包含（create 起步的完整流程）；临时目录按 attempt 换新。
/// 只吞 panic 消息含 `rename:`/`NotFound(` 的瞬态臂，其余原样 resume
/// ——断言失败照常红，断言力不变。
fn vault_body_retried<T>(tag: &str, mut f: impl FnMut(&Path) -> T) -> T {
    let mut last_msg = String::new();
    for attempt in 0..3 {
        let path = tmp_vault_path(&format!("{tag}-{attempt}"));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&path)));
        match result {
            Ok(v) => {
                cleanup(&path);
                return v;
            }
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default();
                if msg.contains("rename:") || msg.contains("NotFound(") {
                    last_msg = msg;
                } else {
                    std::panic::resume_unwind(payload);
                }
            }
        }
    }
    panic!("vault 测试连续 3 次遇外力瞬态（rename:/NotFound）: {last_msg}");
}

/// argon2 模式全流程：create → set → reopen（锁定）→ list 可见但 get 锁定 →
/// 错误口令拒绝 → 正确口令解锁 → get 还原。
#[test]
fn argon2_roundtrip_lock_and_unlock() {
    vault_body_retried("argon2-roundtrip", |path| {
        {
            let mut store = create_retrying(path, VaultMode::Argon2id, Some("correct horse"));
            assert!(store.is_unlocked());
            store
                .set("openai-main", "sk-SUPERSECRET123", "openai", "主力模型 key")
                .unwrap();
            store.save().unwrap();
        }
        // 重开：锁定态。
        let store = VaultStore::open(path).unwrap();
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
    });
}

/// 创建后未 save（create 内部已 save）重开即丢——不，create 落盘；
/// 这里验证 create 后立刻 reopen 可读。
#[test]
fn create_persists_immediately() {
    let path = tmp_vault_path("create-persist");
    create_retrying(&path, VaultMode::Argon2id, Some("pw"));
    let store = VaultStore::open(&path).unwrap();
    assert!(store.list().is_empty());
    cleanup(&path);
}

/// 已存在文件 create → FileExists。
#[test]
fn create_rejects_existing_file() {
    let path = tmp_vault_path("create-exists");
    create_retrying(&path, VaultMode::Argon2id, Some("pw"));
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
    let mut store = create_retrying(&path, VaultMode::Argon2id, Some("pw"));
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
    let mut store = create_retrying(&path, VaultMode::Argon2id, Some("pw"));
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
    let mut store = create_retrying(&path, VaultMode::Argon2id, Some("pw"));
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
    // body 级重跑（2026-11-13 coverage 实录：create 返回后的 save 原子写
    // 窗口遇外力删目录，rename ENOENT——create_retrying 管不了 create
    // 之后的窗口，同 vault_body_retried 所记 2026-11-09 臂）。
    vault_body_retried("aad", |path| {
        {
            let mut store = create_retrying(path, VaultMode::Argon2id, Some("pw"));
            store.set("real", "secret-value", "", "").unwrap();
            store.save().unwrap();
        }
        // 手工把 real 的密文挪到 decoy 名下。
        let raw = std::fs::read(path).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let entry = json["entries"]["real"].clone();
        json["entries"]["decoy"] = entry;
        std::fs::write(path, serde_json::to_vec(&json).unwrap()).unwrap();
        // 解锁后 decoy 解密必须失败，real 不受影响。
        let mut store = VaultStore::open(path).unwrap();
        store.unlock("pw").unwrap();
        assert!(matches!(store.get("decoy"), Err(VaultError::Corrupted(_))));
        assert_eq!(store.get("real").unwrap(), "secret-value");
    });
}

/// 防泄漏：密文与元数据层面都不得出现明文 secret。
#[test]
fn plaintext_never_on_disk_or_in_listings() {
    let path = tmp_vault_path("no-leak");
    let secret = "sk-LEAKCHECK-9f8e7d6c";
    let mut store = create_retrying(&path, VaultMode::Argon2id, Some("pw"));
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
        let mut store = create_retrying(&path, VaultMode::Dpapi, None);
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
    create_retrying(&path, VaultMode::Dpapi, None);
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
// agent 迁移：export_entries / import_entries。
// ---------------------------------------------------------------------------

/// 导出→导入 roundtrip：值还原、元数据保留、错误口令拒绝、
/// 导出 blob 是 argon2id 形态（可跨机器）。
#[test]
fn export_import_roundtrip_preserves_values_and_meta() {
    let src_path = tmp_vault_path("xfer-src");
    let dst_path = tmp_vault_path("xfer-dst");
    let mut src = create_retrying(&src_path, VaultMode::Argon2id, Some("src-pw"));
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
    let mut dst = create_retrying(&dst_path, VaultMode::Argon2id, Some("dst-pw"));
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
    let mut store = create_retrying(&path, VaultMode::Argon2id, Some("pw"));
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
    let dst_locked = create_retrying(&dst_path, VaultMode::Argon2id, Some("x"));
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
// 模型 key 迁移核心（自 nemesisbot/commands/vault.rs 下沉）：
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
    let store = create_retrying(&vp, mode, pw);
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

// --- already_vault 残留清扫臂在场守卫（vault 缺席绝不删 creds 明文） ---

#[test]
fn migrate_vault_residue_cleanup_skipped_when_vault_missing_alias() {
    // 损坏态现场：config 指 vault:ghost 但 vault 无该条目（vault.enc 丢失/
    // 重置），creds 仍留同名明文 = 最后一份可恢复副本。清扫臂必须在场验证，
    // 绝不能销毁副本（config 的 vault:ghost 引用将无处解析且明文已灭）。
    let (dir, mut store) = migrate_store("cleanup-guard");
    let mut config = serde_json::json!({
        "model_list": [
            { "model_name": "m1", "model": "openai/m1", "api_key": "vault:ghost" }
        ]
    });
    let mut creds = Some(nemesis_config::credentials::CredentialsFile {
        keys: [("ghost".to_string(), "sk-last-copy".to_string())]
            .into_iter()
            .collect(),
    });

    let report = migrate_model_keys_to_vault(&mut config, &mut store, creds.as_mut()).unwrap();

    assert_eq!(report.already_vault, 1);
    assert!(!report.creds_changed, "vault 缺席不得动 creds");
    assert!(report.yaml_aliases_removed.is_empty(), "不得记移除");
    let creds = creds.as_ref().unwrap();
    assert!(creds.keys.contains_key("ghost"), "明文副本必须保留");
    assert!(report.is_noop(), "无迁移无断引 = 幂等重跑面");
    cleanup(&dir);
}

#[test]
fn migrate_vault_residue_cleanup_fires_when_vault_has_alias() {
    // 既有语义回归锁（收敛路径）：vault 真在场时残留
    // 清扫照常 fires——守卫只拦损坏态，不拦正常收敛。
    let (dir, mut store) = migrate_store("cleanup-fires");
    store.set("legacy", "sk-legacy-real", "model", "").unwrap();
    let mut config = serde_json::json!({
        "model_list": [
            { "model_name": "m1", "model": "openai/m1", "api_key": "vault:legacy" }
        ]
    });
    let mut creds = Some(nemesis_config::credentials::CredentialsFile {
        keys: [("legacy".to_string(), "sk-legacy-real".to_string())]
            .into_iter()
            .collect(),
    });

    let report = migrate_model_keys_to_vault(&mut config, &mut store, creds.as_mut()).unwrap();

    assert_eq!(report.already_vault, 1);
    assert!(report.creds_changed, "vault 在场 = 残留明文清扫照常");
    assert_eq!(report.yaml_aliases_removed, vec!["legacy".to_string()]);
    let creds = creds.as_ref().unwrap();
    assert!(!creds.keys.contains_key("legacy"), "明文彻底离盘");
    cleanup(&dir);
}

/// oracle 契约 pin：`UnknownAlias` 的 Display 以
/// 「别名不存在」开头——nemesis-agent 注入层凭这个字面量 marker 识别
/// 「别名不存在」根因并改写为通用文案（不给存在性 oracle）。thiserror
/// 文案漂移会静默拆闸，这里钉死。
#[test]
fn unknown_alias_display_marker_pinned_for_credential_oracle() {
    let e = VaultError::UnknownAlias("some-alias".to_string());
    let text = e.to_string();
    assert!(
        text.starts_with("别名不存在"),
        "UnknownAlias Display marker drifted (L26 oracle contract): {text}"
    );
}
