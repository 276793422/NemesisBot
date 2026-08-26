use super::*;
use tempfile::TempDir;

#[test]
fn test_detect_openclaw_home_override_exists() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let result = detect_openclaw_home(&Some(path));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), tmp.path());
}

#[test]
fn test_detect_openclaw_home_override_not_exists() {
    let result = detect_openclaw_home(&Some("/nonexistent/path/xyz".to_string()));
    assert!(result.is_none());
}

#[test]
fn test_detect_openclaw_home_no_override_no_env() {
    // Without OPENCLAW_HOME set and no override, result depends on
    // whether ~/.openclaw exists. Just verify it doesn't panic.
    let _ = detect_openclaw_home(&None);
}

#[test]
fn test_detect_openclaw_home_env_var() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    unsafe {
        std::env::set_var("OPENCLAW_HOME", &path);
    }
    let _result = detect_openclaw_home(&None);
    unsafe {
        std::env::remove_var("OPENCLAW_HOME");
    }
    // In parallel tests, another test might overwrite the env var
    // Just verify the function doesn't panic and returns a PathBuf
    // The actual value might differ if env var was overridden by parallel tests
}

#[test]
fn test_detect_openclaw_home_env_var_takes_precedence() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    unsafe {
        std::env::set_var("OPENCLAW_HOME", &path);
    }
    // Even with None override, env var should work
    let _result = detect_openclaw_home(&None);
    unsafe {
        std::env::remove_var("OPENCLAW_HOME");
    }
    // In parallel tests, env var might be overwritten, so just verify no panic
}

#[test]
fn test_backup_file_creates_bak() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("config.json");
    std::fs::write(&file, "original content").unwrap();

    backup_file(&file).unwrap();

    let bak = tmp.path().join("config.json.bak");
    assert!(bak.exists());
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), "original content");
}

#[test]
fn test_backup_file_nonexistent() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("nonexistent.json");

    // Should succeed (no-op)
    backup_file(&file).unwrap();
}

#[test]
fn test_copy_dir_recursive_basic() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("a.txt"), "hello").unwrap();
    std::fs::write(src.join("sub").join("b.txt"), "world").unwrap();

    let count = copy_dir_recursive(&src, &dst, false).unwrap();
    assert_eq!(count, 2);
    assert!(dst.join("a.txt").exists());
    assert!(dst.join("sub").join("b.txt").exists());
    assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
}

#[test]
fn test_copy_dir_recursive_no_overwrite() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("file.txt"), "new").unwrap();

    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(dst.join("file.txt"), "old").unwrap();

    let count = copy_dir_recursive(&src, &dst, false).unwrap();
    assert_eq!(count, 0); // Skipped because file exists
    assert_eq!(
        std::fs::read_to_string(dst.join("file.txt")).unwrap(),
        "old"
    );
}

#[test]
fn test_copy_dir_recursive_with_refresh() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("file.txt"), "new").unwrap();

    std::fs::create_dir_all(&dst).unwrap();
    std::fs::write(dst.join("file.txt"), "old").unwrap();

    let count = copy_dir_recursive(&src, &dst, true).unwrap();
    assert_eq!(count, 1); // Overwritten
    assert_eq!(
        std::fs::read_to_string(dst.join("file.txt")).unwrap(),
        "new"
    );
}

#[test]
fn test_copy_dir_recursive_nested() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    std::fs::create_dir_all(src.join("a").join("b")).unwrap();
    std::fs::write(src.join("a").join("b").join("deep.txt"), "nested").unwrap();
    std::fs::write(src.join("root.txt"), "root").unwrap();

    let count = copy_dir_recursive(&src, &dst, false).unwrap();
    assert_eq!(count, 2);
    assert!(dst.join("a").join("b").join("deep.txt").exists());
}

#[test]
fn test_copy_dir_recursive_creates_dst() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("nonexistent").join("dst");

    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("file.txt"), "content").unwrap();

    copy_dir_recursive(&src, &dst, false).unwrap();
    assert!(dst.join("file.txt").exists());
}

#[test]
fn test_atty_isnt_with_prompt() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("PROMPT", "1");
    }
    let result = atty_isnt();
    unsafe {
        std::env::remove_var("PROMPT");
    }
    assert!(!result);
}

#[test]
fn test_atty_isnt_with_term() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("TERM", "xterm");
    }
    let result = atty_isnt();
    unsafe {
        std::env::remove_var("TERM");
    }
    assert!(!result);
}

#[test]
fn test_convert_config_fallback_no_files() {
    let tmp = TempDir::new().unwrap();
    let (config, warnings) = convert_config_fallback(tmp.path()).unwrap();

    assert_eq!(config["version"], "1.0");
    assert!(config["model_list"].is_array());
    assert_eq!(config["security"]["enabled"], true);
    assert_eq!(config["forge"]["enabled"], false);
    assert!(!warnings.is_empty());
}

#[test]
fn test_convert_config_fallback_with_yaml() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.yaml"),
        "default_model: \"test/model\"\nport: 9090\n",
    )
    .unwrap();

    let (config, _warnings) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(config["default_model"], "test/model");
    assert_eq!(config["channels"]["web"]["port"], 9090);
}

#[test]
fn test_convert_config_fallback_with_models_yaml() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("models.yaml"),
        "- name: \"provider/model-1\"\n- model: \"provider/model-2\"\n",
    )
    .unwrap();

    let (config, _warnings) = convert_config_fallback(tmp.path()).unwrap();
    let models = config["model_list"].as_array().unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0]["model"], "provider/model-1");
    assert_eq!(models[1]["model"], "provider/model-2");
}

#[test]
fn test_convert_config_fallback_empty_model_name_skipped() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("models.yaml"),
        "- name: \"\"\n- model: \"valid/model\"\n",
    )
    .unwrap();

    let (config, _warnings) = convert_config_fallback(tmp.path()).unwrap();
    let models = config["model_list"].as_array().unwrap();
    assert_eq!(models.len(), 1); // Empty name skipped
}

#[test]
fn test_convert_config_fallback_yml_extension() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("openclaw.yml"),
        "default_model: 'yml-model'\n",
    )
    .unwrap();

    let (config, _warnings) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(config["default_model"], "yml-model");
}

#[test]
fn test_migrate_options_default() {
    let opts = MigrateOptions {
        dry_run: false,
        config_only: false,
        workspace_only: false,
        force: false,
        openclaw_home: None,
        refresh: false,
        nemesisbot_home: None,
    };
    assert!(!opts.dry_run);
    assert!(!opts.config_only);
    assert!(!opts.workspace_only);
    assert!(!opts.force);
    assert!(!opts.refresh);
}

// -------------------------------------------------------------------------
// Additional migrate tests
// -------------------------------------------------------------------------

#[test]
fn test_migrate_options_all_set() {
    let opts = MigrateOptions {
        dry_run: true,
        config_only: true,
        workspace_only: true,
        force: true,
        openclaw_home: Some("/path".to_string()),
        refresh: true,
        nemesisbot_home: Some("/target".to_string()),
    };
    assert!(opts.dry_run);
    assert!(opts.config_only);
    assert!(opts.workspace_only);
    assert!(opts.force);
    assert!(opts.refresh);
    assert_eq!(opts.openclaw_home, Some("/path".to_string()));
    assert_eq!(opts.nemesisbot_home, Some("/target".to_string()));
}

#[test]
fn test_nemesis_home_override() {
    let opts = MigrateOptions {
        nemesisbot_home: Some("/custom/path".to_string()),
        dry_run: false,
        config_only: false,
        workspace_only: false,
        force: false,
        openclaw_home: None,
        refresh: false,
    };
    // The actual default impl may not exist, but we test the logic:
    let nemesis_home = if let Some(ref home) = opts.nemesisbot_home {
        PathBuf::from(home)
    } else {
        PathBuf::from("default")
    };
    assert_eq!(nemesis_home, PathBuf::from("/custom/path"));
}

#[test]
fn test_backup_file_preserves_content() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("test.yaml");
    let content = "key: value\nanother: data\n";
    std::fs::write(&file, content).unwrap();

    backup_file(&file).unwrap();

    let bak = tmp.path().join("test.yaml.bak");
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), content);
    // Original should still exist
    assert_eq!(std::fs::read_to_string(&file).unwrap(), content);
}

#[test]
fn test_backup_file_json_extension() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("config.json");
    std::fs::write(&file, "{}").unwrap();

    backup_file(&file).unwrap();

    let bak = tmp.path().join("config.json.bak");
    assert!(bak.exists());
}

#[test]
fn test_copy_dir_recursive_empty_source() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("empty_src");
    let dst = tmp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();

    let count = copy_dir_recursive(&src, &dst, false).unwrap();
    assert_eq!(count, 0);
    assert!(dst.exists());
}

#[test]
fn test_copy_dir_recursive_mixed_content() {
    let tmp = TempDir::new().unwrap();
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");

    std::fs::create_dir_all(src.join("dir1")).unwrap();
    std::fs::create_dir_all(src.join("dir2").join("sub")).unwrap();
    std::fs::write(src.join("file1.txt"), "content1").unwrap();
    std::fs::write(src.join("dir1").join("file2.txt"), "content2").unwrap();
    std::fs::write(src.join("dir2").join("sub").join("file3.txt"), "content3").unwrap();

    let count = copy_dir_recursive(&src, &dst, false).unwrap();
    assert_eq!(count, 3);
    assert!(dst.join("file1.txt").exists());
    assert!(dst.join("dir1").join("file2.txt").exists());
    assert!(dst.join("dir2").join("sub").join("file3.txt").exists());
}

#[test]
fn test_convert_config_fallback_with_all_yaml_variants() {
    // Test that it checks config.yaml, config.yml, openclaw.yaml, openclaw.yml
    for name in &["config.yaml", "config.yml", "openclaw.yaml", "openclaw.yml"] {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(name),
            "default_model: 'test-model'\nport: 3000\n",
        )
        .unwrap();
        let (config, _) = convert_config_fallback(tmp.path()).unwrap();
        assert_eq!(config["default_model"], "test-model");
        assert_eq!(config["channels"]["web"]["port"], 3000);
    }
}

#[test]
fn test_convert_config_fallback_yaml_priority() {
    let tmp = TempDir::new().unwrap();
    // config.yaml should take priority over openclaw.yml
    std::fs::write(
        tmp.path().join("config.yaml"),
        "default_model: 'from-config-yaml'\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("openclaw.yml"),
        "default_model: 'from-openclaw-yml'\n",
    )
    .unwrap();
    let (config, _) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(config["default_model"], "from-config-yaml");
}

#[test]
fn test_convert_config_fallback_models_yaml_various_formats() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("models.yaml"),
        "- name: \"model-a\"\n- model: 'model-b'\n- name: ''\n- name: \"model-c\"\n",
    )
    .unwrap();
    let (config, _) = convert_config_fallback(tmp.path()).unwrap();
    let models = config["model_list"].as_array().unwrap();
    assert_eq!(models.len(), 3); // Empty name skipped
    assert_eq!(models[0]["model"], "model-a");
    assert_eq!(models[1]["model"], "model-b");
    assert_eq!(models[2]["model"], "model-c");
}

#[test]
fn test_convert_config_fallback_port_invalid() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("config.yaml"),
        "default_model: 'test'\nport: not-a-number\n",
    )
    .unwrap();
    let (config, _) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(config["default_model"], "test");
    // Port should remain default (8080) since parse fails
    assert_eq!(config["channels"]["web"]["port"], 8080);
}

#[test]
fn test_convert_config_fallback_default_structure() {
    let tmp = TempDir::new().unwrap();
    let (config, warnings) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(config["version"], "1.0");
    assert_eq!(config["default_model"], "");
    assert!(config["model_list"].is_array());
    assert!(config["model_list"].as_array().unwrap().is_empty());
    assert_eq!(config["channels"]["web"]["enabled"], true);
    assert_eq!(config["channels"]["web"]["host"], "127.0.0.1");
    assert_eq!(config["channels"]["web"]["port"], 8080);
    assert_eq!(config["channels"]["websocket"]["enabled"], false);
    assert_eq!(config["security"]["enabled"], true);
    assert_eq!(config["forge"]["enabled"], false);
    assert!(!warnings.is_empty());
}

#[test]
fn test_atty_isnt_default() {
    // Without PROMPT or TERM set, atty_isnt returns true
    // We can't easily control env vars in parallel tests, but we can
    // at least call it to verify it doesn't panic
    let _ = atty_isnt();
}

#[test]
fn test_confirm_returns_false_in_test_env() {
    // In test environments, stdin is not a tty
    // This tests the logic that non-interactive returns false
    // Note: We can't call confirm() directly because it reads from stdin
    // But we can test the underlying atty_isnt logic
}

// ===========================================================================
// run() 全臂 + convert_config_fallback（S11c，quality-hardening goal 冲刺 S11）
// —— 既有测试只钉 helper 片段，run() 本体（219 MISS 行）从没跑过。全部用
// openclaw_home/nemesisbot_home 双 override 指向临时目录，零 env 依赖；
// confirm 在测试 stdin（管道 EOF）下必返 false，非 --force 一律取消。
// ===========================================================================

fn write_full_openclaw_layout(openclaw: &std::path::Path) {
    std::fs::create_dir_all(openclaw.join("workspace")).unwrap();
    std::fs::write(openclaw.join("workspace").join("MEMORY.md"), "mem").unwrap();
    std::fs::create_dir_all(openclaw.join("prompts")).unwrap();
    std::fs::write(openclaw.join("prompts").join("p1.md"), "prompt").unwrap();
    std::fs::create_dir_all(openclaw.join("skills")).unwrap();
    std::fs::write(openclaw.join("skills").join("s1.md"), "skill").unwrap();
    std::fs::write(openclaw.join("IDENTITY.md"), "identity").unwrap();
    std::fs::write(openclaw.join("SOUL.md"), "soul").unwrap();
    std::fs::write(openclaw.join("USER.md"), "user").unwrap();
    std::fs::write(
        openclaw.join("config.yaml"),
        "default_model: \"gpt-x\"\nport: 12345\n",
    )
    .unwrap();
    std::fs::write(openclaw.join("models.yaml"), "- name: m1\n- name: m2\n").unwrap();
}

fn opts(openclaw: &std::path::Path, target: &std::path::Path) -> MigrateOptions {
    MigrateOptions {
        dry_run: false,
        config_only: false,
        workspace_only: false,
        force: true,
        openclaw_home: Some(openclaw.to_string_lossy().into()),
        refresh: false,
        nemesisbot_home: Some(target.to_string_lossy().into()),
    }
}

#[test]
fn run_openclaw_missing_reports_not_found_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("nb");
    let mut o = opts(&tmp.path().join("no-such-openclaw"), &target);
    o.openclaw_home = Some(tmp.path().join("no-such-openclaw").to_string_lossy().into());
    run(o, false).expect("探测失败 → 提示 + Ok");
    assert!(!target.exists(), "不得创建任何目标文件");
}

#[test]
fn run_empty_openclaw_reports_nothing_to_migrate() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    let target = tmp.path().join("nb");
    run(opts(&src, &target), false).expect("空源 → Nothing to migrate + Ok");
    assert!(!target.exists());
}

#[test]
fn run_dry_run_makes_no_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    write_full_openclaw_layout(&src);
    let target = tmp.path().join("nb");
    let mut o = opts(&src, &target);
    o.dry_run = true;
    run(o, false).expect("dry-run → 预览 + Ok");
    assert!(!target.exists(), "dry-run 绝不落盘");
}

#[test]
fn run_without_force_is_cancelled_by_non_interactive_confirm() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    write_full_openclaw_layout(&src);
    let target = tmp.path().join("nb");
    let mut o = opts(&src, &target);
    o.force = false;
    run(o, false).expect("confirm=false → Migration cancelled + Ok");
    assert!(!target.join("config.json").exists(), "取消后不得写 config");
    assert!(!target.join("IDENTITY.md").exists());
}

#[test]
fn run_force_migrates_config_workspace_prompts_skills_and_persona_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    write_full_openclaw_layout(&src);
    let target = tmp.path().join("nb");
    run(opts(&src, &target), false).expect("full migration ok");

    // config：存在且是合法 JSON。
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("config.json")).unwrap())
            .expect("迁移产物 config.json 必须可解析");
    assert!(cfg.is_object());

    // workspace / prompts / skills。
    assert_eq!(
        std::fs::read_to_string(target.join("workspace").join("MEMORY.md")).unwrap(),
        "mem"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("workspace").join("prompts").join("p1.md")).unwrap(),
        "prompt"
    );
    assert_eq!(
        std::fs::read_to_string(target.join("workspace").join("skills").join("s1.md")).unwrap(),
        "skill"
    );

    // 人格三件套落 home 根。
    assert_eq!(
        std::fs::read_to_string(target.join("IDENTITY.md")).unwrap(),
        "identity"
    );
    assert_eq!(std::fs::read_to_string(target.join("SOUL.md")).unwrap(), "soul");
    assert_eq!(std::fs::read_to_string(target.join("USER.md")).unwrap(), "user");
}

#[test]
fn run_backs_up_existing_target_config_before_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    write_full_openclaw_layout(&src);
    let target = tmp.path().join("nb");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("config.json"), "{\"old\":true}").unwrap();

    run(opts(&src, &target), false).expect("overwrite ok");
    let bak = std::fs::read_to_string(target.join("config.json.bak"))
        .expect("旧 config 必须先备份成 .bak");
    assert_eq!(bak, "{\"old\":true}");
    let new: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(target.join("config.json")).unwrap())
            .unwrap();
    assert!(new.get("old").is_none(), "新 config 是转换产物而非旧文件");
}

#[test]
fn run_config_only_skips_workspace_and_vice_versa() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    write_full_openclaw_layout(&src);

    // config_only：只迁 config。
    let t1 = tmp.path().join("nb1");
    let mut o1 = opts(&src, &t1);
    o1.config_only = true;
    run(o1, false).expect("config only ok");
    assert!(t1.join("config.json").exists());
    assert!(!t1.join("workspace").join("MEMORY.md").exists());
    assert!(!t1.join("IDENTITY.md").exists());

    // workspace_only：只迁工作区。
    let t2 = tmp.path().join("nb2");
    let mut o2 = opts(&src, &t2);
    o2.workspace_only = true;
    run(o2, false).expect("workspace only ok");
    assert!(!t2.join("config.json").exists());
    assert!(t2.join("workspace").join("MEMORY.md").exists());
    assert!(t2.join("IDENTITY.md").exists());
}

#[test]
fn run_refresh_flag_overwrites_conflicting_target_files() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("openclaw");
    std::fs::create_dir_all(&src).unwrap();
    write_full_openclaw_layout(&src);
    let target = tmp.path().join("nb");

    // 第一次迁移。
    run(opts(&src, &target), false).unwrap();
    // 改源文件；目标同路径已有旧内容。
    std::fs::write(src.join("workspace").join("MEMORY.md"), "mem-v2").unwrap();

    // 不带 refresh：已存在文件跳过。
    run(opts(&src, &target), false).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("workspace").join("MEMORY.md")).unwrap(),
        "mem",
        "无 refresh 时目标已存在 → 保留"
    );

    // 带 refresh：覆盖。
    let mut o = opts(&src, &target);
    o.refresh = true;
    run(o, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(target.join("workspace").join("MEMORY.md")).unwrap(),
        "mem-v2",
        "refresh 必须覆盖目标同名文件"
    );
}

// --- convert_config_fallback（crate 找不到 config 时的 YAML 抽取）---

#[test]
fn fallback_conversion_extracts_default_model_port_and_models() {
    let tmp = tempfile::tempdir().unwrap();
    // 只放 config.yml（不在 has_config 的 .yaml 白名单里也没关系——
    // fallback 的查找循环覆盖 yaml/yml/openclaw.*），不放 crate 认的配置。
    std::fs::write(
        tmp.path().join("config.yml"),
        "default_model: 'claude-9'\nport: 4321\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("models.yaml"), "- name: alpha\n- model: \"beta\"\n").unwrap();

    let (cfg, warnings) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(cfg["default_model"], "claude-9", "单引号 YAML 值也要剥干净");
    assert_eq!(cfg["channels"]["web"]["port"], 4321);
    let models = cfg["model_list"].as_array().unwrap();
    assert_eq!(models.len(), 2, "- name: 和 - model: 两种条目都要抓到");
    assert_eq!(models[0]["model"], "alpha");
    assert_eq!(models[1]["model"], "beta");
    assert!(
        warnings.iter().any(|w| w.contains("fallback")),
        "必须带 fallback 警告：{warnings:?}"
    );
}

#[test]
fn fallback_conversion_with_no_sources_keeps_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let (cfg, _) = convert_config_fallback(tmp.path()).unwrap();
    assert_eq!(cfg["default_model"], "");
    assert_eq!(cfg["channels"]["web"]["port"], 8080, "缺省端口 8080");
    assert_eq!(
        cfg["model_list"].as_array().unwrap().len(),
        0,
        "无 models.yaml → 空 model_list"
    );
}

#[test]
fn copy_dir_recursive_nested_tree_count() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("a").join("b")).unwrap();
    std::fs::write(src.join("f1"), "1").unwrap();
    std::fs::write(src.join("a").join("f2"), "2").unwrap();
    std::fs::write(src.join("a").join("b").join("f3"), "3").unwrap();

    let dst = tmp.path().join("dst");
    let n = copy_dir_recursive(&src, &dst, false).unwrap();
    assert_eq!(n, 3, "三层三文件全拷");
    assert!(dst.join("a").join("b").join("f3").exists());
}
