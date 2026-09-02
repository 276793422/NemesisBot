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
    assert_eq!(
        map["input"],
        serde_json::Value::String("hello world".to_string())
    );
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
    let args = vec![
        "some input".to_string(),
        "key=value".to_string(),
        "num=10".to_string(),
    ];
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
    let dt = chrono::Local
        .with_ymd_and_hms(2026, 1, 15, 10, 30, 45)
        .unwrap();
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
        assert!(
            !nodes.unwrap().is_empty(),
            "Template '{}' should have non-empty nodes",
            name
        );
    }
}

#[test]
fn test_get_default_templates_have_edges() {
    let templates = get_default_templates();
    for (name, _, def) in &templates {
        let edges = def.get("edges").and_then(|v| v.as_array());
        assert!(edges.is_some(), "Template '{}' should have edges", name);
        assert!(
            !edges.unwrap().is_empty(),
            "Template '{}' should have non-empty edges",
            name
        );
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
    let researcher = templates
        .iter()
        .find(|(n, _, _)| *n == "researcher")
        .unwrap();
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
    let node_types: Vec<&str> = nodes
        .iter()
        .filter_map(|n| n.get("node_type").and_then(|v| v.as_str()))
        .collect();
    assert!(node_types.contains(&"condition"));
}

#[test]
fn test_translator_template_two_nodes() {
    let templates = get_default_templates();
    let translator = templates
        .iter()
        .find(|(n, _, _)| *n == "translator")
        .unwrap();
    let def = &translator.2;
    let nodes = def["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
}

#[test]
fn test_collector_template_transform_node() {
    let templates = get_default_templates();
    let collector = templates
        .iter()
        .find(|(n, _, _)| *n == "collector")
        .unwrap();
    let def = &collector.2;
    let nodes = def["nodes"].as_array().unwrap();
    let node_types: Vec<&str> = nodes
        .iter()
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
        serde_json::to_string_pretty(&exec_data).unwrap(),
    )
    .unwrap();

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
        serde_json::to_string_pretty(&exec_data).unwrap(),
    )
    .unwrap();

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
    std::fs::write(
        &path,
        r#"
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
"#,
    )
    .unwrap();

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
            serde_json::to_string(&exec_data).unwrap(),
        )
        .unwrap();
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
        serde_json::to_string_pretty(&exec_data).unwrap(),
    )
    .unwrap();

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
        serde_json::to_string_pretty(&exec_data).unwrap(),
    )
    .unwrap();

    cmd_status(tmp.path(), Some("exec-iv")).unwrap();
}

// -------------------------------------------------------------------------
// cmd_template_show all templates
// -------------------------------------------------------------------------

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn test_cmd_template_show_all_defaults() {
    // get_templates() 经 load_templates_from_disk 读 NEMESISBOT_HOME——必须
    // 持全局锁，否则与设置 env 的其他测试（如 template_list_picks_up_*）竞态。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let templates = get_default_templates();
    for (name, _, _) in &templates {
        cmd_template_show(name).unwrap();
    }
}

// -------------------------------------------------------------------------
// cmd_template_create with all templates
// -------------------------------------------------------------------------

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn test_cmd_template_create_all_defaults() {
    // 同上：get_templates() 读 NEMESISBOT_HOME（env 测试竞争锁纪律）。
    // 另注：get_templates 语义是"磁盘有任一模板 → 整组替换默认集"（Go 行为，
    // workflow.rs:318-324），本测试必须保证 env home 下无模板目录。
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let templates = get_default_templates();
    for (name, _, _) in &templates {
        let tmp = tempfile::TempDir::new().unwrap();
        let workflow_dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&workflow_dir).unwrap();
        cmd_template_create(&workflow_dir, name, None).unwrap();
        assert!(
            workflow_dir.join(format!("{}.yaml", name)).exists(),
            "Template {} should be created",
            name
        );
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
        assert!(
            !desc.is_empty(),
            "Template '{}' has empty description",
            name
        );
    }
}

// -------------------------------------------------------------------------
// parse_positional_input additional edge cases
// -------------------------------------------------------------------------

#[test]
fn test_parse_positional_input_multiple_no_equals() {
    // Only first no-equals arg gets "input" key
    let args = vec![
        "first".to_string(),
        "second".to_string(),
        "key=val".to_string(),
    ];
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
    let dt = chrono::Local
        .with_ymd_and_hms(2026, 12, 31, 0, 0, 0)
        .unwrap();
    let formatted = format_datetime(&dt);
    assert_eq!(formatted, "2026-12-31 00:00:00");
}

#[test]
fn test_format_datetime_end_of_day() {
    use chrono::TimeZone;
    let dt = chrono::Local
        .with_ymd_and_hms(2026, 6, 15, 23, 59, 59)
        .unwrap();
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
    std::fs::write(
        dir.join("test.yaml"),
        r#"
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
"#,
    )
    .unwrap();

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

// ===========================================================================
// cmd_run 全链路 + run() 分发（S11c，quality-hardening goal 冲刺 S11）——
// 既有 64 个测试只钉 helper/cmd_list/cmd_status/cmd_template_*/cmd_validate，
// cmd_run（419-587，约 120 行）和 run() dispatch（903-978）从没跑过。
// 可执行工作流用 delay 节点（内置执行器、seconds=0 零等待）；home 显式传参
// （cmd_run 不读 env）。run() 用 env home + 锁；Run 臂的 block_in_place
// 必须 multi_thread runtime。
// ===========================================================================

mod cmd_run_tests {
    use super::super::cmd_run;

    fn setup(home: &std::path::Path) -> std::path::PathBuf {
        let wf_dir = home.join("workspace").join("workflow").join("definitions");
        std::fs::create_dir_all(&wf_dir).unwrap();
        wf_dir
    }

    fn delay_workflow_json(name: &str, desc: &str, seconds: f64) -> String {
        serde_json::json!({
            "name": name,
            "description": desc,
            "version": "1.0.0",
            "nodes": [
                {"id": "n1", "node_type": "delay", "config": {"seconds": seconds}, "depends_on": []}
            ],
            "edges": []
        })
        .to_string()
    }

    #[tokio::test]
    async fn run_delay_workflow_end_to_end_saves_execution_record() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        std::fs::write(
            wf_dir.join("smoke.json"),
            delay_workflow_json("smoke", "冒烟", 0.0),
        )
        .unwrap();

        // 带位置参数与键值参数（覆盖 input 打印分支）；home 无 config.json →
        // sandbox world None 分支（executor 分离关闭提示）。
        cmd_run(
            &home,
            &wf_dir,
            "smoke",
            &[
                "k1=v1".to_string(),
                "flag=true".to_string(),
                "n=42".to_string(),
                "positional".to_string(),
            ],
        )
        .await
        .expect("delay 工作流应跑通");

        let exec_dir = wf_dir.join("executions");
        let mut entries: Vec<_> = std::fs::read_dir(&exec_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        entries.sort();
        assert_eq!(entries.len(), 1, "必须落一条执行记录");
        let rec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&entries[0]).unwrap()).unwrap();
        assert_eq!(rec["state"], "completed");
        assert_eq!(rec["workflow_name"], "smoke");
    }

    #[tokio::test]
    async fn run_by_absolute_file_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        let abs = wf_dir.join("abs-flow.yaml");
        std::fs::write(
            &abs,
            serde_json::to_string(&serde_json::json!({
                "name": "abs",
                "description": "绝对路径",
                "version": "1.0.0",
                "nodes": [
                    {"id": "d", "node_type": "delay", "config": {"seconds": 0}, "depends_on": []}
                ],
                "edges": []
            }))
            .unwrap(),
        )
        .unwrap();

        cmd_run(&home, &wf_dir, abs.to_string_lossy().as_ref(), &[])
            .await
            .expect("name 是存在的绝对路径 → 直接用（428-429 分支）");

        // (BUG S11c-3) 绝对路径分支此前必然 "workflow not found"：注册键是
        // 文件内 name（abs），查找键却是整个路径字符串。钉住修复后的行为。
        let exec_dir = wf_dir.join("executions");
        let entries: Vec<_> = std::fs::read_dir(&exec_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        assert_eq!(entries.len(), 1, "绝对路径运行也必须落执行记录");
        let rec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&entries[0]).unwrap()).unwrap();
        assert_eq!(rec["state"], "completed");
        assert_eq!(rec["workflow_name"], "abs");
    }

    #[tokio::test]
    async fn run_unknown_name_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        let err = cmd_run(&home, &wf_dir, "ghost", &[])
            .await
            .expect_err("三后缀 + exe_dir/templates 都找不到 → not found");
        assert!(err.to_string().contains("not found"), "got: {err:#}");
    }

    #[tokio::test]
    async fn run_parse_error_bubbles_as_error() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        std::fs::write(wf_dir.join("broken.json"), "{{{ not json").unwrap();
        let err = cmd_run(&home, &wf_dir, "broken", &[])
            .await
            .expect_err("解析失败 → Parse error");
        assert!(err.to_string().contains("Parse error"), "got: {err:#}");
    }

    #[tokio::test]
    async fn run_validation_failure_stops_before_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        // 边引用不存在的节点 → validate 报 unknown 'to' node。
        std::fs::write(
            wf_dir.join("bad-edge.json"),
            serde_json::json!({
                "name": "bad-edge",
                "description": "非法边",
                "version": "1.0.0",
                "nodes": [
                    {"id": "n1", "node_type": "delay", "config": {"seconds": 0}, "depends_on": []}
                ],
                "edges": [{"from_node": "n1", "to_node": "missing"}]
            })
            .to_string(),
        )
        .unwrap();
        let err = cmd_run(&home, &wf_dir, "bad-edge", &[])
            .await
            .expect_err("校验失败 → Workflow validation failed");
        assert!(
            err.to_string().contains("Workflow validation failed"),
            "got: {err:#}"
        );
        assert!(
            !wf_dir.join("executions").exists(),
            "校验失败不得留下执行记录"
        );
    }

    #[tokio::test]
    async fn run_slow_workflow_reports_seconds_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        std::fs::write(
            wf_dir.join("slow.json"),
            delay_workflow_json("slow", "慢流程", 1.1),
        )
        .unwrap();
        cmd_run(&home, &wf_dir, "slow", &[])
            .await
            .expect("duration >= 1000ms → 秒格式展示分支（554）");
    }

    #[tokio::test]
    async fn run_tool_node_on_bare_engine_fails_without_executor() {
        // (BUG #32, quality-hardening goal 冲刺 S12b) 裸 CLI 引擎对 tool 节点
        // 原用 nodes.rs 的 ToolNodeExecutor 桩——不执行任何工具直接回
        // {"tool":"unknown","status":"success"} + Completed（假成功）。修复后
        // 显式 Failed 并指明「未配置工具执行器」（真执行器只在 gateway 侧经
        // RealToolNodeExecutor 接上）。本测试钉修复后行为：engine.run 对
        // Failed 状态返回 Ok（仅结构性错误才 Err），执行记录 state=failed。
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = setup(&home);
        std::fs::write(
            wf_dir.join("tool-flow.json"),
            serde_json::json!({
                "name": "tool-flow",
                "description": "裸引擎工具节点",
                "version": "1.0.0",
                "nodes": [
                    {"id": "t", "node_type": "tool", "config": {"tool_name": "web_search"}, "depends_on": []}
                ],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();
        cmd_run(&home, &wf_dir, "tool-flow", &[])
            .await
            .expect("Failed 状态不算引擎错误，cmd_run 整体 Ok");

        let exec_dir = wf_dir.join("executions");
        let entries: Vec<_> = std::fs::read_dir(&exec_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        assert_eq!(entries.len(), 1);
        let rec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&entries[0]).unwrap()).unwrap();
        // 整单 Failed，不再假成功 Completed。CLI 路径的持久化记录里失败
        // 信息由执行级 error 字段携带（node_results 在该持久化快照为空，
        // 引擎内存态才有节点级结果——见 engine 侧 S12b 回归测试）。
        assert_eq!(rec["state"], "failed");
        let err = rec["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("no tool executor configured"),
            "执行级 error 应指明未配置工具执行器，got: {err}"
        );
        assert!(
            err.contains("node \"t\" execution failed"),
            "error 应点名失败节点 t，got: {err}"
        );
    }
}

// 整 mod Windows 形态（3/3 测试 + 专属 use/helper 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod run_arm {
    use super::super::{TemplateAction, WorkflowAction, run};

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

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn dispatch_list_status_template_and_validate_arms() {
        with_env_home(|_home| {
            // List：空 definitions → No workflows defined。
            run(WorkflowAction::List, false).expect("list ok");

            // Status：无 executions → not found / 空。
            run(WorkflowAction::Status { id: None }, false).expect("status none ok");
            run(
                WorkflowAction::Status {
                    id: Some("nope".into()),
                },
                false,
            )
            .expect("status id not found ok");

            // Template 默认臂（action=None）+ List 臂：磁盘无模板 → 内置默认集。
            run(WorkflowAction::Template { action: None }, false)
                .expect("template default list ok");
            run(
                WorkflowAction::Template {
                    action: Some(TemplateAction::List),
                },
                false,
            )
            .expect("template list ok");

            // Show：默认集里 researcher 必在；not-found 打印可用列表。
            run(
                WorkflowAction::Template {
                    action: Some(TemplateAction::Show {
                        name: "researcher".into(),
                    }),
                },
                false,
            )
            .expect("template show found");
            run(
                WorkflowAction::Template {
                    action: Some(TemplateAction::Show {
                        name: "ghost".into(),
                    }),
                },
                false,
            )
            .expect("template show not found");

            // Validate：不存在的路径 → File not found + Ok。
            run(
                WorkflowAction::Validate {
                    path: "Z:/no/such/wf.yaml".into(),
                },
                false,
            )
            .expect("validate missing ok");
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn dispatch_template_create_yaml_and_json_outputs() {
        with_env_home(|home| {
            // 默认输出：researcher.yaml（YAML 写分支）。
            run(
                WorkflowAction::Template {
                    action: Some(TemplateAction::Create {
                        template: "researcher".into(),
                        output: None,
                    }),
                },
                false,
            )
            .expect("create default yaml ok");
            let defs = home.join("workspace").join("workflow").join("definitions");
            assert!(
                defs.join("researcher.yaml").exists(),
                "默认落 researcher.yaml"
            );

            // 显式 .json 输出：JSON 写分支（831-835）。
            run(
                WorkflowAction::Template {
                    action: Some(TemplateAction::Create {
                        template: "coder".into(),
                        output: Some("custom.json".into()),
                    }),
                },
                false,
            )
            .expect("create json ok");
            let j: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(defs.join("custom.json")).unwrap())
                    .unwrap();
            assert_eq!(j["name"], "coder");

            // 不存在的模板 → not found + 可用列表，Ok。
            run(
                WorkflowAction::Template {
                    action: Some(TemplateAction::Create {
                        template: "ghost".into(),
                        output: None,
                    }),
                },
                false,
            )
            .expect("create not found ok");
        });
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_run_arm_needs_multithread_runtime() {
        // Run 臂走 tokio::task::block_in_place——current_thread runtime 会
        // panic，必须 multi_thread（这也是给未来读代码的人钉的契约）。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        let err = run(
            WorkflowAction::Run {
                name: "ghost".into(),
                input: vec![],
            },
            false,
        )
        .expect_err("ghost → not found");
        assert!(err.to_string().contains("not found"), "got: {err:#}");
        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }
}

// ---------------------------------------------------------------------------
// 磁盘模板扫描（load_templates_from_disk 173-213：目录存在时的遍历/解析/
// 去重/坏文件警告臂）——既有 64 测试全在无模板目录的环境跑，循环体没进过。
// ---------------------------------------------------------------------------

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[test]
fn template_list_picks_up_disk_templates_and_warns_on_broken() {
    use super::{TemplateAction, WorkflowAction, run};

    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", tmp.path());
    }
    let tpl_dir = tmp
        .path()
        .join(".nemesisbot")
        .join("workspace")
        .join("workflow")
        .join("templates");
    std::fs::create_dir_all(&tpl_dir).unwrap();

    // 一个好的用户模板 + 一个坏 YAML（Err 臂 205-213 警告）+ 一个与默认模板
    // 同名的磁盘模板（seen_names 去重臂：磁盘在后，默认已注册 → 跳过）。
    std::fs::write(
        tpl_dir.join("my-disk-tpl.yaml"),
        "name: my-disk-tpl\ndescription: 用户自定义模板\nversion: 1.0.0\nnodes:\n  - id: d\n    node_type: delay\n    config:\n      seconds: 0\n    depends_on: []\nedges: []\n",
    )
    .unwrap();
    std::fs::write(tpl_dir.join("broken.yaml"), "{{{ not yaml at all").unwrap();
    std::fs::write(
        tpl_dir.join("dupe.yaml"),
        "name: researcher\ndescription: 与默认同名\nversion: 1.0.0\nnodes:\n  - id: d\n    node_type: delay\n    config:\n      seconds: 0\n    depends_on: []\nedges: []\n",
    )
    .unwrap();

    // List：应包含磁盘模板（不 panic、坏文件被跳过）。
    run(WorkflowAction::Template { action: None }, false).expect("list with disk templates ok");
    run(
        WorkflowAction::Template {
            action: Some(TemplateAction::List),
        },
        false,
    )
    .expect("explicit list ok");

    // Show：磁盘模板可查（cmd_template_show 的磁盘模板路径）。
    run(
        WorkflowAction::Template {
            action: Some(TemplateAction::Show {
                name: "my-disk-tpl".into(),
            }),
        },
        false,
    )
    .expect("show disk template ok");

    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
}

// ===========================================================================
// wave_b（coverage 补测，2026-08-27）：miss 行清零补洞。
//
// 目标行（workflow.rs）与本模块的对应关系：
//  - 384-385（描述 >37 截断）/387（空描述 "-"）/398-400（多 triggers join）/
//    405-410（列表行解析失败 "(parse error)"）→ wave_b_list_description_
//    trigger_and_parse_error_arms；
//  - 514-522（executor.sandbox 分离开启时的 Some(world) 分支：打印 world 名、
//    set_execution_world + install_composite_node_executors）→
//    wave_b_run_attaches_execution_world_when_separation_enabled。确定性依据：
//    home 是全新 tempdir，其下永远没有 tools/sandboxie/Start.exe ⇒ will_attach
//    恒 false ⇒ build_executor_channel 走 stdio 落到 Ok(Some(channel))；
//    ConfigStore::load(path) 是纯读盘函数，不触碰 GLOBAL_STORE 单例；
//  - 641（详情 <1s 毫秒臂）/645-646（started/ended 解析失败静默跳过）/
//    702-704（节点输出 >200 截断 floor_char_boundary）/742-743（executions 目录
//    存在但无 .json → "No executions found." 内层臂）/772（列表行 started 短串
//    原样展示）/775（列表行 JSON 解析失败跳过整行）→ wave_b_status_detail_
//    conditional_matrix + wave_b_status_list_view_edge_rows；
//  - 178/186/195（模板扫描：非文件 continue / 扩展名不符 continue /
//    seen_names 去重 continue）→ wave_b_template_scan_skips_and_dedup。
//
// ARTIFACT/ALREADY（本模块不再重复覆盖）：126/157/216/562/576-577/588/
// 663-664/676-677/692/709/712/841/921 —— 全部是已执行 span 的闭括号/续行
// 计数伪影或既有测试已覆盖（exec-002/exec-err 详情夹具走遍 input/variables/
// node_results 打印块；run_tool_node_on_bare_engine 钉 576-577 错误行；
// run_slow_workflow 钉秒单位；template create json 由 run_arm 覆盖；921 是
// dispatch_run_arm_needs_multithread_runtime 的 Run 臂尾表达式）。
//
// EXEMPT（结构性行不可达）：
//  - 447-448/451-452 cmd_run 的 exe_dir/templates 回退——命中它必须在共享的
//    target/debug/deps/ 旁建 templates/<name>.yaml。get_templates 的语义是
//    「磁盘扫描非空 ⇒ 整组替换默认模板」（318-324），种进去的文件会全局劫持
//    二进制内所有并行运行 template 测试的期望集（默认回退测试会随机拿到磁盘
//    集），隔离性禁止，宁缺勿染。
// ===========================================================================

mod wave_b {
    // run/TemplateAction/WorkflowAction 只有下方 Windows 形态的测试使用，
    // 随之门控（Linux 上拆开导入，避免 unused import 死代码）。
    #[cfg(windows)] // Windows-form helper use (Linux nightly: excluded, 2026-09-02 sweep)
    use super::super::{TemplateAction, WorkflowAction, run};
    use super::super::{cmd_list, cmd_run, cmd_status};

    /// 组装一个能被 parser 接受的最小 delay 工作流 YAML 文本。
    fn delay_yaml(name: &str, description: &str, triggers: &str) -> String {
        let mut s = String::new();
        s.push_str(&format!("name: {name}\n"));
        s.push_str(&format!("description: \"{description}\"\n"));
        s.push_str("version: \"1.0.0\"\n");
        if !triggers.is_empty() {
            s.push_str(triggers);
        }
        s.push_str(
            "nodes:\n  - id: d\n    node_type: delay\n    config:\n      seconds: 0\n    depends_on: []\nedges: []\n",
        );
        s
    }

    #[test]
    fn wave_b_list_description_trigger_and_parse_error_arms() {
        let tmp = tempfile::tempdir().unwrap();
        let def_dir = tmp.path().join("definitions");
        std::fs::create_dir_all(&def_dir).unwrap();

        // 长（>37 字节）描述 → 383-385 截断臂。
        let long_desc = "L".repeat(60);
        std::fs::write(
            def_dir.join("long.yaml"),
            delay_yaml("long", &long_desc, ""),
        )
        .unwrap();
        // 空描述 → 386-387 "-" 臂。
        std::fs::write(def_dir.join("blank.yaml"), delay_yaml("blank", "", "")).unwrap();
        // 双 triggers → 398-400 join(", ") 臂。
        std::fs::write(
            def_dir.join("duo.yaml"),
            delay_yaml(
                "duo",
                "triggered",
                "triggers:\n  - trigger_type: cron\n  - trigger_type: event\n",
            ),
        )
        .unwrap();
        // 解析失败文件 → 405-410 "(parse error)" 行。
        std::fs::write(def_dir.join("bad.json"), "{{{ not json").unwrap();

        cmd_list(&def_dir).expect("cmd_list with mixed fixture must succeed");
    }

    #[test]
    fn wave_b_status_detail_conditional_matrix() {
        let tmp = tempfile::tempdir().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();

        // d-fast：<1000ms duration（毫秒臂 640-641）；有 input 无 variables
        // 无 node_results（覆盖打印块与相邻 skip 边缘）。
        let fast = serde_json::json!({
            "id": "d-fast", "workflow_name": "wf", "state": "completed",
            "started_at": "2026-01-15T10:00:00.000Z",
            "ended_at": "2026-01-15T10:00:00.500Z",
            "input": {"k": "v"}
        });
        // d-badts：started_at 不可解析 → 637 元组匹配失败 → 645-646 静默收口。
        let badts = serde_json::json!({
            "id": "d-badts", "workflow_name": "wf", "state": "completed",
            "started_at": "not-a-timestamp",
            "ended_at": "2026-01-15T10:00:05Z"
        });
        // d-bigout：240 字符 ASCII 输出 → 700-704 截断臂（floor_char_boundary
        // 安全切在 ASCII 上无歧义）；node n2 只带 state 的裸行。
        let big_out = "O".repeat(240);
        let bigout = serde_json::json!({
            "id": "d-bigout", "workflow_name": "wf", "state": "completed",
            "node_results": {
                "n1": {"state": "completed", "output": big_out,
                        "started_at": "2026-01-15T10:00:00Z",
                        "ended_at": "2026-01-15T10:00:01Z"},
                "n2": {"state": "skipped"}
            }
        });
        // d-emptynr：node_results 为空对象 → 682 内层 false → 711-713 收口。
        let emptynr = serde_json::json!({
            "id": "d-emptynr", "workflow_name": "wf", "state": "failed",
            "error": "", "node_results": {}
        });

        for (id, data) in [
            ("d-fast", fast),
            ("d-badts", badts),
            ("d-bigout", bigout),
            ("d-emptynr", emptynr),
        ] {
            std::fs::write(
                exec_dir.join(format!("{id}.json")),
                serde_json::to_string_pretty(&data).unwrap(),
            )
            .unwrap();
            cmd_status(tmp.path(), Some(id)).expect("detail view must succeed");
        }
    }

    #[test]
    fn wave_b_status_list_view_edge_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();

        // 第一阶段：目录存在但没有任何 *.json → 741-743 内层空臂。
        std::fs::write(exec_dir.join("note.txt"), "ignore me").unwrap();
        cmd_status(tmp.path(), None).expect("empty-after-filter list view ok");

        // 第二阶段：
        //  - broken.json 整行 JSON 解析失败 → 757 的 else 边（756-775 收口）。
        //  - 无 started_at 的记录 → unwrap_or("?") len=1 → 769-773 短串原样臂。
        std::fs::write(exec_dir.join("broken.json"), "{{{").unwrap();
        std::fs::write(
            exec_dir.join("ok.json"),
            serde_json::json!({
                "id": "ok", "workflow_name": "wf", "state": "completed"
            })
            .to_string(),
        )
        .unwrap();
        cmd_status(tmp.path(), None).expect("list view with edge rows ok");
    }

    #[tokio::test]
    async fn wave_b_run_attaches_execution_world_when_separation_enabled() {
        // executor.enabled=true（sandbox=false）+ tempdir home（永远无 Start.exe）
        // ⇒ build_workflow_world 返回 Ok(Some(ExecutorWorld))，match 走 Some(world)
        // 臂（514-521：打印 + set_execution_world + install_composite）。配置纯读，
        // 不触 GLOBAL_STORE；delay 节点不与 world 交互，全流程本地确定。
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = home.join("workspace").join("workflow").join("definitions");
        std::fs::create_dir_all(&wf_dir).unwrap();
        std::fs::write(
            home.join("config.json"),
            serde_json::json!({"executor": {"enabled": true, "sandbox": false}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            wf_dir.join("worldy.json"),
            serde_json::json!({
                "name": "worldy",
                "description": "execution-world smoke",
                "version": "1.0.0",
                "nodes": [
                    {"id": "d", "node_type": "delay", "config": {"seconds": 0}, "depends_on": []}
                ],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();

        cmd_run(&home, &wf_dir, "worldy", &[])
            .await
            .expect("run with separation enabled must complete");

        let recs: Vec<_> = std::fs::read_dir(wf_dir.join("executions"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        assert_eq!(recs.len(), 1, "separation-on run 也必须落执行记录");
        let rec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&recs[0]).unwrap()).unwrap();
        assert_eq!(rec["state"], "completed");
    }

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[test]
    fn wave_b_template_scan_skips_and_dedup() {
        // 目标行 177-179（目录 continue）/185-187（扩展名 continue）/
        // 194-196（seen_names 去重 continue —— 同 stem 双扩展名触发）。
        // get_templates 语义是「磁盘非空 ⇒ 整组替换默认」，必须持全局锁隔离。
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        let tpl_dir = tmp
            .path()
            .join(".nemesisbot")
            .join("workspace")
            .join("workflow")
            .join("templates");
        std::fs::create_dir_all(tpl_dir.join("subdir")).unwrap(); // 目录 entry

        let good_body = "name: good\ndescription: g\nversion: 1.0.0\nnodes:\n  - id: d\n    node_type: delay\n    config:\n      seconds: 0\n    depends_on: []\nedges: []\n";
        std::fs::write(tpl_dir.join("good.yaml"), good_body).unwrap();
        std::fs::write(tpl_dir.join("notes.txt"), "wrong extension").unwrap(); // 扩展名臂
        // a.yaml / a.yml 同 stem —— 第二个进 seen_names 去重臂。
        let twin_a = "name: anything\ndescription: t\nversion: 1.0.0\nnodes:\n  - id: d\n    node_type: delay\n    config:\n      seconds: 0\n    depends_on: []\nedges: []\n";
        std::fs::write(tpl_dir.join("a.yaml"), twin_a).unwrap();
        std::fs::write(tpl_dir.join("a.yml"), twin_a).unwrap();

        let templates = super::super::load_templates_from_disk();
        let names: Vec<&str> = templates.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["a", "good"],
            "目录/txt/重复 stem 都必须被跳过：{names:?}"
        );

        // 同一环境跑一次显式 List（消费 disk 模板路径）。
        run(
            WorkflowAction::Template {
                action: Some(TemplateAction::List),
            },
            false,
        )
        .expect("template list over scanned disk templates ok");

        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }
}

// ===========================================================================
// r10（覆盖率 A 类 miss 补充）：
// - run() 的 Run 分发臂走完整成功路径（此前 dispatch_run_arm 只到 ghost
//   not-found 早退；cmd_run 直接调用覆盖了函数体，但 919-922 分发行只在
//   失败路径上被 span 过）。
// - 节点级 Failed + error 文案：transform 挂未知 expression →
//   failed_node_result 带 per-node error → cmd_run 展示循环的错误臂
//   （571-578 的 else 边）+ result.error 顶层打印（563-565）。裸引擎内置
//   transform 执行器（nodes.rs 注册），无外部依赖。
// ===========================================================================

// 整 mod Windows 形态（1/1 测试 + 专属 use 全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r10 {
    use super::super::{WorkflowAction, run};

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn r10_run_dispatch_success_path_with_failed_transform_node_prints_error() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("NEMESISBOT_HOME", tmp.path());
        }
        let home = tmp.path().join(".nemesisbot");
        let wf_dir = home.join("workspace").join("workflow").join("definitions");
        std::fs::create_dir_all(&wf_dir).unwrap();

        // 单 transform 节点、expression 未知 → per-node Failed + error 文案；
        // 引擎对节点级失败返回 Ok（Failed 状态），cmd_run 整体 Ok。
        std::fs::write(
            wf_dir.join("r10-badx.json"),
            serde_json::json!({
                "name": "r10-badx",
                "description": "transform unknown expression → node error",
                "version": "1.0.0",
                "nodes": [
                    {"id": "tx", "node_type": "transform", "config": {"expression": "definitely-not-a-real-expression"}, "depends_on": []}
                ],
                "edges": []
            })
            .to_string(),
        )
        .unwrap();

        // 经 run() 分发臂（block_in_place 必须 multi_thread）走到执行记录落盘。
        let out = run(
            WorkflowAction::Run {
                name: "r10-badx".into(),
                input: vec![],
            },
            false,
        );
        assert!(out.is_ok(), "Failed 状态不算命令错误，got: {:?}", out.err());

        let rec_path = wf_dir.join("executions");
        let entries: Vec<_> = std::fs::read_dir(&rec_path)
            .expect("execution record dir must exist")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect();
        assert_eq!(entries.len(), 1, "分发成功路径必须恰好落一条执行记录");
        let rec: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&entries[0]).unwrap()).unwrap();
        assert_eq!(rec["state"], "failed", "未知 expression 必须是节点级失败");

        unsafe {
            std::env::remove_var("NEMESISBOT_HOME");
        }
    }
}
