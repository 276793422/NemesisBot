    use super::*;
    use nemesis_tools::registry::Tool as RegistryTool;

    /// The adapter must delegate to the wrapped agent tool: a `write_file` call
    /// through the nemesis-tools `Tool` trait should actually write the file
    /// and return a success result. (This is the core of the fix — the workflow
    /// tool node previously hit an empty registry and always failed.)
    #[tokio::test]
    async fn test_adapter_delegates_to_agent_tool() {
        // Use a temp path but drop the handle so write_file can create the
        // file fresh (avoids Windows handle locks).
        let tmp_path = std::env::temp_dir().join(format!(
            "nemesis_adapter_test_{}.txt",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&tmp_path);
        let path_str = tmp_path.to_string_lossy().to_string();

        let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::WriteFileTool);
        #[cfg(feature = "security")]
        let adapter = AgentToolAdapter::new("write_file".to_string(), inner, None);
        #[cfg(not(feature = "security"))]
        let adapter = AgentToolAdapter::new("write_file".to_string(), inner);

        let args = serde_json::json!({ "path": path_str, "content": "hello workflow" });
        let result = adapter.execute(&args).await;
        assert!(
            !result.is_error,
            "expected success, got: {}",
            result.for_llm
        );
        assert!(result.for_llm.contains("Successfully wrote"));

        let written = std::fs::read_to_string(&tmp_path).unwrap();
        assert_eq!(written, "hello workflow");
        let _ = std::fs::remove_file(&tmp_path);
    }

    /// Agent-tool errors must surface as `is_error` ToolResults (not panics),
    /// so the workflow node marks itself Failed and the run can branch.
    #[tokio::test]
    async fn test_adapter_surfaces_agent_tool_error() {
        let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::ReadFileTool);
        #[cfg(feature = "security")]
        let adapter = AgentToolAdapter::new("read_file".to_string(), inner, None);
        #[cfg(not(feature = "security"))]
        let adapter = AgentToolAdapter::new("read_file".to_string(), inner);

        let args = serde_json::json!({ "path": "/no/such/path/nemesis_adapter_missing_file_xyz" });
        let result = adapter.execute(&args).await;
        assert!(result.is_error, "expected error for missing file");
    }

    /// `parameters()` must forward the agent tool's schema verbatim so the
    /// workflow canvas can render a schema-driven form.
    #[test]
    fn test_adapter_forwards_schema_and_metadata() {
        let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::WriteFileTool);
        #[cfg(feature = "security")]
        let adapter = AgentToolAdapter::new("write_file".to_string(), inner, None);
        #[cfg(not(feature = "security"))]
        let adapter = AgentToolAdapter::new("write_file".to_string(), inner);

        assert_eq!(adapter.name(), "write_file");
        assert!(!adapter.description().is_empty());
        let params = adapter.parameters();
        let required = params
            .get("required")
            .and_then(|v| v.as_array())
            .expect("schema has required array");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"path"));
        assert!(names.contains(&"content"));
    }
