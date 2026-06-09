use super::*;

    #[test]
    fn test_parse_positional_input_key_value() {
        let args = vec!["name=hello".to_string(), "count=42".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["name"], serde_json::Value::String("hello".to_string()));
        assert_eq!(map["count"], serde_json::Value::Number(42i64.into()));
    }

    #[test]
    fn test_parse_positional_input_boolean() {
        let args = vec!["enabled=true".to_string(), "disabled=false".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["enabled"], serde_json::Value::Bool(true));
        assert_eq!(map["disabled"], serde_json::Value::Bool(false));
    }

    #[test]
    fn test_parse_positional_input_float() {
        let args = vec!["rate=3.14".to_string()];
        let map = parse_positional_input(&args);
        // Float should be a number
        assert!(map["rate"].is_number());
    }

    #[test]
    fn test_parse_positional_input_string_no_equals() {
        let args = vec!["hello world".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["input"], serde_json::Value::String("hello world".to_string()));
    }

    #[test]
    fn test_parse_positional_input_no_equals_only_first() {
        let args = vec!["first".to_string(), "second".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map.len(), 1); // Only first gets "input" key
        assert_eq!(map["input"], serde_json::Value::String("first".to_string()));
    }

    #[test]
    fn test_parse_positional_input_mixed() {
        let args = vec!["some input".to_string(), "key=value".to_string(), "num=10".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map.len(), 3);
        assert_eq!(map["input"], "some input");
        assert_eq!(map["key"], "value");
        assert_eq!(map["num"], 10);
    }

    #[test]
    fn test_parse_positional_input_empty() {
        let args: Vec<String> = vec![];
        let map = parse_positional_input(&args);
        assert!(map.is_empty());
    }

    #[test]
    fn test_scan_workflow_files_nonexistent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("nonexistent");
        let files = scan_workflow_files(&dir);
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_workflow_files_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        let files = scan_workflow_files(&dir);
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_workflow_files_finds_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.yaml"), "name: test").unwrap();
        std::fs::write(dir.join("test2.yml"), "name: test2").unwrap();
        std::fs::write(dir.join("data.txt"), "not a workflow").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 2);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"test"));
        assert!(names.contains(&"test2"));
    }

    #[test]
    fn test_scan_workflow_files_finds_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workflow.json"), r#"{"name": "test"}"#).unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "workflow");
    }

    #[test]
    fn test_scan_workflow_files_skips_executions_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        let exec_dir = dir.join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(dir.join("real.yaml"), "name: real").unwrap();
        std::fs::write(exec_dir.join("exec1.json"), "{}").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "real");
    }

    #[test]
    fn test_scan_workflow_files_sorted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("z_last.yaml"), "name: z").unwrap();
        std::fs::write(dir.join("a_first.yaml"), "name: a").unwrap();
        std::fs::write(dir.join("m_middle.yaml"), "name: m").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files[0].0, "a_first");
        assert_eq!(files[1].0, "m_middle");
        assert_eq!(files[2].0, "z_last");
    }

    #[test]
    fn test_count_executions_no_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(count_executions(tmp.path()), 0);
    }

    #[test]
    fn test_count_executions_with_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(exec_dir.join("exec1.json"), "{}").unwrap();
        std::fs::write(exec_dir.join("exec2.json"), "{}").unwrap();
        std::fs::write(exec_dir.join("not_json.txt"), "text").unwrap();

        assert_eq!(count_executions(tmp.path()), 2);
    }

    #[test]
    fn test_format_datetime() {
        use chrono::TimeZone;
        let dt = chrono::Local.with_ymd_and_hms(2026, 1, 15, 10, 30, 45).unwrap();
        let formatted = format_datetime(&dt);
        assert_eq!(formatted, "2026-01-15 10:30:45");
    }

    #[test]
    fn test_get_default_templates_count() {
        let templates = get_default_templates();
        assert_eq!(templates.len(), 5); // researcher, coder, monitor, collector, translator
    }

    #[test]
    fn test_get_default_templates_names() {
        let templates = get_default_templates();
        let names: Vec<&str> = templates.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"coder"));
        assert!(names.contains(&"monitor"));
        assert!(names.contains(&"collector"));
        assert!(names.contains(&"translator"));
    }

    #[test]
    fn test_get_default_templates_have_nodes() {
        let templates = get_default_templates();
        for (name, _, def) in &templates {
            let nodes = def.get("nodes").and_then(|v| v.as_array());
            assert!(nodes.is_some(), "Template '{}' should have nodes", name);
            assert!(!nodes.unwrap().is_empty(), "Template '{}' should have non-empty nodes", name);
        }
    }

    #[test]
    fn test_get_default_templates_have_edges() {
        let templates = get_default_templates();
        for (name, _, def) in &templates {
            let edges = def.get("edges").and_then(|v| v.as_array());
            assert!(edges.is_some(), "Template '{}' should have edges", name);
            assert!(!edges.unwrap().is_empty(), "Template '{}' should have non-empty edges", name);
        }
    }

    #[test]
    fn test_cmd_list_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&dir).unwrap();
        cmd_list(&dir).unwrap();
    }

    #[test]
    fn test_cmd_status_no_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        cmd_status(tmp.path(), None).unwrap();
    }

    #[test]
    fn test_cmd_status_specific_id_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        cmd_status(tmp.path(), Some("nonexistent-id")).unwrap();
    }

    #[test]
    fn test_cmd_template_show_not_found() {
        cmd_template_show("nonexistent_template").unwrap();
    }

    #[test]
    fn test_cmd_template_show_found() {
        cmd_template_show("researcher").unwrap();
    }

    #[test]
    fn test_cmd_validate_nonexistent() {
        cmd_validate("/nonexistent/file.yaml").unwrap();
    }

    #[test]
    fn test_parse_positional_input_whitespace() {
        let args = vec!["  key  =  value  ".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["key"], "value");
    }

    #[test]
    fn test_parse_positional_input_negative_number() {
        let args = vec!["offset=-5".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["offset"], -5);
    }

    // -------------------------------------------------------------------------
    // get_default_templates detailed tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_researcher_template_structure() {
        let templates = get_default_templates();
        let researcher = templates.iter().find(|(n, _, _)| *n == "researcher").unwrap();
        let def = &researcher.2;
        assert_eq!(def["name"], "researcher");
        assert_eq!(def["version"], "1.0.0");
        let nodes = def["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        let edges = def["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 2);
    }

    #[test]
    fn test_coder_template_structure() {
        let templates = get_default_templates();
        let coder = templates.iter().find(|(n, _, _)| *n == "coder").unwrap();
        let def = &coder.2;
        let nodes = def["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);
        // Review has condition on edge
        let edges = def["edges"].as_array().unwrap();
        assert_eq!(edges[1]["condition"], "approved");
    }

    #[test]
    fn test_monitor_template_structure() {
        let templates = get_default_templates();
        let monitor = templates.iter().find(|(n, _, _)| *n == "monitor").unwrap();
        let def = &monitor.2;
        // Has a condition node
        let nodes = def["nodes"].as_array().unwrap();
        let node_types: Vec<&str> = nodes.iter()
            .filter_map(|n| n.get("node_type").and_then(|v| v.as_str()))
            .collect();
        assert!(node_types.contains(&"condition"));
    }

    #[test]
    fn test_translator_template_two_nodes() {
        let templates = get_default_templates();
        let translator = templates.iter().find(|(n, _, _)| *n == "translator").unwrap();
        let def = &translator.2;
        let nodes = def["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn test_collector_template_transform_node() {
        let templates = get_default_templates();
        let collector = templates.iter().find(|(n, _, _)| *n == "collector").unwrap();
        let def = &collector.2;
        let nodes = def["nodes"].as_array().unwrap();
        let node_types: Vec<&str> = nodes.iter()
            .filter_map(|n| n.get("node_type").and_then(|v| v.as_str()))
            .collect();
        assert!(node_types.contains(&"transform"));
    }

    // -------------------------------------------------------------------------
    // scan_workflow_files additional edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_scan_workflow_files_nested_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        let nested = dir.join("category1").join("sub1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("nested.yaml"), "name: nested").unwrap();
        std::fs::write(dir.join("root.json"), r#"{"name": "root"}"#).unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_scan_workflow_files_ignores_non_workflow_extensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("valid.yaml"), "name: test").unwrap();
        std::fs::write(dir.join("readme.md"), "# docs").unwrap();
        std::fs::write(dir.join("data.csv"), "a,b,c").unwrap();
        std::fs::write(dir.join("config.toml"), "key = 'val'").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "valid");
    }

    // -------------------------------------------------------------------------
    // parse_positional_input additional edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_positional_input_zero() {
        let args = vec!["count=0".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["count"], 0);
    }

    #[test]
    fn test_parse_positional_input_large_integer() {
        let args = vec!["big=9999999999".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["big"], 9999999999i64);
    }

    #[test]
    fn test_parse_positional_input_equals_in_value() {
        let args = vec!["key=val=ue".to_string()];
        let map = parse_positional_input(&args);
        // split_once only splits on first '='
        assert_eq!(map["key"], "val=ue");
    }

    #[test]
    fn test_parse_positional_input_empty_value() {
        let args = vec!["key=".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["key"], "");
    }

    // -------------------------------------------------------------------------
    // cmd_status with actual execution files
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_status_with_execution_data() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        let exec_data = serde_json::json!({
            "id": "exec-001",
            "workflow_name": "test-flow",
            "state": "completed",
            "started_at": "2026-01-15T10:30:00Z",
            "ended_at": "2026-01-15T10:30:45Z"
        });
        std::fs::write(
            exec_dir.join("exec-001.json"),
            serde_json::to_string_pretty(&exec_data).unwrap()
        ).unwrap();

        cmd_status(tmp.path(), None).unwrap();
    }

    #[test]
    fn test_cmd_status_with_specific_execution() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        let exec_data = serde_json::json!({
            "id": "exec-002",
            "workflow_name": "detailed-flow",
            "state": "running",
            "started_at": "2026-01-15T10:00:00Z",
            "input": {"query": "test"},
            "variables": {"var1": "value1"},
            "node_results": {
                "node1": {
                    "state": "completed",
                    "started_at": "2026-01-15T10:00:00Z",
                    "ended_at": "2026-01-15T10:00:10Z"
                }
            }
        });
        std::fs::write(
            exec_dir.join("exec-002.json"),
            serde_json::to_string_pretty(&exec_data).unwrap()
        ).unwrap();

        cmd_status(tmp.path(), Some("exec-002")).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_template_create tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_template_create_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workflow_dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&workflow_dir).unwrap();

        cmd_template_create(&workflow_dir, "researcher", None).unwrap();

        let created = workflow_dir.join("researcher.yaml");
        assert!(created.exists());
    }

    #[test]
    fn test_cmd_template_create_json_explicit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workflow_dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&workflow_dir).unwrap();

        cmd_template_create(&workflow_dir, "coder", Some("myflow.json")).unwrap();

        let created = workflow_dir.join("myflow.json");
        assert!(created.exists());
    }

    #[test]
    fn test_cmd_template_create_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workflow_dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&workflow_dir).unwrap();

        cmd_template_create(&workflow_dir, "nonexistent_template", None).unwrap();
        // Should print error but not create file
        assert!(std::fs::read_dir(&workflow_dir).unwrap().count() == 0);
    }

    // -------------------------------------------------------------------------
    // cmd_validate with valid YAML workflow
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_validate_valid_workflow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.yaml");
        std::fs::write(&path, r#"
name: test-workflow
description: A test workflow
version: "1.0.0"
nodes:
  - id: step1
    node_type: tool
    config:
      tool_name: http_request
    depends_on: []
edges:
  - from_node: step1
    to_node: step1
"#).unwrap();

        cmd_validate(&path.to_string_lossy()).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_validate with invalid content
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_validate_invalid_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("bad.yaml");
        std::fs::write(&path, "not: valid: yaml: [[[[").unwrap();
        cmd_validate(&path.to_string_lossy()).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_status with multiple executions
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_status_multiple_executions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();

        for i in 0..5 {
            let exec_data = serde_json::json!({
                "id": format!("exec-{:03}", i),
                "workflow_name": format!("flow-{}", i),
                "state": if i % 2 == 0 { "completed" } else { "failed" },
                "started_at": "2026-01-15T10:00:00Z"
            });
            std::fs::write(
                exec_dir.join(format!("exec-{:03}.json", i)),
                serde_json::to_string(&exec_data).unwrap()
            ).unwrap();
        }

        cmd_status(tmp.path(), None).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_status with error in execution
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_status_execution_with_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        let exec_data = serde_json::json!({
            "id": "exec-err",
            "workflow_name": "failing-flow",
            "state": "failed",
            "started_at": "2026-01-15T10:00:00Z",
            "ended_at": "2026-01-15T10:00:10Z",
            "error": "Something went wrong",
            "node_results": {
                "node1": {
                    "state": "failed",
                    "error": "Node execution error",
                    "started_at": "2026-01-15T10:00:00Z",
                    "ended_at": "2026-01-15T10:00:05Z",
                    "output": "partial result"
                }
            }
        });
        std::fs::write(
            exec_dir.join("exec-err.json"),
            serde_json::to_string_pretty(&exec_data).unwrap()
        ).unwrap();

        cmd_status(tmp.path(), Some("exec-err")).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_status with variables and input
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_status_with_input_and_vars() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        let exec_data = serde_json::json!({
            "id": "exec-iv",
            "workflow_name": "param-flow",
            "state": "completed",
            "started_at": "2026-01-15T10:00:00Z",
            "ended_at": "2026-01-15T10:00:30Z",
            "input": {"query": "test query", "limit": 10},
            "variables": {"result_count": 5, "status": "ok"}
        });
        std::fs::write(
            exec_dir.join("exec-iv.json"),
            serde_json::to_string_pretty(&exec_data).unwrap()
        ).unwrap();

        cmd_status(tmp.path(), Some("exec-iv")).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_template_show all templates
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_template_show_all_defaults() {
        let templates = get_default_templates();
        for (name, _, _) in &templates {
            cmd_template_show(name).unwrap();
        }
    }

    // -------------------------------------------------------------------------
    // cmd_template_create with all templates
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_template_create_all_defaults() {
        let templates = get_default_templates();
        for (name, _, _) in &templates {
            let tmp = tempfile::TempDir::new().unwrap();
            let workflow_dir = tmp.path().join("workflow");
            std::fs::create_dir_all(&workflow_dir).unwrap();
            cmd_template_create(&workflow_dir, name, None).unwrap();
            assert!(workflow_dir.join(format!("{}.yaml", name)).exists(), "Template {} should be created", name);
        }
    }

    // -------------------------------------------------------------------------
    // cmd_validate with various invalid workflows
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_validate_empty_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("empty.yaml");
        std::fs::write(&path, "").unwrap();
        cmd_validate(&path.to_string_lossy()).unwrap();
    }

    #[test]
    fn test_cmd_validate_valid_json_workflow() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("test.json");
        std::fs::write(&path, r#"{"name": "test", "version": "1.0.0", "nodes": [{"id": "s1", "node_type": "tool", "config": {"tool_name": "echo"}, "depends_on": []}], "edges": []}"#).unwrap();
        cmd_validate(&path.to_string_lossy()).unwrap();
    }

    // -------------------------------------------------------------------------
    // get_default_templates descriptions
    // -------------------------------------------------------------------------

    #[test]
    fn test_get_default_templates_descriptions() {
        let templates = get_default_templates();
        for (name, desc, _) in &templates {
            assert!(!desc.is_empty(), "Template '{}' has empty description", name);
        }
    }

    // -------------------------------------------------------------------------
    // parse_positional_input additional edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_positional_input_multiple_no_equals() {
        // Only first no-equals arg gets "input" key
        let args = vec!["first".to_string(), "second".to_string(), "key=val".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map.len(), 2);
        assert_eq!(map["input"], "first");
        assert_eq!(map["key"], "val");
    }

    #[test]
    fn test_parse_positional_input_special_chars_in_value() {
        let args = vec!["path=/usr/local/bin".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["path"], "/usr/local/bin");
    }

    // -------------------------------------------------------------------------
    // format_datetime additional tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_format_datetime_midnight() {
        use chrono::TimeZone;
        let dt = chrono::Local.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap();
        let formatted = format_datetime(&dt);
        assert_eq!(formatted, "2026-12-31 00:00:00");
    }

    #[test]
    fn test_format_datetime_end_of_day() {
        use chrono::TimeZone;
        let dt = chrono::Local.with_ymd_and_hms(2026, 6, 15, 23, 59, 59).unwrap();
        let formatted = format_datetime(&dt);
        assert_eq!(formatted, "2026-06-15 23:59:59");
    }

    // -------------------------------------------------------------------------
    // scan_workflow_files edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_scan_workflow_files_deeply_nested() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        let deep = dir.join("a").join("b").join("c").join("d");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep.yaml"), "name: deep").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "deep");
    }

    #[test]
    fn test_scan_workflow_files_all_extensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.yaml"), "name: a").unwrap();
        std::fs::write(dir.join("b.yml"), "name: b").unwrap();
        std::fs::write(dir.join("c.json"), "{}").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 3);
    }

    // -------------------------------------------------------------------------
    // count_executions edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_count_executions_with_subdirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        let subdir = exec_dir.join("subdir");
        std::fs::create_dir_all(&subdir).unwrap();
        std::fs::write(exec_dir.join("exec1.json"), "{}").unwrap();
        std::fs::write(subdir.join("exec2.json"), "{}").unwrap();

        // Only counts files, not subdirs
        assert_eq!(count_executions(tmp.path()), 1);
    }

    #[test]
    fn test_count_executions_mixed_extensions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(exec_dir.join("exec1.json"), "{}").unwrap();
        std::fs::write(exec_dir.join("exec2.yaml"), "name: test").unwrap();
        std::fs::write(exec_dir.join("exec3.txt"), "text").unwrap();

        assert_eq!(count_executions(tmp.path()), 1);
    }

    // -------------------------------------------------------------------------
    // cmd_list with actual workflow files
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_list_with_workflow_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.yaml"), r#"
name: test-flow
description: A test
version: "1.0.0"
nodes:
  - id: s1
    node_type: tool
    config:
      tool_name: echo
    depends_on: []
edges: []
"#).unwrap();

        cmd_list(&dir).unwrap();
    }

    #[test]
    fn test_cmd_list_with_executions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflow");
        let exec_dir = dir.join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(exec_dir.join("e1.json"), "{}").unwrap();
        std::fs::write(exec_dir.join("e2.json"), "{}").unwrap();

        cmd_list(&dir).unwrap();
    }
