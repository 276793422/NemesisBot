//! S10b (quality-hardening goal 冲刺, web 批次 2): CORSManager error arms the
//! existing tests skip — invalid JSON load, un-creatable parent directory
//! (`create_dir_all` failure), the atomic-rename fallback (Windows read-only
//! target), and the CDN `Url::parse` failure arm in `check_origin`.

use super::*;

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
    // `create_dir_all` fails when writing the default config.
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("cors.json");
    assert!(!path.exists(), "path itself does not exist");

    let err = match CORSManager::new(&path) {
        Ok(_) => panic!("file-as-parent config path must fail"),
        Err(e) => e,
    };
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotADirectory
        ),
        "create_dir_all on a file parent fails: {:?}",
        err.kind()
    );
}

#[test]
fn rename_fallback_hits_when_destination_is_readonly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cors.json");
    // Seed a valid config file, then make it read-only so the atomic
    // rename (tmp → path) fails and save_to_file falls back to a direct
    // write (which also fails → add_origin surfaces the error).
    std::fs::write(&path, serde_json::to_string(&CORSConfig::default()).unwrap()).unwrap();
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
        // fallback direct write also fails → Err. Note: the in-memory config
        // was already mutated before the failed persist (push happens under
        // the write lock first) — only the disk write is refused.
        assert!(res.is_err(), "read-only destination surfaces write failure");
        assert!(
            mgr.list_origins().contains(&"https://fallback.com".to_string()),
            "in-memory add precedes the failed persist"
        );
        // The read-only file on disk keeps its original content (no origin).
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(!disk.contains("fallback.com"), "disk unchanged: {disk}");
    } else {
        // POSIX rename ignores file permissions (directory is writable) → Ok.
        assert!(res.is_ok(), "unix rename over read-only file succeeds: {:?}", res);
    }
    let _ = std::fs::remove_file(dir.path().join("cors.json.tmp"));
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
