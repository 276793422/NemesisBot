    use super::*;
    use crate::{Config, ExecutorSeparationConfig};

    fn tmp_store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, serde_json::to_string_pretty(&Config::default()).unwrap()).unwrap();
        let store = ConfigStore::load(&path).unwrap();
        (dir, store)
    }

    fn exec_cfg(enabled: bool, sandbox: bool) -> ExecutorSeparationConfig {
        ExecutorSeparationConfig { enabled, sandbox }
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
        store.update(|c| c.executor = Some(exec_cfg(true, true))).unwrap();
        assert!(clones.iter().all(|c| c.read().executor.as_ref().unwrap().sandbox));
    }
