use super::*;
use crate::{Config, ExecutorSeparationConfig};

fn tmp_store() -> (tempfile::TempDir, ConfigStore) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&Config::default()).unwrap(),
    )
    .unwrap();
    let store = ConfigStore::load(&path).unwrap();
    (dir, store)
}

fn exec_cfg(enabled: bool, sandbox: bool) -> ExecutorSeparationConfig {
    // P5-2 加了 strict 字段；`..Default::default()` 让本 helper 不随字段
    // 增减再碎一次（默认 strict=false = 现状语义）。
    ExecutorSeparationConfig {
        enabled,
        sandbox,
        allow_network: false,
        ..Default::default()
    }
}

/// D4（DACL 定向档）：serde 双路径陷阱回归钉——executor 段无 acl 键 /
/// executor 段整个缺失时，两条构造路径都必须收敛到同一组默认值
/// （max_files=20000 而非 derive 的 0；backend="auto" 而非 derive 的 ""）。
/// 假红史：hook_flips 真进程测试抓出 max_files=0 → 铺树预算耗尽 →
/// DACL 档静默降级（2026-09-27，手写 Default 修复）。
#[test]
fn executor_acl_defaults_converge_across_serde_and_default_paths() {
    // 路径 1：结构体级 Default::default()（serde 嵌套补齐 / unwrap_or_default 走这条）。
    let d = ExecutorSeparationConfig::default();
    assert_eq!(d.backend, "auto", "derive Default 会给空串——选型拒绝值");
    assert_eq!(
        d.acl.max_files, 20_000,
        "derive Default 会给 0——铺树预算耗尽"
    );
    assert!(!d.acl.dacl && !d.acl.strict);

    // 路径 2：serde 反序列化，executor 段存在但无 acl/backend 键。
    let e: ExecutorSeparationConfig =
        serde_json::from_str(r#"{ "enabled": true }"#).expect("deserialize");
    assert_eq!(e.backend, "auto");
    assert_eq!(e.acl.max_files, 20_000);
    assert!(!e.acl.dacl);

    // 路径 3：显式 acl 键覆盖仍正常（字段级 serde default 在显式嵌套对象内生效）。
    let e2: ExecutorSeparationConfig =
        serde_json::from_str(r#"{ "enabled": true, "acl": { "dacl": true } }"#)
            .expect("deserialize");
    assert!(e2.acl.dacl);
    assert_eq!(
        e2.acl.max_files, 20_000,
        "显式 acl 对象缺 max_files → 字段级 default"
    );
}

#[test]
fn handle_sees_update_live() {
    let (_dir, store) = tmp_store();
    let h1 = store.handle();
    let h2 = store.handle();
    store
        .update(|c| c.executor = Some(exec_cfg(true, true)))
        .unwrap();
    // Both handles see the new value immediately (shared Arc<RwLock>).
    assert!(h1.read().executor.as_ref().unwrap().sandbox);
    assert!(h2.read().executor.as_ref().unwrap().sandbox);
}

#[test]
fn update_persists_to_disk() {
    let (dir, store) = tmp_store();
    store
        .update(|c| c.executor = Some(exec_cfg(false, false)))
        .unwrap();
    // Brand-new store off the same file must see the persisted write.
    let store2 = ConfigStore::load(&dir.path().join("config.json")).unwrap();
    let e = store2.handle().read().executor.clone().unwrap();
    assert!(!e.enabled && !e.sandbox);
}

#[test]
fn reload_picks_up_external_disk_change() {
    let (dir, store) = tmp_store();
    // External edit (simulating CLI / text editor writing the file).
    let mut cfg = Config::default();
    cfg.executor = Some(exec_cfg(true, false));
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
    store.reload().unwrap();
    let e = store.handle().read().executor.clone().unwrap();
    assert!(e.enabled && !e.sandbox);
}

#[test]
fn handle_clone_is_cheap_and_shared() {
    let (_dir, store) = tmp_store();
    let h = store.handle();
    // Many clones all observe the same live state.
    let clones: Vec<_> = (0..50).map(|_| h.clone()).collect();
    store
        .update(|c| c.executor = Some(exec_cfg(true, true)))
        .unwrap();
    assert!(
        clones
            .iter()
            .all(|c| c.read().executor.as_ref().unwrap().sandbox)
    );
}

#[test]
fn store_path_returns_backing_file() {
    let (dir, store) = tmp_store();
    assert_eq!(store.path(), dir.path().join("config.json").as_path());
}

// The process-wide singleton is a OnceLock: only the FIRST set_global takes
// effect. This is therefore the ONLY test in the crate that may call
// set_global — everything the singleton surface needs (set_global / global /
// load_live / save_live) is verified here in one shot. No other crate test
// reads global()/load_live()/save_live (grepped), so the lingering global
// (pointing at this test's tempdir) cannot poison parallel runs.
#[test]
fn global_singleton_set_get_load_live_save_live() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&Config::default()).unwrap(),
    )
    .unwrap();

    let store = std::sync::Arc::new(ConfigStore::load(&path).unwrap());
    set_global(store.clone());

    // global() hands back the SAME store (Arc identity), exposing its path.
    let g = global().expect("global() Some after set_global");
    assert!(std::sync::Arc::ptr_eq(&g, &store));
    assert_eq!(g.path(), path.as_path());

    // load_live reads through the global store.
    let live = load_live().expect("load_live Some after set_global");
    assert_eq!(live.gateway.port, Config::default().gateway.port);

    // save_live replaces the config AND persists to the backing file.
    let mut new_cfg = Config::default();
    new_cfg.gateway.port = 12345;
    let res = save_live(new_cfg);
    assert!(matches!(res, Some(Ok(()))), "save_live Some(Ok): {res:?}");
    assert_eq!(load_live().unwrap().gateway.port, 12345);

    // Persisted: a fresh load off the same file sees the write.
    let reloaded = crate::load_config(&path).unwrap();
    assert_eq!(reloaded.gateway.port, 12345);
}

// ---------------------------------------------------------------------------
// P24（2026-09-25 能力扩展 WS1）：executor.backend 往返与兼容
// ---------------------------------------------------------------------------

/// typed save round-trip：`executor.backend` 显式值经 store.update 落盘后，
/// 全新 load 读回不丢（同款回归锁先例：per-model untyped 键 round-trip）。
#[test]
fn executor_backend_roundtrip_preserves_explicit_value() {
    let (dir, store) = tmp_store();
    store
        .update(|c| {
            c.executor = Some(ExecutorSeparationConfig {
                backend: "acl".to_string(),
                ..Default::default()
            })
        })
        .unwrap();

    let store2 = ConfigStore::load(&dir.path().join("config.json")).unwrap();
    let e = store2.handle().read().executor.clone().unwrap();
    assert_eq!(e.backend, "acl", "显式 backend=acl 落盘后读回不丢");
}

/// 缺键兼容：老 config.json（无 `executor.backend` 键）→ 反序列化 = "auto"，
/// 且 typed save 把缺省值**显式写回**（不新增非法形态，语义与升级前一致）。
#[test]
fn executor_backend_defaults_to_auto_for_legacy_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, r#"{ "executor": { "enabled": true } }"#).unwrap();

    let store = ConfigStore::load(&path).unwrap();
    let e = store.handle().read().executor.clone().unwrap();
    assert_eq!(e.backend, "auto", "缺键 = auto（serde default fn）");
    assert!(e.enabled);

    // typed save（store.update 触发持久化）写回后仍可加载、值稳定。
    store
        .update(|c| {
            c.executor.as_mut().unwrap().sandbox = true;
        })
        .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        v["executor"]["backend"].as_str(),
        Some("auto"),
        "typed save 显式写回 backend 键"
    );
}
