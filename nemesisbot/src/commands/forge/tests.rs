use super::*;

    #[test]
    fn test_default_forge_config() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
        assert_eq!(cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()), Some(3600));
        assert_eq!(cfg.get("min_experiences").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(cfg.get("learning_enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_load_forge_config_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = load_forge_config(tmp.path());
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
    }

    #[test]
    fn test_save_and_load_forge_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        }
        save_forge_config(tmp.path(), &cfg).unwrap();
        let loaded = load_forge_config(tmp.path());
        assert_eq!(loaded.get("learning_enabled").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_load_registry_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reg = load_registry(tmp.path());
        assert!(reg.is_empty());
    }

    #[test]
    fn test_load_registry_with_data() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        std::fs::write(&registry_path, r#"[{"id":"test-1","type":"skill","name":"test","status":"draft"}]"#).unwrap();
        let reg = load_registry(tmp.path());
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].get("id").and_then(|v| v.as_str()), Some("test-1"));
    }

    // -------------------------------------------------------------------------
    // default_forge_config comprehensive tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_forge_config_collect_interval() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
    }

    #[test]
    fn test_default_forge_config_reflect_interval() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()), Some(3600));
    }

    #[test]
    fn test_default_forge_config_min_experiences() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("min_experiences").and_then(|v| v.as_u64()), Some(5));
    }

    #[test]
    fn test_default_forge_config_llm_semantic_analysis() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("llm_semantic_analysis").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_default_forge_config_default_artifact_status() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("default_artifact_status").and_then(|v| v.as_str()), Some("draft"));
    }

    #[test]
    fn test_default_forge_config_trace_collection() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("trace_collection").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_default_forge_config_learning_enabled_false() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("learning_enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_default_forge_config_learning_subsection() {
        let cfg = default_forge_config();
        let learning = cfg.get("learning").unwrap();
        assert_eq!(learning.get("min_pattern_frequency").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(learning.get("high_confidence_threshold").and_then(|v| v.as_f64()), Some(0.8));
        assert_eq!(learning.get("max_auto_creates").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(learning.get("max_refine_rounds").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(learning.get("min_outcome_samples").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(learning.get("monitor_window_days").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(learning.get("degrade_threshold").and_then(|v| v.as_f64()), Some(-0.2));
        assert_eq!(learning.get("degrade_cooldown_days").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(learning.get("llm_budget_tokens").and_then(|v| v.as_u64()), Some(8000));
    }

    // -------------------------------------------------------------------------
    // load_forge_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_load_forge_config_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        // Write invalid JSON
        std::fs::write(forge_dir.join("forge.json"), "not valid json {{{").unwrap();
        let cfg = load_forge_config(&forge_dir);
        // Should fall back to defaults
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
    }

    // -------------------------------------------------------------------------
    // save_forge_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_save_forge_config_creates_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let new_dir = tmp.path().join("new_forge_dir");
        assert!(!new_dir.exists());
        save_forge_config(&new_dir, &default_forge_config()).unwrap();
        assert!(new_dir.exists());
        assert!(new_dir.join("forge.json").exists());
    }

    #[test]
    fn test_save_forge_config_overwrites() {
        let tmp = tempfile::TempDir::new().unwrap();
        save_forge_config(tmp.path(), &default_forge_config()).unwrap();

        let mut custom = default_forge_config();
        if let Some(obj) = custom.as_object_mut() {
            obj.insert("collect_interval_sec".to_string(), serde_json::Value::Number(600.into()));
        }
        save_forge_config(tmp.path(), &custom).unwrap();

        let loaded = load_forge_config(tmp.path());
        assert_eq!(loaded.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(600));
    }

    // -------------------------------------------------------------------------
    // load_registry edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_load_registry_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        std::fs::write(&registry_path, "invalid json").unwrap();
        let reg = load_registry(tmp.path());
        assert!(reg.is_empty());
    }

    #[test]
    fn test_load_registry_empty_array() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        std::fs::write(&registry_path, "[]").unwrap();
        let reg = load_registry(tmp.path());
        assert!(reg.is_empty());
    }

    #[test]
    fn test_load_registry_multiple_artifacts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        let data = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "skill1", "status": "active", "version": "1.0"},
            {"id": "a2", "type": "script", "name": "script1", "status": "draft", "version": "0.1"},
            {"id": "a3", "type": "mcp", "name": "mcp1", "status": "active", "version": "2.0"}
        ]);
        std::fs::write(&registry_path, serde_json::to_string(&data).unwrap()).unwrap();
        let reg = load_registry(tmp.path());
        assert_eq!(reg.len(), 3);
        assert_eq!(reg[0].get("type").and_then(|v| v.as_str()), Some("skill"));
        assert_eq!(reg[1].get("type").and_then(|v| v.as_str()), Some("script"));
        assert_eq!(reg[2].get("type").and_then(|v| v.as_str()), Some("mcp"));
    }

    // -------------------------------------------------------------------------
    // cmd_status (requires config file)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_status_no_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        // Don't create config file, should report disabled
        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_status_with_forge_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_status_with_forge_disabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": false}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_list tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_list_empty_registry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        cmd_list(&forge_dir, "all").unwrap();
    }

    #[test]
    fn test_cmd_list_with_registry_artifacts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"},
            {"id": "a2", "type": "script", "name": "Test Script", "version": "0.5", "status": "draft"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_list(&forge_dir, "all").unwrap();
    }

    #[test]
    fn test_cmd_list_filter_by_type() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"},
            {"id": "a2", "type": "script", "name": "Test Script", "version": "0.5", "status": "draft"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        // Filter by type "skill" - should only show skill artifacts
        cmd_list(&forge_dir, "skill").unwrap();
    }

    #[test]
    fn test_cmd_list_filter_no_match() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        // Filter by non-existent type
        cmd_list(&forge_dir, "nonexistent").unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_evaluate tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_evaluate_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "art-1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active", "score": 0.95, "usage_count": 42}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_evaluate(&forge_dir, "art-1").unwrap();
    }

    #[test]
    fn test_cmd_evaluate_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "art-1", "type": "skill", "name": "Test Skill"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_evaluate(&forge_dir, "nonexistent-id").unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_learning_status tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_learning_status_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        cmd_learning_status(&forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_learning_status_custom_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        }
        save_forge_config(&forge_dir, &cfg).unwrap();
        cmd_learning_status(&forge_dir).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_enable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_enable_creates_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        cmd_enable(&cfg_path, &forge_dir).unwrap();

        // Check all 7 + prompts directories were created
        for d in &["experiences", "reflections", "skills", "scripts", "mcp", "traces", "learning"] {
            assert!(forge_dir.join(d).exists(), "Directory '{}' should exist", d);
        }
        assert!(forge_dir.join("prompts").exists());
        assert!(forge_dir.join("forge.json").exists());
        assert!(forge_dir.join("registry.json").exists());
    }

    #[test]
    fn test_cmd_enable_preserves_existing_forge_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"some_field": "preserved"}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        cmd_enable(&cfg_path, &forge_dir).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(loaded.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()), Some(true));
        assert_eq!(loaded.get("forge").and_then(|f| f.get("some_field")).and_then(|v| v.as_str()), Some("preserved"));
    }

    // -------------------------------------------------------------------------
    // cmd_disable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_disable_sets_false() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let cfg = serde_json::json!({"forge": {"enabled": true, "some_field": "kept"}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        cmd_disable(&cfg_path).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(loaded.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()), Some(false));
        assert_eq!(loaded.get("forge").and_then(|f| f.get("some_field")).and_then(|v| v.as_str()), Some("kept"));
    }

    // -------------------------------------------------------------------------
    // cmd_learning_enable / cmd_learning_disable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_learning_enable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        save_forge_config(&forge_dir, &default_forge_config()).unwrap();

        cmd_learning_enable(&forge_dir).unwrap();

        let loaded = load_forge_config(&forge_dir);
        assert_eq!(loaded.get("learning_enabled").and_then(|v| v.as_bool()), Some(true));
        // Should also auto-enable trace collection
        assert_eq!(loaded.get("trace_collection").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_cmd_learning_disable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        }
        save_forge_config(&forge_dir, &cfg).unwrap();

        cmd_learning_disable(&forge_dir).unwrap();

        let loaded = load_forge_config(&forge_dir);
        assert_eq!(loaded.get("learning_enabled").and_then(|v| v.as_bool()), Some(false));
    }

    // -------------------------------------------------------------------------
    // cmd_learning_history tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_learning_history_no_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        cmd_learning_history(&forge_dir, 10).unwrap();
    }

    #[test]
    fn test_cmd_learning_history_with_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        let learning_dir = forge_dir.join("learning");
        std::fs::create_dir_all(&learning_dir).unwrap();
        let entries = serde_json::json!([
            {"timestamp": "2026-01-01T00:00:00Z", "patterns_found": 5, "actions_generated": 3, "actions_deployed": 2},
            {"timestamp": "2026-01-02T00:00:00Z", "patterns_found": 8, "actions_generated": 6, "actions_deployed": 4}
        ]);
        let jsonl: String = entries.as_array().unwrap().iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(learning_dir.join("learning_cycles.jsonl"), jsonl).unwrap();

        cmd_learning_history(&forge_dir, 10).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_reflect edge cases (non-runtime parts)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_reflect_forge_not_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": false}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Should print "not enabled" and return Ok
        // This doesn't need tokio runtime since it returns early
        cmd_reflect(&cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_reflect_forge_dir_not_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        // forge_dir doesn't exist - should print error and return Ok
        cmd_reflect(&cfg_path, &forge_dir).unwrap();
    }

    // -------------------------------------------------------------------------
    // Additional forge tests for coverage
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_enable_no_existing_config_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        // Don't create config file - cmd_enable only writes if config exists
        std::fs::create_dir_all(&home).unwrap();

        cmd_enable(&cfg_path, &forge_dir).unwrap();

        // Directories should still be created
        assert!(forge_dir.join("experiences").exists());
        assert!(forge_dir.join("forge.json").exists());
        assert!(forge_dir.join("registry.json").exists());
    }

    #[test]
    fn test_cmd_disable_no_config_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        // Don't create config file - should be a no-op
        cmd_disable(&cfg_path).unwrap();
    }

    #[test]
    fn test_cmd_status_with_forge_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Create some forge directories with content
        std::fs::create_dir_all(forge_dir.join("experiences")).unwrap();
        std::fs::write(forge_dir.join("experiences").join("exp1.json"), "{}").unwrap();
        std::fs::create_dir_all(forge_dir.join("reflections")).unwrap();

        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_status_with_registry_artifacts_and_types() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&forge_dir).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Create registry with various types and statuses
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "status": "active"},
            {"id": "a2", "type": "skill", "status": "draft"},
            {"id": "a3", "type": "script", "status": "active"},
            {"id": "a4", "type": "mcp", "status": "deprecated"}
        ]);
        std::fs::write(
            forge_dir.join("registry.json"),
            serde_json::to_string(&registry).unwrap()
        ).unwrap();

        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_evaluate_with_score_and_usage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "eval-1", "type": "skill", "name": "Scored Skill", "version": "2.0", "status": "active", "score": 0.85, "usage_count": 150}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_evaluate(&forge_dir, "eval-1").unwrap();
    }

    #[test]
    fn test_cmd_list_fallback_directory_scan() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        // Don't create registry.json - should fall back to directory scan
        std::fs::create_dir_all(forge_dir.join("skills")).unwrap();
        std::fs::write(forge_dir.join("skills").join("skill1.json"), "{}").unwrap();
        std::fs::create_dir_all(forge_dir.join("scripts")).unwrap();
        // scripts dir is empty

        cmd_list(&forge_dir, "all").unwrap();
    }

    #[test]
    fn test_cmd_list_fallback_specific_type() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        // No registry, scan specific type directory
        std::fs::create_dir_all(forge_dir.join("mcp")).unwrap();
        std::fs::write(forge_dir.join("mcp").join("server1.json"), "{}").unwrap();

        cmd_list(&forge_dir, "mcp").unwrap();
    }

    #[test]
    fn test_cmd_learning_history_with_limit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        let learning_dir = forge_dir.join("learning");
        std::fs::create_dir_all(&learning_dir).unwrap();

        // Create 5 entries, limit to 2
        let entries: Vec<String> = (0..5).map(|i| {
            serde_json::json!({
                "timestamp": format!("2026-01-0{}T00:00:00Z", i + 1),
                "patterns_found": i,
                "actions_generated": i * 2,
                "actions_deployed": i
            }).to_string()
        }).collect();
        std::fs::write(learning_dir.join("learning_cycles.jsonl"), entries.join("\n")).unwrap();

        cmd_learning_history(&forge_dir, 2).unwrap();
    }

    #[test]
    fn test_cmd_learning_history_invalid_jsonl_line() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        let learning_dir = forge_dir.join("learning");
        std::fs::create_dir_all(&learning_dir).unwrap();

        // Mix of valid and invalid JSON lines
        let jsonl = r#"{"timestamp":"2026-01-01","patterns_found":1,"actions_generated":1,"actions_deployed":1}
invalid json line
{"timestamp":"2026-01-02","patterns_found":2,"actions_generated":2,"actions_deployed":2}"#;
        std::fs::write(learning_dir.join("learning_cycles.jsonl"), jsonl).unwrap();

        cmd_learning_history(&forge_dir, 10).unwrap();
    }

    #[test]
    fn test_learning_enable_creates_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        save_forge_config(&forge_dir, &default_forge_config()).unwrap();

        cmd_learning_enable(&forge_dir).unwrap();

        assert!(forge_dir.join("learning").exists());
        assert!(forge_dir.join("traces").exists());
    }

    #[test]
    fn test_forge_config_round_trip_preserves_custom_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("custom_field".to_string(), serde_json::json!("custom_value"));
            obj.insert("collect_interval_sec".to_string(), serde_json::json!(600));
        }
        save_forge_config(tmp.path(), &cfg).unwrap();

        let loaded = load_forge_config(tmp.path());
        assert_eq!(loaded["custom_field"], "custom_value");
        assert_eq!(loaded["collect_interval_sec"], 600);
    }
