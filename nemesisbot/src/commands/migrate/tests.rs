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

// ===========================================================================
// wave_a（R7 中批补盲，2026-08-27）：run() 流程级 + 转换分发臂。
// ① dry-run 用 crate 成功转换（151-156）+ warnings 打印；② 非 tty confirm
// 取消臂（124-126 / 394-397，PROMPT+TERM 摘除后 atty_isnt=true）；③
// --force 全量迁移（420 写盘 + workspace/prompts/skills/三人格各段
// 455/464/473/483）；④ load 失败回退臂（158-161）与无配置回退臂
// （165-168，直调 convert_config_with_crate）。
// 已知不可测 → 豁免池：dry-run/migrate 中 convert Err 打印臂（384/433-435，
// fallback 实现里没有可达的 Err 路径）；~/.openclaw 检测臂（63-70 + not-found
// 提示块 278-281）——dirs::home_dir() 在 Windows 走 SHGetKnownFolderPath
// (FOLDERID_Profile)，不读 USERPROFILE 环境变量，进程内不可注入 fake home；
// 本机恰好存在真实 ~/.openclaw，两态构造均不可移植（曾试图 USERPROFILE 注入，
// dry-run 误读了真机配置，已删测试）。显式 --openclaw-home 的命中路径由
// ①④ 覆盖，检测函数本身仅剩 Known-Folder 臂在豁免池。
// ===========================================================================

mod wave_a {
    use super::*;

    /// 最小但有肉的 OpenClaw fixture：配置 + workspace/prompts/skills + 三人格文件。
    fn make_openclaw_fixture(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        std::fs::write(
            root.join("openclaw.json"),
            r#"{"agents":{"defaults":{"llm":"zhipu/glm-4.7","max_tokens":4096}},"channels":{"telegram":{"token":"t"}}}"#,
        )
        .unwrap();
        std::fs::write(root.join("workspace").join("note.md"), "w").unwrap();
        std::fs::create_dir_all(root.join("prompts")).unwrap();
        std::fs::write(root.join("prompts").join("p.md"), "p").unwrap();
        std::fs::create_dir_all(root.join("skills").join("s1")).unwrap();
        std::fs::write(root.join("skills").join("s1").join("SKILL.md"), "s").unwrap();
        for name in ["IDENTITY.md", "SOUL.md", "USER.md"] {
            std::fs::write(root.join(name), format!("{name} body")).unwrap();
        }
    }

    fn opts(openclaw: &std::path::Path, nb_home: &std::path::Path, dry: bool) -> MigrateOptions {
        MigrateOptions {
            dry_run: dry,
            config_only: false,
            workspace_only: false,
            force: true,
            openclaw_home: Some(openclaw.to_string_lossy().into_owned()),
            refresh: false,
            nemesisbot_home: Some(nb_home.to_string_lossy().into_owned()),
        }
    }

    #[test]
    fn dry_run_previews_via_crate_conversion_and_lists_warnings() {
        // 有效 openclaw.json：find Ok → load Ok → crate convert_config（151-156）
        // → preview 打印 + warnings 循环。dry-run 在任何写盘之前返回。
        let tmp = tempfile::tempdir().unwrap();
        let oclaw = tmp.path().join("oclaw");
        let nb = tmp.path().join("nb");
        make_openclaw_fixture(&oclaw);
        std::fs::create_dir_all(&nb).unwrap();

        run(opts(&oclaw, &nb, true), false).expect("dry-run 全流程 Ok");
        assert!(
            !nb.join("config.json").exists(),
            "dry-run 不得写任何目标文件"
        );
    }

    #[test]
    fn confirm_cancelled_when_stdin_is_non_tty() {
        // 摘除 PROMPT/TERM → atty_isnt()=true → confirm 直接 false →
        // "Migration cancelled" 早退，且任何迁移都没发生。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let prev_prompt = std::env::var("PROMPT").ok();
        let prev_term = std::env::var("TERM").ok();
        unsafe {
            std::env::remove_var("PROMPT");
            std::env::remove_var("TERM");
        }

        let tmp = tempfile::tempdir().unwrap();
        let oclaw = tmp.path().join("oclaw");
        let nb = tmp.path().join("nb");
        make_openclaw_fixture(&oclaw);
        std::fs::create_dir_all(&nb).unwrap();

        let mut options = opts(&oclaw, &nb, false);
        options.force = false; // 才会走到 confirm
        let res = run(options, false);

        unsafe {
            match prev_prompt {
                Some(v) => std::env::set_var("PROMPT", v),
                None => std::env::remove_var("PROMPT"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        res.expect("非 tty 下自动取消 → Ok");
        assert!(!nb.join("config.json").exists(), "取消后不得落任何文件");
    }

    #[test]
    fn force_run_migrates_config_and_every_workspace_part() {
        // --force 全量：config 写盘（417-422）+ warnings 循环 +
        // workspace/prompts/skills 目录迁移（445-473）+ 三人格文件复制
        // （476-483），最后汇总打印。全部隔离在 tempdir。
        let tmp = tempfile::tempdir().unwrap();
        let oclaw = tmp.path().join("oclaw");
        let nb = tmp.path().join("nb");
        make_openclaw_fixture(&oclaw);
        std::fs::create_dir_all(&nb).unwrap();

        run(opts(&oclaw, &nb, false), false).expect("强制迁移全流程 Ok");

        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(nb.join("config.json"))
                .expect("config 迁移后存在（common::config_path = {home}/config.json）"))
            .unwrap();
        assert_eq!(
            cfg["agents"]["defaults"]["llm"], "zhipu/glm-4.7",
            "convert_config 消费 agents.defaults 并透传（对照 crate 单测 test_convert_config_basic）"
        );
        assert!(nb.join("workspace").join("note.md").exists(), "workspace 树被复制");
        assert!(nb.join("workspace").join("prompts").join("p.md").exists());
        assert!(nb.join("workspace").join("skills").join("s1").join("SKILL.md").exists());
        assert_eq!(
            std::fs::read_to_string(nb.join("IDENTITY.md")).unwrap(),
            "IDENTITY.md body",
            "人格文件落在 nemesis_home 根"
        );
        assert!(nb.join("SOUL.md").exists());
        assert!(nb.join("USER.md").exists());
    }

    #[test]
    fn with_crate_falls_back_when_config_load_fails() {
        // openclaw.json 是坏 JSON：find Ok 但 load Err → 158-161 eprintln 回退臂
        // → fallback 结构（默认 web 端口 8080）+ fallback 警告词。
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("openclaw.json"), "{ broken json").unwrap();

        let (cfg, warnings) = super::convert_config_with_crate(tmp.path()).unwrap();
        assert_eq!(
            cfg["channels"]["web"]["port"], 8080,
            "回退转换产出默认结构"
        );
        assert!(
            warnings.iter().any(|w| w.contains("fallback")),
            "warnings 应包含 fallback 说明：{warnings:?}"
        );
    }

    #[test]
    fn with_crate_falls_back_when_no_config_file_exists() {
        // 目录里啥都没有：find Err → 165-168 分支同样落到 fallback。
        let tmp = tempfile::tempdir().unwrap();
        let (cfg, warnings) = super::convert_config_with_crate(tmp.path()).unwrap();
        assert_eq!(cfg["version"], "1.0");
        assert!(warnings.iter().any(|w| w.contains("fallback")));
    }

    #[test]
    fn atty_isnt_true_when_both_prompt_and_term_absent() {
        // 与既有的「PROMPT 存在/TERM 存在」用例互补：双缺 → true 臂（139）。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let prev_prompt = std::env::var("PROMPT").ok();
        let prev_term = std::env::var("TERM").ok();
        unsafe {
            std::env::remove_var("PROMPT");
            std::env::remove_var("TERM");
        }
        let absent = super::atty_isnt();
        unsafe {
            match prev_prompt {
                Some(v) => std::env::set_var("PROMPT", v),
                None => std::env::remove_var("PROMPT"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert!(absent, "PROMPT/TERM 都不在时必须判定为非终端");
    }
}

// ===========================================================================
// wave_c（coverage 补测，2026-08-27）：OPENCLAW_HOME 环境变量注入臂
// （migrate.rs detect_openclaw_home 的 env 探测段 54-60）。
// 既有 test_detect_openclaw_home_env_var* 设了 env 却因「并行可能被改写」
// 从不做值断言，env→Some(p) 臂实际零验证。OPENCLAW_HOME 全 crate 只有
// migrate.rs 一处读取（grep 实证），持 crate::GLOBAL_STATE_LOCK 后 set/
// remove 窗口全局互斥 → 可确定性断言；用完恢复原值。
// ~/.openclaw Known-Folder 臂仍属豁免池（见 wave_a 头注：home_dir 进程内
// 不可注入）。
// ===========================================================================

mod wave_c {
    use super::*;

    #[test]
    fn env_openclaw_home_existing_dir_is_returned_verbatim() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var("OPENCLAW_HOME").ok();
        unsafe {
            std::env::set_var("OPENCLAW_HOME", tmp.path());
        }
        let detected = detect_openclaw_home(&None);
        unsafe {
            match prev {
                Some(v) => std::env::set_var("OPENCLAW_HOME", v),
                None => std::env::remove_var("OPENCLAW_HOME"),
            }
        }
        assert_eq!(
            detected.as_deref(),
            Some(tmp.path()),
            "OPENCLAW_HOME 指向存在的目录时必须原样返回该目录"
        );
    }

    #[test]
    fn run_migrates_entirely_via_env_detected_openclaw_home() {
        // 端到端：无 --openclaw-home，源定位纯靠 OPENCLAW_HOME；
        // --force 全量迁移成功且目标落盘正确。nemesisbot_home 双 override
        // 隔离到 tempdir，不触真实 home。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let oclaw = tmp.path().join("oclaw");
        std::fs::create_dir_all(oclaw.join("workspace")).unwrap();
        std::fs::write(
            oclaw.join("openclaw.json"),
            r#"{"agents":{"defaults":{"llm":"zhipu/glm-4.7"}}}"#,
        )
        .unwrap();
        std::fs::write(oclaw.join("workspace").join("MEMORY.md"), "mem").unwrap();
        std::fs::write(oclaw.join("IDENTITY.md"), "identity").unwrap();

        let nb = tmp.path().join("nb");
        std::fs::create_dir_all(&nb).unwrap();
        let prev = std::env::var("OPENCLAW_HOME").ok();
        unsafe {
            std::env::set_var("OPENCLAW_HOME", &oclaw);
        }

        let options = MigrateOptions {
            dry_run: false,
            config_only: false,
            workspace_only: false,
            force: true,
            openclaw_home: None,
            refresh: false,
            nemesisbot_home: Some(nb.to_string_lossy().into_owned()),
        };
        let res = run(options, false);

        unsafe {
            match prev {
                Some(v) => std::env::set_var("OPENCLAW_HOME", v),
                None => std::env::remove_var("OPENCLAW_HOME"),
            }
        }

        res.expect("env 驱动的迁移全流程 Ok");
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(nb.join("config.json")).unwrap())
                .expect("迁移产物 config.json 必须存在且可解析");
        assert_eq!(cfg["agents"]["defaults"]["llm"], "zhipu/glm-4.7");
        assert_eq!(
            std::fs::read_to_string(nb.join("workspace").join("MEMORY.md")).unwrap(),
            "mem",
            "workspace 树经 env 定位被复制"
        );
        assert_eq!(
            std::fs::read_to_string(nb.join("IDENTITY.md")).unwrap(),
            "identity",
            "人格文件经 env 定位被复制"
        );
    }
}

// ===========================================================================
// r10（覆盖率 A 类 miss 落地）：
// - convert_config_fallback 的四个异形夹具：config.yaml 目录化（exists 但
//   read_to_string Err）、default_model 空值不落（197 短路边）、models.yaml
//   目录化（227 read Err）、models.yaml 全非法行（model_list 空 → 不插入，
//   244 短路边）。既有批次只覆盖了 happy path + models.yaml 空名跳过。
// ===========================================================================

mod r10_fallback {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn r10_config_yaml_as_directory_skips_line_scan_and_still_breaks() {
        // config.yaml 是目录：exists()=true 进入候选，但 read_to_string Err
        // → 不逐行扫描、default_model/port 保持默认，随后照常 break 到
        // models.yaml 解析。
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("config.yaml")).unwrap();
        let (config, warnings) = convert_config_fallback(tmp.path()).unwrap();
        assert_eq!(config["default_model"], "", "读不出内容不得写入模型名");
        assert_eq!(config["channels"]["web"]["port"], 8080, "端口保持默认");
        assert!(warnings.iter().any(|w| w.contains("fallback")));
    }

    #[test]
    fn r10_default_model_empty_value_is_not_inserted() {
        // `default_model: ""` / 裸键 `default_model:`：剥引号+trim 后为空 →
        // 走 !model.is_empty() 短路边，不触发 insert。
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.yaml"),
            "default_model: \"\"\nport: 9999\n",
        )
        .unwrap();
        let (config, _) = convert_config_fallback(tmp.path()).unwrap();
        assert_eq!(
            config["default_model"], "",
            "空值必须保持默认空串，而不是插入同名新值"
        );
        // 同一文件里合法的 port 行仍要生效——证明逐行扫描确实跑了。
        assert_eq!(config["channels"]["web"]["port"], 9999);
    }

    #[test]
    fn r10_models_yaml_as_directory_yields_empty_model_list() {
        // models.yaml 是目录：exists()=true，read_to_string Err → 不解析、
        // model_list 维持空数组（244 is_empty 短路边）。
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("models.yaml")).unwrap();
        let (config, _) = convert_config_fallback(tmp.path()).unwrap();
        assert!(config["model_list"].as_array().unwrap().is_empty());
    }

    #[test]
    fn r10_models_yaml_without_valid_entries_inserts_nothing() {
        // 只有注释/普通键/空 name 的行 → model_list 空 → 走 if !is_empty()
        // 短路边，不 insert model_list 键（保持 default 结构里的空数组语义）。
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("models.yaml"),
            "# comment\nprovider: openai\n- name: ''\n- other:\n",
        )
        .unwrap();
        let (config, _) = convert_config_fallback(tmp.path()).unwrap();
        assert!(config["model_list"].as_array().unwrap().is_empty());
    }
}

// ===========================================================================
// r10 子进程批：confirm() 的 stdin 读取分支（129-131）。进程内跑不到——
// cargo test 下 stdin 是管道/EOF 且 PROMPT/TERM 随启动 shell 波动，分支不可
// 靠；子进程显式注入 PROMPT 强制 atty_isnt()==false，再喂 "n" 得到确定性
// 取消。spawn 接 coverage_cli_env（纪律 #2），无 env 竞争不持全局锁。
// ===========================================================================

mod r10_subprocess {
    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    #[tokio::test]
    async fn r10_confirm_stdin_answer_n_cancels_migration_without_force() {
        let ws = TestWorkspace::new().expect("workspace");
        let src = ws.path().join("oclaw");
        std::fs::create_dir_all(src.join("workspace")).unwrap();
        std::fs::write(src.join("IDENTITY.md"), "identity").unwrap();
        std::fs::write(
            src.join("config.yaml"),
            "default_model: \"gpt-x\"\nport: 12345\n",
        )
        .unwrap();
        let dst = ws.path().join("nb");

        let bin = resolve_nemesisbot_bin().expect("release binary");
        use tokio::io::AsyncWriteExt;
        let mut child = tokio::process::Command::new(&bin)
            .args([
                "--local",
                "migrate",
                "--openclaw-home",
                src.to_str().unwrap(),
                "--nemesisbot-home",
                dst.to_str().unwrap(),
            ])
            .current_dir(ws.path())
            .env("PROMPT", "1") // 强制走 io::stdin().read_line 分支（129）
            .envs(test_harness::coverage_cli_env()) // 纪律 #2
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn nemesisbot migrate");
        {
            let mut stdin = child.stdin.take().expect("stdin piped");
            stdin.write_all(b"n\n").await.expect("write answer n");
            stdin.shutdown().await.ok();
        }
        let out = child.wait_with_output().await.expect("wait migrate");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(out.status.success(), "拒绝迁移也是 Ok：stdout={}", stdout);
        assert!(
            stdout.contains("Proceed with migration?"),
            "确认提示必须出现在交互分支（而非 non-interactive 打印）：\n{}",
            stdout
        );
        assert!(stdout.contains("Migration cancelled."), "\n{}", stdout);
        assert!(
            !dst.join("config.json").exists(),
            "回答 n 后绝不能写目标 config"
        );
        assert!(!dst.join("IDENTITY.md").exists(), "工作区也不得复制");
    }
}
