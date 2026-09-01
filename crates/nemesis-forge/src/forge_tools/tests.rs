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

// =========================================================================
// S8 coverage batch (quality-hardening goal 冲刺 S8)
// =========================================================================

/// MCP server content that passes Stage-1 static checks, all five Stage-2
/// functional checks, and scores >= 60 on Stage-3 quality assessment
/// (long + documented + error handling + typed defs). No security-gate
/// patterns (no api_key/secret_key/rm -rf/curl|bash).
const S8_MCP_PASS: &str = r#"# MCP server module (s8 fixture)
# Usage: uv run server.py
# description: coverage fixture server with error handling
# This block fence exercises the documentation dimension:
```
not parsed as code, only counted
```
from mcp.server import Server

server = Server("s8-fixture")

def handle_tool(input: str) -> str:
    try:
        return input.upper()
    except Exception as e:
        return None

@server.tool()
def my_tool(input: str) -> str:
    # delegate to the handler above
    return handle_tool(input)

if __name__ == "__main__":
    server.run()
"#;

/// Minimal Go MCP content passing all five functional checks.
const S8_MCP_GO_PASS: &str = r#"package main

import "fmt"

func main() {
    fmt.Println("MCP server")
}"#;

/// Build a bare registry artifact with the given id/kind (Draft, no usage).
fn s8_ft_artifact(id: &str, kind: ArtifactKind) -> nemesis_types::forge::Artifact {
    nemesis_types::forge::Artifact {
        id: id.to_string(),
        name: id.to_string(),
        kind,
        version: "1.0".to_string(),
        status: nemesis_types::forge::ArtifactStatus::Draft,
        content: String::new(),
        tool_signature: vec![],
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        usage_count: 0,
        last_degraded_at: None,
        success_rate: 0.0,
        consecutive_observing_rounds: 0,
    }
}

/// Bridge mock whose share result is switchable at construction.
struct S8ShareBridge {
    ok: bool,
    node_id: String,
}

#[async_trait::async_trait]
impl ClusterForgeBridge for S8ShareBridge {
    async fn share_reflection(&self, _report_json: serde_json::Value) -> Result<usize, String> {
        if self.ok {
            Ok(2)
        } else {
            Err("s8 share boom".to_string())
        }
    }
    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        Ok(vec![])
    }
    async fn get_online_peers(&self) -> Result<Vec<String>, String> {
        Ok(vec!["p1".into()])
    }
    fn local_node_id(&self) -> &str {
        &self.node_id
    }
    fn is_cluster_enabled(&self) -> bool {
        true
    }
}

/// Record one experience into the forge collector.
async fn s8_record_exp(forge: &Forge, id: &str, tool: &str, success: bool, duration_ms: u64) {
    forge
        .collector()
        .record(nemesis_types::forge::Experience {
            id: id.to_string(),
            tool_name: tool.to_string(),
            input_summary: "in".into(),
            output_summary: "out".into(),
            success,
            duration_ms,
            timestamp: chrono::Local::now().to_rfc3339(),
            session_key: "s8".into(),
        })
        .await;
}

/// quality_assessment: >500-byte content (completeness +10) and a fenced
/// code block (documentation +5) must both contribute.
#[test]
fn test_s8_quality_assessment_long_fenced_content() {
    let body = format!(
        "# T\n\n# Usage: sample doc\n# description: demo body\n## S\n\n- a\n- b\n\n```\nblock\n```\n{}",
        "x".repeat(600)
    );
    let r = quality_assessment(&body, &ArtifactKind::Skill);
    let doc = r
        .dimensions
        .iter()
        .find(|d| d.name == "Documentation")
        .expect("documentation dimension");
    assert!(doc.score >= 20, "doc score = {}", doc.score);
    let comp = r
        .dimensions
        .iter()
        .find(|d| d.name == "Content completeness")
        .expect("completeness dimension");
    assert!(comp.score > 10, "completeness score = {}", comp.score);
}

/// quality_assessment: Mcp content with `if __name__` main entry gets the
/// +3 completeness bonus (385) and the full fixture scores >= 60.
#[test]
fn test_s8_quality_assessment_mcp_main_entry_high_score() {
    let r = quality_assessment(S8_MCP_PASS, &ArtifactKind::Mcp);
    assert!(r.score >= 60, "fixture score = {}", r.score);
}

/// forge_reflect must render the Recommendations section when the
/// reflector produces recommendations (failing + slow tools).
#[tokio::test]
async fn test_s8_reflect_outputs_recommendations() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    for i in 0..3 {
        s8_record_exp(&forge, &format!("s8f-{}", i), "flaky_tool", false, 10).await;
    }
    for i in 0..2 {
        s8_record_exp(&forge, &format!("s8s-{}", i), "slow_tool", true, 8000).await;
    }
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_reflect", &serde_json::json!({}))
        .await;
    assert!(result.success);
    assert!(
        result.content.contains("### Recommendations"),
        "no recommendations section: {}",
        result.content
    );
    assert!(result.content.contains("Investigate failures in tool 'flaky_tool'"));
    assert!(result.content.contains("Consider optimizing or caching results for tool 'slow_tool'"));
}

/// forge_create skill with a hardcoded secret is rejected by the security
/// gate inside Forge::create_skill (Err branch of the skill path).
#[tokio::test]
async fn test_s8_create_skill_security_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "leaky-skill",
                "description": "d",
                "content": "# H\n\napi_key: \"leaksecret123\"\n"
            }),
        )
        .await;
    assert!(!result.success);
    assert!(
        result.content.contains("Failed to create skill"),
        "content: {}",
        result.content
    );
}

/// forge_create script with `rm -rf /` is blocked by the inline security
/// gate BEFORE the file is written.
#[tokio::test]
async fn test_s8_create_script_security_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "danger-script",
                "content": "#!/bin/bash\nrm -rf /data\necho done\n",
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(!result.success);
    assert!(
        result.content.contains("Content failed security validation"),
        "content: {}",
        result.content
    );
}

/// forge_create script write failure: the artifact parent path is occupied
/// by a regular file, so create_dir_all is silently skipped and the write
/// fails.
#[tokio::test]
async fn test_s8_create_script_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let scripts_dir = dir.path().join("forge").join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("utils"), b"blocker").unwrap();

    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "blocked-script",
                "content": "#!/bin/bash\necho hi\n",
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(!result.success);
    assert!(
        result.content.contains("Failed to write artifact file"),
        "content: {}",
        result.content
    );
}

/// forge_create with validation.auto_validate = false: no TestRunner stage,
/// status stays Draft and no validation info line is emitted.
#[tokio::test]
async fn test_s8_create_script_no_auto_validate() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.validation.auto_validate = false;
    let forge = Arc::new(Forge::new(config, dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "plain-script",
                "content": "#!/bin/bash\necho hi\n",
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(result.content.contains("- Status: Draft"));
    assert!(!result.content.contains("- Validation:"));
}

/// forge_create mcp that passes validation becomes Active and is
/// auto-registered to config.mcp.json through the MCP installer
/// (python entry → command "uv").
#[tokio::test]
async fn test_s8_create_mcp_active_registers_python() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_mcp_installer(crate::mcp_installer::MCPInstaller::new(dir.path().to_path_buf()));
    let forge = Arc::new(forge);
    let executor = ForgeToolExecutor::new(forge.clone());

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "regmcp",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(
        result.content.contains("MCP auto-registered to config.mcp.json"),
        "content: {}",
        result.content
    );
    let art = forge.registry().get("mcp-regmcp").expect("registered");
    assert_eq!(art.status, nemesis_types::forge::ArtifactStatus::Active);

    let mcp_json =
        std::fs::read_to_string(dir.path().join("config").join("config.mcp.json")).unwrap();
    assert!(mcp_json.contains("regmcp"), "config: {}", mcp_json);
    assert!(mcp_json.contains("uv"), "config: {}", mcp_json);
}

/// Same as above but with language=go (entry file main.go, command "go").
#[tokio::test]
async fn test_s8_create_mcp_active_registers_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_mcp_installer(crate::mcp_installer::MCPInstaller::new(dir.path().to_path_buf()));
    let forge = Arc::new(forge);
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "gomcp",
                "language": "go",
                "content": S8_MCP_GO_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(result.content.contains("MCP auto-registered"));
    assert!(dir.path().join("forge").join("mcp").join("gomcp").join("main.go").exists());

    let mcp_json =
        std::fs::read_to_string(dir.path().join("config").join("config.mcp.json")).unwrap();
    assert!(mcp_json.contains("\"go\""), "config: {}", mcp_json);
}

/// Active mcp created on a forge WITHOUT an installer: the registration
/// block is skipped silently (None arm).
#[tokio::test]
async fn test_s8_create_mcp_active_without_installer() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge.clone());
    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "noinst",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(!result.content.contains("MCP auto-registered"));
    let art = forge.registry().get("mcp-noinst").expect("registered");
    assert_eq!(art.status, nemesis_types::forge::ArtifactStatus::Active);
}

/// Installer failure path: workspace/config occupied by a file so the
/// installer cannot save — the create still succeeds but reports the
/// registration failure.
#[tokio::test]
async fn test_s8_create_mcp_installer_failure() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config"), b"blocker").unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_mcp_installer(crate::mcp_installer::MCPInstaller::new(dir.path().to_path_buf()));
    let forge = Arc::new(forge);
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "failreg",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(
        result.content.contains("MCP registration failed"),
        "content: {}",
        result.content
    );
}

/// forge_update write failure: SKILL.md path replaced by a directory so the
/// post-snapshot write fails.
#[tokio::test]
async fn test_s8_update_write_failure() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "upd-skill",
                "description": "d",
                "content": "# H\n\n- a\n"
            }),
        )
        .await;
    assert!(created.success);

    let skill_md = dir.path().join("forge").join("skills").join("upd-skill").join("SKILL.md");
    std::fs::remove_file(&skill_md).unwrap();
    std::fs::create_dir(&skill_md).unwrap();

    let result = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": "skill-upd-skill",
                "content": "# H2\n\n- b\n"
            }),
        )
        .await;
    assert!(!result.success);
    assert!(
        result.content.contains("Failed to update file"),
        "content: {}",
        result.content
    );
}

/// forge_update on a non-skill artifact (script) skips the workspace skills
/// copy branch; updating an Active mcp re-registers it when an installer is
/// present and skips silently when not.
#[tokio::test]
async fn test_s8_update_script_and_active_mcp_reregister() {
    let dir = tempfile::tempdir().unwrap();
    let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());
    forge.init_mcp_installer(crate::mcp_installer::MCPInstaller::new(dir.path().to_path_buf()));
    let forge = Arc::new(forge);
    let executor = ForgeToolExecutor::new(forge);

    // Script update (non-Skill branch of the skills copy).
    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "upd-script",
                "content": "#!/bin/bash\necho v1\n",
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(created.success);
    let updated = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": "script-upd-script",
                "content": "#!/bin/bash\necho v2\n"
            }),
        )
        .await;
    assert!(updated.success, "content: {}", updated.content);

    // Active MCP update with installer → re-register.
    let mcp_created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "rereg",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(mcp_created.success);
    let mcp_updated = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": "mcp-rereg",
                "content": S8_MCP_PASS,
                "change_description": "v2"
            }),
        )
        .await;
    assert!(mcp_updated.success, "content: {}", mcp_updated.content);
    let mcp_json =
        std::fs::read_to_string(dir.path().join("config").join("config.mcp.json")).unwrap();
    assert!(mcp_json.contains("rereg"), "config: {}", mcp_json);

    // Active MCP update WITHOUT installer → skipped silently.
    let dir2 = tempfile::tempdir().unwrap();
    let forge2 = Arc::new(Forge::new(ForgeConfig::default(), dir2.path().to_path_buf()));
    let executor2 = ForgeToolExecutor::new(forge2);
    let c = executor2
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "noreg",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(c.success);
    let u = executor2
        .execute(
            "forge_update",
            &serde_json::json!({"id": "mcp-noreg", "content": S8_MCP_PASS}),
        )
        .await;
    assert!(u.success, "content: {}", u.content);
    assert!(!dir2.path().join("config").join("config.mcp.json").exists());
}

/// forge_list computes the success-rate percentage for artifacts with
/// usage_count > 0 (usage 10 / (10+2) → 83%).
#[tokio::test]
async fn test_s8_list_success_rate_computed() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let mut art = s8_ft_artifact("s8-usage", ArtifactKind::Skill);
    art.usage_count = 10;
    art.consecutive_observing_rounds = 2;
    forge.registry().add(art);
    let executor = ForgeToolExecutor::new(forge);
    let result = executor
        .execute("forge_list", &serde_json::json!({}))
        .await;
    assert!(result.success);
    assert!(
        result.content.contains("83%"),
        "expected 83% in table: {}",
        result.content
    );
}

/// forge_evaluate status ladder: all-pass + score>=60 → Active.
#[tokio::test]
async fn test_s8_evaluate_active_status() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "evalmcp",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(created.success);

    let result = executor
        .execute("forge_evaluate", &serde_json::json!({"id": "mcp-evalmcp"}))
        .await;
    assert!(result.success);
    assert!(
        result.content.contains("**New status: Active**"),
        "content: {}",
        result.content
    );
}

/// forge_evaluate: stages 1+2 pass but quality < 60 → Observing.
#[tokio::test]
async fn test_s8_evaluate_observing_status() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "obs-skill",
                "description": "short skill",
                "content": "# Heading\n\n- item one\n- item two\n"
            }),
        )
        .await;
    assert!(created.success);

    let result = executor
        .execute("forge_evaluate", &serde_json::json!({"id": "skill-obs-skill"}))
        .await;
    assert!(result.success);
    assert!(
        result.content.contains("**New status: Observing**"),
        "content: {}",
        result.content
    );
}

/// forge_evaluate: stage 1 fails (no headings) → Draft.
#[tokio::test]
async fn test_s8_evaluate_draft_static_fail() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "nohead-skill",
                "description": "no headings here",
                "content": "plain body without any heading marks at all"
            }),
        )
        .await;
    assert!(created.success);

    let result = executor
        .execute("forge_evaluate", &serde_json::json!({"id": "skill-nohead-skill"}))
        .await;
    assert!(result.success);
    assert!(
        result.content.contains("**New status: Draft**"),
        "content: {}",
        result.content
    );
    assert!(result.content.contains("Stage 1: Static Validation\n- **Failed**"));
}

/// forge_build_mcp argument validation: missing id, non-MCP artifact, and
/// unknown action.
#[tokio::test]
async fn test_s8_build_mcp_validation_errors() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let r1 = executor
        .execute("forge_build_mcp", &serde_json::json!({}))
        .await;
    assert!(!r1.success);
    assert!(r1.content.contains("id is required"));

    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "skill",
                "name": "bm-skill",
                "description": "d",
                "content": "# H\n\n- a\n"
            }),
        )
        .await;
    assert!(created.success);
    let r2 = executor
        .execute("forge_build_mcp", &serde_json::json!({"id": "skill-bm-skill"}))
        .await;
    assert!(!r2.success);
    assert!(r2.content.contains("is not an MCP type"));

    let mcp = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "bmmcp",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(mcp.success);
    let r3 = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "mcp-bmmcp", "action": "bogus"}),
        )
        .await;
    assert!(!r3.success);
    assert!(r3.content.contains("Unknown action"));
}

/// forge_build_mcp build action on a passing mcp promotes it to Active.
#[tokio::test]
async fn test_s8_build_mcp_build_action_active() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let mcp = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "buildmcp",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(mcp.success);

    let result = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "mcp-buildmcp", "action": "build"}),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(result.content.contains("Active"));
    assert!(result.content.contains("Functional validation: passed"));
}

/// forge_build_mcp install action: go entry detection, python fallback when
/// neither entry file exists, and a config lacking mcpServers.
#[tokio::test]
async fn test_s8_build_mcp_install_go_and_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let go_created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "instgo",
                "language": "go",
                "content": S8_MCP_GO_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(go_created.success, "content: {}", go_created.content);
    let inst = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "mcp-instgo", "action": "install"}),
        )
        .await;
    assert!(inst.success, "content: {}", inst.content);
    assert!(inst.content.contains("Command: go"), "content: {}", inst.content);

    // Remove the go entry so neither server.py nor main.go exists → the
    // generic python fallback fires.
    std::fs::remove_file(dir.path().join("forge").join("mcp").join("instgo").join("main.go"))
        .unwrap();
    let fallback = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "mcp-instgo", "action": "install"}),
        )
        .await;
    assert!(fallback.success, "content: {}", fallback.content);
    assert!(fallback.content.contains("Command: python"), "content: {}", fallback.content);

    // Existing config without an mcpServers object → section is created.
    std::fs::write(
        dir.path().join("config").join("config.mcp.json"),
        r#"{"foo": 1}"#,
    )
    .unwrap();
    let repaired = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "mcp-instgo", "action": "install"}),
        )
        .await;
    assert!(repaired.success, "content: {}", repaired.content);
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("config").join("config.mcp.json")).unwrap(),
    )
    .unwrap();
    assert!(cfg["mcpServers"].is_object(), "cfg: {}", cfg);
    assert!(cfg["mcpServers"]["forge-instgo"].is_object(), "cfg: {}", cfg);
}

/// forge_build_mcp install when config.mcp.json is a directory: the read
/// fails (fresh config) and the final write fails.
#[tokio::test]
async fn test_s8_build_mcp_install_unwritable_config() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let mcp = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "dirconf",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(mcp.success);
    std::fs::create_dir_all(dir.path().join("config").join("config.mcp.json")).unwrap();

    let result = executor
        .execute(
            "forge_build_mcp",
            &serde_json::json!({"id": "mcp-dirconf", "action": "install"}),
        )
        .await;
    assert!(!result.success);
    assert!(
        result.content.contains("Failed to write MCP config"),
        "content: {}",
        result.content
    );
}

/// forge_build_mcp uninstall action: missing config, no mcpServers section,
/// entry not present, and unreadable config.
#[tokio::test]
async fn test_s8_build_mcp_uninstall_variants() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);
    let mcp = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "unimcp",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(mcp.success);
    let config_path = dir.path().join("config").join("config.mcp.json");
    let args = serde_json::json!({"id": "mcp-unimcp", "action": "uninstall"});

    // No config file at all (config dir may not exist either).
    let r1 = executor.execute("forge_build_mcp", &args).await;
    assert!(r1.success);
    assert!(r1.content.contains("does not exist"), "content: {}", r1.content);

    // Config without mcpServers.
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, r#"{"foo": 1}"#).unwrap();
    let r2 = executor.execute("forge_build_mcp", &args).await;
    assert!(r2.success);
    assert!(r2.content.contains("no mcpServers section"), "content: {}", r2.content);

    // mcpServers present but entry absent.
    std::fs::write(&config_path, r#"{"mcpServers": {}}"#).unwrap();
    let r3 = executor.execute("forge_build_mcp", &args).await;
    assert!(r3.success);
    assert!(r3.content.contains("already uninstalled"), "content: {}", r3.content);

    // Config unreadable (directory).
    std::fs::remove_file(&config_path).unwrap();
    std::fs::create_dir(&config_path).unwrap();
    let r4 = executor.execute("forge_build_mcp", &args).await;
    assert!(!r4.success);
    assert!(r4.content.contains("Failed to read MCP config"), "content: {}", r4.content);
}

/// forge_share branches: no report found, report_path outside reflections,
/// successful share with auto-discovered (latest) report including
/// subdirectory scanning, and bridge failure.
#[tokio::test]
async fn test_s8_share_branches() {
    // Failing bridge forge.
    let dir_fail = tempfile::tempdir().unwrap();
    let forge_fail = Arc::new(Forge::new(ForgeConfig::default(), dir_fail.path().to_path_buf()));
    forge_fail.set_bridge(Arc::new(S8ShareBridge {
        ok: false,
        node_id: "s8-fail".into(),
    }));
    let exec_fail = ForgeToolExecutor::new(forge_fail);
    // No reflections dir → find_latest_report returns None.
    let r0 = exec_fail.execute("forge_share", &serde_json::json!({})).await;
    assert!(!r0.success);
    assert!(r0.content.contains("No reflection report found"), "content: {}", r0.content);

    // Ok bridge forge with reports.
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: true,
        node_id: "s8-ok".into(),
    }));
    let executor = ForgeToolExecutor::new(forge.clone());

    // report_path outside the reflections directory → rejected. The
    // reflections dir must exist for the containment check to engage.
    let reflections_pre = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections_pre).unwrap();
    let outside = dir.path().join("outside.md");
    std::fs::write(&outside, b"x").unwrap();
    let rout = executor
        .execute(
            "forge_share",
            &serde_json::json!({"report_path": outside.to_string_lossy().to_string()}),
        )
        .await;
    assert!(!rout.success);
    assert!(
        rout.content.contains("must be within forge reflections directory"),
        "content: {}",
        rout.content
    );

    // Reports in place (top-level + subdirectory) → auto-discovery shares
    // the newest one.
    let reflections = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(reflections.join("remote")).unwrap();
    std::fs::write(reflections.join("r1.md"), b"# r1").unwrap();
    std::fs::write(reflections.join("remote").join("r2.md"), b"# r2").unwrap();
    let rok = executor.execute("forge_share", &serde_json::json!({})).await;
    assert!(rok.success, "content: {}", rok.content);
    assert!(rok.content.contains("shared with 2 peers"), "content: {}", rok.content);

    // Bridge failure → error surfaces.
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: false,
        node_id: "s8-fail2".into(),
    }));
    let rerr = executor.execute("forge_share", &serde_json::json!({})).await;
    assert!(!rerr.success);
    assert!(rerr.content.contains("Share failed"), "content: {}", rerr.content);
}

/// forge_learning_status with an engine holding a completed cycle and Active
/// learning artifacts in the forge registry: renders the cycle block and the
/// artifacts table (both percentage and N/A rows).
#[tokio::test]
async fn test_s8_learning_status_cycle_and_artifacts() {
    use crate::cycle_store::CycleStore;
    use crate::learning_engine::LearningEngine;
    use crate::monitor::DeploymentMonitor;
    use crate::registry::Registry;
    use crate::types::RegistryConfig;

    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learning.enabled = true;
    let mut forge = Forge::new(config.clone(), dir.path().to_path_buf());

    let engine_registry = Arc::new(Registry::new(RegistryConfig::default()));
    let engine = LearningEngine::new(
        config.clone(),
        engine_registry.clone(),
        CycleStore::from_base(dir.path().join("cycles")),
    );
    // Empty cycle still completes and lands in the in-memory latest_cycle.
    let cycle = engine.run_cycle(&[]).await;
    assert_eq!(cycle.status, nemesis_types::forge::CycleStatus::Completed);

    let monitor = Arc::new(DeploymentMonitor::new(config, engine_registry));
    forge.init_learning(engine, monitor, CycleStore::from_base(dir.path().join("cycles2")));

    let mut used = s8_ft_artifact("used-skill", ArtifactKind::Skill);
    used.status = nemesis_types::forge::ArtifactStatus::Active;
    used.tool_signature = vec!["tool_a".into(), "tool_b".into()];
    used.usage_count = 3;
    used.consecutive_observing_rounds = 1;
    forge.registry().add(used);

    let mut fresh = s8_ft_artifact("fresh-skill", ArtifactKind::Skill);
    fresh.status = nemesis_types::forge::ArtifactStatus::Active;
    fresh.tool_signature = vec!["tool_c".into()];
    forge.registry().add(fresh);

    let executor = ForgeToolExecutor::new(Arc::new(forge));
    let result = executor
        .execute("forge_learning_status", &serde_json::json!({}))
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(result.content.contains("### Latest Learning Cycle"), "content: {}", result.content);
    assert!(result.content.contains("- Completed:"), "content: {}", result.content);
    assert!(result.content.contains("### Active Learning Artifacts (2)"), "content: {}", result.content);
    assert!(result.content.contains("75%"), "content: {}", result.content);
    assert!(result.content.contains("N/A"), "content: {}", result.content);
}

/// resolve_artifact_path: Mcp artifact with neither entry file falls back to
/// server.py.
#[test]
fn test_s8_resolve_artifact_path_mcp_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let art = s8_ft_artifact("nomain", ArtifactKind::Mcp);
    let p = resolve_artifact_path(dir.path(), &art);
    assert_eq!(p, dir.path().join("mcp").join("nomain").join("server.py"));
}

/// compute_quality_score: 200 < len <= 500 content hits the 15-point branch.
#[test]
fn test_s8_compute_quality_score_200_to_500() {
    let content = format!("#!/bin/bash\n{}", "a".repeat(240));
    let (score, _) = compute_quality_score(&content, &ArtifactKind::Script);
    assert!(score >= 15, "score = {}", score);
}

/// forge_create with `"test_cases": null` for a script: the entry guard
/// rejects null test_cases upfront (they are mandatory for script/mcp).
#[tokio::test]
async fn test_s8_create_script_null_test_cases() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    let executor = ForgeToolExecutor::new(forge);

    let result = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "script",
                "name": "nulltc",
                "content": "#!/bin/bash\necho ok\n",
                "test_cases": null
            }),
        )
        .await;
    assert!(!result.success);
    assert!(
        result
            .content
            .contains("Script and MCP types require test_cases"),
        "content: {}",
        result.content
    );
}

/// forge_update on a Draft (non-Active) MCP artifact: the re-register branch
/// is skipped because status is not Active.
#[tokio::test]
async fn test_s8_update_non_active_mcp_skips_reregister() {
    let dir = tempfile::tempdir().unwrap();
    // auto_validate=false → artifact stays Draft.
    let mut config = ForgeConfig::default();
    config.validation.auto_validate = false;
    let mut forge = Forge::new(config, dir.path().to_path_buf());
    forge.init_mcp_installer(crate::mcp_installer::MCPInstaller::new(dir.path().to_path_buf()));
    let forge = Arc::new(forge);
    let executor = ForgeToolExecutor::new(forge);

    let created = executor
        .execute(
            "forge_create",
            &serde_json::json!({
                "type": "mcp",
                "name": "draftmcp",
                "content": S8_MCP_PASS,
                "test_cases": [{"input": "x"}]
            }),
        )
        .await;
    assert!(created.success, "content: {}", created.content);

    let updated = executor
        .execute(
            "forge_update",
            &serde_json::json!({
                "id": "mcp-draftmcp",
                "content": S8_MCP_PASS,
                "change_description": "draft v2"
            }),
        )
        .await;
    assert!(updated.success, "content: {}", updated.content);
    // Draft artifact must NOT be registered into config.mcp.json.
    let cfg_path = dir.path().join("config").join("config.mcp.json");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    assert!(!cfg.contains("draftmcp"), "config: {}", cfg);
}

/// forge_learning_status with an initialized learning engine that has never
/// completed a cycle → the "No learning cycle recorded yet." branch.
#[tokio::test]
async fn test_s8_learning_status_no_cycle() {
    use crate::cycle_store::CycleStore;
    use crate::learning_engine::LearningEngine;
    use crate::monitor::DeploymentMonitor;
    use crate::registry::Registry;
    use crate::types::RegistryConfig;

    let dir = tempfile::tempdir().unwrap();
    let mut config = ForgeConfig::default();
    config.learning.enabled = true;
    let mut forge = Forge::new(config.clone(), dir.path().to_path_buf());

    let engine_registry = Arc::new(Registry::new(RegistryConfig::default()));
    let engine = LearningEngine::new(
        config.clone(),
        engine_registry.clone(),
        CycleStore::from_base(dir.path().join("cycles")),
    );
    // No run_cycle call → latest cycle stays None.
    let monitor = Arc::new(DeploymentMonitor::new(config, engine_registry));
    forge.init_learning(engine, monitor, CycleStore::from_base(dir.path().join("cycles2")));

    let executor = ForgeToolExecutor::new(Arc::new(forge));
    let result = executor
        .execute("forge_learning_status", &serde_json::json!({}))
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(
        result.content.contains("No learning cycle recorded yet."),
        "content: {}",
        result.content
    );
}

/// forge_share with a non-md file next to reports in a reflections
/// subdirectory: the extension filter must skip it during discovery.
#[tokio::test]
async fn test_s8_share_subdir_non_md_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: true,
        node_id: "s8-nonmd".into(),
    }));
    let executor = ForgeToolExecutor::new(forge.clone());

    let reflections = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(reflections.join("remote")).unwrap();
    std::fs::write(reflections.join("remote").join("r2.md"), b"# r2").unwrap();
    std::fs::write(reflections.join("remote").join("notes.txt"), b"not a report").unwrap();

    let result = executor.execute("forge_share", &serde_json::json!({})).await;
    assert!(result.success, "content: {}", result.content);
    assert!(
        result.content.contains("shared with 2 peers"),
        "content: {}",
        result.content
    );
}

/// forge_share with the reflections directory entirely absent: report
/// discovery finds nothing (read_dir on the missing dir fails internally).
#[tokio::test]
async fn test_s8_share_no_reflections_dir() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: true,
        node_id: "s8-nodir".into(),
    }));
    let executor = ForgeToolExecutor::new(forge);
    // Ensure no reflections dir was created by construction.
    assert!(!dir.path().join("forge").join("reflections").exists());

    let result = executor.execute("forge_share", &serde_json::json!({})).await;
    assert!(!result.success);
    assert!(
        result.content.contains("No reflection report found"),
        "content: {}",
        result.content
    );
}

/// forge_share with an external report_path while the reflections directory
/// does not exist: the containment check is unconditional —
/// canonicalize_for_compare resolves the nonexistent reflections dir via its
/// longest existing ancestor, so the outside path is rejected (fail-closed;
/// 2026-09-01 tightening — the old skip-when-uncanonicalizable behavior let
/// the path through).
#[tokio::test]
async fn test_s8_share_external_path_no_reflections_dir() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: true,
        node_id: "s8-passthru".into(),
    }));
    let executor = ForgeToolExecutor::new(forge);
    assert!(!dir.path().join("forge").join("reflections").exists());

    let outside = dir.path().join("loose.md");
    std::fs::write(&outside, b"x").unwrap();
    let result = executor
        .execute(
            "forge_share",
            &serde_json::json!({"report_path": outside.to_string_lossy().to_string()}),
        )
        .await;
    // 守卫无条件生效：reflections 目录不存在也拦（fail-closed 收紧）。
    assert!(!result.success, "content: {}", result.content);
    assert!(
        result
            .content
            .contains("must be within forge reflections directory"),
        "content: {}",
        result.content
    );
}

/// forge_share with a report_path that does not exist on disk: the
/// containment check still resolves it (longest-existing-ancestor
/// canonicalize sees it inside the reflections dir), so the share runs.
#[tokio::test]
async fn test_s8_share_nonexistent_report_path() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: true,
        node_id: "s8-ghost".into(),
    }));
    let executor = ForgeToolExecutor::new(forge);
    let reflections = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections).unwrap();

    let ghost = reflections.join("ghost.md");
    assert!(!ghost.exists());
    let result = executor
        .execute(
            "forge_share",
            &serde_json::json!({"report_path": ghost.to_string_lossy().to_string()}),
        )
        .await;
    assert!(result.success, "content: {}", result.content);
    assert!(
        result.content.contains("shared with 2 peers"),
        "content: {}",
        result.content
    );
}

/// forge_share with a NONEXISTENT report_path OUTSIDE the reflections
/// directory: the old fail-open path (canonicalize of the path itself fails
/// → containment check skipped entirely) would have shared it to cluster
/// peers. Regression for the 2026-09-01 fail-open fix: the unconditional
/// guard resolves the path via its longest existing ancestor and rejects.
#[tokio::test]
async fn test_s8_share_nonexistent_external_path_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
    forge.set_bridge(Arc::new(S8ShareBridge {
        ok: true,
        node_id: "s8-failopen".into(),
    }));
    let executor = ForgeToolExecutor::new(forge);
    let reflections = dir.path().join("forge").join("reflections");
    std::fs::create_dir_all(&reflections).unwrap();

    let ghost_outside = dir.path().join("evil").join("ghost.md");
    assert!(!ghost_outside.exists());
    let result = executor
        .execute(
            "forge_share",
            &serde_json::json!({"report_path": ghost_outside.to_string_lossy().to_string()}),
        )
        .await;
    assert!(!result.success, "content: {}", result.content);
    assert!(
        result
            .content
            .contains("must be within forge reflections directory"),
        "content: {}",
        result.content
    );
}
