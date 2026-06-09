use super::*;

    fn make_artifact(kind: ArtifactKind, content: &str) -> Artifact {
        Artifact {
            id: "test-id".into(),
            name: "test".into(),
            kind,
            version: "1.0".into(),
            status: nemesis_types::forge::ArtifactStatus::Draft,
            content: content.into(),
            tool_signature: vec![],
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        }
    }

    // ---- Skill tests ----

    #[test]
    fn test_validate_skill_pass_all_5() {
        let runner = TestRunner::new();
        let artifact = make_artifact(
            ArtifactKind::Skill,
            "---\nname: my-skill\ndescription: A test skill\n---\n## Overview\n- Step 1\n- Step 2",
        );
        let result = runner.run_tests(&artifact);
        assert!(result.stage.passed);
        assert_eq!(result.tests_passed, 5);
    }

    #[test]
    fn test_validate_skill_json_frontmatter() {
        let runner = TestRunner::new();
        let artifact = make_artifact(
            ArtifactKind::Skill,
            "---\n{\"name\": \"json-skill\", \"description\": \"JSON meta\"}\n---\n## Body\nSome content here",
        );
        let result = runner.run_tests(&artifact);
        assert!(result.stage.passed);
        assert_eq!(result.tests_passed, 5);
    }

    #[test]
    fn test_validate_skill_fail_empty() {
        let runner = TestRunner::new();
        let artifact = make_artifact(ArtifactKind::Skill, "");
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
    }

    #[test]
    fn test_validate_skill_invalid_name() {
        let runner = TestRunner::new();
        let artifact = make_artifact(
            ArtifactKind::Skill,
            "---\nname: invalid name!\ndescription: test\n---\n## Body\nContent",
        );
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
        assert!(result.stage.errors.iter().any(|e| e.contains("Invalid")));
    }

    #[test]
    fn test_validate_skill_no_markdown_structure() {
        let runner = TestRunner::new();
        let artifact = make_artifact(
            ArtifactKind::Skill,
            "---\nname: plain-skill\ndescription: no markdown\n---\nJust plain text without structure.",
        );
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
        assert!(result.stage.errors.iter().any(|e| e.contains("Markdown")));
    }

    #[test]
    fn test_validate_skill_description_too_long() {
        let long_desc = "x".repeat(2000);
        let content = format!(
            "---\nname: long-desc\ndescription: {}\n---\n## Body\nContent",
            long_desc
        );
        let runner = TestRunner::new();
        let artifact = make_artifact(ArtifactKind::Skill, &content);
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
        assert!(result.stage.errors.iter().any(|e| e.contains("1024")));
    }

    // ---- Script tests ----

    #[test]
    fn test_validate_script_pass() {
        let runner = TestRunner::new();
        let artifact = make_artifact(ArtifactKind::Script, "#!/bin/bash\necho hello\nassert result");
        let result = runner.run_tests(&artifact);
        assert!(result.stage.passed);
        assert_eq!(result.tests_passed, 2);
    }

    #[test]
    fn test_validate_script_fail_empty() {
        let runner = TestRunner::new();
        let artifact = make_artifact(ArtifactKind::Script, "");
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
    }

    #[test]
    fn test_validate_script_fail_no_structure() {
        let runner = TestRunner::new();
        let artifact = make_artifact(ArtifactKind::Script, "just some random text");
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
    }

    // ---- MCP tests ----

    #[test]
    fn test_validate_mcp_python_pass() {
        let runner = TestRunner::new();
        let content = r#"from mcp.server import Server
server = Server("test")

@server.tool()
def my_tool(input):
    return "result"

if __name__ == "__main__":
    server.run()
"#;
        let artifact = make_artifact(ArtifactKind::Mcp, content);
        let result = runner.run_tests(&artifact);
        assert!(result.stage.passed, "Errors: {:?}", result.stage.errors);
        assert_eq!(result.tests_passed, 5);
    }

    #[test]
    fn test_validate_mcp_go_pass() {
        let runner = TestRunner::new();
        let content = r#"package main

import "fmt"

func main() {
    fmt.Println("MCP server")
}"#;
        let artifact = make_artifact(ArtifactKind::Mcp, content);
        let result = runner.run_tests(&artifact);
        assert!(result.stage.passed, "Errors: {:?}", result.stage.errors);
        assert_eq!(result.tests_passed, 5);
    }

    #[test]
    fn test_validate_mcp_fail_empty() {
        let runner = TestRunner::new();
        let artifact = make_artifact(ArtifactKind::Mcp, "");
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
    }

    #[test]
    fn test_validate_mcp_bracket_imbalance() {
        let runner = TestRunner::new();
        let content = "def tool():\n    return {\n";
        let artifact = make_artifact(ArtifactKind::Mcp, content);
        let result = runner.run_tests(&artifact);
        assert!(!result.stage.passed);
        assert!(result.stage.errors.iter().any(|e| e.contains("bracket") || e.contains("brace")));
    }

    // ---- Helper function tests ----

    #[test]
    fn test_extract_frontmatter() {
        let fm = super::extract_frontmatter("---\nname: test\n---\nBody");
        assert_eq!(fm, "name: test");
    }

    #[test]
    fn test_extract_frontmatter_empty() {
        let fm = super::extract_frontmatter("No frontmatter here");
        assert!(fm.is_empty());
    }

    #[test]
    fn test_strip_frontmatter() {
        let body = super::strip_frontmatter("---\nname: test\n---\nBody content");
        assert_eq!(body.trim(), "Body content");
    }

    #[test]
    fn test_parse_simple_yaml() {
        let (name, desc) = super::parse_simple_yaml("name: my-skill\ndescription: A test");
        assert_eq!(name, "my-skill");
        assert_eq!(desc, "A test");
    }

    #[test]
    fn test_parse_simple_yaml_quoted() {
        let (name, desc) = super::parse_simple_yaml("name: \"my skill\"\ndescription: 'a desc'");
        assert_eq!(name, "my skill");
        assert_eq!(desc, "a desc");
    }

    #[test]
    fn test_is_valid_skill_name() {
        assert!(super::is_valid_skill_name("my-skill"));
        assert!(super::is_valid_skill_name("skill123"));
        assert!(super::is_valid_skill_name("a-b-c"));
        assert!(!super::is_valid_skill_name("invalid name"));
        assert!(!super::is_valid_skill_name(""));
        assert!(!super::is_valid_skill_name(&"x".repeat(65)));
    }

    #[test]
    fn test_check_bracket_balance_ok() {
        assert!(super::check_bracket_balance("func() { [1, 2] }").is_ok());
    }

    #[test]
    fn test_check_bracket_balance_missing_close() {
        assert!(super::check_bracket_balance("func() { [1, 2").is_err());
    }

    #[test]
    fn test_check_bracket_balance_in_string() {
        assert!(super::check_bracket_balance("x = \"{[()]}'\"").is_ok());
    }

    #[test]
    fn test_detect_mcp_language_python() {
        assert_eq!(super::detect_mcp_language("def tool(): pass"), "python");
        assert_eq!(super::detect_mcp_language("#!/usr/bin/python\nimport os"), "python");
    }

    #[test]
    fn test_detect_mcp_language_go() {
        assert_eq!(super::detect_mcp_language("package main\nfunc main() {}"), "go");
    }

    #[test]
    fn test_detect_mcp_language_unknown() {
        assert!(super::detect_mcp_language("unknown code").is_empty());
    }
