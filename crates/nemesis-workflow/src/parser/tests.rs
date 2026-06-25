use super::*;
    use crate::types::{Edge, NodeDef, TriggerConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_workflow(name: &str, nodes: Vec<NodeDef>) -> Workflow {
        Workflow {
            name: name.to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: vec![],
            nodes,
            edges: vec![],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    fn make_node(id: &str, node_type: &str) -> NodeDef {
        NodeDef {
            id: id.to_string(),
            node_type: node_type.to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        is_terminal: false,
        }
    }

    #[test]
    fn test_parse_json_workflow() {
        let json = r#"{
            "name": "test_wf",
            "description": "A test workflow",
            "version": "1.0.0",
            "nodes": [
                {"id": "n1", "node_type": "llm", "config": {}, "depends_on": [], "retry_count": 0}
            ],
            "edges": [],
            "triggers": [],
            "variables": {}
        }"#;

        let wf = parse_json(json.as_bytes()).unwrap();
        assert_eq!(wf.name, "test_wf");
        assert_eq!(wf.nodes.len(), 1);
        assert_eq!(wf.nodes[0].id, "n1");
    }

    #[test]
    fn test_parse_invalid_json() {
        let json = r#"{"invalid": json}"#;
        let result = parse_json(json.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_valid_workflow() {
        let wf = make_workflow("test", vec![make_node("n1", "llm")]);
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn test_validate_no_name() {
        let wf = make_workflow("", vec![make_node("n1", "llm")]);
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn test_validate_no_nodes() {
        let wf = make_workflow("test", vec![]);
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least one node"));
    }

    #[test]
    fn test_validate_duplicate_node_id() {
        let wf = make_workflow(
            "test",
            vec![make_node("n1", "llm"), make_node("n1", "tool")],
        );
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("duplicate"));
    }

    #[test]
    fn test_validate_invalid_edge() {
        let mut wf = make_workflow("test", vec![make_node("n1", "llm")]);
        wf.edges.push(Edge {
            from_node: "n1".to_string(),
            to_node: "nonexistent".to_string(),
            condition: None,
        });
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown"));
    }

    #[test]
    fn test_validate_invalid_depends_on() {
        let mut wf = make_workflow("test", vec![make_node("n1", "llm")]);
        wf.nodes[0].depends_on = vec!["nonexistent".to_string()];
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("depends_on unknown"));
    }

    #[test]
    fn test_validate_unknown_trigger_type() {
        let mut wf = make_workflow("test", vec![make_node("n1", "llm")]);
        wf.triggers.push(TriggerConfig {
            trigger_type: "unknown".to_string(),
            config: HashMap::new(),
        });
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown type"));
    }

    #[test]
    fn test_parse_yaml_workflow() {
        let yaml = r#"
name: yaml_test
description: A YAML test workflow
version: "1.0.0"
nodes:
  - id: n1
    node_type: llm
    config: {}
    depends_on: []
    retry_count: 0
edges: []
triggers: []
variables: {}
"#;
        let wf = parse(yaml.as_bytes()).unwrap();
        assert_eq!(wf.name, "yaml_test");
        assert_eq!(wf.nodes.len(), 1);
    }

    #[test]
    fn test_validate_empty_node_id() {
        let wf = make_workflow(
            "test",
            vec![NodeDef {
                id: String::new(),
                node_type: "llm".to_string(),
                config: HashMap::new(),
                depends_on: vec![],
                retry_count: 0,
                timeout: None,
            is_terminal: false,
            }],
        );
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("missing id"));
    }

    #[test]
    fn test_validate_edge_from_unknown_node() {
        let mut wf = make_workflow("test", vec![make_node("n1", "llm")]);
        wf.edges.push(Edge {
            from_node: "nonexistent".to_string(),
            to_node: "n1".to_string(),
            condition: None,
        });
        let result = validate(&wf);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("'from'"));
    }

    #[test]
    fn test_validate_valid_trigger_types() {
        for trigger_type in &["cron", "webhook", "event", "message"] {
            let mut wf = make_workflow("test", vec![make_node("n1", "llm")]);
            wf.triggers.push(TriggerConfig {
                trigger_type: trigger_type.to_string(),
                config: HashMap::new(),
            });
            assert!(validate(&wf).is_ok(), "Trigger type '{}' should be valid", trigger_type);
        }
    }

    #[test]
    fn test_validate_multi_node_dag() {
        let wf = Workflow {
            name: "multi_node".to_string(),
            description: String::new(),
            version: "1.0.0".to_string(),
            triggers: vec![],
            nodes: vec![
                make_node("start", "trigger"),
                make_node("process", "llm"),
                make_node("end", "output"),
            ],
            edges: vec![
                Edge { from_node: "start".to_string(), to_node: "process".to_string(), condition: None },
                Edge { from_node: "process".to_string(), to_node: "end".to_string(), condition: None },
            ],
            variables: HashMap::new(),
            metadata: HashMap::new(),
        };
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn test_parse_json_with_variables() {
        let json = r#"{
            "name": "var_wf",
            "nodes": [{"id": "n1", "node_type": "llm", "config": {}, "depends_on": [], "retry_count": 0}],
            "edges": [],
            "variables": {"input": "hello", "count": "5"}
        }"#;

        let wf = parse_json(json.as_bytes()).unwrap();
        assert_eq!(wf.variables.get("input").unwrap(), "hello");
    }

    #[test]
    fn test_parse_json_with_metadata() {
        let json = r#"{
            "name": "meta_wf",
            "nodes": [{"id": "n1", "node_type": "llm", "config": {}, "depends_on": [], "retry_count": 0}],
            "edges": [],
            "metadata": {"author": "test", "version": "2.0"}
        }"#;

        let wf = parse_json(json.as_bytes()).unwrap();
        assert_eq!(wf.metadata.get("author").unwrap(), "test");
    }

    #[test]
    fn test_parse_yaml_invalid() {
        let yaml = "not: valid: yaml: [";
        let result = parse(yaml.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_file_nonexistent() {
        let result = parse_file(&PathBuf::from("/nonexistent/workflow.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.json");
        let json = r#"{
            "name": "file_wf",
            "nodes": [{"id": "n1", "node_type": "llm", "config": {}, "depends_on": [], "retry_count": 0}],
            "edges": []
        }"#;
        std::fs::write(&file_path, json).unwrap();

        let wf = parse_file(&file_path).unwrap();
        assert_eq!(wf.name, "file_wf");
    }

    #[test]
    fn test_validate_node_with_depends_on() {
        let mut wf = make_workflow("test", vec![
            make_node("n1", "llm"),
            NodeDef {
                id: "n2".to_string(),
                node_type: "tool".to_string(),
                config: HashMap::new(),
                depends_on: vec!["n1".to_string()],
                retry_count: 0,
                timeout: None,
            is_terminal: false,
            },
        ]);
        wf.edges.push(Edge {
            from_node: "n1".to_string(),
            to_node: "n2".to_string(),
            condition: None,
        });
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn test_validate_conditional_edge() {
        let mut wf = make_workflow("test", vec![
            make_node("n1", "llm"),
            make_node("n2", "tool"),
            make_node("n3", "output"),
        ]);
        wf.edges.push(Edge {
            from_node: "n1".to_string(),
            to_node: "n2".to_string(),
            condition: Some("success".to_string()),
        });
        wf.edges.push(Edge {
            from_node: "n1".to_string(),
            to_node: "n3".to_string(),
            condition: Some("failure".to_string()),
        });
        assert!(validate(&wf).is_ok());
    }

    #[test]
    fn test_parse_json_with_triggers() {
        let json = r#"{
            "name": "triggered_wf",
            "nodes": [{"id": "n1", "node_type": "llm", "config": {}, "depends_on": [], "retry_count": 0}],
            "edges": [],
            "triggers": [
                {"trigger_type": "cron", "config": {"schedule": "0 * * * *"}},
                {"trigger_type": "webhook", "config": {"path": "/api/trigger"}}
            ]
        }"#;

        let wf = parse_json(json.as_bytes()).unwrap();
        assert_eq!(wf.triggers.len(), 2);
        assert_eq!(wf.triggers[0].trigger_type, "cron");
        assert_eq!(wf.triggers[1].trigger_type, "webhook");
    }
