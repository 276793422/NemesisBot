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
