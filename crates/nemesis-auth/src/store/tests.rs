use super::*;
use chrono::Local;

fn test_cred() -> AuthCredential {
    AuthCredential {
        access_token: "at_123".to_string(),
        refresh_token: Some("rt_456".to_string()),
        expires_at: Some(Local::now() + chrono::Duration::hours(1)),
        provider: "test".to_string(),
        auth_method: "oauth".to_string(),
        account_id: Some("acct_1".to_string()),
    }
}

#[test]
fn test_save_and_get() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("test", test_cred()).unwrap();
    let cred = store.get("test").unwrap();
    assert_eq!(cred.access_token, "at_123");
}

#[test]
fn test_remove() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("test", test_cred()).unwrap();
    store.remove("test").unwrap();
    assert!(store.get("test").is_none());
}

#[test]
fn test_list_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("p1", test_cred()).unwrap();
    store.save("p2", test_cred()).unwrap();
    let mut providers = store.list_providers();
    providers.sort();
    assert_eq!(providers, vec!["p1", "p2"]);
}

#[test]
fn test_new_with_nonexistent_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("nonexistent")
        .join("auth.json")
        .to_string_lossy()
        .to_string();
    // Should succeed even when path doesn't exist
    let store = AuthStore::new(&path);
    assert!(store.list_providers().is_empty());
}

#[test]
fn test_new_with_empty_path() {
    let store = AuthStore::new("");
    assert!(store.list_providers().is_empty());
}

#[test]
fn test_new_with_corrupted_json() {
    // 必须持有捕获 guard：本测试与 test_corrupted_store_load_warns_loudly
    // 共享 store.rs `AuthStore::new` 同一条 warn! 语句的 callsite，而 tracing
    // 的 callsite interest 缓存是进程级全局单值——若本测试（无 subscriber）
    // 在对方 set_default 与 emit 之间的窗口内先执行该 warn 语句，callsite 会
    // 在本测试线程上注册并解析到 NoSubscriber（register_callsite 返回
    // Interest::never），缓存为 NEVER 后对方的 emit 被宏层永久短路 → 假红。
    // 持 guard 后任何注册时序/全量重建都解析到 fmt subscriber，坑不可达。
    let (_, _guard) = warn_capture_setup();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{invalid json!!!}").unwrap();
    let path_str = path.to_string_lossy().to_string();
    // Load 失败时构造继续但 loud 警告（BUG-20261025-39：不再静默丢弃错误），
    // 见 test_corrupted_store_load_warns_loudly；此处钉行为面=按空存储继续。
    let store = AuthStore::new(&path_str);
    assert!(store.list_providers().is_empty());
}

#[test]
fn test_delete_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("p1", test_cred()).unwrap();
    store.save("p2", test_cred()).unwrap();
    assert_eq!(store.list_providers().len(), 2);
    store.delete_all().unwrap();
    assert!(store.list_providers().is_empty());
    assert!(store.get("p1").is_none());
    // File should be deleted
    assert!(!std::path::Path::new(&path).exists());
}

#[test]
fn test_delete_all_when_no_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    // Should succeed even when file doesn't exist
    store.delete_all().unwrap();
    assert!(store.list_providers().is_empty());
}

#[test]
fn test_persistence_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();

    // Create store, save data
    let store1 = AuthStore::new(&path);
    store1.save("myprovider", test_cred()).unwrap();

    // Create new store from same file
    let store2 = AuthStore::new(&path);
    let cred = store2.get("myprovider").unwrap();
    assert_eq!(cred.access_token, "at_123");
    assert_eq!(cred.provider, "test");
}

#[test]
fn test_get_removed_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("provider1", test_cred()).unwrap();
    assert!(store.get("provider1").is_some());
    store.remove("provider1").unwrap();
    assert!(store.get("provider1").is_none());
}

#[test]
fn test_remove_nonexistent_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    // Should succeed even for nonexistent provider
    store.remove("nonexistent").unwrap();
}

#[test]
fn test_save_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);

    let mut cred1 = test_cred();
    cred1.access_token = "token_v1".to_string();
    store.save("provider", cred1).unwrap();

    let mut cred2 = test_cred();
    cred2.access_token = "token_v2".to_string();
    store.save("provider", cred2).unwrap();

    let loaded = store.get("provider").unwrap();
    assert_eq!(loaded.access_token, "token_v2");
}

#[test]
fn test_get_nonexistent_provider() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn test_get_from_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    assert!(store.get("any").is_none());
}

#[test]
fn test_list_providers_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    assert!(store.list_providers().is_empty());
}

// ===========================================================================
// BUG-20261028-104d 红证：list_providers 消费 HashMap 迭代序——auth status
// 输出顺序逐次漂移。用户可见清单必须确定性（排序）。
// ===========================================================================
#[test]
fn test_list_providers_is_sorted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    // 8 个乱序插入：HashMap 迭代序巧合有序的概率 ≈ 1/8!，红证在此概率下必现
    for name in [
        "zeta", "alpha", "mid", "beta", "omega", "kappa", "iota", "delta",
    ] {
        store.save(name, test_cred()).unwrap();
    }
    let providers = store.list_providers();
    let mut sorted = providers.clone();
    sorted.sort();
    assert_eq!(
        providers, sorted,
        "list_providers 必须返回排序后的确定性清单"
    );
}

#[test]
fn test_list_providers_after_partial_remove() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("p1", test_cred()).unwrap();
    store.save("p2", test_cred()).unwrap();
    store.save("p3", test_cred()).unwrap();
    store.remove("p2").unwrap();
    let mut providers = store.list_providers();
    providers.sort();
    assert_eq!(providers, vec!["p1", "p3"]);
}

#[test]
fn test_save_creates_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("subdir")
        .join("nested")
        .join("auth.json")
        .to_string_lossy()
        .to_string();
    let store = AuthStore::new(&path);
    store.save("test", test_cred()).unwrap();
    assert!(store.get("test").is_some());
    // File should exist in nested dir
    assert!(std::path::Path::new(&path).exists());
}

#[test]
fn test_persistence_multiple_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();

    let store1 = AuthStore::new(&path);
    let mut cred_a = test_cred();
    cred_a.access_token = "token_a".to_string();
    let mut cred_b = test_cred();
    cred_b.access_token = "token_b".to_string();
    store1.save("provider_a", cred_a).unwrap();
    store1.save("provider_b", cred_b).unwrap();

    // Reload from file
    let store2 = AuthStore::new(&path);
    let mut providers = store2.list_providers();
    providers.sort();
    assert_eq!(providers, vec!["provider_a", "provider_b"]);
    assert_eq!(store2.get("provider_a").unwrap().access_token, "token_a");
    assert_eq!(store2.get("provider_b").unwrap().access_token, "token_b");
}

#[test]
fn test_persistence_preserves_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();

    let store1 = AuthStore::new(&path);
    let cred = AuthCredential {
        access_token: "at_abc".to_string(),
        refresh_token: Some("rt_def".to_string()),
        expires_at: Some(Local::now() + chrono::Duration::hours(2)),
        provider: "myprov".to_string(),
        auth_method: "oauth".to_string(),
        account_id: Some("acct_123".to_string()),
    };
    store1.save("myprov", cred).unwrap();

    let store2 = AuthStore::new(&path);
    let loaded = store2.get("myprov").unwrap();
    assert_eq!(loaded.access_token, "at_abc");
    assert_eq!(loaded.refresh_token.unwrap(), "rt_def");
    assert!(loaded.expires_at.is_some());
    assert_eq!(loaded.provider, "myprov");
    assert_eq!(loaded.auth_method, "oauth");
    assert_eq!(loaded.account_id.unwrap(), "acct_123");
}

#[test]
fn test_credential_without_optional_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();

    let store = AuthStore::new(&path);
    let cred = AuthCredential {
        access_token: "simple_token".to_string(),
        refresh_token: None,
        expires_at: None,
        provider: "simple".to_string(),
        auth_method: "api_key".to_string(),
        account_id: None,
    };
    store.save("simple", cred).unwrap();

    let loaded = store.get("simple").unwrap();
    assert_eq!(loaded.access_token, "simple_token");
    assert!(loaded.refresh_token.is_none());
    assert!(loaded.expires_at.is_none());
    assert!(loaded.account_id.is_none());
}

#[test]
fn test_delete_all_clears_memory_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("a", test_cred()).unwrap();
    store.save("b", test_cred()).unwrap();
    assert!(std::path::Path::new(&path).exists());

    store.delete_all().unwrap();

    assert!(store.list_providers().is_empty());
    assert!(store.get("a").is_none());
    assert!(store.get("b").is_none());
    assert!(!std::path::Path::new(&path).exists());
}

#[test]
fn test_delete_all_then_save_again() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("old", test_cred()).unwrap();
    store.delete_all().unwrap();

    // Should be able to save again after delete_all
    store.save("new", test_cred()).unwrap();
    assert!(store.get("new").is_some());
    assert!(store.get("old").is_none());
}

#[test]
fn test_remove_last_provider_leaves_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("only_one", test_cred()).unwrap();
    store.remove("only_one").unwrap();
    assert!(store.list_providers().is_empty());
}

#[test]
fn test_persistence_file_format_is_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("prov", test_cred()).unwrap();

    // Read the raw file and verify it's valid JSON
    let content = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.is_object());
    assert!(parsed.get("prov").is_some());
}

/// 路径含 NUL 字节时 create_dir_all(父目录) 确定性失败 → persist 经
/// write_file_atomic 的 mkdir 错误臂（错误前缀 "atomic write"，外层包
/// "write:"）。load 阶段 exists() 对 NUL 路径返回 false，
/// 所以 AuthStore::new 仍能成功构造。
#[test]
fn test_save_fails_when_parent_directory_cannot_be_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("bad\0dir")
        .join("auth.json")
        .to_string_lossy()
        .to_string();
    let store = AuthStore::new(&path);
    let result = store.save("prov", test_cred());
    let err = result.unwrap_err();
    assert!(err.contains("atomic write"), "{err}");
    assert!(err.contains("mkdir"), "{err}");
}

/// 空路径的 `Path::parent()` 返回 None → write_file_atomic 回落 "." 父目录
/// （mkdir 成功、临时文件建在 cwd），rename 到空路径确定性失败。
/// 错误链：外层 "write:" + "atomic write ...: rename: ..."。
#[test]
fn test_save_empty_path_skips_parent_creation_then_write_fails() {
    let store = AuthStore::new("");
    let result = store.save("prov", test_cred());
    let err = result.unwrap_err();
    assert!(err.starts_with("write"), "{err}");
    assert!(err.contains("atomic write"), "{err}");
    assert!(err.contains("rename"), "{err}");
}

// ===== BUG-20261025-39 回归锁：auth.json 落盘原子写 + 0o600 + 损坏加载 loud =====

use std::sync::{Arc, Mutex};

/// BUG-20261025-39：auth.json 存 OAuth bearer token，落盘必须 0o600
/// （unix；Windows ACL 为诚实边界，测试编译期排除）。
#[cfg(unix)]
#[test]
fn test_saved_file_permissions_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json").to_string_lossy().to_string();
    let store = AuthStore::new(&path);
    store.save("test", test_cred()).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o600,
        "auth.json 权限必须 0o600，实际 {:o}",
        mode & 0o777
    );
}

/// 日志捕获 sink（同 BUG-20261025-38 harness 形态）。DefaultGuard 必须由
/// 测试体持有——关在 setup 内会随 setup 返回卸载 thread-local subscriber，
/// 之后的 emit 全部丢失（R113 实录教训）。
fn warn_capture_setup() -> (Arc<Mutex<String>>, tracing::subscriber::DefaultGuard) {
    let buf = Arc::new(Mutex::new(String::new()));
    let sink = LogSink39(buf.clone());
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || sink.clone())
            .finish(),
    );
    (buf, guard)
}

#[derive(Clone)]
struct LogSink39(Arc<Mutex<String>>);
impl std::io::Write for LogSink39 {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().push_str(&String::from_utf8_lossy(b));
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// BUG-20261025-39：构造期加载失败必须 loud 警告（此前 `let _ = store.load()`
/// 静默——盘上文件损坏时用户只看到空 store，下次 save 覆盖即全量 token 丢失）。
#[test]
fn test_corrupted_store_load_warns_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth.json");
    std::fs::write(&path, "{torn json from a crashed write}").unwrap();
    let path_str = path.to_string_lossy().to_string();
    let (buf, _guard) = warn_capture_setup();
    let store = AuthStore::new(&path_str);
    assert!(store.list_providers().is_empty());
    let log = buf.lock().unwrap();
    assert!(
        log.contains("凭据存储加载失败"),
        "必须 loud 警告加载失败，实际日志：{log}"
    );
    assert!(
        log.contains("按空存储继续"),
        "警告必须说明按空存储继续（覆盖风险），实际日志：{log}"
    );
}
