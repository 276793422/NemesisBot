use super::*;
use crate::bridge::ClusterForgeBridge;
use crate::config::ForgeConfig;

#[test]
fn test_forge_tool_definitions() {
    let tools = forge_tool_definitions();
    assert_eq!(tools.len(), 8);

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"forge_reflect"));
    assert!(names.contains(&"forge_create"));
    assert!(names.contains(&"forge_update"));
    assert!(names.contains(&"forge_list"));
    assert!(names.contains(&"forge_evaluate"));
    assert!(names.contains(&"forge_build_mcp"));
    assert!(names.contains(&"forge_share"));
    assert!(names.contains(&"forge_learning_status"));
}

#[test]
fn test_increment_version() {
    assert_eq!(increment_version("1.0"), "1.1");
    assert_eq!(increment_version("1.0.0"), "1.0.1");
    assert_eq!(increment_version("2.3"), "2.4");
    assert_eq!(increment_version("1"), "1.1");
}

#[test]
fn test_tool_result_ok() {
    let result = ForgeToolResult::ok("success");
    assert!(result.success);
    assert_eq!(result.content, "success");
}

#[test]
fn test_tool_result_err() {
    let result = ForgeToolResult::err("failure");
    assert!(!result.success);
    assert_eq!(result.content, "failure");
}

#[tokio::test]
async fn test_version_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let artifact_path = dir.path().join("test.md");
    tokio::fs::write(&artifact_path, "original content")
        .await
        .unwrap();

    version::save_snapshot(&artifact_path, "1.0").await.unwrap();

    let loaded = version::load_snapshot(&artifact_path, "1.0").await.unwrap();
    assert_eq!(loaded, "original content");
}

#[tokio::test]
async fn test_execute_create_skill() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "test-skill",
                "content": "---\nname: test-skill\n---\n\nTest skill content"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Forge artifact created"));
    assert!(result.content.contains("skill"));
}

#[tokio::test]
async fn test_execute_create_script_requires_test_cases() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "test-script",
                "content": "#!/bin/bash\necho hello"
            }),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("test_cases"));
}

#[tokio::test]
async fn test_execute_create_mcp_with_test_cases() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "test-mcp",
                "content": "from mcp.server import Server\nserver = Server('test')\n@server.tool()\ndef my_tool(x): return x\nif __name__ == '__main__': server.run()",
                "test_cases": [{"input": "hello", "expected": "hello"}]
            }),
        )
        .await;
    assert!(result.success, "Error: {}", result.content);
    assert!(result.content.contains("mcp"));
}

#[tokio::test]
async fn test_execute_create_mcp_generates_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "py-mcp",
                "content": "from mcp.server import Server",
                "test_cases": [{"input": "x"}],
                "language": "python"
            }),
        )
        .await;
    assert!(result.success);

    // Check requirements.txt was created
    let req_path = dir
        .path()
        .join("forge")
        .join("mcp")
        .join("py-mcp")
        .join("requirements.txt");
    assert!(req_path.exists(), "requirements.txt should be created");
    let req_content = tokio::fs::read_to_string(&req_path).await.unwrap();
    assert!(req_content.contains("mcp"));
}

#[tokio::test]
async fn test_execute_create_missing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
            }),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("required"));
}

#[tokio::test]
async fn test_execute_list_empty() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor.execute("forge_list", &serde_json::json!({})).await;
    assert!(result.success);
    assert!(result.content.contains("No Forge artifacts"));
}

#[tokio::test]
async fn test_execute_evaluate_missing_id() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute("forge_evaluate", &serde_json::json!({}))
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_learning_status_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute("forge_learning_status", &serde_json::json!({}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("not enabled"));
}

#[tokio::test]
async fn test_execute_learning_status_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learning.enabled = true;
    let forge = Arc::new(Forge::new(config, dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute("forge_learning_status", &serde_json::json!({}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("Enabled"));
}

#[tokio::test]
async fn test_execute_unknown_tool() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute("unknown_tool", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("Unknown forge tool"));
}

#[tokio::test]
async fn test_execute_update_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create first
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "my-skill",
                "content": "initial content"
            }),
        )
        .await;
    assert!(create_result.success);

    // Extract the ID from the result
    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Update it
    let update_result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": id,
                "content": "updated content",
                "change_description": "Updated the skill"
            }),
        )
        .await;
    assert!(update_result.success);
    assert!(update_result.content.contains("1.1"));

    // List should show the artifact
    let list_result = executor.execute("forge_list", &serde_json::json!({})).await;
    assert!(list_result.success);
    assert!(list_result.content.contains("my-skill"));
}

#[test]
fn test_compute_quality_score() {
    let (score, notes) = compute_quality_score(
        "---\nname: test\n---\n## Overview\n- Step 1\nHandle error cases\nline1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\n",
        &ArtifactKind::Skill,
    );
    assert!(score > 0);
    assert!(!notes.is_empty());
}

#[test]
fn test_resolve_artifact_path_skill() {
    let forge_dir = std::path::Path::new("/tmp/forge");
    let artifact = nemesis_types::forge::Artifact {
        id: "skill-test".into(),
        name: "my-skill".into(),
        kind: ArtifactKind::Skill,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Draft,
        content: String::new(),
        tool_signature: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    let path = resolve_artifact_path(forge_dir, &artifact);
    assert_eq!(
        path,
        std::path::PathBuf::from("/tmp/forge/skills/my-skill/SKILL.md")
    );
}

// -- Edge case tests matching Go's forge_coverage2_test.go and forge_coverage3_test.go --------

/// Edge case: MCP creation with Go language generates go.mod
/// (matches Go's TestForgeCreateTool_Execute_MCP_Go)
#[tokio::test]
async fn test_execute_create_mcp_go_language() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "go-mcp",
                "content": "package main\n\nfunc main() {\n    fmt.Println(\"hello\")\n}",
                "test_cases": [{"input": "x"}],
                "language": "go"
            }),
        )
        .await;
    assert!(result.success, "Error: {}", result.content);

    // Check go.mod was created
    let go_mod_path = dir
        .path()
        .join("forge")
        .join("mcp")
        .join("go-mcp")
        .join("go.mod");
    assert!(go_mod_path.exists(), "go.mod should be created for Go MCP");
    let go_mod_content = tokio::fs::read_to_string(&go_mod_path).await.unwrap();
    assert!(go_mod_content.contains("module forge-mcp-go-mcp"));
    assert!(go_mod_content.contains("go 1.21"));
}

/// Edge case: forge_build_mcp install action writes config.mcp.json
/// (matches Go's TestForgeBuildMCPTool_Execute_Install)
#[tokio::test]
async fn test_execute_build_mcp_install_action() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // First create an MCP artifact
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "install-test",
                "content": "from mcp.server import Server",
                "test_cases": [{"input": "x"}],
                "language": "python"
            }),
        )
        .await;
    assert!(
        create_result.success,
        "Create failed: {}",
        create_result.content
    );

    // Extract the ID
    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Create server.py file so install action finds it
    let mcp_dir = dir.path().join("forge").join("mcp").join("install-test");
    tokio::fs::write(mcp_dir.join("server.py"), "from mcp.server import Server")
        .await
        .unwrap();

    // Now execute the install action
    let result = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({
                "id": id,
                "action": "install"
            }),
        )
        .await;
    assert!(result.success, "Install failed: {}", result.content);
    assert!(result.content.contains("installed to config.mcp.json"));

    // Verify config.mcp.json was created (install writes to workspace/config/)
    let config_path = dir.path().join("config").join("config.mcp.json");
    assert!(config_path.exists(), "config.mcp.json should be created");
    let config_content = tokio::fs::read_to_string(&config_path).await.unwrap();
    assert!(config_content.contains("forge-install-test"));
}

/// Edge case: forge_update with rollback_version restores previous content
/// (matches Go's TestForgeUpdateTool_Execute_Rollback)
#[tokio::test]
async fn test_execute_update_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create a skill
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "rollback-skill",
                "content": "---\nname: rollback-skill\n---\n\nOriginal content"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Update to new content (creates version snapshot "1.0")
    let update_result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": id,
                "content": "Updated content v2",
                "change_description": "Second version"
            }),
        )
        .await;
    assert!(update_result.success);
    assert!(update_result.content.contains("1.1"));

    // Update again (creates version snapshot "1.1")
    let update2_result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": id,
                "content": "Updated content v3",
                "change_description": "Third version"
            }),
        )
        .await;
    assert!(update2_result.success);
    assert!(update2_result.content.contains("1.2"));

    // Now rollback to version "1.0" (the original content)
    let rollback_result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": id,
                "rollback_version": "1.0"
            }),
        )
        .await;
    assert!(
        rollback_result.success,
        "Rollback failed: {}",
        rollback_result.content
    );
    assert!(rollback_result.content.contains("rolled back from 1.0"));
}

/// Edge case: forge_share with bridge and reflection data
/// (matches Go's TestForgeShareTool_Execute_WithReflections)
#[tokio::test]
async fn test_execute_share_with_bridge() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

    // Create a mock bridge that returns 3 peers
    struct MockShareBridge {
        node_id: String,
        share_count: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl ClusterForgeBridge for MockShareBridge {
        async fn share_reflection(&self, _report: serde_json::Value) -> Result<usize, String> {
            self.share_count.fetch_add(1, Ordering::SeqCst);
            Ok(3)
        }
        async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
            Ok(Vec::new())
        }
        async fn get_online_peers(&self) -> Result<Vec<String>, String> {
            Ok(vec!["peer-1".into(), "peer-2".into(), "peer-3".into()])
        }
        fn local_node_id(&self) -> &str {
            &self.node_id
        }
        fn is_cluster_enabled(&self) -> bool {
            true
        }
    }

    let share_count = Arc::new(AtomicUsize::new(0));
    let bridge = Arc::new(MockShareBridge {
        node_id: "test-node".into(),
        share_count: share_count.clone(),
    });
    forge.set_bridge(bridge);
    let forge = Arc::new(forge);
    let executor = ForgeToolExecutor::new(forge);

    // Create a fake reflection report on disk
    let reflections_dir = dir.path().join("forge").join("reflections");
    tokio::fs::create_dir_all(&reflections_dir).await.unwrap();
    let report_path = reflections_dir.join("report-2026-01-01.json");
    tokio::fs::write(
        &report_path,
        r#"{"id":"r1","period_start":"2026-01-01","period_end":"2026-01-02","insights":["test"],"recommendations":[],"statistics":{},"is_remote":false}"#,
    )
    .await
    .unwrap();

    let result = executor
        .execute(
            "forge_share",
            &serde_json::json!({
                "report_path": report_path.to_string_lossy().to_string()
            }),
        )
        .await;
    assert!(result.success, "Share failed: {}", result.content);
    assert!(result.content.contains("shared with 3 peers"));
    assert_eq!(share_count.load(Ordering::SeqCst), 1);
}

/// Edge case: forge_list with mixed artifact types and type filtering
/// (matches Go's TestForgeListTool_Execute_WithType)
#[tokio::test]
async fn test_execute_list_with_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create a skill
    executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "list-skill",
                "content": "---\nname: list-skill\n---\n\nSkill content"
            }),
        )
        .await;

    // Create a script
    executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "list-script",
                "content": "#!/bin/bash\necho hello",
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;

    // Create an MCP
    executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "list-mcp",
                "content": "from mcp.server import Server",
                "test_cases": [{"input": "x"}],
                "language": "python"
            }),
        )
        .await;

    // List all — should contain all 3
    let list_all = executor.execute("forge_list", &serde_json::json!({})).await;
    assert!(list_all.success);
    assert!(list_all.content.contains("list-skill"));
    assert!(list_all.content.contains("list-script"));
    assert!(list_all.content.contains("list-mcp"));

    // Filter by type=skill — should only show skill
    let list_skill = executor
        .execute("forge_list", &serde_json::json!({"type": "skill"}))
        .await;
    assert!(list_skill.success);
    assert!(list_skill.content.contains("list-skill"));
    // Other types should not appear in the filtered list
    assert!(!list_skill.content.contains("list-script"));
    assert!(!list_skill.content.contains("list-mcp"));

    // Filter by type=script
    let list_script = executor
        .execute("forge_list", &serde_json::json!({"type": "script"}))
        .await;
    assert!(list_script.success);
    assert!(list_script.content.contains("list-script"));
    assert!(!list_script.content.contains("list-skill"));
}

// ============================================================
// Additional tests for static_validation, quality_assessment,
// ForgeToolResult serialization, tool definitions
// ============================================================

#[test]
fn test_forge_tool_result_ok() {
    let result = ForgeToolResult::ok("test content");
    assert!(result.success);
    assert_eq!(result.content, "test content");
}

#[test]
fn test_forge_tool_result_err() {
    let result = ForgeToolResult::err("something failed");
    assert!(!result.success);
    assert_eq!(result.content, "something failed");
}

#[test]
fn test_forge_tool_result_serialization() {
    let ok_result = ForgeToolResult::ok("success data");
    let json = serde_json::to_string(&ok_result).unwrap();
    let restored: ForgeToolResult = serde_json::from_str(&json).unwrap();
    assert!(restored.success);
    assert_eq!(restored.content, "success data");

    let err_result = ForgeToolResult::err("error msg");
    let json = serde_json::to_string(&err_result).unwrap();
    let restored: ForgeToolResult = serde_json::from_str(&json).unwrap();
    assert!(!restored.success);
    assert_eq!(restored.content, "error msg");
}

#[test]
fn test_forge_tool_result_serialization_roundtrip() {
    let result = ForgeToolResult {
        success: true,
        content: "multi\nline\ncontent".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let restored: ForgeToolResult = serde_json::from_str(&json).unwrap();
    assert_eq!(result.success, restored.success);
    assert_eq!(result.content, restored.content);
}

#[test]
fn test_forge_tool_definitions_count() {
    let defs = forge_tool_definitions();
    // Should have 8 tools: reflect, create, update, list, evaluate, build_mcp, share, learning_status
    assert_eq!(defs.len(), 8);
}

#[test]
fn test_forge_tool_definitions_names() {
    let defs = forge_tool_definitions();
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"forge_reflect"));
    assert!(names.contains(&"forge_create"));
    assert!(names.contains(&"forge_update"));
    assert!(names.contains(&"forge_list"));
    assert!(names.contains(&"forge_evaluate"));
    assert!(names.contains(&"forge_build_mcp"));
    assert!(names.contains(&"forge_share"));
    assert!(names.contains(&"forge_learning_status"));
}

#[test]
fn test_forge_tool_definitions_have_descriptions() {
    let defs = forge_tool_definitions();
    for def in &defs {
        assert!(
            !def.description.is_empty(),
            "Tool {} missing description",
            def.name
        );
    }
}

#[test]
fn test_forge_tool_definitions_have_parameters() {
    let defs = forge_tool_definitions();
    for def in &defs {
        assert!(
            def.parameters.is_object(),
            "Tool {} missing parameters",
            def.name
        );
    }
}

#[test]
fn test_forge_tool_serialization() {
    let tool = ForgeTool {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: serde_json::json!({"type": "object"}),
    };
    let json = serde_json::to_string(&tool).unwrap();
    let restored: ForgeTool = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.name, "test_tool");
    assert_eq!(restored.description, "A test tool");
}

#[test]
fn test_static_validation_empty_content() {
    // Empty content should fail for any kind
    for kind in &[ArtifactKind::Skill, ArtifactKind::Script, ArtifactKind::Mcp] {
        let result = static_validation("", kind);
        assert!(!result.passed, "Empty content should fail for {:?}", kind);
        assert!(!result.checks.is_empty());
    }
}

#[test]
fn test_static_validation_skill_valid() {
    let content = "# Test Skill\n\n## Overview\nA good skill.\n\n## Steps\n- Step 1\n- Step 2\n- Step 3\nSome more content to reach 50 chars minimum threshold";
    let result = static_validation(content, &ArtifactKind::Skill);
    assert!(
        result.passed,
        "Valid skill should pass: {:?}",
        result.checks
    );
}

#[test]
fn test_static_validation_skill_too_short() {
    let content = "Short";
    let result = static_validation(content, &ArtifactKind::Skill);
    assert!(!result.passed, "Too-short content should fail");
}

#[test]
fn test_static_validation_skill_no_headings() {
    let content = "A ".repeat(30); // 60 chars but no headings
    let result = static_validation(&content, &ArtifactKind::Skill);
    assert!(!result.passed, "Skill without headings should fail");
}

#[test]
fn test_static_validation_script_valid() {
    let content =
        "#!/bin/bash\n# A script that does useful work\necho hello world\nexit 0\n# End of script";
    let result = static_validation(content, &ArtifactKind::Script);
    assert!(
        result.passed,
        "Valid script should pass: {:?}",
        result.checks
    );
}

#[test]
fn test_static_validation_script_main_function() {
    let content =
        "# My Script\n\ndef main():\n    print('hello world')\n    print('goodbye')\n    return 0";
    let result = static_validation(content, &ArtifactKind::Script);
    assert!(
        result.passed,
        "Script with main function should pass: {:?}",
        result.checks
    );
}

#[test]
fn test_static_validation_script_no_entry() {
    let content = "A ".repeat(30); // 60 chars but no shebang or main
    let result = static_validation(&content, &ArtifactKind::Script);
    assert!(!result.passed, "Script without entry point should fail");
}

#[test]
fn test_static_validation_mcp_valid() {
    let content =
        "from mcp.server import Server\n\nclass MyServer:\n    def handle(self):\n        pass";
    let result = static_validation(content, &ArtifactKind::Mcp);
    assert!(result.passed, "Valid MCP should pass: {:?}", result.checks);
}

#[test]
fn test_static_validation_mcp_no_server() {
    let content = "A ".repeat(30); // No Server or server keyword
    let result = static_validation(&content, &ArtifactKind::Mcp);
    assert!(!result.passed, "MCP without server should fail");
}

#[test]
fn test_quality_assessment_skill() {
    let content = "# My Skill\n\nThis is a detailed skill with good content.\n\n## Steps\n1. Step one\n2. Step two\n3. Step three";
    let result = quality_assessment(content, &ArtifactKind::Skill);
    assert!(result.score > 0, "Quality score should be > 0");
    assert!(result.score <= 100, "Quality score should be <= 100");
}

#[test]
fn test_quality_assessment_short_content() {
    let content = "Hi";
    let result = quality_assessment(content, &ArtifactKind::Skill);
    // Short content should have a low score
    assert!(
        result.score < 60,
        "Short content should have low score, got {}",
        result.score
    );
}

#[test]
fn test_quality_assessment_empty_content() {
    let result = quality_assessment("", &ArtifactKind::Skill);
    assert!(
        result.score < 50,
        "Empty content should have very low score"
    );
}

#[test]
fn test_quality_assessment_script() {
    let content = "#!/bin/bash\n# This script does useful work\necho 'hello world'\nexit 0";
    let result = quality_assessment(content, &ArtifactKind::Script);
    assert!(result.score > 0);
}

#[test]
fn test_quality_assessment_mcp() {
    let content = "from mcp.server import Server\n\nclass MyServer(Server):\n    def __init__(self):\n        super().__init__()\n\n    async def handle(self, request):\n        return {'result': 'ok'}";
    let result = quality_assessment(content, &ArtifactKind::Mcp);
    assert!(result.score > 0);
}

#[tokio::test]
async fn test_execute_unknown_tool_name() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("nonexistent_tool", &serde_json::json!({}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("unknown") || result.content.contains("Unknown"));
}

#[tokio::test]
async fn test_execute_reflect_no_provider() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_reflect", &serde_json::json!({"period": "today"}))
        .await;
    // Should return a result even without provider (statistical analysis)
    // Or should return an error
    assert!(result.success || result.content.contains("error") || result.content.contains("no"));
}

#[tokio::test]
async fn test_execute_learning_status_no_engine() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_learning_status", &serde_json::json!({}))
        .await;
    // Without a learning engine configured, should return status indicating not available
    assert!(
        !result.success || result.content.contains("disabled") || result.content.contains("not")
    );
}

#[tokio::test]
async fn test_execute_create_missing_required_fields() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_create", &serde_json::json!({"type": "skill"}))
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_evaluate_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_evaluate",
            &serde_json::json!({"id": "nonexistent-id"}),
        )
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_update_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_update",
            &serde_json::json!({"id": "nonexistent-id", "content": "new"}),
        )
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_build_mcp_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "nonexistent-id"}),
        )
        .await;
    assert!(!result.success);
}

// ============================================================
// Additional tests for uncovered code paths
// ============================================================

#[tokio::test]
async fn test_execute_create_script_type() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "my-script",
                "content": "#!/bin/bash\necho hello",
                "test_cases": [{"input": "test"}]
            }),
        )
        .await;
    assert!(result.success, "Script create failed: {}", result.content);
}

#[tokio::test]
async fn test_execute_create_with_description() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "described-skill",
                "content": "---\nname: described-skill\n---\n\n## Overview\nA described skill",
                "description": "A skill with a custom description"
            }),
        )
        .await;
    assert!(
        result.success,
        "Create with description failed: {}",
        result.content
    );
}

#[tokio::test]
async fn test_execute_create_mcp_python_language() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "py-mcp",
                "content": "from mcp.server import Server",
                "test_cases": [{"input": "x"}],
                "language": "python"
            }),
        )
        .await;
    assert!(
        result.success,
        "MCP python create failed: {}",
        result.content
    );

    // Check requirements.txt was created
    let req_path = dir
        .path()
        .join("forge")
        .join("mcp")
        .join("py-mcp")
        .join("requirements.txt");
    assert!(
        req_path.exists(),
        "requirements.txt should be created for Python MCP"
    );
}

#[tokio::test]
async fn test_execute_create_invalid_type() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "invalid",
                "name": "test",
                "content": "content"
            }),
        )
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_update_rollback_nonexistent_version() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create a skill first
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "rollback-test",
                "content": "---\nname: rollback-test\n---\n\nOriginal"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Try to rollback to a nonexistent version
    let result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": id,
                "rollback_version": "99.0"
            }),
        )
        .await;
    assert!(!result.success);
}

#[tokio::test]
async fn test_execute_share_no_bridge() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute("forge_share", &serde_json::json!({}))
        .await;
    // Without bridge, share should fail or indicate not available
    assert!(!result.success || result.content.contains("not") || result.content.contains("no"));
}

#[tokio::test]
async fn test_execute_build_mcp_uninstall_action() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // First create an MCP artifact
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "uninstall-test",
                "content": "from mcp.server import Server",
                "test_cases": [{"input": "x"}],
                "language": "python"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Create config.mcp.json so uninstall can remove the entry
    let config_dir = dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.mcp.json"),
        r#"{"mcpServers":{"forge-uninstall-test":{"command":"python","args":["server.py"]}}}"#,
    )
    .unwrap();

    let result = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({
                "id": id,
                "action": "uninstall"
            }),
        )
        .await;
    assert!(result.success, "Uninstall failed: {}", result.content);
}

#[test]
fn test_compute_quality_score_high_score() {
    let content = "---\nname: my-skill\n---\n\n# My Skill\n\n## Overview\nThis is a great skill with enough content to pass the 500 byte threshold for maximum length score.\n\n## Steps\n1. Do step one with care and attention to detail\n2. Do step two with precision and thoroughness\n3. Verify all works well before proceeding\n4. Document your findings carefully\n\n## Error Handling\nHandle error cases gracefully and log all failures for debugging purposes.\n\nAdditional content line 1 here\nAdditional content line 2 here\nAdditional content line 3 here\nAdditional content line 4 here\nAdditional content line 5 here\n";
    let (score, notes) = compute_quality_score(content, &ArtifactKind::Skill);
    assert!(
        score > 50,
        "Score should be high: {}, notes: {}",
        score,
        notes
    );
    assert!(!notes.is_empty());
}

#[test]
fn test_compute_quality_score_script() {
    let content = "#!/bin/bash\n# A well documented script\nset -e\n\necho 'hello world'\nexit 0";
    let (score, notes) = compute_quality_score(content, &ArtifactKind::Script);
    assert!(score > 0);
    let _ = notes;
}

#[test]
fn test_compute_quality_score_mcp() {
    let content = "from mcp.server import Server\n\nclass MyServer(Server):\n    async def handle(self, req):\n        return {'result': 'ok'}\n";
    let (score, notes) = compute_quality_score(content, &ArtifactKind::Mcp);
    assert!(score > 0);
    let _ = notes;
}

#[test]
fn test_static_validation_skill_with_lists() {
    let content = "# Skill\n\n## Overview\nA skill.\n\n- Item 1\n- Item 2\n- Item 3\nExtra padding content to pass 50 char minimum requirement";
    let result = static_validation(content, &ArtifactKind::Skill);
    assert!(
        result.passed,
        "Skill with lists should pass: {:?}",
        result.checks
    );
}

#[test]
fn test_static_validation_script_shebang() {
    let content = "#!/usr/bin/env python3\nimport sys\nprint('hello')\nsys.exit(0)\n# End";
    let result = static_validation(content, &ArtifactKind::Script);
    assert!(
        result.passed,
        "Script with shebang should pass: {:?}",
        result.checks
    );
}

#[test]
fn test_static_validation_mcp_with_server_keyword() {
    let content = "import server\n\ndef handle():\n    pass\n# Additional content for length padding purposes";
    let result = static_validation(content, &ArtifactKind::Mcp);
    // Should at least have server keyword
    let _ = result;
}

#[test]
fn test_resolve_artifact_path_script() {
    let forge_dir = std::path::Path::new("/tmp/forge");
    let artifact = nemesis_types::forge::Artifact {
        id: "script-test".into(),
        name: "my-script".into(),
        kind: ArtifactKind::Script,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Draft,
        content: String::new(),
        tool_signature: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    let path = resolve_artifact_path(forge_dir, &artifact);
    assert!(path.to_string_lossy().contains("scripts"));
    assert!(path.to_string_lossy().contains("my-script"));
}

#[test]
fn test_resolve_artifact_path_mcp() {
    let forge_dir = std::path::Path::new("/tmp/forge");
    let artifact = nemesis_types::forge::Artifact {
        id: "mcp-test".into(),
        name: "my-mcp".into(),
        kind: ArtifactKind::Mcp,
        version: "1.0".into(),
        status: nemesis_types::forge::ArtifactStatus::Draft,
        content: String::new(),
        tool_signature: vec![],
        created_at: String::new(),
        updated_at: String::new(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    };
    let path = resolve_artifact_path(forge_dir, &artifact);
    assert_eq!(
        path,
        std::path::PathBuf::from("/tmp/forge/mcp/my-mcp/server.py")
    );
}

#[tokio::test]
async fn test_execute_evaluate_existing_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create a skill first
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "eval-skill",
                "content": "---\nname: eval-skill\n---\n\n## Overview\nA skill to evaluate\n\n## Steps\n- Step 1\n- Step 2\n- Step 3\nHandle error cases carefully"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Evaluate the artifact
    let eval_result = executor
        .execute("forge_evaluate", &serde_json::json!({"id": id}))
        .await;
    assert!(
        eval_result.success,
        "Evaluate failed: {}",
        eval_result.content
    );
    assert!(
        eval_result.content.contains("score")
            || eval_result.content.contains("validation")
            || eval_result.content.contains("passed")
    );
}

#[tokio::test]
async fn test_execute_build_mcp_build_action() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create MCP first
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "build-test",
                "content": "from mcp.server import Server",
                "test_cases": [{"input": "x"}],
                "language": "python"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    let result = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({
                "id": id,
                "action": "build"
            }),
        )
        .await;
    assert!(result.success, "Build failed: {}", result.content);
}

#[tokio::test]
async fn test_execute_list_with_status_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create a skill
    executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "status-skill",
                "content": "---\nname: status-skill\n---\n\n## Overview\nContent"
            }),
        )
        .await;

    // List with status=draft
    let result = executor
        .execute("forge_list", &serde_json::json!({"status": "draft"}))
        .await;
    assert!(result.success);
}

// --- Additional coverage tests for forge_tools ---

#[tokio::test]
async fn test_execute_list_with_deprecated_status_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    // "deprecated" maps to Archived
    let result = executor
        .execute("forge_list", &serde_json::json!({"status": "deprecated"}))
        .await;
    assert!(result.success);
    assert!(result.content.contains("No Forge artifacts") || result.content.contains("Total"));
}

#[tokio::test]
async fn test_execute_list_with_observing_status_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_list", &serde_json::json!({"status": "observing"}))
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_with_testing_status_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_list", &serde_json::json!({"status": "testing"}))
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_with_degraded_status_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_list", &serde_json::json!({"status": "degraded"}))
        .await;
    assert!(result.success);
}

#[tokio::test]
async fn test_execute_list_unknown_status_filter() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_list",
            &serde_json::json!({"status": "invalid_status"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("Unknown status"));
}

#[tokio::test]
async fn test_execute_update_no_content_no_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create first
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "nocontent-skill",
                "content": "---\nname: nocontent-skill\n---\n\nInitial content"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    // Update without content or rollback
    let result = executor
        .execute("forge_update", &serde_json::json!({"id": id}))
        .await;
    assert!(!result.success);
    assert!(result.content.contains("required"));
}

#[tokio::test]
async fn test_execute_update_missing_id() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_update",
            &serde_json::json!({"content": "new content"}),
        )
        .await;
    assert!(!result.success);
    assert!(result.content.contains("required"));
}

#[tokio::test]
async fn test_execute_evaluate_with_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // Create a well-formed skill
    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "eval-skill",
                "content": "---\nname: eval-skill\n---\n\n# Eval Skill\n\n## Overview\nA skill to evaluate.\n\n## Steps\n- Step 1\n- Step 2\n- Step 3\n\n## Error Handling\nHandle errors gracefully.\n\nTry-catch blocks should be used.\n\nAdditional content to reach threshold for evaluation quality score."
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    let result = executor
        .execute("forge_evaluate", &serde_json::json!({"id": id}))
        .await;
    assert!(result.success, "Evaluate failed: {}", result.content);
    assert!(result.content.contains("Stage 1"));
    assert!(result.content.contains("Stage 2"));
    assert!(result.content.contains("Stage 3"));
}

#[tokio::test]
async fn test_execute_create_script_with_category() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "my-script",
                "content": "#!/bin/bash\necho hello world\nexit 0",
                "test_cases": [{"input": "test"}],
                "category": "deploy"
            }),
        )
        .await;
    assert!(
        result.success,
        "Script with category failed: {}",
        result.content
    );
    // Check script is in the deploy category directory
    let script_path = dir
        .path()
        .join("forge")
        .join("scripts")
        .join("deploy")
        .join("my-script");
    assert!(
        script_path.exists(),
        "Script should be in deploy category dir"
    );
}

#[test]
fn test_increment_version_non_numeric() {
    // Last segment is non-numeric
    assert_eq!(increment_version("1.abc"), "1.abc.1");
}

#[test]
fn test_increment_version_single_segment() {
    assert_eq!(increment_version("5"), "5.1");
}

#[tokio::test]
async fn test_version_snapshot_load_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let artifact_path = dir.path().join("test.md");
    tokio::fs::write(&artifact_path, "content").await.unwrap();

    let result = version::load_snapshot(&artifact_path, "nonexistent").await;
    assert!(result.is_err());
}

#[test]
fn test_save_version_snapshot_basic() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("artifact.md");
    std::fs::write(&file_path, "original content").unwrap();

    save_version_snapshot(file_path.to_str().unwrap(), "1.0").unwrap();

    let loaded = load_version_snapshot(file_path.to_str().unwrap(), "1.0").unwrap();
    assert_eq!(loaded, "original content");
}

#[test]
fn test_load_version_snapshot_nonexistent() {
    let result = load_version_snapshot("/nonexistent/path/artifact.md", "1.0");
    assert!(result.is_err());
}

#[test]
fn test_static_validation_mcp_no_handler() {
    // MCP with server but no handler functions
    let content = "from mcp.server import Server\n\n# Just a server reference with no functions\n# Additional padding for minimum length requirement";
    let result = static_validation(content, &ArtifactKind::Mcp);
    assert!(!result.passed, "MCP without handler functions should fail");
}

#[test]
fn test_static_validation_too_few_lines() {
    let content = "ab";
    let result = static_validation(content, &ArtifactKind::Skill);
    assert!(!result.passed);
}

#[test]
fn test_quality_assessment_with_error_handling() {
    let content = "def process():\n    try:\n        handle_error()\n        return None\n    except:\n        return 'error'\n";
    let result = quality_assessment(content, &ArtifactKind::Script);
    // Should detect error handling patterns
    let error_dim = result
        .dimensions
        .iter()
        .find(|d| d.name == "Error handling");
    assert!(error_dim.is_some());
    assert!(error_dim.unwrap().score > 0);
}

#[test]
fn test_quality_assessment_with_documentation() {
    let content = "# Doc\n\n## Section\n\n'''python\ncode example\n'''\n\n# Comment 1\n# Comment 2\n# Comment 3\nDescription and usage guide.";
    let result = quality_assessment(content, &ArtifactKind::Skill);
    let doc_dim = result.dimensions.iter().find(|d| d.name == "Documentation");
    assert!(doc_dim.is_some());
    assert!(doc_dim.unwrap().score > 0);
}

#[test]
fn test_quality_assessment_with_code_quality() {
    let content = "def main() -> int:\n    value: str = 'hello'\n    return 0\n";
    let result = quality_assessment(content, &ArtifactKind::Script);
    let quality_dim = result.dimensions.iter().find(|d| d.name == "Code quality");
    assert!(quality_dim.is_some());
    assert!(quality_dim.unwrap().score > 0);
}

#[tokio::test]
async fn test_execute_reflect_with_focus() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_reflect",
            &serde_json::json!({"period": "week", "focus": "skill"}),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("week"));
    assert!(result.content.contains("skill"));
}

#[tokio::test]
async fn test_execute_create_mcp_other_language() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    // MCP with unsupported language should still succeed (no extra project files)
    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "other-mcp",
                "content": "Server instance\n\ndef handler():\n    pass",
                "test_cases": [{"input": "x"}],
                "language": "rust"
            }),
        )
        .await;
    assert!(
        result.success,
        "MCP with other language failed: {}",
        result.content
    );
}

#[tokio::test]
async fn test_execute_update_with_change_description() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let create_result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "desc-skill",
                "content": "---\nname: desc-skill\n---\n\nInitial"
            }),
        )
        .await;
    assert!(create_result.success);

    let id_line = create_result
        .content
        .lines()
        .find(|l| l.contains("ID:"))
        .unwrap();
    let id = id_line.split("ID:").nth(1).unwrap().trim();

    let result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": id,
                "content": "Updated content",
                "change_description": "Major revision"
            }),
        )
        .await;
    assert!(result.success);
    assert!(result.content.contains("Major revision"));
}

#[test]
fn test_compute_quality_score_low_content() {
    let (score, notes) = compute_quality_score("short", &ArtifactKind::Skill);
    assert_eq!(score, 0);
    assert!(notes.contains("5 bytes"));
}

#[test]
fn test_compute_quality_score_medium_content() {
    let content = "A ".repeat(30); // ~60 bytes
    let (score, _) = compute_quality_score(&content, &ArtifactKind::Skill);
    assert!(score >= 5);
}
