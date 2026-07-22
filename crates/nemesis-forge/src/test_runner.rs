//! Test runner - Stage 2 functional validation for artifacts.
//!
//! Performs content-based validation for skills, scripts, and MCP modules.
//! Mirrors Go's 5-check skill validation, 2-check script validation, and
//! 5-check MCP validation.

use regex::Regex;
use std::sync::LazyLock;

use nemesis_types::forge::{Artifact, ArtifactKind};

use super::pipeline::{FunctionalValidationResult, ValidationStage};

// Pre-compiled patterns
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\r?\n(.*?)\r?\n---\r?\n*").unwrap());

static SKILL_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9]+(-[a-zA-Z0-9]+)*$").unwrap());

static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#{1,6}\s").unwrap());

static UNORDERED_LIST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^[\-\*]\s").unwrap());

static ORDERED_LIST_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\d+\.\s").unwrap());

static PYTHON_DEF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^(?:async\s+)?def\s+\w+.*:\s*$").unwrap());

static GO_FUNC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^func\s+main\s*\(").unwrap());

/// Content-based test runner for artifact validation.
pub struct TestRunner;

impl TestRunner {
    /// Create a new test runner.
    pub fn new() -> Self {
        Self
    }

    /// Run functional tests on an artifact.
    pub fn run_tests(&self, artifact: &Artifact) -> FunctionalValidationResult {
        let mut result = FunctionalValidationResult {
            stage: ValidationStage {
                passed: false,
                timestamp: chrono::Local::now().to_rfc3339(),
                errors: Vec::new(),
            },
            tests_run: 0,
            tests_passed: 0,
        };

        match artifact.kind {
            ArtifactKind::Skill => self.validate_skill_content(&artifact.content, &mut result),
            ArtifactKind::Script => self.validate_script_tests(&artifact.content, &mut result),
            ArtifactKind::Mcp => self.validate_mcp_tests(&artifact.content, &mut result),
        }

        result.stage.passed = result.stage.errors.is_empty();
        result
    }

    // ---- Skill Validation (5 checks) ----

    fn validate_skill_content(&self, content: &str, result: &mut FunctionalValidationResult) {
        result.tests_run = 5;

        // Extract frontmatter
        let fm = extract_frontmatter(content);
        let (skill_name, skill_desc) = parse_frontmatter(&fm);

        // Check 1: Frontmatter has name and description
        if !skill_name.is_empty() && !skill_desc.is_empty() {
            result.tests_passed += 1;
        } else {
            result
                .stage
                .errors
                .push("Skill lacks valid frontmatter (needs name and description)".into());
        }

        // Check 2: Name pattern (alphanumeric + hyphens, max 64 chars)
        if !skill_name.is_empty() {
            if is_valid_skill_name(&skill_name) {
                result.tests_passed += 1;
            } else {
                result.stage.errors.push(format!(
                    "Invalid skill name: {:?} (only alphanumeric and hyphens, max 64 chars)",
                    skill_name
                ));
            }
        } else {
            result.stage.errors.push("Skill lacks name field".into());
        }

        // Check 3: Description length (non-empty, max 1024)
        if !skill_desc.is_empty() {
            if skill_desc.len() <= 1024 {
                result.tests_passed += 1;
            } else {
                result
                    .stage
                    .errors
                    .push("Skill description exceeds 1024 character limit".into());
            }
        } else {
            result
                .stage
                .errors
                .push("Skill lacks description field".into());
        }

        // Check 4: Body non-empty after stripping frontmatter
        let body = strip_frontmatter(content);
        if !body.trim().is_empty() {
            result.tests_passed += 1;
        } else {
            result
                .stage
                .errors
                .push("Skill body is empty (no content after frontmatter)".into());
        }

        // Check 5: Markdown structure (headings or lists)
        let has_headings = HEADING_RE.is_match(&body);
        let has_unordered = UNORDERED_LIST_RE.is_match(&body);
        let has_ordered = ORDERED_LIST_RE.is_match(&body);
        if has_headings || has_unordered || has_ordered {
            result.tests_passed += 1;
        } else {
            result
                .stage
                .errors
                .push("Skill body lacks Markdown structure (headings or lists)".into());
        }
    }

    // ---- Script Validation (2 checks) ----

    fn validate_script_tests(&self, content: &str, result: &mut FunctionalValidationResult) {
        result.tests_run = 2;

        // Check 1: Non-empty content
        if content.trim().is_empty() {
            result.stage.errors.push("Script content is empty".into());
            return;
        }
        result.tests_passed += 1;

        // Check 2: Has some test case structure (name/input fields)
        // For content-based validation, we check that the script has
        // recognizable structure (shebang, echo/test statements)
        let has_structure = content.contains("#!/bin/")
            || content.contains("#!/usr/bin/")
            || content.contains("echo ")
            || content.contains("test ")
            || content.contains("assert");
        if has_structure {
            result.tests_passed += 1;
        } else {
            result
                .stage
                .errors
                .push("Script lacks recognizable structure".into());
        }
    }

    // ---- MCP Validation (5 checks) ----

    fn validate_mcp_tests(&self, content: &str, result: &mut FunctionalValidationResult) {
        result.tests_run = 5;

        if content.trim().is_empty() {
            result.stage.errors.push("MCP content is empty".into());
            return;
        }

        // Check 1: Bracket balance
        if let Err(e) = check_bracket_balance(content) {
            result
                .stage
                .errors
                .push(format!("Bracket imbalance in MCP: {}", e));
        } else {
            result.tests_passed += 1;
        }

        // Check 2: Language detection
        let lang = detect_mcp_language(content);
        if lang.is_empty() {
            result
                .stage
                .errors
                .push("Cannot detect MCP language (needs Python or Go)".into());
        } else {
            result.tests_passed += 1;
        }

        // Check 3: MCP protocol structure
        if let Err(e) = check_mcp_server_structure(content, &lang) {
            result
                .stage
                .errors
                .push(format!("MCP protocol structure: {}", e));
        } else {
            result.tests_passed += 1;
        }

        // Check 4: Function completeness
        if let Err(e) = check_function_completeness(content, &lang) {
            result
                .stage
                .errors
                .push(format!("Function completeness: {}", e));
        } else {
            result.tests_passed += 1;
        }

        // Check 5: Has tool/server references
        let has_references = content.contains("tool")
            || content.contains("server")
            || content.contains("mcp")
            || content.contains("Server")
            || content.contains("MCP");
        if has_references {
            result.tests_passed += 1;
        } else {
            result
                .stage
                .errors
                .push("MCP missing tool/server references".into());
        }
    }
}

impl Default for TestRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Helper functions ----

/// Extract YAML/JSON frontmatter from content.
fn extract_frontmatter(content: &str) -> String {
    match FRONTMATTER_RE.captures(content) {
        Some(caps) => caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// Strip frontmatter block from content.
fn strip_frontmatter(content: &str) -> String {
    FRONTMATTER_RE.replace_all(content, "").to_string()
}

/// Parse frontmatter (try JSON then YAML).
fn parse_frontmatter(fm: &str) -> (String, String) {
    if fm.is_empty() {
        return (String::new(), String::new());
    }

    // Try JSON first
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(fm) {
        let name = v["name"].as_str().unwrap_or("").to_string();
        let desc = v["description"].as_str().unwrap_or("").to_string();
        if !name.is_empty() || !desc.is_empty() {
            return (name, desc);
        }
    }

    // Fall back to simple YAML
    parse_simple_yaml(fm)
}

/// Parse simple key: value YAML format.
fn parse_simple_yaml(content: &str) -> (String, String) {
    let mut name = String::new();
    let mut desc = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if key == "name" {
                name = value.to_string();
            } else if key == "description" {
                desc = value.to_string();
            }
        }
    }

    (name, desc)
}

/// Check if a skill name is valid.
fn is_valid_skill_name(name: &str) -> bool {
    name.len() <= 64 && SKILL_NAME_RE.is_match(name)
}

/// Check bracket balance in code.
fn check_bracket_balance(code: &str) -> Result<(), String> {
    let (mut paren, mut bracket, mut brace) = (0i32, 0i32, 0i32);
    let mut in_string = false;
    let mut string_char = b'\0';
    let bytes = code.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];

        if in_string {
            if ch == b'\\' {
                i += 2;
                continue;
            }
            if ch == string_char {
                in_string = false;
            }
        } else {
            match ch {
                b'"' | b'\'' => {
                    in_string = true;
                    string_char = ch;
                }
                b'`' => {
                    in_string = true;
                    string_char = ch;
                }
                b'(' => paren += 1,
                b')' => paren -= 1,
                b'[' => bracket += 1,
                b']' => bracket -= 1,
                b'{' => brace += 1,
                b'}' => brace -= 1,
                _ => {}
            }
        }
        i += 1;
    }

    let mut errs = Vec::new();
    if paren < 0 {
        errs.push("extra closing parenthesis )".to_string());
    } else if paren > 0 {
        errs.push(format!("missing {} closing parenthesis )", paren));
    }
    if bracket < 0 {
        errs.push("extra closing bracket ]".to_string());
    } else if bracket > 0 {
        errs.push(format!("missing {} closing brackets ]", bracket));
    }
    if brace < 0 {
        errs.push("extra closing brace }".to_string());
    } else if brace > 0 {
        errs.push(format!("missing {} closing braces }}", brace));
    }

    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs.join("; "))
    }
}

/// Detect MCP language from code content.
fn detect_mcp_language(code: &str) -> String {
    // Go detection
    if code.contains("package ") && code.contains("func ") {
        return "go".to_string();
    }
    // Python detection
    if code.contains("def ") || (code.contains("import ") && code.contains("from ")) {
        return "python".to_string();
    }
    if code.contains("async def ") || PYTHON_DEF_RE.is_match(code) {
        return "python".to_string();
    }
    let trimmed = code.trim();
    if trimmed.starts_with("#!") && code.contains("python") {
        return "python".to_string();
    }
    String::new()
}

/// Check MCP server protocol structure.
fn check_mcp_server_structure(code: &str, lang: &str) -> Result<(), String> {
    match lang {
        "python" => {
            let has_server = code.contains("Server(")
                || code.contains("FastMCP(")
                || code.contains("MCPServer(");
            if !has_server {
                return Err("Python MCP lacks Server/FastMCP initialization".into());
            }

            let has_tool_reg = code.contains("@server.tool")
                || code.contains("@mcp.tool")
                || code.contains("server.tool(")
                || code.contains("mcp.tool(");
            if !has_tool_reg {
                return Err("Python MCP lacks tool registration".into());
            }

            let has_run =
                code.contains(".run(") || code.contains(".serve(") || code.contains("__main__");
            if !has_run {
                return Err("Python MCP lacks run entry (.run() / .serve() / __main__)".into());
            }
            Ok(())
        }
        "go" => {
            if !GO_FUNC_RE.is_match(code) {
                return Err("Go MCP lacks func main()".into());
            }
            Ok(())
        }
        _ => Err(format!("Unknown language: {:?}", lang)),
    }
}

/// Check function completeness.
fn check_function_completeness(code: &str, lang: &str) -> Result<(), String> {
    match lang {
        "python" => {
            for caps in PYTHON_DEF_RE.captures_iter(code) {
                let m = caps.get(0).unwrap();
                let after = &code[m.end()..];
                // Find the first non-empty line after the def
                let mut found_body = false;
                for line in after.lines().take(5) {
                    if line.trim().is_empty() {
                        continue; // Skip blank lines between def and body
                    }
                    // Check the original (untrimmed) line for leading whitespace
                    let first_char = line.chars().next();
                    if let Some(c) = first_char {
                        if c == ' ' || c == '\t' {
                            found_body = true;
                            break;
                        } else {
                            return Err("Python function body lacks indentation".into());
                        }
                    }
                }
                if !found_body {
                    return Err("Python function definition missing body".into());
                }
            }
            Ok(())
        }
        "go" => {
            for line in code.lines() {
                if line.contains("func ") && line.trim().starts_with("func") {
                    if !line.contains('{') {
                        let trimmed = line.trim();
                        if trimmed.ends_with(')') {
                            continue; // Brace might be on next line
                        }
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
