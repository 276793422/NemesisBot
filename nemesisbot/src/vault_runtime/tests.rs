//! vault 运行时测试：安装→解析全链路（DPAPI/argon2）、缺文件 fail loud、
//! 运行中新增别名即生效。

use super::*;
use nemesis_security::vault::{VaultMode, VaultStore};
use parking_lot::Mutex;
use std::path::PathBuf;

/// 全局解析器槽位 + env 口令是进程单例——触及它们的测试互斥。
static GLOBAL_LOCK: Mutex<()> = Mutex::new(());

/// 造一个带别名的 vault（默认模式；argon2 用测试口令并同步环境变量）。
fn seed_vault(home: &Path, alias: &str, secret: &str) -> PathBuf {
    let vp = nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(home));
    std::fs::create_dir_all(vp.parent().unwrap()).unwrap();
    let mode = VaultStore::default_mode();
    if mode == VaultMode::Argon2id {
        unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "rt-test-pw") };
    }
    let mut store = VaultStore::create(
        &vp,
        mode,
        if mode == VaultMode::Argon2id {
            Some("rt-test-pw")
        } else {
            None
        },
    )
    .unwrap();
    store.set(alias, secret, "test", "").unwrap();
    store.save().unwrap();
    vp
}

/// 本文件四个用例都要额外持 crate 根的 GLOBAL_STATE_LOCK：wave_b 等 CLI
/// 分发测试在**本进程内**走 agent/run 装配路径调用
/// `vault_runtime::install(它们的临时 home)`（后装覆盖先装）。不互斥的话，
/// 本文件刚 install 的解析器会被并发换成名下无 vault 的他人路径——
/// resolve 报"vault 文件不存在"且路径根本不是自己的（2026-09-21 全量
/// 回归 3/3 失败的根因；solo/串行恒绿假象即来自缺省并行度差异）。
//
/// 全链路：install 注册全局解析器 → 引用解析出真值。
#[test]
fn install_and_resolve_roundtrip() {
    let _root = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _g = GLOBAL_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    seed_vault(tmp.path(), "rt-alias", "rt-secret-value");
    install(tmp.path());
    assert!(nemesis_config::global_vault_resolver().is_some());

    let got = nemesis_config::resolve_vault_reference("vault:rt-alias")
        .unwrap()
        .unwrap();
    assert_eq!(got, "rt-secret-value");
}

/// vault 文件缺失：fail loud 带创建指引，绝不降级为空值/字面量。
#[test]
fn missing_vault_fails_loud() {
    let _root = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _g = GLOBAL_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    install(tmp.path());
    let err = nemesis_config::resolve_vault_reference("vault:absent")
        .unwrap()
        .unwrap_err();
    assert!(err.contains("不存在"), "got: {err}");
    assert!(err.contains("vault set"), "got: {err}");
}

/// 运行中经 CLI 新增别名：下一次解析即生效（无缓存语义）。
#[cfg(windows)]
#[test]
fn alias_added_after_install_visible() {
    let _root = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _g = GLOBAL_LOCK.lock();
    let tmp = tempfile::tempdir().unwrap();
    seed_vault(tmp.path(), "first-alias", "v1");
    install(tmp.path());
    assert_eq!(
        nemesis_config::resolve_vault_reference("vault:first-alias")
            .unwrap()
            .unwrap(),
        "v1"
    );

    // 模拟另一进程（CLI）追加别名后，本进程现开文件读到新别名。
    let vp =
        nemesis_path::resolve_vault_path_in_workspace(&crate::common::workspace_path(tmp.path()));
    let mut store = VaultStore::open(&vp).unwrap();
    store.set("late-alias", "late-value", "", "").unwrap();
    store.save().unwrap();

    assert_eq!(
        nemesis_config::resolve_vault_reference("vault:late-alias")
            .unwrap()
            .unwrap(),
        "late-value"
    );
}

/// argon2 模式无口令：报锁定而非 panic/空值。
#[test]
fn argon2_locked_reports_lock() {
    let _root = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _g = GLOBAL_LOCK.lock();
    if VaultStore::default_mode() == VaultMode::Dpapi {
        // DPAPI 平台没有锁定态，本用例只对 argon2 默认平台有意义。
        return;
    }
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
    let tmp = tempfile::tempdir().unwrap();
    seed_vault(tmp.path(), "locked-alias", "v");
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
    install(tmp.path());
    let err = nemesis_config::resolve_vault_reference("vault:locked-alias")
        .unwrap()
        .unwrap_err();
    assert!(err.contains("NEMESISBOT_VAULT_PASSPHRASE"), "got: {err}");
    unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", "rt-test-pw") };
}
