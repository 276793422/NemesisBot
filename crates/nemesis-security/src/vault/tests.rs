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
