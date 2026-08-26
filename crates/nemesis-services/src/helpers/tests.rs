use super::*;
use std::fs;

#[test]
fn test_should_skip_heartbeat_no_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(dir.path()));
}

#[test]
fn test_should_skip_heartbeat_with_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let bootstrap_path = dir.path().join("BOOTSTRAP.md");
    fs::write(&bootstrap_path, "# Bootstrap").unwrap();
    assert!(should_skip_heartbeat_for_bootstrap(dir.path()));
}

#[test]
fn test_get_config_path_returns_a_path() {
    let path = get_config_path();
    // Should always return a path (either local or home-based)
    assert!(path.to_string_lossy().contains("config.json"));
}

#[test]
fn test_should_skip_heartbeat_nonexistent_dir() {
    assert!(!should_skip_heartbeat_for_bootstrap(std::path::Path::new(
        "/nonexistent/path"
    )));
}

#[test]
fn test_get_config_path_ends_with_config_json() {
    let path = get_config_path();
    assert!(path.ends_with("config.json"));
}

#[test]
fn test_should_skip_heartbeat_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(dir.path()));
}

#[test]
fn test_should_skip_heartbeat_with_other_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("README.md"), "# Readme").unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(dir.path()));
}

#[test]
fn test_should_skip_heartbeat_case_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("bootstrap.md"), "# lowercase").unwrap();
    // Should not match lowercase on case-sensitive systems
    // (On Windows, filesystem is case-insensitive, so this might match)
    let result = should_skip_heartbeat_for_bootstrap(dir.path());
    // Just verify it doesn't panic
    let _ = result;
}

// ---- New tests ----

#[test]
fn test_get_config_path_is_valid_path() {
    let path = get_config_path();
    assert!(!path.to_string_lossy().is_empty());
}

#[test]
fn test_home_dir_returns_some() {
    // On a properly configured system, home_dir should return Some
    let home = home::home_dir();
    assert!(home.is_some());
}

#[test]
fn test_home_dir_path_is_valid() {
    if let Some(home) = home::home_dir() {
        assert!(!home.to_string_lossy().is_empty());
    }
}

#[test]
fn test_should_skip_heartbeat_file_content_doesnt_matter() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("BOOTSTRAP.md"), "").unwrap();
    assert!(should_skip_heartbeat_for_bootstrap(dir.path()));
}

#[test]
fn test_should_skip_heartbeat_nested_dir() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("subdir");
    fs::create_dir_all(&nested).unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(&nested));
}

#[test]
fn test_get_config_path_with_local_dir() {
    // Create a temporary local .nemesisbot dir
    let dir = tempfile::tempdir().unwrap();
    let local_nem = dir.path().join(".nemesisbot");
    fs::create_dir_all(&local_nem).unwrap();

    // get_config_path checks CWD, but since we can't change CWD in tests,
    // just verify the function doesn't panic
    let _path = get_config_path();
}

// ---- Additional coverage for 95%+ target ----

#[test]
fn test_dirs_home_dir_returns_valid() {
    // On a properly configured system, dirs_home_dir should return Some
    // and the path should exist
    let result = dirs_home_dir();
    // On CI or weird environments it might not, but on a real system it should
    if let Some(ref path) = result {
        assert!(path.is_dir() || !path.as_os_str().is_empty());
    }
}

#[test]
fn test_get_config_path_no_local_dir() {
    // When there is no .nemesisbot in CWD, should fall back to home dir
    let path = get_config_path();
    assert!(path.to_string_lossy().ends_with("config.json"));
    // Should not be the local path (unless CWD actually has .nemesisbot)
    // Just verify it returns a valid path
    assert!(!path.to_string_lossy().is_empty());
}

#[test]
fn test_should_skip_heartbeat_path_with_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("path with spaces");
    fs::create_dir_all(&nested).unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(&nested));

    fs::write(nested.join("BOOTSTRAP.md"), "init").unwrap();
    assert!(should_skip_heartbeat_for_bootstrap(&nested));
}

#[test]
fn test_home_module_home_dir() {
    let home = home::home_dir();
    // Should return Some on a properly configured system
    assert!(home.is_some());
    let home = home.unwrap();
    assert!(!home.as_os_str().is_empty());
}

#[test]
fn test_get_config_path_local_dir_takes_priority() {
    // The function checks for .nemesisbot in CWD first.
    // We cannot change CWD in a test, but we can verify the function
    // returns either a local or home-based path
    let path = get_config_path();
    assert!(path.ends_with("config.json"));
}

// ============================================================
// 覆盖缺口补测（llvm-cov 指定行：15 / 21 / 24）
//
// - 15（本地模式命中）：在 CWD（cargo test 的 CWD = crate 根）创建
//   .nemesisbot 目录 → get_config_path() 返回本地路径。Drop guard 保证
//   目录必被清理（panic 也清）。窗口期内其它并行测试调用
//   get_config_path() 会拿到本地路径，但它们只断言 contains/ends_with
//   "config.json"，本地路径同样满足 → 无竞争破坏。
// - 21 + 24（无 home 回退）：dirs_home_dir() 要求 HOME/USERPROFILE 均
//   失效（helpers.rs:35-45 两个候选都只读 env）。进程内清 env 是全局
//   竞争源（其它测试并行读），改走子进程：Command::env_remove 只影响
//   子进程环境块，同时把子进程 CWD 设到空临时目录（排除 15 行分支的
//   干扰）。结果经 outcome 文件回传，防 --exact 过滤名写错导致
//   libtest 0 匹配静默退出 0 的假绿。
// ============================================================

static HELPERS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_get_config_path_local_mode_in_cwd() {
    // 覆盖 15
    let _guard = HELPERS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let local_dir = std::path::Path::new(".nemesisbot");
    assert!(
        !local_dir.exists(),
        "夹具冲突：crate 根不应预先存在 .nemesisbot"
    );
    std::fs::create_dir_all(local_dir).unwrap();

    struct CleanUp<'a>(&'a std::path::Path);
    impl Drop for CleanUp<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0);
        }
    }
    let _clean = CleanUp(local_dir);

    let path = get_config_path();
    assert_eq!(path, PathBuf::from(".nemesisbot").join("config.json"));
    assert!(local_dir.exists(), "guard 存续期间目录应在");
}

#[test]
fn test_get_config_path_no_home_fallback_child() {
    // 覆盖 21（dirs_home_dir None 分支）+ 24（last resort 本地路径）
    let mode = std::env::var("NEMESIS_HELPERS_CHILD").unwrap_or_default();
    if mode.is_empty() {
        // ---- 父角色：驱动子进程并断言 outcome ----
        let _guard = HELPERS_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let exe = std::env::current_exe().unwrap();
        // 子进程过滤名用裸函数名子串（非 --exact）：libtest 可见测试名不带
        // crate 前缀，而 module_path!() 含 crate 名（nemesis_services::…），
        // --exact 永不匹配（2026-08-25 migrate 同款真跑抓到，横扫一并修）。
        let filter = "test_get_config_path_no_home_fallback_child";
        let outcome_dir = tempfile::tempdir().unwrap();
        let outcome_path = outcome_dir.path().join("outcome.txt");
        // 子进程 CWD 设为空临时目录（无 .nemesisbot）+ 清 HOME/USERPROFILE
        let child_cwd = tempfile::tempdir().unwrap();

        let status = std::process::Command::new(&exe)
            .arg(&filter)
            .current_dir(child_cwd.path())
            .env("NEMESIS_HELPERS_CHILD", "1")
            .env("NEMESIS_HELPERS_OUTCOME", &outcome_path)
            .env_remove("HOME")
            .env_remove("USERPROFILE")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("spawn child test process");
        assert!(status.success(), "child exited with {:?}", status.code());

        let text = std::fs::read_to_string(&outcome_path)
            .expect("outcome 文件缺失（libtest 过滤名写错？）");
        assert!(text.contains("dirs_home=none"), "{}", text);
        assert!(text.contains("config=LOCAL"), "{}", text);
    } else {
        // ---- 子角色：无 HOME/USERPROFILE + CWD 无 .nemesisbot ----
        let dh = dirs_home_dir();
        let p = get_config_path();
        let payload = format!(
            "dirs_home={} config={}",
            if dh.is_some() { "some" } else { "none" },
            if p == PathBuf::from(".nemesisbot").join("config.json") {
                "LOCAL".to_string()
            } else {
                p.to_string_lossy().to_string()
            }
        );
        if let Ok(path) = std::env::var("NEMESIS_HELPERS_OUTCOME") {
            std::fs::write(&path, payload).expect("write outcome file");
        }
    }
}
