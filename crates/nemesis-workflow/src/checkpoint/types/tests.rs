use super::*;

#[test]
fn checkpoint_round_trip_through_json() {
    let cp = Checkpoint {
        id: "cp-1".to_string(),
        execution_id: "exec-1".to_string(),
        saved_at: DateTime::parse_from_rfc3339("2026-06-25T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc),
        completed_nodes: HashSet::from(["n1".to_string(), "n2".to_string()]),
        waiting_node: Some("review".to_string()),
        parent_execution_id: None,
        trigger_source: None,
        terminal: false,
        context_snapshot: SerializableContext {
            variables: HashMap::from([
                ("k".to_string(), serde_json::json!("v")),
                ("n".to_string(), serde_json::json!(42)),
            ]),
            node_results: HashMap::new(),
            input: HashMap::new(),
        },
        workflow_hash: "abcdef".to_string(),
    };

    let json = serde_json::to_string(&cp).unwrap();
    let restored: Checkpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(cp, restored);
}

#[test]
fn checkpoint_loads_with_missing_optional_fields() {
    // Old snapshots may not have waiting_node / parent_execution_id /
    // trigger_source / terminal. All four default safely.
    let json = r#"{
        "id": "cp-1",
        "execution_id": "exec-1",
        "saved_at": "2026-06-25T10:00:00Z",
        "completed_nodes": ["n1"],
        "context_snapshot": {
            "variables": {},
            "node_results": {},
            "input": {}
        },
        "workflow_hash": "abcdef"
    }"#;
    let cp: Checkpoint = serde_json::from_str(json).unwrap();
    assert_eq!(cp.waiting_node, None);
    assert_eq!(cp.parent_execution_id, None);
    assert_eq!(cp.trigger_source, None);
    assert!(!cp.terminal);
}

#[test]
fn checkpoint_round_trip_with_trigger_source_and_terminal() {
    // Gap 1 + Gap 2: trigger_source must survive round-trip, and terminal
    // flag must persist so restore skips it.
    let cp = Checkpoint {
        id: "cp-term".to_string(),
        execution_id: "exec-1".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["n1".to_string(), "n2".to_string()]),
        waiting_node: None,
        parent_execution_id: Some("exec-parent".to_string()),
        trigger_source: Some(crate::types::TriggerSource::Webhook {
            payload: serde_json::json!({"event": "push"}),
        }),
        terminal: true,
        context_snapshot: SerializableContext {
            variables: HashMap::new(),
            node_results: HashMap::new(),
            input: HashMap::new(),
        },
        workflow_hash: "h".to_string(),
    };

    let json = serde_json::to_string(&cp).unwrap();
    let restored: Checkpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(cp, restored);
    assert!(restored.terminal);
    assert!(matches!(
        restored.trigger_source,
        Some(crate::types::TriggerSource::Webhook { .. })
    ));
}

#[test]
fn checkpoint_round_trip_preserves_all_trigger_source_variants() {
    // Belt-and-suspenders: each variant must round-trip. If serde tagging
    // breaks for any variant, restore would silently lose the trigger info.
    let variants: Vec<crate::types::TriggerSource> = vec![
        crate::types::TriggerSource::Cli,
        crate::types::TriggerSource::Cron,
        crate::types::TriggerSource::Webhook {
            payload: serde_json::json!({"x": 1}),
        },
        crate::types::TriggerSource::AgentTool {
            tool_call_id: "tc-1".to_string(),
            recursion_depth: 2,
        },
        crate::types::TriggerSource::Chat {
            chat_id: "c".to_string(),
            session_key: "s".to_string(),
            sender_id: "u".to_string(),
            message: "m".to_string(),
        },
        crate::types::TriggerSource::WebUI {
            session_id: "ws-1".to_string(),
        },
        crate::types::TriggerSource::Event {
            event_type: "push".to_string(),
            data: serde_json::json!({"a": 1}),
        },
    ];

    for (i, ts) in variants.into_iter().enumerate() {
        let cp = Checkpoint {
            id: format!("cp-{i}"),
            execution_id: "exec-1".to_string(),
            saved_at: Utc::now(),
            completed_nodes: HashSet::new(),
            waiting_node: None,
            parent_execution_id: None,
            trigger_source: Some(ts.clone()),
            terminal: false,
            context_snapshot: SerializableContext {
                variables: HashMap::new(),
                node_results: HashMap::new(),
                input: HashMap::new(),
            },
            workflow_hash: "h".to_string(),
        };

        let json = serde_json::to_string(&cp).unwrap();
        let restored: Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.trigger_source, Some(ts));
    }
}

#[test]
fn checkpoint_loads_with_unknown_state_string() {
    // Adding new ExecutionState variants must not break old snapshots.
    // Unknown state strings fall back to Pending.
    let nr = SerializableNodeResult {
        node_id: "n1".to_string(),
        output: serde_json::json!(null),
        error: None,
        state: "some_future_state".to_string(),
        started_at: Utc::now(),
        ended_at: Utc::now(),
        metadata: HashMap::new(),
    };
    let s = nr.state.as_str();
    assert_eq!(parse_state(s), crate::types::ExecutionState::Pending);
}

#[test]
fn checkpoint_meta_from_checkpoint() {
    let cp = Checkpoint {
        id: "cp-1".to_string(),
        execution_id: "exec-1".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["n1".to_string(), "n2".to_string()]),
        waiting_node: Some("review".to_string()),
        parent_execution_id: None,
        trigger_source: None,
        terminal: true,
        context_snapshot: SerializableContext {
            variables: HashMap::new(),
            node_results: HashMap::new(),
            input: HashMap::new(),
        },
        workflow_hash: "h".to_string(),
    };
    let meta = CheckpointMeta::from(&cp);
    assert_eq!(meta.id, "cp-1");
    assert_eq!(meta.execution_id, "exec-1");
    assert_eq!(meta.completed_node_count, 2);
    assert!(meta.has_waiting);
    assert!(meta.terminal);
}

#[test]
fn serializable_context_round_trip_complex() {
    let ctx = SerializableContext {
        variables: HashMap::from([
            ("obj".to_string(), serde_json::json!({"a": 1, "b": [2, 3]})),
            ("arr".to_string(), serde_json::json!([1, 2, 3])),
            ("n".to_string(), serde_json::json!(42)),
            ("s".to_string(), serde_json::json!("text")),
        ]),
        node_results: HashMap::from([(
            "n1".to_string(),
            SerializableNodeResult {
                node_id: "n1".to_string(),
                output: serde_json::json!({"result": "ok"}),
                error: None,
                state: "completed".to_string(),
                started_at: Utc::now(),
                ended_at: Utc::now(),
                metadata: HashMap::new(),
            },
        )]),
        input: HashMap::from([("q".to_string(), serde_json::json!("hello"))]),
    };

    let json = serde_json::to_string(&ctx).unwrap();
    let restored: SerializableContext = serde_json::from_str(&json).unwrap();
    assert_eq!(ctx, restored);
}
