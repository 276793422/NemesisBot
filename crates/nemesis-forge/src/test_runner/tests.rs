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
    let artifact = make_artifact(
        ArtifactKind::Script,
        "#!/bin/bash\necho hello\nassert result",
    );
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
    assert!(
        result
            .stage
            .errors
            .iter()
            .any(|e| e.contains("bracket") || e.contains("brace"))
    );
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
    assert_eq!(
        super::detect_mcp_language("#!/usr/bin/python\nimport os"),
        "python"
    );
}

#[test]
fn test_detect_mcp_language_go() {
    assert_eq!(
        super::detect_mcp_language("package main\nfunc main() {}"),
        "go"
    );
}

#[test]
fn test_detect_mcp_language_unknown() {
    assert!(super::detect_mcp_language("unknown code").is_empty());
}

// ---- S8 coverage batch: MCP/parse/bracket helpers ---- (quality-hardening goal 冲刺 S8)

#[test]
fn test_s8_default_impl_constructs_runner() {
    let _runner = TestRunner::default();
}

#[test]
fn test_s8_mcp_undetectable_language_reports_unknown_lang() {
    // No python/go markers → detect_mcp_language "" → check 2 error,
    // check 3 hits the `_ => Err(Unknown language)` arm, check 4 hits `_ => Ok`.
    let runner = TestRunner::new();
    let artifact = make_artifact(ArtifactKind::Mcp, "just some plain words here");
    let result = runner.run_tests(&artifact);
    assert!(!result.stage.passed);
    assert!(result
        .stage
        .errors
        .iter()
        .any(|e| e.contains("Cannot detect MCP language")));
    assert!(result
        .stage
        .errors
        .iter()
        .any(|e| e.contains("Unknown language")));
}

#[test]
fn test_s8_mcp_python_missing_tool_registration_and_run_entry() {
    // Python detected (has "def "), has Server( → check 3 passes;
    // no tool registration and no run entry → errors 410/416; the
    // `||` chains on 406-408 and 414 are all evaluated (all false).
    let runner = TestRunner::new();
    let content = "def setup():\n    s = Server(\"x\")\n    return s\n";
    let artifact = make_artifact(ArtifactKind::Mcp, content);
    let result = runner.run_tests(&artifact);
    assert!(!result.stage.passed);
    assert!(result
        .stage
        .errors
        .iter()
        .any(|e| e.contains("lacks tool registration")));
    // Server + tool registration present but no run entry: the structure
    // checker proceeds past has_tool_reg and fails on has_run.
    let content2 = "def setup():
    s = Server(\"x\")
    @server.tool
    def t(): pass
";
    let artifact2 = make_artifact(ArtifactKind::Mcp, content2);
    let result2 = runner.run_tests(&artifact2);
    assert!(result2
        .stage
        .errors
        .iter()
        .any(|e| e.contains("lacks run entry")));
}

#[test]
fn test_s8_mcp_go_lacking_func_main_and_brace_on_next_line() {
    // "package main" + "func foo()" detects go, but GO_FUNC_RE (func main) fails.
    let runner = TestRunner::new();
    let artifact = make_artifact(ArtifactKind::Mcp, "package main\n\nfunc helper(x int) {\n}");
    let result = runner.run_tests(&artifact);
    assert!(result
        .stage
        .errors
        .iter()
        .any(|e| e.contains("Go MCP lacks func main()")));

    // func main() with no '{' on the line and ending in ')' → the
    // "brace might be on next line" continue path in completeness check.
    let artifact2 = make_artifact(ArtifactKind::Mcp, "package main\n\nfunc main()\n");
    let result2 = runner.run_tests(&artifact2);
    // structure check passes (GO_FUNC_RE matches), completeness returns Ok.
    assert!(!result2
        .stage
        .errors
        .iter()
        .any(|e| e.contains("completeness")));
}

#[test]
fn test_s8_parse_frontmatter_json_without_name_or_desc_falls_back_to_yaml() {
    // Valid JSON but neither name nor description → falls through the
    // early return into parse_simple_yaml.
    let (name, desc) = parse_frontmatter(r#"{"other": 1}"#);
    assert_eq!(name, "");
    assert_eq!(desc, "");
}

#[test]
fn test_s8_parse_simple_yaml_skips_comments_blanks_and_colonless_lines() {
    let fm = "name: my-skill\n# a comment line\n\ndescription: some desc\nno colon here\n";
    let (name, desc) = parse_simple_yaml(fm);
    assert_eq!(name, "my-skill");
    assert_eq!(desc, "some desc");
}

#[test]
fn test_s8_check_bracket_balance_string_escape_and_backtick() {
    // Backslash escape inside a string literal → the i += 2 skip path.
    assert!(check_bracket_balance(r#"write "a\"b" now"#).is_ok());
    // Backtick opens a string region.
    assert!(check_bracket_balance("let s = `hello ( world`").is_ok());
}

#[test]
fn test_s8_check_bracket_balance_all_error_shapes() {
    // extra closing paren
    let e1 = check_bracket_balance("value )").unwrap_err();
    assert!(e1.contains("extra closing parenthesis"));
    // missing closing paren
    let e2 = check_bracket_balance("(open").unwrap_err();
    assert!(e2.contains("missing 1 closing parenthesis"));
    // extra closing bracket
    let e3 = check_bracket_balance("x ]").unwrap_err();
    assert!(e3.contains("extra closing bracket"));
    // extra closing brace
    let e4 = check_bracket_balance("x }").unwrap_err();
    assert!(e4.contains("extra closing brace"));
}

#[test]
fn test_s8_detect_mcp_language_tab_separated_def() {
    // "def\tfoo():" has no "def " substring (tab), but PYTHON_DEF_RE
    // matches → the third detection arm returns python.
    assert_eq!(detect_mcp_language("def\tfoo():"), "python");
}

#[test]
fn test_s8_check_mcp_server_structure_go_missing_main() {
    let err = check_mcp_server_structure("package main\nfunc foo()", "go").unwrap_err();
    assert!(err.contains("func main()"));
    // Unknown language arm
    let err2 = check_mcp_server_structure("anything", "").unwrap_err();
    assert!(err2.contains("Unknown language"));
}

#[test]
fn test_s8_check_function_completeness_python_body_errors() {
    // Body present but not indented → immediate error.
    let e1 = check_function_completeness("def foo():\nx = 1\n", "python").unwrap_err();
    assert!(e1.contains("lacks indentation"));
    // def at EOF with nothing after → missing body error.
    let e2 = check_function_completeness("def foo():", "python").unwrap_err();
    assert!(e2.contains("missing body"));
    // Unknown language → Ok (no-op arm).
    assert!(check_function_completeness("whatever", "").is_ok());
}

#[test]
fn test_s8_mcp_python_structure_ok_but_completeness_fails() {
    // Structure passes (Server( + @server.tool + .run() present) but the
    // first def has an unindented body → check-4 error in validate_mcp_tests.
    let runner = TestRunner::new();
    let content = "def setup():\nx = 1\ns = Server(\"x\")\n@server.tool\nt.run()\n";
    let artifact = make_artifact(ArtifactKind::Mcp, content);
    let result = runner.run_tests(&artifact);
    assert!(result
        .stage
        .errors
        .iter()
        .any(|e| e.contains("Function completeness")));
}

#[test]
fn test_s8_go_func_line_without_brace_or_paren_ending() {
    // "func helper() int" has no '{' and does not end with ')' → falls past
    // the brace-on-next-line continue.
    assert!(check_function_completeness("package main\nfunc helper() int\n{\n}", "go").is_ok());
}
