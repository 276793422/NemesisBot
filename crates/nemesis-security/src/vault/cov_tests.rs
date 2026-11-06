// vault.rs 覆盖率补充测试（VaultMode Display / VaultStore Debug 防泄漏 /
// create 嵌套父目录 / open 不可读文件 Io 臂 / dpapi 幂等 unlock / aliases
// / path / dpapi_blob 损坏臂 + dpapi 全链路往返）。
//
// 豁免：568（CryptProtectData 失败臂——DPAPI 在本机正常工作，无法确定性
// 逼其失败）、619（take_blob 的 pbData 为 null 臂——DPAPI 成功输出恒非
// null，纯防御分支）。

use super::*;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;

fn temp_dir(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nmb-vault-cov-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `VaultStore::create` 带瞬态重试：CI runner 上的外力（AV 扫描/临时区保留
/// 清理）会在 create 的 DPAPI/argon2 seal → save 原子写窗口内删掉 temp
/// 目录，rename 报 os error 3（2026-11-06 15:16Z shard7 实录）。helper 契约
/// （nemesis-utils/src/file.rs）明示 rename 遇外部占用"由调用方决定重试或
/// 上报"，此处选重试：换新 temp 目录最多 3 次；只吞 Io-rename 瞬态臂，
/// 业务错误照常 panic——断言力不变。`rel` 为 vault 文件相对 temp 目录的
/// 路径（嵌套路径仍走 create 的 create_dir_all 臂，不被此 helper 吃掉）。
fn create_store_retrying(
    tag: &str,
    rel: &str,
    mode: VaultMode,
    passphrase: Option<&str>,
) -> (PathBuf, VaultStore) {
    let mut last_err = String::new();
    for attempt in 0..3 {
        let dir = temp_dir(&format!("{tag}-{attempt}"));
        let p = dir.join(rel);
        match VaultStore::create(&p, mode, passphrase) {
            Ok(store) => return (dir, store),
            Err(VaultError::Io(e)) if e.to_string().contains("rename:") => {
                last_err = e.to_string();
            }
            Err(err) => panic!("create 失败（非瞬态 rename 噪声）: {err}"),
        }
    }
    panic!("create 连续 3 次遇 rename 瞬态失败: {last_err}");
}

/// Display 两种模式拼写与 serde 落盘一致（76/78-82）。
#[test]
fn vault_mode_display_matches_serde_spelling() {
    assert_eq!(VaultMode::Dpapi.to_string(), "dpapi");
    assert_eq!(VaultMode::Argon2id.to_string(), "argon2id");
}

/// Debug 永不输出 DEK 与条目值（172-178）。
#[test]
fn vault_store_debug_never_leaks_secrets() {
    let (dir, mut store) = create_store_retrying("debug", "v.vault", VaultMode::Dpapi, None);
    store
        .set("api-key", "super-secret-value", "example.com", "cov")
        .unwrap();

    let dbg = format!("{store:?}");
    assert!(dbg.contains("VaultStore"), "{dbg}");
    assert!(dbg.contains("unlocked: true"), "{dbg}");
    assert!(
        !dbg.contains("super-secret-value"),
        "Debug 泄漏条目值: {dbg}"
    );
    assert!(!dbg.contains("api-key"), "Debug 泄漏别名: {dbg}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// create 支持嵌套不存在的父目录（create_dir_all 臂，222）。
#[test]
fn create_creates_nested_parent_directories() {
    let (dir, store) =
        create_store_retrying("nested", "level1/level2/v.vault", VaultMode::Dpapi, None);
    let p = store.path().to_path_buf();
    assert!(p.exists());
    assert_eq!(store.path(), p.as_path());

    let _ = std::fs::remove_dir_all(&dir);
}

/// open 目标存在但不可读（共享冲突）→ 非 NotFound 的 Io 臂（236）。
#[test]
fn open_unreadable_file_maps_to_io_error() {
    let (dir, _) = create_store_retrying("unreadable", "v.vault", VaultMode::Dpapi, None);
    let p = dir.join("v.vault");

    // 独占句柄：不共享 READ（0x0001）→ 后续 open 读取共享冲突。
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x0002 | 0x0004)
        .open(&p)
        .unwrap();

    let err = VaultStore::open(&p).unwrap_err();
    let msg = err.to_string();
    assert!(
        !msg.contains("NotFound") && !msg.contains("不存在"),
        "共享冲突不得误报 NotFound: {msg}"
    );

    drop(holder);
    let _ = std::fs::remove_dir_all(&dir);
}

/// dpapi 模式：open 即解锁；unlock 幂等成功（271）；aliases/path getters
/// （352-354 / 368-370）；全链路 set → save → reopen → get。
#[test]
fn dpapi_store_full_lifecycle_with_idempotent_unlock() {
    let (dir, mut store) = create_store_retrying("dpapi", "v.vault", VaultMode::Dpapi, None);
    let p = dir.join("v.vault");

    {
        assert!(store.is_unlocked());
        assert_eq!(store.mode(), VaultMode::Dpapi);
        store.set("alias-b", "value-b", "b.com", "second").unwrap();
        store.set("alias-a", "value-a", "a.com", "first").unwrap();
        store.save().unwrap();

        let mut aliases = store.aliases();
        aliases.sort();
        assert_eq!(aliases, vec!["alias-a".to_string(), "alias-b".to_string()]);
    }

    let mut reopened = VaultStore::open(&p).unwrap();
    assert!(reopened.is_unlocked(), "dpapi 模式 open 即解锁");
    // 已解锁 → unlock 幂等早退 Ok。
    reopened.unlock("whatever").unwrap();
    assert_eq!(
        reopened.get("alias-a").unwrap(),
        "value-a",
        "重开 + 幂等 unlock 后 DEK 仍可用"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// dpapi_blob 字段损坏（非法 base64）→ Corrupted 上抛（478-479）。
#[test]
fn open_with_corrupted_dpapi_blob_reports_corrupted() {
    let (dir, _) = create_store_retrying("corrupt", "v.vault", VaultMode::Dpapi, None);
    let p = dir.join("v.vault");

    let raw = std::fs::read_to_string(&p).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    v["dpapi_blob"] = serde_json::json!("!!!not-base64!!!");
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let err = VaultStore::open(&p).unwrap_err();
    assert!(
        err.to_string().contains("dpapi_blob"),
        "必须报 dpapi_blob 解码失败: {err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// argon2 模式：open 保持锁定 → get 报 Locked → 错口令 WrongPassphrase →
/// 对口令解锁后可读。
#[test]
fn argon2_store_lock_wrong_then_right_passphrase() {
    let (dir, mut store) =
        create_store_retrying("argon2", "v.vault", VaultMode::Argon2id, Some("right-pass"));
    let p = dir.join("v.vault");

    {
        store.set("db-pw", "hunter2", "db.local", "cov").unwrap();
        store.save().unwrap();
    }

    let mut reopened = VaultStore::open(&p).unwrap();
    assert!(!reopened.is_unlocked(), "argon2 模式 open 保持锁定");
    assert!(reopened.get("db-pw").is_err(), "锁定态 get 必须失败");

    reopened.unlock("wrong-pass").unwrap_err();
    assert!(!reopened.is_unlocked(), "错口令后仍锁定");
    reopened.unlock("right-pass").unwrap();
    assert_eq!(reopened.get("db-pw").unwrap(), "hunter2");

    let _ = std::fs::remove_dir_all(&dir);
}
