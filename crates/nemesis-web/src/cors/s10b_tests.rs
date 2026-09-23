//! S10b (quality-hardening goal 冲刺, web 批次 2): CORSManager error arms the
//! existing tests skip — invalid JSON load, un-creatable parent directory
//! (`create_dir_all` failure), the honest rename failure on a read-only
//! destination, and the CDN `Url::parse` failure arm in `check_origin`.

use super::*;

/// 目录内不应残留任何 `.tmp-` 前缀的临时文件（helper 唯一临时名前缀）。
fn assert_no_tmp_leftovers(dir: &std::path::Path, what: &str) {
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{what}: {leftovers:?}");
}

#[test]
fn load_invalid_json_reports_invalid_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    std::fs::write(&path, "{not json").unwrap();

    let err = match CORSManager::new(&path) {
        Ok(_) => panic!("garbage JSON must fail to load"),
        Err(e) => e,
    };
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn new_with_file_as_parent_dir_fails_create_dir_all() {
    // A regular FILE sits where the config's parent directory should be →
    // the atomic helper's internal `create_dir_all` fails when writing the
    // default config (REL-002 后错误经 helper 步骤标签包装，kind 归一为
    // Other，断言改查步骤标签与路径上下文——比裸 kind 更有语义)。
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("cors.json");
    assert!(!path.exists(), "path itself does not exist");

    let err = match CORSManager::new(&path) {
        Ok(_) => panic!("file-as-parent config path must fail"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("atomic write") && msg.contains("mkdir"),
        "error must carry the helper step label: {msg}"
    );
}

#[test]
fn rename_fails_honestly_when_destination_is_readonly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    // Seed a valid config file, then make it read-only so the atomic
    // rename (tmp → path) fails and save_to_file surfaces the error.
    // REL-002 注：旧实现此时会回退为裸写（同失败，但回退语义会把原子性
    // 悄悄降级）；helper 统一后无回退臂，失败即诚实报错。
    std::fs::write(
        &path,
        serde_json::to_string(&CORSConfig::default()).unwrap(),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).unwrap();

    let mgr = CORSManager::new(&path).unwrap();
    let res = mgr.add_origin("https://fallback.com");

    // Restore writability so tempdir cleanup is clean on Windows.
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).unwrap();

    if cfg!(windows) {
        // rename fails (ERROR_ACCESS_DENIED on read-only destination) →
        // Err directly from the helper（无回退臂）。Note: the in-memory
        // config was already mutated before the failed persist (push
        // happens under the write lock first) — only the disk write is
        // refused.
        assert!(res.is_err(), "read-only destination surfaces write failure");
        assert!(
            mgr.list_origins()
                .contains(&"https://fallback.com".to_string()),
            "in-memory add precedes the failed persist"
        );
        // The read-only file on disk keeps its original content (no origin).
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("fallback.com"), "disk unchanged: {disk}");
    } else {
        // POSIX rename ignores file permissions (directory is writable) → Ok.
        assert!(
            res.is_ok(),
            "unix rename over read-only file succeeds: {:?}",
            res
        );
    }
    assert_no_tmp_leftovers(dir.path(), "failed persist must not leave tmp");
}

#[test]
fn check_origin_cdn_parse_failure_arm_denies() {
    // allowed_cdn_domains is set, but the origin is not a parseable URL →
    // the `Url::parse` arm is skipped and the origin is denied.
    let mgr = CORSManager {
        config: RwLock::new(CORSConfig {
            allow_localhost: false,
            allowed_cdn_domains: vec!["cdn.example.com".into()],
            ..CORSConfig::default()
        }),
        config_path: PathBuf::from("unused"),
    };
    assert!(!mgr.check_origin(":::: not a url ::::"));
    // Sanity: a proper CDN subdomain still passes through the same loop.
    assert!(mgr.check_origin("https://abc.cdn.example.com"));
    assert!(!mgr.check_origin("https://fake-cdn.example.com.evil.com"));
}
