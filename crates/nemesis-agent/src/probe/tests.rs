    use super::*;
    use crate::types::ToolCallInfo;

    fn resp_with_tool(name: &str, args: &str) -> LlmResponse {
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc".to_string(),
                name: name.to_string(),
                arguments: args.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        }
    }

    fn read_file_task() -> ProbeTask {
        probe_tasks().into_iter().find(|t| t.expected_tool == "read_file").unwrap()
    }

    #[test]
    fn score_no_tool_call_is_all_zero() {
        let resp = LlmResponse {
            content: "I refuse to use tools.".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        };
        let s = score_response(&resp, &read_file_task());
        assert_eq!(s, ProbeScore::default());
    }

    #[test]
    fn score_correct_tool_valid_args_is_full_marks() {
        let resp = resp_with_tool("read_file", r#"{"path":"README.md"}"#);
        let s = score_response(&resp, &read_file_task());
        assert_eq!(s, ProbeScore { format: 1.0, selection: 1.0, schema: 1.0 });
    }

    #[test]
    fn score_wrong_tool_is_zero_selection() {
        let resp = resp_with_tool("exec", r#"{"command":"cat README.md"}"#);
        let s = score_response(&resp, &read_file_task());
        assert_eq!(s.selection, 0.0);
        assert_eq!(s.format, 1.0); // still used the channel
    }

    #[test]
    fn score_autofixable_args_is_half_schema() {
        // "patch" is edit-distance 1 from "path" → autofixed → 0.5
        let resp = resp_with_tool("read_file", r#"{"patch":"README.md"}"#);
        let s = score_response(&resp, &read_file_task());
        assert_eq!(s.schema, 0.5);
        assert_eq!(s.selection, 1.0);
    }

    #[test]
    fn score_missing_required_is_zero_schema() {
        let resp = resp_with_tool("read_file", r#"{}"#);
        let s = score_response(&resp, &read_file_task());
        assert_eq!(s.schema, 0.0);
    }

    #[test]
    fn tier_mapping() {
        assert_eq!(tier_from_scores(1.0, 1.0, 1.0), ModelTier::Big);
        assert_eq!(tier_from_scores(0.9, 0.85, 0.7), ModelTier::Normal);
        assert_eq!(tier_from_scores(0.3, 0.3, 0.3), ModelTier::Mini);
        assert_eq!(tier_from_scores(0.0, 0.0, 0.0), ModelTier::Mini);
    }

    #[test]
    fn probe_tasks_has_seven_including_cluster() {
        let tasks = probe_tasks();
        assert_eq!(tasks.len(), 7);
        assert!(tasks.iter().any(|t| t.expected_tool == "cluster_rpc"));
    }

    #[test]
    fn probe_tool_defs_dedupes() {
        let defs = probe_tool_defs();
        // 7 tasks but several share read_file/write_file/etc tools; dedup by name.
        let names: Vec<&str> = defs.iter().map(|d| d.function.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names.len(), sorted.len()); // no dupes
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"cluster_rpc"));
    }
