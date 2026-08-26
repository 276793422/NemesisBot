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
    assert!(!expanded.starts_with('~') || !dirs::home_dir().is_some());
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
    assert!(!expanded.starts_with('~') || !dirs::home_dir().is_some());
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
    assert!(!resolved.starts_with('~') || !dirs::home_dir().is_some());
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

    fn seed_home(home: &std::path::Path) {
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join("config.json"), "{}").unwrap();
    }

    fn read_cfg(home: &std::path::Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap()
    }

    // --- Llm 子树 ---

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
                    action: LlmAction::Type { log_type: "raw".into() },
                },
                false,
            )
            .expect("type raw ok");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["save_raw"], true);

            run(
                LogAction::Llm {
                    action: LlmAction::Type { log_type: "default".into() },
                },
                false,
            )
            .expect("type default ok");
            assert_eq!(read_cfg(&home)["logging"]["llm"]["save_raw"], false);

            let err = run(
                LogAction::Llm {
                    action: LlmAction::Type { log_type: "bogus".into() },
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
                    action: GeneralAction::Level { level: "DEBUG".into() },
                },
                false,
            )
            .expect("general level ok");
            assert_eq!(read_cfg(&home)["logging"]["general"]["level"], "DEBUG");

            run(
                LogAction::General {
                    action: GeneralAction::Level { level: "NOPE".into() },
                },
                false,
            )
            .expect("非法 level → 打印错误 + Ok（不写盘）");

            run(
                LogAction::General {
                    action: GeneralAction::File { path: "gen/app.log".into() },
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
                LogAction::SetLevel { level: "WARN".into() },
                false,
            )
            .expect("顶层 SetLevel → general level");
            assert_eq!(read_cfg(&home)["logging"]["general"]["level"], "WARN");

            run(
                LogAction::SetLevel { level: "BAD".into() },
                false,
            )
            .expect("顶层 SetLevel 非法 → Ok-noop");
        });
    }

    #[test]
    fn top_level_file_and_console_switches() {
        with_env_home(|home| {
            seed_home(&home);

            // EnableFile 显式绝对路径（默认相对路径会在测试 cwd 下建 logs/ 目录，
            // 污染仓库工作区——None 分支留白，见 tests.rs 头注）。
            let logfile = home.join("logs").join("app.log");
            run(
                LogAction::EnableFile { path: Some(logfile.to_string_lossy().into()) },
                false,
            )
            .expect("enable file ok");
            assert!(logfile.parent().unwrap().exists(), "父目录被 create_dir_all");
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
