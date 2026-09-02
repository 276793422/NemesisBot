use super::*;
use tempfile::TempDir;

fn make_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({
        "logging": default_logging_config()
    });
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg
}

#[test]
fn test_default_logging_config_structure() {
    let cfg = default_logging_config();
    assert_eq!(cfg["llm"]["enabled"], false);
    assert_eq!(cfg["llm"]["detail_level"], "full");
    assert_eq!(cfg["general"]["enabled"], true);
    assert_eq!(cfg["general"]["level"], "INFO");
    assert_eq!(cfg["general"]["console"], true);
}

#[test]
fn test_expand_tilde_home() {
    let expanded = expand_tilde("~/test/path");
    assert!(!expanded.starts_with('~'));
    assert!(expanded.contains("test") || expanded.contains("path"));
}

#[test]
fn test_expand_tilde_root() {
    let expanded = expand_tilde("~");
    assert!(!expanded.starts_with('~') || dirs::home_dir().is_none());
}

#[test]
fn test_expand_tilde_no_tilde() {
    let expanded = expand_tilde("/absolute/path");
    assert_eq!(expanded, "/absolute/path");
}

#[test]
fn test_expand_tilde_backslash() {
    let expanded = expand_tilde("~\\test");
    // Should expand on Windows
    assert!(!expanded.starts_with('~') || dirs::home_dir().is_none());
}

#[test]
fn test_resolve_path_absolute() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_path("/absolute/path", tmp.path());
    // On Windows, /absolute/path becomes C:/absolute/path
    assert!(resolved.contains("absolute"));
    assert!(resolved.contains("path"));
}

#[test]
fn test_resolve_path_relative() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_path("relative/path", tmp.path());
    assert!(resolved.starts_with(&tmp.path().to_string_lossy().to_string()));
    assert!(resolved.contains("relative"));
}

#[test]
fn test_resolve_path_tilde() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_path("~/logs", tmp.path());
    assert!(!resolved.starts_with('~') || dirs::home_dir().is_none());
}

#[test]
fn test_read_logging_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("nonexistent.json");
    let config = read_logging_config(&cfg_path).unwrap();
    assert_eq!(config["llm"]["enabled"], false);
}

#[test]
fn test_read_logging_config_with_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let config = read_logging_config(&cfg).unwrap();
    assert_eq!(config["general"]["level"], "INFO");
}

#[test]
fn test_read_logging_config_file_without_logging() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, r#"{"other": "data"}"#).unwrap();
    let config = read_logging_config(&cfg).unwrap();
    // Should return default
    assert_eq!(config["llm"]["enabled"], false);
}

#[test]
fn test_write_logging_config_creates_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let logging = default_logging_config();

    write_logging_config(&cfg, &logging).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data.get("logging").is_some());
}

#[test]
fn test_write_logging_config_preserves_other_fields() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, r#"{"other": "data", "version": "1.0"}"#).unwrap();

    let logging = default_logging_config();
    write_logging_config(&cfg, &logging).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["other"], "data");
    assert_eq!(data["version"], "1.0");
    assert!(data.get("logging").is_some());
}

#[test]
fn test_cmd_llm_enable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_enable(&cfg, &workspace).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], true);
}

#[test]
fn test_cmd_llm_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_llm_disable(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], false);
}

#[test]
fn test_cmd_general_enable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // First disable
    cmd_general_disable(&cfg).unwrap();
    // Then enable
    cmd_general_enable(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enabled"], true);
}

#[test]
fn test_cmd_general_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_disable(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enabled"], false);
}

#[test]
fn test_cmd_general_level_valid() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_level(&cfg, "DEBUG").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "DEBUG");
}

#[test]
fn test_cmd_general_level_case_insensitive() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_level(&cfg, "warn").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "WARN");
}

#[test]
fn test_cmd_general_level_invalid() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_level(&cfg, "INVALID").unwrap();

    // Level should remain unchanged
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "INFO");
}

#[test]
fn test_cmd_general_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_file(&cfg, "/tmp/test.log").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["file"], "/tmp/test.log");
}

#[test]
fn test_cmd_general_console_toggle() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Default is true, toggle should set to false
    cmd_general_console(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], false);

    // Toggle again should set to true
    cmd_general_console(&cfg).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], true);
}

#[test]
fn test_cmd_llm_status() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_status(&cfg, &workspace).unwrap();
}

#[test]
fn test_cmd_general_status() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    cmd_general_status(&cfg).unwrap();
}

#[test]
fn test_cmd_all_status() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_all_status(&cfg, &workspace).unwrap();
}

#[test]
fn test_cmd_llm_config_detail_level() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_config(&cfg, &workspace, Some("truncated"), None).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["detail_level"], "truncated");
}

#[test]
fn test_cmd_llm_config_log_dir() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_config(&cfg, &workspace, None, Some("my-logs")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let log_dir = data["logging"]["llm"]["log_dir"].as_str().unwrap();
    assert!(log_dir.contains("my-logs"));
}

#[test]
fn test_cmd_llm_config_no_changes() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");

    cmd_llm_config(&cfg, &workspace, None, None).unwrap();
    // Should succeed with no changes
}

#[test]
fn test_llm_enable_no_existing_section() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, r#"{"other": true}"#).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_enable(&cfg, &workspace).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], true);
}

// -------------------------------------------------------------------------
// Additional log tests for coverage
// -------------------------------------------------------------------------

#[test]
fn test_expand_tilde_no_home_dir() {
    // Just verify it returns the original path for non-tilde paths
    let result = expand_tilde("/absolute/path");
    assert_eq!(result, "/absolute/path");

    let result = expand_tilde("relative/path");
    assert_eq!(result, "relative/path");
}

#[test]
fn test_resolve_path_various() {
    let tmp = TempDir::new().unwrap();

    // Absolute path
    let result = resolve_path("/abs/path", tmp.path());
    assert!(result.contains("abs"));

    // Relative path
    let result = resolve_path("logs/test", tmp.path());
    assert!(result.starts_with(&tmp.path().to_string_lossy().to_string()));

    // Tilde path
    let result = resolve_path("~/my-logs", tmp.path());
    assert!(!result.starts_with('~') || dirs::home_dir().is_none());
}

#[test]
fn test_default_logging_config_completeness() {
    let cfg = default_logging_config();
    // LLM section
    assert!(cfg.get("llm").is_some());
    assert_eq!(cfg["llm"]["enabled"], false);
    assert_eq!(cfg["llm"]["detail_level"], "full");
    assert_eq!(cfg["llm"]["log_dir"], "logs/request_logs");

    // General section
    assert!(cfg.get("general").is_some());
    assert_eq!(cfg["general"]["enabled"], true);
    assert_eq!(cfg["general"]["level"], "INFO");
    assert_eq!(cfg["general"]["console"], true);
    assert_eq!(cfg["general"]["enable_console"], true);
}

#[test]
fn test_write_logging_config_to_new_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("subdir").join("config.json");
    let logging = default_logging_config();

    write_logging_config(&cfg, &logging).unwrap();

    // Directory should be created
    assert!(cfg.parent().unwrap().exists());
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data.get("logging").is_some());
}

#[test]
fn test_cmd_llm_enable_with_empty_log_dir() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({
        "logging": {
            "llm": {
                "enabled": false,
                "detail_level": "",
                "log_dir": ""
            }
        }
    });
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_enable(&cfg, &workspace).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], true);
    // Empty fields should be filled with defaults
    assert_eq!(data["logging"]["llm"]["log_dir"], "logs/request_logs");
    assert_eq!(data["logging"]["llm"]["detail_level"], "full");
}

#[test]
fn test_cmd_general_level_various_valid_levels() {
    for level in &["DEBUG", "INFO", "WARN", "ERROR", "FATAL", "TRACE"] {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config = serde_json::json!({"logging": default_logging_config()});
        std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

        cmd_general_level(&cfg, level).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["level"], *level);
    }
}

#[test]
fn test_cmd_general_level_lowercase_input() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": default_logging_config()});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

    cmd_general_level(&cfg, "error").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "ERROR");
}

#[test]
fn test_cmd_general_console_multiple_toggles() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": default_logging_config()});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

    // Toggle false
    cmd_general_console(&cfg).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], false);

    // Toggle back to true
    cmd_general_console(&cfg).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], true);
}

#[test]
fn test_cmd_all_status_no_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_all_status(&cfg, &workspace).unwrap();
}

#[test]
fn test_cmd_general_status_no_general_section() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": {}});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

    cmd_general_status(&cfg).unwrap();
}

#[test]
fn test_read_logging_config_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, "invalid json {{{").unwrap();

    let result = read_logging_config(&cfg);
    assert!(result.is_err());
}

#[test]
fn test_cmd_llm_config_both_options() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": default_logging_config()});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_config(&cfg, &workspace, Some("truncated"), Some("custom-logs")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["detail_level"], "truncated");
    assert!(
        data["logging"]["llm"]["log_dir"]
            .as_str()
            .unwrap()
            .contains("custom-logs")
    );
}

// ===========================================================================
// run() 全臂 + cmd_llm_status 深分支（S11c，quality-hardening goal 冲刺 S11）
// —— 既有 41 个测试只直调 helper/cmd_*，dispatch 层（log.rs:588-682）从没
// 跑过；cmd_llm_status 的 enabled+目录列表分支（304-335）和 llm 缺失 else
// 分支（337-340）也没到过。env home 隔离 + GLOBAL_STATE_LOCK 串行。
// 注意：cmd_llm_config 传非法 detail_level 会 std::process::exit(1)（370 行）
// 直接杀死测试进程——绝不测非法值。
// ===========================================================================

mod run_arm {
    use super::*;

    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    fn seed_home(home: &std::path::Path) {
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join("config.json"), "{}").unwrap();
    }

    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    fn read_cfg(home: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
    }

    // --- Llm 子树 ---

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn llm_subtree_dispatch_enable_disable_status_config_type() {
        with_env_home(|home| {
            seed_home(&home);

            run(
                LogAction::Llm {
                    action: LlmAction::Enable,
                },
                false,
            )
            .expect("llm enable ok");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["enabled"], true);

            run(
                LogAction::Llm {
                    action: LlmAction::Status,
                },
                false,
            )
            .expect("llm status ok");

            run(
                LogAction::Llm {
                    action: LlmAction::Config {
                        detail_level: Some("truncated".into()),
                        log_dir: Some("req-logs".into()),
                    },
                },
                false,
            )
            .expect("llm config ok（合法 detail_level）");
            let llm = &read_cfg(&home)["logging"]["llm"];
            assert_eq!(llm["detail_level"], "truncated");
            assert!(llm["log_dir"].as_str().unwrap().contains("req-logs"));

            run(
                LogAction::Llm {
                    action: LlmAction::Type {
                        log_type: "raw".into(),
                    },
                },
                false,
            )
            .expect("type raw ok");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["save_raw"], true);

            run(
                LogAction::Llm {
                    action: LlmAction::Type {
                        log_type: "default".into(),
                    },
                },
                false,
            )
            .expect("type default ok");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["save_raw"], false);

            let err = run(
                LogAction::Llm {
                    action: LlmAction::Type {
                        log_type: "bogus".into(),
                    },
                },
                false,
            )
            .expect_err("非法 type → bail（不是 exit）");
            assert!(err.to_string().contains("Unknown log type"), "got: {err:#}");

            run(
                LogAction::Llm {
                    action: LlmAction::Disable,
                },
                false,
            )
            .expect("llm disable ok");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["enabled"], false);
        });
    }

    // --- cmd_llm_status 深分支（enabled + 日志目录列表 / llm 段缺失）---

    #[test]
    fn llm_status_lists_recent_log_dirs_when_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let log_root = tmp.path().join("request_logs");
        for d in ["20260826", "20260825", "20260824"] {
            let dir = log_root.join(d);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("00.request.md"), "x".repeat(2048)).unwrap();
            std::fs::write(dir.join("01.AI.Response.raw.json"), "y").unwrap();
        }
        // 非目录条目应被过滤。
        std::fs::write(log_root.join("README.md"), "not a dir").unwrap();

        let cfg = tmp.path().join("config.json");
        std::fs::write(
            &cfg,
            serde_json::to_string(&serde_json::json!({
                "logging": {"llm": {"enabled": true, "log_dir": log_root.to_string_lossy(), "detail_level": "full"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_status(&cfg, &workspace).expect("enabled + 目录存在 → 列表分支");
    }

    #[test]
    fn llm_status_without_llm_section_prints_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(
            &cfg,
            serde_json::to_string(&serde_json::json!({
                "logging": {"general": {"enabled": true}}
            }))
            .unwrap(),
        )
        .unwrap();
        cmd_llm_status(&cfg, &tmp.path().join("workspace")).expect("llm 段缺失 → else 分支");
    }

    // --- General 子树 ---

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn general_subtree_dispatch_all_actions() {
        with_env_home(|home| {
            seed_home(&home);

            run(
                LogAction::General {
                    action: GeneralAction::Enable,
                },
                false,
            )
            .expect("general enable ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["enabled"], true);

            run(
                LogAction::General {
                    action: GeneralAction::Status,
                },
                false,
            )
            .expect("general status ok");

            run(
                LogAction::General {
                    action: GeneralAction::Level {
                        level: "DEBUG".into(),
                    },
                },
                false,
            )
            .expect("general level ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["level"], "DEBUG");

            run(
                LogAction::General {
                    action: GeneralAction::Level {
                        level: "NOPE".into(),
                    },
                },
                false,
            )
            .expect("非法 level → 打印错误 + Ok（不写盘）");

            run(
                LogAction::General {
                    action: GeneralAction::File {
                        path: "gen/app.log".into(),
                    },
                },
                false,
            )
            .expect("general file ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["file"], "gen/app.log");

            run(
                LogAction::General {
                    action: GeneralAction::Console,
                },
                false,
            )
            .expect("general console toggle ok");

            run(
                LogAction::General {
                    action: GeneralAction::Disable,
                },
                false,
            )
            .expect("general disable ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["enabled"], false);
        });
    }

    // --- 顶层命令（别名 + Status/Config/SetLevel/文件/控制台）---

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn top_level_dispatch_aliases_status_and_config() {
        with_env_home(|home| {
            seed_home(&home);

            // 向后兼容别名。
            run(LogAction::Enable, false).expect("alias Enable → llm enable");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["enabled"], true);
            run(LogAction::Disable, false).expect("alias Disable → llm disable");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["enabled"], false);

            run(LogAction::Status, false).expect("顶层 Status → llm+general 汇总");

            run(
                LogAction::Config {
                    detail_level: Some("full".into()),
                    log_dir: None,
                },
                false,
            )
            .expect("顶层 Config → LLM 设置");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["detail_level"], "full");

            run(
                LogAction::SetLevel {
                    level: "WARN".into(),
                },
                false,
            )
            .expect("顶层 SetLevel → general level");
            assert_eq!(read_cfg(&home)["logging"]["general"]["level"], "WARN");

            run(
                LogAction::SetLevel {
                    level: "BAD".into(),
                },
                false,
            )
            .expect("顶层 SetLevel 非法 → Ok-noop");
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn top_level_file_and_console_switches() {
        with_env_home(|home| {
            seed_home(&home);

            // EnableFile 显式绝对路径（默认相对路径会在测试 cwd 下建 logs/ 目录，
            // 污染仓库工作区——None 分支留白，见 tests.rs 头注）。
            let logfile = home.join("logs").join("app.log");
            run(
                LogAction::EnableFile {
                    path: Some(logfile.to_string_lossy().into()),
                },
                false,
            )
            .expect("enable file ok");
            assert!(
                logfile.parent().unwrap().exists(),
                "父目录被 create_dir_all"
            );
            assert_eq!(
                read_cfg(&home)["logging"]["general"]["file"],
                logfile.to_string_lossy().as_ref()
            );

            run(LogAction::DisableFile, false).expect("disable file ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["file"], "");

            run(LogAction::EnableConsole, false).expect("enable console ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["console"], true);

            run(LogAction::DisableConsole, false).expect("disable console ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["console"], false);
        });
    }
}

// ===========================================================================
// wave_a（R7 中批补盲，2026-08-27）：① logging 段存在但缺 llm 键的默认注入
// （194-198 / 356-360 / 425-432 —— 注意 read_logging_config 对「连 logging
// 段都没有」的输入会兜底整份默认，永远打不进这些注入臂，必须造 partial）、
// ② llm:{} 空对象的双默认填充（204-226 全程）、③ status 空串字段默认填充
// （287/290）、④ 旧 console 键 fallback 闭包（553-557 or_else 体）、⑤
// write_logging_config 写失败冒泡（181 `?`）。
// 已知不可测：cmd_llm_config 非法 detail_level 走 std::process::exit(1)
// （366-370），在测试进程内无法断言 → 豁免池。
// ===========================================================================

mod wave_a {
    // run/LlmAction/LogAction 只有下方 Windows 形态的 dispatch 测试使用，
    // 随之门控（Linux 上拆开导入，避免 unused import 死代码）。
    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    use super::super::{LlmAction, LogAction, run};
    use super::super::{
        cmd_general_console, cmd_llm_enable, cmd_llm_status, default_logging_config,
        write_logging_config,
    };

    #[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
    fn with_env_home(f: impl FnOnce(std::path::PathBuf)) {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        f(tmp.path().join(".nemesisbot"));
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }

    #[test]
    fn llm_enable_inserts_defaults_when_logging_lacks_llm_key() {
        // logging 段真实存在但没有 llm 键 → read_logging_config 原样返回该段，
        // cmd_llm_enable 进 194-198 默认注入（部分段才触发；缺整个 logging 段
        // 时 read_logging_config 兜底整份默认、含 llm，永远到不了这里）。
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"logging":{"general":{"enabled":true}}}"#).unwrap();

        cmd_llm_enable(&cfg, &tmp.path().join("workspace")).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["llm"]["log_dir"], "logs/request_logs");
        assert_eq!(data["logging"]["llm"]["detail_level"], "full");
        assert_eq!(data["logging"]["llm"]["enabled"], true);
        // 原 general 段保留。
        assert_eq!(data["logging"]["general"]["enabled"], true);
    }

    #[test]
    fn llm_empty_object_fills_both_defaults_on_enable() {
        // llm 为空对象：as_object_mut 成功但两字段缺失 → 空 → 双默认插入
        // （204-226 的完整空值路径，含收尾括号区）。
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"logging":{"llm":{}}}"#).unwrap();

        cmd_llm_enable(&cfg, &tmp.path().join("workspace")).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["llm"]["log_dir"], "logs/request_logs");
        assert_eq!(data["logging"]["llm"]["detail_level"], "full");
        assert_eq!(data["logging"]["llm"]["enabled"], true);
    }

    #[test]
    fn llm_status_applies_defaults_for_empty_string_fields() {
        // enabled + 两字段为空串 → 287/290 空值默认填充后正常打印。
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(
            &cfg,
            r#"{"logging":{"llm":{"enabled":true,"detail_level":"","log_dir":""}}}"#,
        )
        .unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_status(&cfg, &workspace).expect("空串字段走默认填充分支");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn dispatch_type_raw_creates_llm_section_with_save_raw() {
        // logging 段存在但没有 llm 键 + dispatch 层 Type raw → 425-432 带
        // save_raw 的默认注入 + 607 dispatch 尾臂。
        with_env_home(|home| {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(home.join("config.json"), r#"{"logging":{}}"#).unwrap();

            run(
                LogAction::Llm {
                    action: LlmAction::Type {
                        log_type: "raw".into(),
                    },
                },
                false,
            )
            .expect("type raw on partial config");

            let data: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap())
                    .unwrap();
            assert_eq!(data["logging"]["llm"]["save_raw"], true);
            assert_eq!(data["logging"]["llm"]["log_dir"], "logs/request_logs");
        });
    }

    #[test]
    fn general_console_toggle_falls_back_to_legacy_console_key() {
        // 只有旧键 console:false、无 enable_console → 553-557 or_else 闭包体
        // 读到 false → 翻转为 true 且双键同步写回。
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(
            &cfg,
            r#"{"logging":{"general":{"console":false,"level":"INFO"}}}"#,
        )
        .unwrap();

        cmd_general_console(&cfg).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            data["logging"]["general"]["enable_console"], true,
            "fallback 读到旧键 false 后必须翻为 true"
        );
        assert_eq!(data["logging"]["general"]["console"], true);
        assert_eq!(data["logging"]["general"]["level"], "INFO", "其它字段保留");
    }

    #[test]
    fn write_logging_config_err_bubbles_when_parent_is_regular_file() {
        // 父路径是普通文件：create_dir_all 被 `let _` 吞掉，写失败 → 181 `?` 冒泡。
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("blocker"), "not a dir").unwrap();
        let cfg = tmp.path().join("blocker").join("config.json");
        assert!(
            write_logging_config(&cfg, &default_logging_config()).is_err(),
            "父路径为文件 → 配置写必须报错"
        );
    }
}

// ===========================================================================
// wave_c（coverage 补测，2026-08-27）：cmd_llm_config 的 llm 段缺省注入臂
// （355-361）。既有 Config 测试全部基于 make_config（llm 键已存在）或
// None/None「无改动」路径；「logging 段存在但缺 llm 键」的 partial 配置只
// 测过 enable（194-198）/ type raw（425-432）两个入口，config 入口的默认
// 注入从未跑过。全部走合法 detail_level——非法值会 std::process::exit(1)
// 杀死测试进程（见 S11c 头注），绝不碰。
// ===========================================================================

mod wave_c {
    use super::*;

    #[test]
    fn wave_c_llm_config_injects_llm_defaults_when_key_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"logging":{"general":{"enabled":true}}}"#).unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_config(&cfg, &workspace, Some("truncated"), None).expect("config ok");

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            data["logging"]["llm"]["detail_level"], "truncated",
            "显式 detail_level 在注入后的 llm 段上生效"
        );
        assert_eq!(
            data["logging"]["llm"]["log_dir"], "logs/request_logs",
            "注入臂带默认 log_dir"
        );
        assert_eq!(
            data["logging"]["general"]["enabled"], true,
            "原 general 段保留"
        );
    }

    #[test]
    fn wave_c_llm_config_log_dir_creates_resolved_dir_and_keeps_defaults() {
        // llm 键缺失 + 只传相对 log_dir：注入默认结构 → resolve_path 落盘并
        // create_dir_all 建目录（390）；未指定的 detail_level 保持注入默认。
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.json");
        std::fs::write(&cfg, r#"{"logging":{"general":{"level":"WARN"}}}"#).unwrap();
        let workspace = tmp.path().join("ws");
        std::fs::create_dir_all(&workspace).unwrap();

        cmd_llm_config(&cfg, &workspace, None, Some("llm-dirs")).expect("config ok");

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let written = data["logging"]["llm"]["log_dir"].as_str().unwrap();
        assert!(
            written.starts_with(workspace.to_string_lossy().as_ref()),
            "相对 log_dir 必须解析到 workspace 下：{written}"
        );
        assert!(std::path::Path::new(written).exists(), "log_dir 目录被创建");
        assert_eq!(
            data["logging"]["llm"]["detail_level"], "full",
            "保持注入默认"
        );
        assert_eq!(data["logging"]["general"]["level"], "WARN", "原字段保留");
    }
}

// ===========================================================================
// r10（覆盖率 A 类 miss 补充）：
// - enable 对「字段存在但为空串」的填充臂（wave_a 只测了字段缺失形态；
//   unwrap_or("") 使两条输入殊途同归，但显式空串是独立夹具、补上更稳）。
// - 非法 detail_level 的 std::process::exit(1)（366-370）：进程内必然杀死
//   测试二进制（wave_a/wave_c 两代头注都判「豁免池」），子进程解锁——
//   run_cli 自动接 coverage_cli_env，断言退码 1 与错误文案。
// ===========================================================================

#[test]
fn r10_llm_enable_fills_explicit_empty_string_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(
        &cfg,
        r#"{"logging":{"llm":{"enabled":false,"detail_level":"","log_dir":""}}}"#,
    )
    .unwrap();

    cmd_llm_enable(&cfg, &tmp.path().join("workspace")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(
        data["logging"]["llm"]["detail_level"], "full",
        "显式空串必须被填成默认 full"
    );
    assert_eq!(data["logging"]["llm"]["log_dir"], "logs/request_logs");
    assert_eq!(data["logging"]["llm"]["enabled"], true);
}

// 整 mod Windows 形态（1/1 测试 + 专属 use 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r10_subprocess {
    use test_harness::{TestWorkspace, resolve_nemesisbot_bin};

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn r10_invalid_detail_level_exits_1_in_child_process() {
        // 进程内 std::process::exit(1) 会杀掉测试二进制 → 只能在子进程钉。
        let ws = TestWorkspace::new().expect("workspace");
        std::fs::create_dir_all(ws.home()).unwrap();
        std::fs::write(ws.config_path(), r#"{"logging":{"llm":{"enabled":true}}}"#).unwrap();

        let bin = resolve_nemesisbot_bin().expect("release binary");
        let out = ws
            .run_cli(
                &bin,
                &["log", "llm", "config", "--detail-level", "r10-bogus-level"],
            )
            .await;
        assert_eq!(
            out.exit_code, 1,
            "非法 detail_level 必须以退码 1 终止：stdout={} stderr={}",
            out.stdout, out.stderr
        );
        assert!(
            out.stdout
                .contains("Invalid detail level 'r10-bogus-level'"),
            "错误文案必须指名非法值：\n{}",
            out.stdout
        );

        // 对照组：合法值正常退码 0 且落盘。
        let ok = ws
            .run_cli(
                &bin,
                &["log", "llm", "config", "--detail-level", "truncated"],
            )
            .await;
        assert!(
            ok.success(),
            "合法 detail_level 应成功：stdout={} stderr={}",
            ok.stdout,
            ok.stderr
        );
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(ws.config_path()).unwrap()).unwrap();
        assert_eq!(cfg["logging"]["llm"]["detail_level"], "truncated");
    }
}
