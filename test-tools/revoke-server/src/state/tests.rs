//! state 单测：AppState 组装（密钥体系加载 + store 构造）+ now_secs。

use super::*;
use nemesis_verify::keygen::generate_hierarchy;
use nemesis_verify::{CrlEntry, RevDim};
use std::sync::atomic::{AtomicU32, Ordering};

static KEYS_SEQ: AtomicU32 = AtomicU32::new(0);

/// 唯一临时密钥文件路径（pid + 序号防并行冲突；调用方负责删除）。
fn temp_keys_path(tag: &str) -> String {
    let n = KEYS_SEQ.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir()
        .join(format!(
            "revoke_state_{tag}_{}_{}.json",
            std::process::id(),
            n
        ))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn now_secs_sane_and_monotonic() {
    let a = now_secs();
    let b = now_secs();
    // 合理区间（2023-11 之后 ~ 2100 年之前）；负值已被 max(0) 钳制
    assert!(a >= 1_700_000_000, "now_secs 过小: {a}");
    assert!(a < 4_102_444_800, "now_secs 过大: {a}");
    assert!(b >= a, "now_secs 不单调: {a} -> {b}");
}

#[test]
fn app_state_new_loads_keys_and_memory_store() {
    let path = temp_keys_path("ok");
    let h = generate_hierarchy(0, u64::MAX);
    h.save(&path).unwrap();
    let state = AppState::new(":memory:", &path, "tok-123".to_string()).unwrap();
    let _ = std::fs::remove_file(&path);
    // 密钥体系按文件加载（root/issuer 公钥 + CA 证书一致）
    assert_eq!(state.hierarchy.root_vk, h.root_vk);
    assert_eq!(state.hierarchy.issuer_vk, h.issuer_vk);
    assert_eq!(state.hierarchy.ca_cert, h.ca_cert);
    // admin token 原样持有
    assert_eq!(state.admin_token, "tok-123");
    // store 可用（内存库，独立于其他测试）
    let ver = state
        .store
        .add_revoke(CrlEntry {
            dim: RevDim::KeyFp,
            value: "fp".into(),
            revoked_at: 1,
            reason: "r".into(),
        })
        .unwrap();
    assert_eq!(ver, 2);
}

#[test]
fn app_state_new_missing_keys_file_errors() {
    let path = temp_keys_path("missing");
    // 防上轮崩溃残留
    let _ = std::fs::remove_file(&path);
    // map(|_| ()) 绕开 unwrap_err 的 T: Debug 约束（AppState 不实现 Debug）。
    let err = AppState::new(":memory:", &path, "t".to_string())
        .map(|_: std::sync::Arc<AppState>| ())
        .unwrap_err();
    // anyhow 上下文链应带上 keys file 路径信息
    let msg = format!("{err:#}");
    assert!(msg.contains("load keys file"), "错误信息应带上下文: {msg}");
}

#[test]
fn app_state_new_file_db_persists() {
    let keys = temp_keys_path("filedb");
    generate_hierarchy(0, u64::MAX).save(&keys).unwrap();
    let db = format!("{keys}.db");
    {
        let state = AppState::new(&db, &keys, "t".to_string()).unwrap();
        state
            .store
            .add_revoke(CrlEntry {
                dim: RevDim::Publisher,
                value: "p".into(),
                revoked_at: 2,
                reason: "r".into(),
            })
            .unwrap();
    } // drop state：Windows 上先释放 DB 文件锁
    let state2 = AppState::new(&db, &keys, "t".to_string()).unwrap();
    let crl = state2.store.list_crl().unwrap();
    assert_eq!(crl.version, 2);
    assert_eq!(crl.entries.len(), 1);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_file(&keys);
}
