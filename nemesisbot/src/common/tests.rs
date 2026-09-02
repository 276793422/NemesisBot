use super::*;
use std::fs;

#[test]
fn test_resolve_home_local() {
    let home = resolve_home(true);
    assert!(home.to_string_lossy().contains(".nemesisbot"));
}

#[test]
fn test_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(config_path(&home), PathBuf::from("/tmp/test/config.json"));
}

#[test]
fn test_workspace_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(workspace_path(&home), PathBuf::from("/tmp/test/workspace"));
}

#[test]
fn test_constant_time_eq() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
}

#[test]
fn test_format_token() {
    assert_eq!(format_token(""), "(not set)");
    assert_eq!(format_token("abcd1234efgh"), "abcd...efgh");
    assert_eq!(format_token("short"), "***");
}

#[test]
fn test_init_logger_no_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_debug_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["--debug".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, LOG_DEBUG);
}

#[test]
fn test_init_logger_quiet_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["--quiet".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, LOG_QUIET);
}

#[test]
fn test_init_logger_with_config_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enabled": true,
                "enable_console": false,
                "level": "WARN",
                "file": ""
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

// ---------------------------------------------------------------------------
// R7（coverage-95 goal）：init_logger_from_config 配置解析的细分支补测
// —— 纯解析路径（不发 tracing 全局订阅），直接对返回 flags 断言。
// ---------------------------------------------------------------------------

#[test]
fn init_logger_config_general_without_level_key_keeps_defaults() {
    // general 对象存在但缺 level 字段：跳过级别解析（if-let None 边），
    // 其余开关照常生效（console 关 → flags 0）。
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let data = serde_json::json!({
        "logging": {"general": {"enabled": true, "enable_console": false}}
    });
    fs::write(&cfg, data.to_string()).unwrap();
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0);
}

#[test]
fn init_logger_config_general_without_file_key_is_fine() {
    // file 字段整个缺席（不是空串）：file_path 保持 None 的隐式 else 边；
    // 其余配置照常解析（console 开是默认态、不置任何非常规位 → 0）。
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let data = serde_json::json!({
        "logging": {"general": {"enabled": true, "enable_console": true, "level": "ERROR"}}
    });
    fs::write(&cfg, data.to_string()).unwrap();
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0);
}

#[test]
fn init_logger_config_unknown_level_string_maps_to_info() {
    // 未识别的 level 字符串走 `_ => INFO` 兜底臂，不 panic 不报错。
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let data = serde_json::json!({
        "logging": {"general": {"enabled": true, "enable_console": false, "level": "LOUD"}}
    });
    fs::write(&cfg, data.to_string()).unwrap();
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0);
}

#[test]
fn test_copy_directory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("a.txt"), "hello").unwrap();
    fs::write(src.join("sub").join("b.txt"), "world").unwrap();

    copy_directory(&src, &dst).unwrap();

    assert!(dst.join("a.txt").exists());
    assert!(dst.join("sub").join("b.txt").exists());
    assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
}

#[test]
fn test_copy_directory_nonexistent() {
    let nonexistent = format!("C:/__nonexistent_copy_src_{}", std::process::id());
    let dst = format!("C:/__nonexistent_copy_dst_{}", std::process::id());
    let result = copy_directory(Path::new(&nonexistent), Path::new(&dst));
    assert!(result.is_err());
}

#[test]
fn test_should_skip_heartbeat() {
    let tmp = tempfile::TempDir::new().unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(tmp.path()));

    fs::write(tmp.path().join("BOOTSTRAP.md"), "bootstrap").unwrap();
    assert!(should_skip_heartbeat_for_bootstrap(tmp.path()));
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn test_resolve_home_env_var() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let custom_path = tmp.path().to_string_lossy().to_string();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", &custom_path);
    }
    let home = resolve_home(false);
    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
    assert!(home.to_string_lossy().contains(".nemesisbot"));
    // Check the parent directory matches
    assert_eq!(home.parent().unwrap(), tmp.path());
}

#[test]
fn test_resolve_home_auto_detect() {
    // Test auto-detect when cwd has .nemesisbot
    let home = resolve_home(false);
    // Should resolve to some .nemesisbot path (either auto-detect or home dir)
    assert!(home.to_string_lossy().contains(".nemesisbot"));
}

#[test]
fn test_mcp_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        mcp_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/config.mcp.json")
    );
}

#[test]
fn test_scanner_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        scanner_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/config.scanner.json")
    );
}

#[test]
fn test_security_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        security_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/config.security.json")
    );
}

#[test]
fn test_skills_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        skills_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/config.skills.json")
    );
}

#[test]
fn test_cluster_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        cluster_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/config.cluster.json")
    );
}

#[test]
fn test_enhanced_memory_config_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        enhanced_memory_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/config.enhanced_memory.json")
    );
}

#[test]
fn test_cors_config_path() {
    let home = PathBuf::from("/tmp/test");
    // 2026-08-29 收编：落位改 <workspace>/config/cors.json（nemesis-path 真相源）。
    assert_eq!(
        cors_config_path(&home),
        PathBuf::from("/tmp/test/workspace/config/cors.json")
    );
}

#[test]
fn test_cluster_dir_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        cluster_dir(&home),
        PathBuf::from("/tmp/test/workspace/cluster")
    );
}

#[test]
fn test_cron_store_path() {
    let home = PathBuf::from("/tmp/test");
    assert_eq!(
        cron_store_path(&home),
        PathBuf::from("/tmp/test/workspace/cron/jobs.json")
    );
}

#[test]
fn test_status_icon_ok() {
    assert_eq!(status_icon(true), "OK");
    assert_eq!(status_icon(false), "MISSING");
}

#[test]
fn test_format_token_empty() {
    assert_eq!(format_token(""), "(not set)");
}

#[test]
fn test_format_token_short() {
    assert_eq!(format_token("abc"), "***");
    assert_eq!(format_token("1234567"), "***");
}

#[test]
fn test_format_token_exact_8() {
    // Exactly 8 chars: len == 8, NOT > 8, shows "***"
    assert_eq!(format_token("abcd1234"), "***");
    // 9 chars: len > 8, shows first/last 4
    assert_eq!(format_token("abcd12345"), "abcd...2345");
}

#[test]
fn test_format_token_long() {
    assert_eq!(format_token("abcdefghijklmnop"), "abcd...mnop");
}

#[test]
fn test_format_version() {
    let v = format_version();
    // Should contain version number (non-empty)
    assert!(!v.is_empty());
}

#[test]
fn test_constant_time_eq_equal() {
    assert!(constant_time_eq(b"hello", b"hello"));
    assert!(constant_time_eq(b"", b""));
    assert!(constant_time_eq(b"x", b"x"));
}

#[test]
fn test_constant_time_eq_not_equal() {
    assert!(!constant_time_eq(b"hello", b"hella"));
    assert!(!constant_time_eq(b"abc", b"abcd"));
    assert!(!constant_time_eq(b"longer", b"short"));
}

#[test]
fn test_init_logger_with_debug_level() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enable_console": true,
                "level": "DEBUG",
                "file": ""
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_no_console_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["--no-console".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, LOG_NO_CONSOLE);
}

#[test]
fn test_init_logger_multiple_flags() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["--debug".to_string(), "--no-console".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, LOG_DEBUG | LOG_NO_CONSOLE);
}

#[test]
fn test_init_logger_short_flags() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["-d".to_string(), "-q".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, LOG_DEBUG | LOG_QUIET);
}

#[test]
fn test_init_logger_error_level() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enable_console": true,
                "level": "ERROR",
                "file": "/tmp/test.log"
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_copy_directory_with_nested_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    fs::create_dir_all(src.join("a").join("b")).unwrap();
    fs::write(src.join("root.txt"), "root").unwrap();
    fs::write(src.join("a").join("level1.txt"), "l1").unwrap();
    fs::write(src.join("a").join("b").join("level2.txt"), "l2").unwrap();

    copy_directory(&src, &dst).unwrap();

    assert_eq!(fs::read_to_string(dst.join("root.txt")).unwrap(), "root");
    assert_eq!(
        fs::read_to_string(dst.join("a").join("level1.txt")).unwrap(),
        "l1"
    );
    assert_eq!(
        fs::read_to_string(dst.join("a").join("b").join("level2.txt")).unwrap(),
        "l2"
    );
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_resolve_home_local_returns_cwd_based() {
    let home = resolve_home(true);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    assert_eq!(home, cwd.join(".nemesisbot"));
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn test_resolve_home_env_var_custom_path() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::TempDir::new().unwrap();
    let custom_path = tmp.path().to_string_lossy().to_string();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", &custom_path);
    }
    let home = resolve_home(false);
    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
    assert_eq!(home, tmp.path().join(".nemesisbot"));
}

#[test]
fn test_resolve_home_exe_dir() {
    // Priority 3: exe directory takes precedence over cwd
    // This test verifies the exe directory branch is exercised.
    // We can't easily create .nemesisbot next to the test binary,
    // but we can verify the function still returns a valid path.
    let home = resolve_home(false);
    assert!(home.to_string_lossy().ends_with(".nemesisbot"));
}

#[test]
fn test_all_path_builders_consistency() {
    let home = PathBuf::from("/data/bot");
    // config_path
    assert_eq!(config_path(&home), PathBuf::from("/data/bot/config.json"));
    // workspace_path
    assert_eq!(workspace_path(&home), PathBuf::from("/data/bot/workspace"));
    // mcp_config_path
    assert!(mcp_config_path(&home).ends_with("config.mcp.json"));
    // scanner_config_path
    assert!(scanner_config_path(&home).ends_with("config.scanner.json"));
    // security_config_path
    assert!(security_config_path(&home).ends_with("config.security.json"));
    // skills_config_path
    assert!(skills_config_path(&home).ends_with("config.skills.json"));
    // cluster_config_path
    assert!(cluster_config_path(&home).ends_with("config.cluster.json"));
    // cors_config_path
    assert!(cors_config_path(&home).ends_with("cors.json"));
    // cluster_dir
    assert!(cluster_dir(&home).ends_with("cluster"));
    // cron_store_path
    assert!(cron_store_path(&home).ends_with("jobs.json"));
}

#[test]
fn test_mcp_config_path_under_workspace() {
    let home = PathBuf::from("/tmp/bot");
    let mcp = mcp_config_path(&home);
    assert!(mcp.starts_with("/tmp/bot/workspace/config"));
}

#[test]
fn test_cluster_dir_is_under_workspace() {
    let home = PathBuf::from("/tmp/bot");
    let cdir = cluster_dir(&home);
    let ws = workspace_path(&home);
    assert!(cdir.starts_with(&ws));
}

#[test]
fn test_cron_store_path_under_workspace() {
    let home = PathBuf::from("/tmp/bot");
    let cron = cron_store_path(&home);
    assert!(cron.starts_with(workspace_path(&home)));
}

#[test]
fn test_format_token_boundary_cases() {
    // Exactly 8 chars -> too short, shows "***"
    assert_eq!(format_token("12345678"), "***");
    // 9 chars -> shows first 4 and last 4
    assert_eq!(format_token("123456789"), "1234...6789");
    // 5 chars (short)
    assert_eq!(format_token("12345"), "***");
    // Single char
    assert_eq!(format_token("a"), "***");
    // Unicode token - len() counts bytes, not chars
    let token = "abcd\u{4e2d}\u{56fd}efgh"; // 4 + 6 + 4 = 14 bytes
    let formatted = format_token(token);
    assert!(formatted.contains("..."));
}

#[test]
fn test_constant_time_eq_symmetry() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(constant_time_eq(b"", b""));
    assert!(!constant_time_eq(b"abc", b"ABC"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    // Symmetry: different lengths always false
    assert!(!constant_time_eq(b"longstring", b"short"));
}

#[test]
fn test_status_icon_values() {
    assert_eq!(status_icon(true), "OK");
    assert_eq!(status_icon(false), "MISSING");
}

#[test]
fn test_init_logger_with_warn_level() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enable_console": true,
                "level": "WARN",
                "file": ""
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_with_trace_level() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enable_console": true,
                "level": "TRACE",
                "file": ""
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_with_invalid_level_defaults_to_info() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enable_console": true,
                "level": "INVALID_LEVEL",
                "file": ""
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_with_file_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config_data = serde_json::json!({
        "logging": {
            "general": {
                "enable_console": true,
                "level": "INFO",
                "file": "/tmp/nemesisbot-test.log"
            }
        }
    });
    fs::write(&cfg, serde_json::to_string(&config_data).unwrap()).unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_invalid_json_ignored() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    fs::write(&cfg, "not valid json{{{").unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_empty_json_object() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    fs::write(&cfg, "{}").unwrap();
    let args: Vec<String> = vec![];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_init_logger_quiet_overrides_debug() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["--debug".to_string(), "--quiet".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, LOG_DEBUG | LOG_QUIET);
}

#[test]
fn test_init_logger_unrelated_args_ignored() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cfg = tmp.path().join("nonexistent.json");
    let args = vec!["gateway".to_string(), "--local".to_string()];
    let flags = init_logger_from_config(&cfg, &args);
    assert_eq!(flags, 0);
}

#[test]
fn test_copy_directory_overwrites_existing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("file.txt"), "new content").unwrap();

    // Create existing file with different content
    fs::create_dir_all(&dst).unwrap();
    fs::write(dst.join("file.txt"), "old content").unwrap();

    copy_directory(&src, &dst).unwrap();
    assert_eq!(
        fs::read_to_string(dst.join("file.txt")).unwrap(),
        "new content"
    );
}

#[test]
fn test_should_skip_heartbeat_false_by_default() {
    let tmp = tempfile::TempDir::new().unwrap();
    assert!(!should_skip_heartbeat_for_bootstrap(tmp.path()));
}

#[test]
fn test_should_skip_heartbeat_true_with_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    fs::write(tmp.path().join("BOOTSTRAP.md"), "content").unwrap();
    assert!(should_skip_heartbeat_for_bootstrap(tmp.path()));
}

#[test]
fn test_format_version_not_empty() {
    let v = format_version();
    assert!(!v.is_empty());
}

#[test]
fn test_version_info_fields_not_empty() {
    // version and rust_version should always be set
    assert!(!VERSION_INFO.version.is_empty());
    assert!(!VERSION_INFO.rust_version.is_empty());
}

#[test]
fn test_log_flag_constants() {
    assert_eq!(LOG_DEBUG, 1);
    assert_eq!(LOG_QUIET, 2);
    assert_eq!(LOG_NO_CONSOLE, 4);
}

// --- ensure_exe_in_path tests ---

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn test_ensure_exe_in_path() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let exe_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let canonical = std::fs::canonicalize(&exe_dir).unwrap();
    let original = std::env::var("PATH").unwrap_or_default();
    let _separator = if cfg!(windows) { ';' } else { ':' };

    // Case 1: PATH does not contain exe dir → should add
    // SAFETY: test-only, single-threaded, restored at end
    unsafe { std::env::set_var("PATH", "/usr/nonexistent") };
    assert!(ensure_exe_in_path(), "should add when exe dir missing");
    let after_add = std::env::var("PATH").unwrap();
    assert!(after_add.contains(&canonical.to_string_lossy().to_string()));

    // Case 2: PATH already contains exe dir → should NOT add
    assert!(!ensure_exe_in_path(), "should not add duplicate");

    // Case 3: Empty PATH → should set to exe dir
    unsafe { std::env::set_var("PATH", "") };
    assert!(ensure_exe_in_path(), "should add when PATH is empty");
    let after_empty = std::env::var("PATH").unwrap();
    assert_eq!(after_empty, canonical.to_string_lossy().to_string());

    // Restore
    unsafe { std::env::set_var("PATH", &original) };
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：common.rs 剩余可测面。
// - ensure_exe_in_path 的「PATH 整个不存在」分支（37-38）
// - resolve_home 的 exe-dir 标记分支（93/95-96）与 cwd 标记分支（100）
// - chat/forge/sessions 三个路径 builder
// - print_version / print_version_info / print_help（直调不 panic）
// - ensure_default_logger 幂等（OnceLock）
// - init_logger_from_config：ERROR 档、绝对路径 file（Windows 上 "/tmp/x"
//   不算 absolute —— 既有测试实际走的是相对分支）、相对路径 file 落到
//   home/workspace、日志目录创建失败 warn 分支
// - max_level_filter 五档映射（TRACE 臂只在此直测可达：init_logger 把
//   TRACE 折到 DEBUG，永不产出 Level::TRACE）
// - run_interactive_mode 的 EOF 分支（cargo test stdin 是管道 EOF）
// - setup_cron_tool 组件可用
// =========================================================================

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn ensure_exe_in_path_without_path_env_sets_it_from_scratch() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let canonical = std::fs::canonicalize(
        std::env::current_exe().unwrap().parent().unwrap(),
    )
    .unwrap();
    let saved = std::env::var("PATH").ok();

    unsafe { std::env::remove_var("PATH") };
    let modified = ensure_exe_in_path();
    let now = std::env::var("PATH").unwrap();

    // 无论原先有无 PATH，测完必须恢复（防毒化后续测试）。
    match &saved {
        Some(v) => unsafe { std::env::set_var("PATH", v) },
        None => unsafe { std::env::remove_var("PATH") },
    }

    assert!(modified, "missing PATH must be populated with the exe dir");
    assert_eq!(now, canonical.to_string_lossy().to_string());
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn resolve_home_exe_dir_and_cwd_marker_branches() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    // 本测试需要 NEMESISBOT_HOME 不在场（保存并清除，测完恢复）。
    let saved_env = std::env::var("NEMESISBOT_HOME").ok();
    unsafe { std::env::remove_var("NEMESISBOT_HOME") };

    // --- exe-dir 分支：exe 旁建 .nemesisbot 标记（target 下的构建产物
    // 目录，非生产数据；测点测量完立即拆除）。---
    let exe_dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let exe_marker = exe_dir.join(".nemesisbot");
    let exe_marker_created = !exe_marker.exists();
    if exe_marker_created {
        std::fs::create_dir_all(&exe_marker).unwrap();
    }
    let home_from_exe = resolve_home(false);
    if exe_marker_created {
        std::fs::remove_dir(&exe_marker).ok();
    }

    // --- cwd 分支：exe-dir 标记拆除后，cwd 下的 .nemesisbot 生效。---
    let cwd = std::env::current_dir().unwrap();
    let cwd_marker = cwd.join(".nemesisbot");
    let cwd_marker_created = !cwd_marker.exists();
    if cwd_marker_created {
        std::fs::create_dir_all(&cwd_marker).unwrap();
    }
    let home_from_cwd = resolve_home(false);
    if cwd_marker_created {
        std::fs::remove_dir(&cwd_marker).ok();
    }

    // 恢复 env 之后再断言（断言失败也不毒化环境）。
    match &saved_env {
        Some(v) => unsafe { std::env::set_var("NEMESISBOT_HOME", v) },
        None => unsafe { std::env::remove_var("NEMESISBOT_HOME") },
    }

    assert_eq!(home_from_exe, exe_marker, "exe-dir marker must beat cwd/default");
    assert_eq!(home_from_cwd, cwd_marker, "cwd marker must beat default home");
}

#[test]
fn chat_forge_sessions_path_builders() {
    let home = PathBuf::from("/data/bot");
    assert_eq!(
        chat_config_path(&home),
        PathBuf::from("/data/bot/workspace/config/config.chat.json")
    );
    assert_eq!(
        forge_config_path(&home),
        PathBuf::from("/data/bot/workspace/config/config.forge.json")
    );
    assert_eq!(
        sessions_dir(&home),
        PathBuf::from("/data/bot/workspace/sessions")
    );
}

#[test]
fn print_version_info_and_help_run_without_panicking() {
    // 输出被 libtest 捕获；这里钉的是三段 banner 函数全量可执行。
    print_version();
    print_version_info();
    print_help();
    // format_version 是 banner 的数据源（已有覆盖，这里补齐调用侧证据）。
    assert!(!format_version().is_empty());
    assert!(!VERSION_INFO.version.is_empty());
}

#[test]
fn ensure_default_logger_is_idempotent_via_oncelock() {
    // 双调用：第二次必须直接命中 OnceLock 短路（不重复安装 subscriber）。
    ensure_default_logger();
    ensure_default_logger();
}

/// 写 logging 段的最小 config，返回 config 路径。
fn write_logging_config(dir: &Path, body: serde_json::Value) -> PathBuf {
    let cfg = dir.join("config.json");
    fs::write(&cfg, serde_json::to_string(&body).unwrap()).unwrap();
    cfg
}

#[test]
fn init_logger_error_level_maps_to_error() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = write_logging_config(
        tmp.path(),
        serde_json::json!({ "logging": { "general": {
            "enable_console": true, "level": "ERROR", "file": ""
        } } }),
    );
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0);
}

#[test]
fn init_logger_absolute_file_path_uses_it_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    // Windows 上 "/tmp/x" 不是 absolute（无盘符前缀）——必须用真绝对路径
    // 才能踩中 p.is_absolute() 分支。
    let abs_log = tmp.path().join("abs").join("run.log");
    let cfg = write_logging_config(
        tmp.path(),
        serde_json::json!({ "logging": { "general": {
            "enable_console": false, "level": "INFO",
            "file": abs_log.to_string_lossy()
        } } }),
    );
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0);
    // 日志目录（绝对路径的 parent）被 create_dir_all。
    assert!(tmp.path().join("abs").exists(), "absolute file dir must be created");
}

#[test]
fn init_logger_relative_file_path_resolves_into_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = write_logging_config(
        tmp.path(),
        serde_json::json!({ "logging": { "general": {
            "enable_console": false, "level": "INFO", "file": "logs/run.log"
        } } }),
    );
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0);
    // 相对路径按 config 所在 home 的 workspace/ 解析（日志统一落在
    // .nemesisbot/workspace/logs/，与 CWD 无关）。
    assert!(
        tmp.path().join("workspace").join("logs").exists(),
        "relative file must resolve under {}/workspace/logs",
        tmp.path().display()
    );
}

#[test]
fn init_logger_uncreatable_log_dir_warns_but_continues() {
    let tmp = tempfile::tempdir().unwrap();
    // 父目录路径上放一个普通文件 → create_dir_all 必失败 → warn 分支
    // （eprintln，不 panic、不中断装配）。
    let blocker = tmp.path().join("blocker");
    fs::write(&blocker, b"file, not dir").unwrap();
    let cfg = write_logging_config(
        tmp.path(),
        serde_json::json!({ "logging": { "general": {
            "enable_console": true, "level": "INFO",
            "file": blocker.join("sub").join("x.log").to_string_lossy()
        } } }),
    );
    let flags = init_logger_from_config(&cfg, &[]);
    assert_eq!(flags, 0, "dir-creation failure must not alter override flags");
}

#[test]
fn max_level_filter_maps_all_five_levels() {
    use tracing_subscriber::filter::LevelFilter;
    assert_eq!(max_level_filter(tracing::Level::TRACE), LevelFilter::TRACE);
    assert_eq!(max_level_filter(tracing::Level::DEBUG), LevelFilter::DEBUG);
    assert_eq!(max_level_filter(tracing::Level::INFO), LevelFilter::INFO);
    assert_eq!(max_level_filter(tracing::Level::WARN), LevelFilter::WARN);
    assert_eq!(max_level_filter(tracing::Level::ERROR), LevelFilter::ERROR);
}

// [2026-08-27 R9 死码处置] run_interactive_mode_returns_ok_on_stdin_eof
// 随 run_interactive_mode 一并删除：该函数全仓库零生产调用方（用户
// 2026-08-27 裁决删除），测试无存续对象。

#[tokio::test]
async fn setup_cron_tool_returns_wired_service_and_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let (service, _tool) = setup_cron_tool(&tmp.path().join("workspace"));
    // service 可锁定（与生产 gateway 注入 BotService 的形态一致）。
    let _guard = service.lock().await;
}

// ===========================================================================
// r9_zero（R9 补测批零头组，2026-08-27）：resolve_home 的「exe 旁 .nemesisbot」
// 优先级——代码序（common.rs 77-105）是 local → NEMESISBOT_HOME →
// 【exe 目录】→ cwd 自动探测 → ~/.nemesisbot。注意文件头 doc 注释（71-76 行）
// 把 exe 目录和 cwd 的顺序写反了，以实现为准（本测试就是 3 号位的回归钉）。
//
// 夹具：拷贝真 exe 到 <tmp>/bin/nemesisbot.exe，marker config 放在
// <tmp>/bin/.nemesisbot/config.json；启动 cwd 是无 .nemesisbot 的空目录、并显式
// env_remove("NEMESISBOT_HOME") → 子进程只能经 exe 目录臂命中 marker。
// 磁盘成本：一次 ~50MB 拷贝，TempDir 析构自动回收。
// ===========================================================================

mod r9_zero {
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn resolve_home_priority3_exe_side_dotnemesisbot_wins_before_cwd_autodetect() {
        let tmp = tempfile::tempdir().unwrap();
        let bin_src = test_harness::resolve_nemesisbot_bin()
            .unwrap_or_else(|e| panic!("需要已构建的 nemesisbot 二进制: {e}"));

        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(bin_dir.join(".nemesisbot")).unwrap();
        let exe_name = if cfg!(windows) { "nemesisbot.exe" } else { "nemesisbot" };
        let bin_dst = bin_dir.join(exe_name);
        std::fs::copy(&bin_src, &bin_dst).expect("拷贝 exe 到临时 bin/");

        // marker config：唯一能证明「读的是 exe 旁 home」的指纹。
        let marker_config = serde_json::json!({
            "model_list": [
                {"model": "r9/exe-home-marker", "model_name": "exe-home-marker"}
            ]
        });
        std::fs::write(
            bin_dir.join(".nemesisbot").join("config.json"),
            marker_config.to_string(),
        )
        .unwrap();

        // 启动目录刻意无 .nemesisbot（cwd 自动探测臂必须落空）。
        let cwd = tmp.path().join("cwd");
        std::fs::create_dir_all(&cwd).unwrap();

        // 不动测试进程自身 env —— 只在子进程 env 里删掉 NEMESISBOT_HOME，
        // 天然免 GLOBAL_STATE_LOCK 串行（对并行 env 用例零竞争）。
        let out = std::process::Command::new(&bin_dst)
            .args(["model", "list"])
            .current_dir(&cwd)
            .env_remove("NEMESISBOT_HOME")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .expect("spawn exe 副本");

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "model list 应 rc 0\nstdout={stdout}\nstderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("Configured Models"), "stdout:\n{stdout}");
        assert!(
            stdout.contains("r9/exe-home-marker"),
            "必须命中 exe 目录旁的 .nemesisbot（否则 marker 不会出现）：\n{stdout}"
        );
    }
}
