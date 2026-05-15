//! Forge tools - agent tool definitions and Execute logic for the self-learning system.
//!
//! Defines tools that can be registered with the agent's tool registry:
//! forge_reflect, forge_create, forge_update, forge_list, forge_evaluate,
//! forge_build_mcp, forge_share, forge_learning_status.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use nemesis_types::utils;

use crate::forge::Forge;

// ---------------------------------------------------------------------------
// Tool definition types
// ---------------------------------------------------------------------------

/// Tool definition for forge operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Tool parameter schema (JSON Schema).
    pub parameters: serde_json::Value,
}

/// Result of executing a forge tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeToolResult {
    /// Whether the execution succeeded.
    pub success: bool,
    /// Result content (markdown or error message).
    pub content: String,
}

impl ForgeToolResult {
    /// Create a successful result.
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            success: true,
            content: content.into(),
        }
    }

    /// Create an error result.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            content: message.into(),
        }
    }
}

/// Returns all forge tool definitions.
pub fn forge_tool_definitions() -> Vec<ForgeTool> {
    vec![
        ForgeTool {
            name: "forge_reflect".into(),
            description: "Analyze recent experience data and generate insights".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "period": {"type": "string", "default": "today", "description": "Analysis period: today, week, all"},
                    "focus": {"type": "string", "default": "all", "description": "Focus type: skill, script, mcp, all"},
                },
            }),
        },
        ForgeTool {
            name: "forge_create".into(),
            description: "Create a new forge artifact (skill/script/mcp)".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "type": {"type": "string", "enum": ["skill", "script", "mcp"]},
                    "name": {"type": "string"},
                    "content": {"type": "string"},
                    "description": {"type": "string"},
                    "test_cases": {"type": "array", "description": "Test cases (required for script/mcp types)"},
                    "category": {"type": "string", "default": "utils"},
                    "language": {"type": "string", "default": "python", "description": "Language for MCP: python or go"},
                },
                "required": ["type", "name", "content"],
            }),
        },
        ForgeTool {
            name: "forge_update".into(),
            description: "Update an existing forge artifact with version tracking".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "content": {"type": "string"},
                    "change_description": {"type": "string"},
                    "rollback_version": {"type": "string", "description": "Rollback to a specific version"},
                },
                "required": ["id"],
            }),
        },
        ForgeTool {
            name: "forge_list".into(),
            description: "List all forge artifacts and their status".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "type": {"type": "string", "default": "all"},
                    "status": {"type": "string", "description": "Filter by status: draft, active, deprecated"},
                },
            }),
        },
        ForgeTool {
            name: "forge_evaluate".into(),
            description: "Evaluate forge artifact quality with 3-stage validation".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                },
                "required": ["id"],
            }),
        },
        ForgeTool {
            name: "forge_build_mcp".into(),
            description: "Build/validate/install an MCP server".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": {"type": "string"},
                    "action": {"type": "string", "enum": ["build", "install", "uninstall"], "default": "build"},
                },
                "required": ["id"],
            }),
        },
        ForgeTool {
            name: "forge_share".into(),
            description: "Share the latest reflection report with cluster peers".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "report_path": {"type": "string"},
                },
            }),
        },
        ForgeTool {
            name: "forge_learning_status".into(),
            description: "View closed-loop learning status and recent cycles".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Three-stage validation pipeline (matching Go's Static/Functional/Quality stages)
// ---------------------------------------------------------------------------

/// Result of a single static validation check.
#[derive(Debug, Clone)]
struct StaticCheck {
    name: String,
    passed: bool,
    detail: Option<String>,
}

/// Result of Stage 1: Static Validation.
#[derive(Debug, Clone)]
struct StaticValidationResult {
    passed: bool,
    checks: Vec<StaticCheck>,
}

/// Result of Stage 2: Functional Validation.
#[derive(Debug, Clone)]
struct FunctionalValidationResult {
    passed: bool,
    tests_run: u32,
    tests_passed: u32,
    errors: Vec<String>,
}

/// A single quality dimension score.
#[derive(Debug, Clone)]
struct QualityDimension {
    name: String,
    score: i32,
    max_score: i32,
    note: Option<String>,
}

/// Result of Stage 3: Quality Assessment.
#[derive(Debug, Clone)]
struct QualityAssessmentResult {
    score: i32,
    dimensions: Vec<QualityDimension>,
}

/// Stage 1: Static validation checks content structure, required sections, and syntax.
fn static_validation(content: &str, kind: &ArtifactKind) -> StaticValidationResult {
    let mut checks = Vec::new();
    let mut all_passed = true;

    // Check 1: Non-empty content
    let non_empty = !content.trim().is_empty();
    checks.push(StaticCheck {
        name: "Content non-empty".into(),
        passed: non_empty,
        detail: if non_empty {
            Some(format!("{} bytes", content.len()))
        } else {
            Some("Content is empty or whitespace-only".into())
        },
    });
    if !non_empty { all_passed = false; }

    // Check 2: Minimum content length
    let min_len = content.len() >= 50;
    checks.push(StaticCheck {
        name: "Minimum content length".into(),
        passed: min_len,
        detail: if min_len {
            Some(format!("{} bytes (>= 50 minimum)", content.len()))
        } else {
            Some(format!("{} bytes (< 50 minimum)", content.len()))
        },
    });
    if !min_len { all_passed = false; }

    // Check 3: Type-specific structure
    match kind {
        ArtifactKind::Skill => {
            // Skills should have markdown structure
            let has_sections = content.contains("## ") || content.contains("# ");
            checks.push(StaticCheck {
                name: "Skill has headings".into(),
                passed: has_sections,
                detail: if has_sections { Some("Contains markdown headings".into()) } else { Some("Missing markdown headings".into()) },
            });
            if !has_sections { all_passed = false; }

            let has_lists = content.contains("- ") || content.contains("* ");
            checks.push(StaticCheck {
                name: "Skill has list items".into(),
                passed: has_lists,
                detail: if has_lists { Some("Contains list items".into()) } else { None },
            });
            // Lists are recommended but not required
        }
        ArtifactKind::Script => {
            // Scripts should have a shebang or main entry
            let has_shebang = content.starts_with("#!/");
            let has_entry = content.contains("def main") || content.contains("function main") || content.contains("func main");
            let has_valid_entry = has_shebang || has_entry;
            checks.push(StaticCheck {
                name: "Script has entry point".into(),
                passed: has_valid_entry,
                detail: if has_shebang { Some("Has shebang line".into()) } else if has_entry { Some("Has main function".into()) } else { Some("No shebang or main function found".into()) },
            });
            if !has_valid_entry { all_passed = false; }
        }
        ArtifactKind::Mcp => {
            // MCP servers should have server-related code
            let has_server = content.contains("Server") || content.contains("server") || content.contains("tool(") || content.contains("@tool");
            checks.push(StaticCheck {
                name: "MCP has server setup".into(),
                passed: has_server,
                detail: if has_server { Some("Contains server initialization".into()) } else { Some("No server setup detected".into()) },
            });
            if !has_server { all_passed = false; }

            let has_handler = content.contains("def ") || content.contains("func ") || content.contains("async def ");
            checks.push(StaticCheck {
                name: "MCP has handler functions".into(),
                passed: has_handler,
                detail: if has_handler { Some("Contains handler functions".into()) } else { Some("No handler functions detected".into()) },
            });
            if !has_handler { all_passed = false; }
        }
    }

    // Check 4: No syntax-level issues (basic checks)
    let line_count = content.lines().count();
    let has_reasonable_lines = line_count >= 3;
    checks.push(StaticCheck {
        name: "Reasonable line count".into(),
        passed: has_reasonable_lines,
        detail: Some(format!("{} lines", line_count)),
    });
    if !has_reasonable_lines { all_passed = false; }

    StaticValidationResult {
        passed: all_passed,
        checks,
    }
}

/// Stage 3: Quality assessment combining multiple dimensions.
fn quality_assessment(content: &str, kind: &ArtifactKind) -> QualityAssessmentResult {
    let mut dimensions = Vec::new();
    let mut total_score: i32 = 0;

    // Dimension 1: Content completeness (0-25)
    let completeness_score = {
        let mut s = 0i32;
        let len = content.len();
        if len > 500 { s += 10; }
        else if len > 200 { s += 5; }
        if content.lines().count() > 15 { s += 8; }
        else if content.lines().count() > 5 { s += 4; }
        match kind {
            ArtifactKind::Skill => {
                if content.contains("## ") { s += 4; }
                if content.contains("- ") || content.contains("* ") { s += 3; }
            }
            ArtifactKind::Script => {
                if content.contains("#!") { s += 4; }
                if content.contains("echo ") || content.contains("print(") { s += 3; }
            }
            ArtifactKind::Mcp => {
                if content.contains("Server") || content.contains("server") { s += 4; }
                if content.contains("if __name__") || content.contains("func main") { s += 3; }
            }
        }
        s.min(25)
    };
    dimensions.push(QualityDimension {
        name: "Content completeness".into(),
        score: completeness_score,
        max_score: 25,
        note: Some(format!("{} bytes, {} lines", content.len(), content.lines().count())),
    });
    total_score += completeness_score;

    // Dimension 2: Error handling (0-25)
    let error_handling_score = {
        let mut s = 0i32;
        if content.contains("error") || content.contains("Error") || content.contains("err") {
            s += 8;
        }
        if content.contains("try") || content.contains("catch") || content.contains("except") {
            s += 8;
        }
        if content.contains("handle") || content.contains("Handle") {
            s += 5;
        }
        if content.contains("return") && (content.contains("error") || content.contains("nil") || content.contains("None")) {
            s += 4;
        }
        s.min(25)
    };
    dimensions.push(QualityDimension {
        name: "Error handling".into(),
        score: error_handling_score,
        max_score: 25,
        note: if error_handling_score > 0 { Some("Has error handling patterns".into()) } else { Some("No error handling detected".into()) },
    });
    total_score += error_handling_score;

    // Dimension 3: Documentation (0-25)
    let documentation_score = {
        let mut s = 0i32;
        let comment_lines = content.lines().filter(|l| {
            l.trim().starts_with('#')
                || l.trim().starts_with("//")
                || l.trim().starts_with("/*")
                || l.trim().starts_with("* ")
                || l.trim().starts_with("'''")
                || l.trim().starts_with("\"\"\"")
        }).count();
        if comment_lines >= 3 { s += 10; }
        else if comment_lines >= 1 { s += 5; }
        if content.contains("README") || content.contains("Usage") || content.contains("usage") {
            s += 5;
        }
        if content.contains("description") || content.contains("Description") || content.contains("docstring") {
            s += 5;
        }
        if content.contains("```") {
            s += 5;
        }
        s.min(25)
    };
    dimensions.push(QualityDimension {
        name: "Documentation".into(),
        score: documentation_score,
        max_score: 25,
        note: None,
    });
    total_score += documentation_score;

    // Dimension 4: Code quality (0-25)
    let code_quality_score = {
        let mut s = 0i32;
        // Consistent indentation
        let spaces = content.lines().filter(|l| l.starts_with("  ")).count();
        let tabs = content.lines().filter(|l| l.starts_with('\t')).count();
        if spaces > 0 || tabs > 0 { s += 5; }

        // Function/method definitions
        let fn_count = content.matches("def ").count() + content.matches("func ").count() + content.matches("fn ").count();
        if fn_count > 0 { s += 8; }

        // Type hints or type annotations
        if content.contains(": str") || content.contains(": int") || content.contains("-> ")
            || content.contains(": String") || content.contains(": i32") {
            s += 7;
        }

        // Return statements
        if content.contains("return ") { s += 5; }

        s.min(25)
    };
    dimensions.push(QualityDimension {
        name: "Code quality".into(),
        score: code_quality_score,
        max_score: 25,
        note: None,
    });
    total_score += code_quality_score;

    QualityAssessmentResult {
        score: total_score.min(100),
        dimensions,
    }
}

// ---------------------------------------------------------------------------
// Tool Executor
// ---------------------------------------------------------------------------

/// Executes a forge tool by name, delegating to the Forge subsystems.
pub struct ForgeToolExecutor {
    forge: Arc<Forge>,
}

impl ForgeToolExecutor {
    /// Create a new executor backed by the given Forge instance.
    pub fn new(forge: Arc<Forge>) -> Self {
        Self { forge }
    }

    /// Execute a tool by name with the provided arguments.
    pub async fn execute(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> ForgeToolResult {
        match tool_name {
            "forge_reflect" => self.execute_reflect(args).await,
            "forge_create" => self.execute_create(args).await,
            "forge_update" => self.execute_update(args).await,
            "forge_list" => self.execute_list(args).await,
            "forge_evaluate" => self.execute_evaluate(args).await,
            "forge_build_mcp" => self.execute_build_mcp(args).await,
            "forge_share" => self.execute_share(args).await,
            "forge_learning_status" => self.execute_learning_status(args).await,
            _ => ForgeToolResult::err(format!("Unknown forge tool: {}", tool_name)),
        }
    }

    // -- forge_reflect ------------------------------------------------------

    async fn execute_reflect(&self, args: &serde_json::Value) -> ForgeToolResult {
        let period = args["period"].as_str().unwrap_or("today").to_string();
        let focus = args["focus"].as_str().unwrap_or("all").to_string();

        // Use the reflector to generate a reflection
        let experiences = self.forge.collector().experiences();
        let reflector = crate::reflector::Reflector::new();
        let reflection = reflector.generate_reflection(&experiences);

        let mut output = format!("## Reflection Report\n\n");
        output.push_str(&format!("Period: {}\nFocus: {}\n\n", period, focus));

        for insight in &reflection.insights {
            output.push_str(&format!("- {}\n", insight));
        }

        if !reflection.recommendations.is_empty() {
            output.push_str("\n### Recommendations\n\n");
            for rec in &reflection.recommendations {
                output.push_str(&format!("- {}\n", rec));
            }
        }

        ForgeToolResult::ok(output)
    }

    // -- forge_create -------------------------------------------------------

    async fn execute_create(&self, args: &serde_json::Value) -> ForgeToolResult {
        let artifact_type = args["type"].as_str().unwrap_or("").to_string();
        let name = args["name"].as_str().unwrap_or("").to_string();
        let content = args["content"].as_str().unwrap_or("").to_string();
        let description = args["description"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if artifact_type.is_empty() || name.is_empty() || content.is_empty() {
            return ForgeToolResult::err("type, name, content are required fields");
        }

        // Validate: scripts and MCP require test_cases
        if (artifact_type == "script" || artifact_type == "mcp")
            && args.get("test_cases").map_or(true, |v| v.is_null())
        {
            return ForgeToolResult::err(
                "Script and MCP types require test_cases. Provide test cases and retry.",
            );
        }

        // Validate artifact type
        match artifact_type.as_str() {
            "skill" | "script" | "mcp" => {}
            _ => {
                return ForgeToolResult::err(format!(
                    "type must be 'skill', 'script', or 'mcp'"
                ))
            }
        }

        // Sanitize name
        let name = name.to_lowercase().replace(' ', "-");

        // Use shared CreateSkill method for skill type (matching Go's behavior:
        // t.forge.CreateSkill(ctx, name, content, description, nil))
        if artifact_type == "skill" {
            match self.forge.create_skill(&name, &content, &description, Vec::new()) {
                Ok(artifact) => {
                    let status = format!("{:?}", artifact.status);
                    let mut validation_info = String::new();
                    if self.forge.config().validation.auto_validate {
                        validation_info.push_str(&format!(
                            "\n- Auto-validation: enabled (status={})", status
                        ));
                    }
                    return ForgeToolResult::ok(format!(
                        "Forge artifact created:\n- Type: skill\n- Name: {}\n- Path: forge/skills/{}/SKILL.md\n- Status: {}\n- ID: {}{}",
                        name, name, status, artifact.id, validation_info
                    ));
                }
                Err(e) => {
                    return ForgeToolResult::err(format!("Failed to create skill: {}", e));
                }
            }
        }

        // Non-skill types: inline creation logic (script/mcp)
        use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus};

        let kind = match artifact_type.as_str() {
            "script" => ArtifactKind::Script,
            "mcp" => ArtifactKind::Mcp,
            _ => unreachable!(), // skill already handled above
        };

        let forge_dir = self.forge.workspace().join("forge");
        let artifact_path = match artifact_type.as_str() {
            "script" => {
                let category = args["category"].as_str().unwrap_or("utils");
                forge_dir.join("scripts").join(category).join(&name)
            }
            "mcp" => {
                let language = args["language"].as_str().unwrap_or("python");
                let ext = if language == "go" { "go" } else { "py" };
                let entry = if language == "go" {
                    "main"
                } else {
                    "server"
                };
                forge_dir
                    .join("mcp")
                    .join(&name)
                    .join(format!("{}.{}", entry, ext))
            }
            _ => forge_dir.join(&name),
        };

        // Create directory and write content
        if let Some(parent) = artifact_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Err(e) = tokio::fs::write(&artifact_path, &content).await {
            return ForgeToolResult::err(format!("Failed to write artifact file: {}", e));
        }

        // Generate MCP project structure files
        if artifact_type == "mcp" {
            let language = args["language"].as_str().unwrap_or("python");
            if let Some(mcp_dir) = artifact_path.parent() {
                match language {
                    "python" => {
                        let requirements = "mcp>=1.0.0\n";
                        let _ = tokio::fs::write(mcp_dir.join("requirements.txt"), requirements).await;
                        let readme = format!(
                            "# {}\n\nForge-generated MCP server.\n\n## Usage\n\n```bash\nuv run server.py\n```\n",
                            name
                        );
                        let _ = tokio::fs::write(mcp_dir.join("README.md"), readme).await;
                    }
                    "go" => {
                        let go_mod = format!("module forge-mcp-{}\n\ngo 1.21\n", name);
                        let _ = tokio::fs::write(mcp_dir.join("go.mod"), go_mod).await;
                    }
                    _ => {}
                }
            }
        }

        // Write test cases if provided
        if let Some(test_cases) = args.get("test_cases") {
            if !test_cases.is_null() {
                if let Ok(test_data) = serde_json::to_string_pretty(test_cases) {
                    if let Some(parent) = artifact_path.parent() {
                        let test_dir = parent.join("tests");
                        let _ = tokio::fs::create_dir_all(&test_dir).await;
                        let _ = tokio::fs::write(test_dir.join("test_cases.json"), test_data).await;
                    }
                }
            }
        }

        // Register in registry
        let artifact = Artifact {
            id: format!("{}-{}", artifact_type, name),
            name: name.clone(),
            kind,
            version: "1.0".to_string(),
            status: ArtifactStatus::Draft,
            content: content.clone(),
            tool_signature: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
        };

        let artifact_id = self.forge.registry().add(artifact);

        // Auto-validate if configured (run test runner as a basic pipeline)
        let mut validation_info = String::new();
        if self.forge.config().validation.auto_validate {
            let test_runner = crate::test_runner::TestRunner::new();
            let artifact = self.forge.registry().get(&artifact_id).unwrap();
            let result = test_runner.run_tests(&artifact);

            validation_info.push_str(&format!(
                "\n- Validation: Stage2={} (passed {}/{})",
                result.stage.passed, result.tests_passed, result.tests_run
            ));

            // Update status based on validation
            let new_status = if result.stage.passed {
                ArtifactStatus::Active
            } else {
                ArtifactStatus::Draft
            };
            self.forge.registry().update(&artifact_id, |a| {
                a.status = new_status;
            });
        }

        // Auto-register MCP to config.mcp.json if active (mirrors Go behavior)
        if artifact_type == "mcp" {
            let final_artifact = self.forge.registry().get(&artifact_id);
            if let Some(ref a) = final_artifact {
                if matches!(a.status, nemesis_types::forge::ArtifactStatus::Active) {
                    if let Some(installer) = self.forge.mcp_installer() {
                        let _mcp_dir = artifact_path.parent().unwrap_or(&artifact_path);
                        let command = if args["language"].as_str() == Some("go") {
                            "go".to_string()
                        } else {
                            "uv".to_string()
                        };
                        let cmd_args = if args["language"].as_str() == Some("go") {
                            vec!["run".to_string(), artifact_path.file_name().unwrap_or_default().to_string_lossy().to_string()]
                        } else {
                            vec!["run".to_string(), artifact_path.file_name().unwrap_or_default().to_string_lossy().to_string()]
                        };
                        match installer.install(&name, &command, cmd_args).await {
                            Ok(()) => {
                                validation_info.push_str("\n- MCP auto-registered to config.mcp.json");
                            }
                            Err(e) => {
                                validation_info.push_str(&format!(
                                    "\n- MCP registration failed: {} (please register manually)",
                                    e
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Determine final status
        let final_status = self
            .forge
            .registry()
            .get(&artifact_id)
            .map(|a| format!("{:?}", a.status))
            .unwrap_or_else(|| "draft".to_string());

        ForgeToolResult::ok(format!(
            "Forge artifact created:\n- Type: {}\n- Name: {}\n- Path: {}\n- Status: {}\n- ID: {}{}",
            artifact_type, name, artifact_path.display(), final_status, artifact_id, validation_info
        ))
    }

    // -- forge_update -------------------------------------------------------

    async fn execute_update(&self, args: &serde_json::Value) -> ForgeToolResult {
        let id = args["id"].as_str().unwrap_or("").to_string();
        let mut content = args["content"].as_str().unwrap_or("").to_string();
        let mut change_desc = args["change_description"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let rollback_version = args["rollback_version"]
            .as_str()
            .unwrap_or("")
            .to_string();

        if id.is_empty() {
            return ForgeToolResult::err("id is required");
        }

        let artifact = match self.forge.registry().get(&id) {
            Some(a) => a,
            None => return ForgeToolResult::err(format!("Artifact {} does not exist", id)),
        };

        // Handle rollback
        if !rollback_version.is_empty() {
            let forge_dir = self.forge.workspace().join("forge");
            let artifact_path = resolve_artifact_path(&forge_dir, &artifact);
            match version::load_snapshot(&artifact_path, &rollback_version).await {
                Ok(snapshot) => {
                    content = snapshot;
                    change_desc = format!("Rollback to version {}", rollback_version);
                }
                Err(e) => {
                    return ForgeToolResult::err(format!(
                        "Failed to load version snapshot: {}",
                        e
                    ))
                }
            }
        }

        if content.is_empty() {
            return ForgeToolResult::err("content or rollback_version is required");
        }

        // Save version snapshot before updating
        let forge_dir = self.forge.workspace().join("forge");
        let artifact_path = resolve_artifact_path(&forge_dir, &artifact);
        let _ = version::save_snapshot(&artifact_path, &artifact.version).await;

        // Update file on disk
        if let Err(e) = tokio::fs::write(&artifact_path, &content).await {
            return ForgeToolResult::err(format!("Failed to update file: {}", e));
        }

        // Update registry
        let new_version = increment_version(&artifact.version);
        let _ = self.forge.registry().update(&id, |a| {
            a.version = new_version.clone();
            a.content = content.clone();
        });

        // Update skill copy if it's a skill type
        if artifact.kind == ArtifactKind::Skill {
            let skills_dir = self
                .forge
                .workspace()
                .join("skills")
                .join(format!("{}-forge", artifact.name));
            let _ = tokio::fs::create_dir_all(&skills_dir).await;
            let _ = tokio::fs::write(skills_dir.join("SKILL.md"), &content).await;
        }

        // Re-register MCP if active after update (mirrors Go behavior)
        if artifact.kind == ArtifactKind::Mcp {
            let updated = self.forge.registry().get(&id);
            if let Some(ref a) = updated {
                if matches!(a.status, nemesis_types::forge::ArtifactStatus::Active) {
                    if let Some(installer) = self.forge.mcp_installer() {
                        let _mcp_dir = artifact_path.parent().unwrap_or(&artifact_path);
                        let _ = installer.install(
                            &artifact.name,
                            "uv",
                            vec!["run".to_string(), artifact_path.file_name().unwrap_or_default().to_string_lossy().to_string()],
                        ).await;
                    }
                }
            }
        }

        let rollback_info = if !rollback_version.is_empty() {
            format!(" (rolled back from {})", rollback_version)
        } else {
            String::new()
        };

        ForgeToolResult::ok(format!(
            "Artifact {} updated to version {}{}: {}",
            id, new_version, rollback_info, change_desc
        ))
    }

    // -- forge_list ---------------------------------------------------------

    async fn execute_list(&self, args: &serde_json::Value) -> ForgeToolResult {
        let artifact_type = args["type"].as_str().unwrap_or("all").to_string();
        let status_filter = args["status"].as_str().unwrap_or("").to_string();

        use nemesis_types::forge::{ArtifactKind, ArtifactStatus};

        let kind_filter = match artifact_type.as_str() {
            "skill" => Some(ArtifactKind::Skill),
            "script" => Some(ArtifactKind::Script),
            "mcp" => Some(ArtifactKind::Mcp),
            _ => None,
        };

        let mut artifacts = self.forge.registry().list(kind_filter, None);

        // Apply status filter
        if !status_filter.is_empty() {
            let target_status = match status_filter.as_str() {
                "draft" => ArtifactStatus::Draft,
                "active" => ArtifactStatus::Active,
                "deprecated" => ArtifactStatus::Archived,
                "testing" => ArtifactStatus::Observing,
                "observing" => ArtifactStatus::Observing,
                "degraded" => ArtifactStatus::Degraded,
                _ => {
                    return ForgeToolResult::err(format!(
                        "Unknown status filter: {}",
                        status_filter
                    ))
                }
            };
            artifacts.retain(|a| a.status == target_status);
        }

        if artifacts.is_empty() {
            return ForgeToolResult::ok("No Forge artifacts found");
        }

        let mut output = format!("Total {} Forge artifacts:\n\n", artifacts.len());
        output.push_str("| ID | Type | Name | Version | Status | Usage Count | Success Rate |\n");
        output.push_str("|-----|------|------|---------|--------|-------------|-------------|\n");
        for a in &artifacts {
            let sr = if a.usage_count > 0 {
                let total = a.usage_count + a.consecutive_observing_rounds as u64;
                format!("{:.0}%", a.usage_count as f64 / total.max(1) as f64 * 100.0)
            } else {
                "N/A".to_string()
            };
            output.push_str(&format!(
                "| {} | {:?} | {} | {} | {:?} | {} | {} |\n",
                a.id, a.kind, a.name, a.version, a.status, a.usage_count, sr
            ));
        }

        ForgeToolResult::ok(output)
    }

    // -- forge_evaluate -----------------------------------------------------

    async fn execute_evaluate(&self, args: &serde_json::Value) -> ForgeToolResult {
        let id = args["id"].as_str().unwrap_or("").to_string();
        if id.is_empty() {
            return ForgeToolResult::err("id is required");
        }

        let artifact = match self.forge.registry().get(&id) {
            Some(a) => a,
            None => return ForgeToolResult::err(format!("Artifact {} does not exist", id)),
        };

        let old_status = format!("{:?}", artifact.status);

        // Try to read content from file first, fallback to stored content
        let forge_dir = self.forge.workspace().join("forge");
        let artifact_path = resolve_artifact_path(&forge_dir, &artifact);
        let file_content = tokio::fs::read_to_string(&artifact_path)
            .await
            .unwrap_or_else(|_| artifact.content.clone());

        // --- Stage 1: Static Validation ---
        let stage1 = static_validation(&file_content, &artifact.kind);

        // --- Stage 2: Functional Validation ---
        let test_runner = crate::test_runner::TestRunner::new();
        let mut eval_artifact = artifact.clone();
        eval_artifact.content = file_content.clone();
        let test_result = test_runner.run_tests(&eval_artifact);
        let stage2 = FunctionalValidationResult {
            passed: test_result.stage.passed,
            tests_run: test_result.tests_run,
            tests_passed: test_result.tests_passed,
            errors: test_result.stage.errors.clone(),
        };

        // --- Stage 3: Quality Assessment ---
        let stage3 = quality_assessment(&file_content, &artifact.kind);

        // Determine new status based on all three stages
        use nemesis_types::forge::ArtifactStatus;
        let new_status = if stage1.passed && stage2.passed && stage3.score >= 60 {
            ArtifactStatus::Active
        } else if stage1.passed && stage2.passed {
            ArtifactStatus::Observing
        } else if stage1.passed {
            ArtifactStatus::Draft
        } else {
            ArtifactStatus::Draft
        };

        // Update registry with validation results
        self.forge.registry().update(&id, |a| {
            a.status = new_status.clone();
        });

        // Format results
        let mut output = format!("## Forge Artifact Evaluation: {}\n\n", id);
        output.push_str(&format!(
            "**Status: {} -> {:?}**\n\n",
            old_status, new_status
        ));

        // Stage 1: Static validation
        output.push_str("### Stage 1: Static Validation\n");
        output.push_str(&format!(
            "- **{}**\n",
            if stage1.passed { "Passed" } else { "Failed" }
        ));
        for check in &stage1.checks {
            output.push_str(&format!(
                "  - {}: {}\n",
                check.name,
                if check.passed { "OK" } else { "FAIL" }
            ));
            if let Some(ref detail) = check.detail {
                output.push_str(&format!("    - {}\n", detail));
            }
        }

        // Stage 2: Functional validation
        output.push_str("\n### Stage 2: Functional Validation\n");
        if stage2.tests_run > 0 {
            output.push_str(&format!(
                "- **{}** ({}/{} tests passed)\n",
                if stage2.passed { "Passed" } else { "Failed" },
                stage2.tests_passed,
                stage2.tests_run
            ));
        } else {
            output.push_str("- Skipped (no tests available)\n");
        }
        for e in &stage2.errors {
            output.push_str(&format!("  - Error: {}\n", e));
        }

        // Stage 3: Quality assessment
        output.push_str("\n### Stage 3: Quality Assessment\n");
        output.push_str(&format!("- **Score: {}/100**\n", stage3.score));
        for dim in &stage3.dimensions {
            output.push_str(&format!(
                "  - {}: {}/{} {}\n",
                dim.name, dim.score, dim.max_score,
                if dim.score >= dim.max_score / 2 { "(good)" } else { "(needs improvement)" }
            ));
            if let Some(ref note) = dim.note {
                output.push_str(&format!("    - {}\n", note));
            }
        }

        output.push_str(&format!("\n**New status: {:?}**\n", new_status));

        ForgeToolResult::ok(output)
    }

    // -- forge_build_mcp ----------------------------------------------------

    async fn execute_build_mcp(&self, args: &serde_json::Value) -> ForgeToolResult {
        let id = args["id"].as_str().unwrap_or("").to_string();
        let action = args["action"]
            .as_str()
            .unwrap_or("build")
            .to_string();

        if id.is_empty() {
            return ForgeToolResult::err("id is required");
        }

        let artifact = match self.forge.registry().get(&id) {
            Some(a) => a,
            None => return ForgeToolResult::err(format!("Artifact {} does not exist", id)),
        };

        use nemesis_types::forge::ArtifactKind;
        if artifact.kind != ArtifactKind::Mcp {
            return ForgeToolResult::err(format!("Artifact {} is not an MCP type", id));
        }

        let forge_dir = self.forge.workspace().join("forge");
        let artifact_path = resolve_artifact_path(&forge_dir, &artifact);

        match action.as_str() {
            "build" => {
                // Read content from file
                let file_content = tokio::fs::read_to_string(&artifact_path)
                    .await
                    .unwrap_or_else(|_| artifact.content.clone());

                // Run validation pipeline
                let test_runner = crate::test_runner::TestRunner::new();
                let mut eval_artifact = artifact.clone();
                eval_artifact.content = file_content.clone();
                let result = test_runner.run_tests(&eval_artifact);

                let stage1_passed = !file_content.is_empty();
                let stage2_passed = result.stage.passed;

                use nemesis_types::forge::ArtifactStatus;
                let new_status = if stage1_passed && stage2_passed {
                    ArtifactStatus::Active
                } else {
                    ArtifactStatus::Draft
                };

                self.forge.registry().update(&id, |a| {
                    a.status = new_status.clone();
                });

                let mut output = format!("MCP build validation: {}\n", id);
                output.push_str(&format!(
                    "Status: {:?} → {:?}\n",
                    artifact.status, new_status
                ));
                output.push_str(&format!(
                    "- Static validation: {}\n",
                    if stage1_passed { "passed" } else { "failed" }
                ));
                output.push_str(&format!(
                    "- Functional validation: {} ({}/{} tests)\n",
                    if stage2_passed { "passed" } else { "failed" },
                    result.tests_passed,
                    result.tests_run
                ));

                ForgeToolResult::ok(output)
            }
            "install" => {
                // Register MCP to config.mcp.json — actually write the config.
                let mcp_dir = artifact_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| artifact_path.clone());

                // Determine the command and args for the MCP server entry
                let entry_name = format!("forge-{}", artifact.name);
                let (command, args_vec) = if mcp_dir.join("server.py").exists() {
                    ("python".to_string(), vec![mcp_dir.join("server.py").to_string_lossy().to_string()])
                } else if mcp_dir.join("main.go").exists() {
                    ("go".to_string(), vec!["run".to_string(), mcp_dir.join("main.go").to_string_lossy().to_string()])
                } else {
                    ("python".to_string(), vec![artifact_path.to_string_lossy().to_string()])
                };

                // Read or create the MCP config file
                let config_dir = self.forge.workspace().join("config");
                let mcp_config_path = config_dir.join("config.mcp.json");

                let _ = tokio::fs::create_dir_all(&config_dir).await;

                let mut config: serde_json::Value = if mcp_config_path.exists() {
                    match tokio::fs::read_to_string(&mcp_config_path).await {
                        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| {
                            serde_json::json!({"mcpServers": {}})
                        }),
                        Err(_) => serde_json::json!({"mcpServers": {}}),
                    }
                } else {
                    serde_json::json!({"mcpServers": {}})
                };

                // Ensure mcpServers object exists
                if !config.get("mcpServers").map_or(false, |v| v.is_object()) {
                    config["mcpServers"] = serde_json::json!({});
                }

                // Add the new MCP server entry
                config["mcpServers"][&entry_name] = serde_json::json!({
                    "command": command,
                    "args": args_vec,
                    "env": {}
                });

                // Write back to disk
                match serde_json::to_string_pretty(&config) {
                    Ok(pretty) => {
                        if let Err(e) = tokio::fs::write(&mcp_config_path, &pretty).await {
                            ForgeToolResult::err(format!(
                                "Failed to write MCP config: {}", e
                            ))
                        } else {
                            ForgeToolResult::ok(format!(
                                "MCP server '{}' installed to config.mcp.json\n- Command: {}\n- Args: {:?}\n- Path: {}",
                                entry_name, command, args_vec, mcp_config_path.display()
                            ))
                        }
                    }
                    Err(e) => ForgeToolResult::err(format!(
                        "Failed to serialize MCP config: {}", e
                    )),
                }
            }
            "uninstall" => {
                // Remove MCP from config.mcp.json
                let entry_name = format!("forge-{}", artifact.name);
                let config_dir = self.forge.workspace().join("config");
                let mcp_config_path = config_dir.join("config.mcp.json");

                if mcp_config_path.exists() {
                    match tokio::fs::read_to_string(&mcp_config_path).await {
                        Ok(content) => {
                            let mut config: serde_json::Value =
                                serde_json::from_str(&content).unwrap_or_else(|_| {
                                    serde_json::json!({"mcpServers": {}})
                                });
                            if let Some(servers) = config.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                                if servers.remove(&entry_name).is_some() {
                                    if let Ok(pretty) = serde_json::to_string_pretty(&config) {
                                        let _ = tokio::fs::write(&mcp_config_path, &pretty).await;
                                    }
                                    ForgeToolResult::ok(format!(
                                        "MCP server '{}' removed from config.mcp.json",
                                        entry_name
                                    ))
                                } else {
                                    ForgeToolResult::ok(format!(
                                        "MCP server '{}' was not in config.mcp.json (already uninstalled)",
                                        entry_name
                                    ))
                                }
                            } else {
                                ForgeToolResult::ok(format!(
                                    "MCP server '{}' was not in config.mcp.json (no mcpServers section)",
                                    entry_name
                                ))
                            }
                        }
                        Err(e) => ForgeToolResult::err(format!(
                            "Failed to read MCP config: {}", e
                        )),
                    }
                } else {
                    ForgeToolResult::ok(format!(
                        "MCP server '{}' was not installed (config.mcp.json does not exist)",
                        entry_name
                    ))
                }
            }
            _ => ForgeToolResult::err(format!(
                "Unknown action: {} (supported: build, install, uninstall)",
                action
            )),
        }
    }

    // -- forge_share --------------------------------------------------------

    async fn execute_share(&self, args: &serde_json::Value) -> ForgeToolResult {
        // Check if bridge is configured
        match self.forge.bridge() {
            Some(bridge) => {
                let report_path = args["report_path"].as_str().unwrap_or("").to_string();

                // Find latest report if path is empty
                let report_path = if report_path.is_empty() {
                    match find_latest_report(self.forge.workspace()) {
                        Some(path) => path,
                        None => {
                            return ForgeToolResult::err(
                                "No reflection report found. Run forge_reflect first.",
                            )
                        }
                    }
                } else {
                    // Validate report_path is within reflections directory
                    let reflections_dir = self
                        .forge
                        .workspace()
                        .join("forge")
                        .join("reflections");
                    let abs_path = PathBuf::from(&report_path);
                    if let Ok(canonical) = abs_path.canonicalize() {
                        if let Ok(refl_canonical) = reflections_dir.canonicalize() {
                            if !canonical.starts_with(&refl_canonical) {
                                return ForgeToolResult::err(
                                    "report_path must be within forge reflections directory",
                                );
                            }
                        }
                    }
                    report_path
                };

                let report = serde_json::json!({
                    "source": bridge.local_node_id(),
                    "report_path": report_path,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                });

                match bridge.share_reflection(report).await {
                    Ok(count) => ForgeToolResult::ok(format!(
                        "Reflection report shared with {} peers",
                        count
                    )),
                    Err(e) => ForgeToolResult::err(format!("Share failed: {}", e)),
                }
            }
            None => ForgeToolResult::ok(
                "Forge cluster sharing is not enabled. Ensure cluster mode is on and bridge is configured.",
            ),
        }
    }

    // -- forge_learning_status ----------------------------------------------

    async fn execute_learning_status(&self, _args: &serde_json::Value) -> ForgeToolResult {
        let config = self.forge.config();

        if !config.learning.enabled {
            return ForgeToolResult::ok(
                "Forge closed-loop learning is not enabled. Set learning.enabled = true in forge.json to enable.",
            );
        }

        let mut output = String::from("## Forge Closed-Loop Learning Status\n\n");
        output.push_str("- Learning engine: Enabled\n");
        output.push_str(&format!(
            "- Min pattern frequency: {}\n",
            config.learning.min_pattern_frequency
        ));
        output.push_str(&format!(
            "- High confidence threshold: {:.2}\n",
            config.learning.high_conf_threshold
        ));
        output.push_str(&format!(
            "- Max auto-creates per cycle: {}\n",
            config.learning.max_auto_creates
        ));

        // Latest cycle info
        if let Some(le) = self.forge.learning_engine() {
            if let Some(cycle) = le.get_latest_cycle() {
                output.push_str("\n### Latest Learning Cycle\n");
                output.push_str(&format!("- ID: {}\n", cycle.id));
                output.push_str(&format!("- Started: {}\n", cycle.started_at));
                if let Some(ref completed) = cycle.completed_at {
                    output.push_str(&format!("- Completed: {}\n", completed));
                }
                output.push_str(&format!(
                    "- Patterns found: {}\n",
                    cycle.patterns_found
                ));
                output.push_str(&format!("- Actions taken: {}\n", cycle.actions_taken));
            } else {
                output.push_str("\nNo learning cycle recorded yet.\n");
            }
        }

        // Show active artifacts
        let artifacts = self.forge.registry().list(None, None);
        let active: Vec<_> = artifacts
            .iter()
            .filter(|a| {
                a.status == nemesis_types::forge::ArtifactStatus::Active
                    && !a.tool_signature.is_empty()
            })
            .collect();

        if !active.is_empty() {
            output.push_str(&format!(
                "\n### Active Learning Artifacts ({})\n\n",
                active.len()
            ));
            output.push_str("| ID | Name | Tool Signature | Usage Count | Success Rate |\n");
            output.push_str("|-----|------|----------------|-------------|-------------|\n");
            for a in &active {
                let sig = a.tool_signature.join("->");
                let truncated = utils::truncate(&sig, 30);
                let sr = if a.usage_count > 0 {
                    let total = a.usage_count + a.consecutive_observing_rounds as u64;
                    format!(
                        "{:.0}%",
                        a.usage_count as f64 / total.max(1) as f64 * 100.0
                    )
                } else {
                    "N/A".to_string()
                };
                output.push_str(&format!(
                    "| {} | {} | {} | {} | {} |\n",
                    a.id, a.name, truncated, a.usage_count, sr
                ));
            }
        }

        ForgeToolResult::ok(output)
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

use nemesis_types::forge::ArtifactKind;

/// Resolve the on-disk path for an artifact based on its type and name.
fn resolve_artifact_path(forge_dir: &std::path::Path, artifact: &nemesis_types::forge::Artifact) -> PathBuf {
    match artifact.kind {
        ArtifactKind::Skill => forge_dir.join("skills").join(&artifact.name).join("SKILL.md"),
        ArtifactKind::Script => forge_dir.join("scripts").join(&artifact.name),
        ArtifactKind::Mcp => {
            // Try both Python and Go entry points
            let mcp_dir = forge_dir.join("mcp").join(&artifact.name);
            if mcp_dir.join("server.py").exists() {
                mcp_dir.join("server.py")
            } else if mcp_dir.join("main.go").exists() {
                mcp_dir.join("main.go")
            } else {
                mcp_dir.join("server.py")
            }
        }
    }
}

/// Find the latest reflection report in the workspace.
fn find_latest_report(workspace: &std::path::Path) -> Option<String> {
    let reflections_dir = workspace.join("forge").join("reflections");
    if !reflections_dir.exists() {
        return None;
    }

    let mut latest: Option<(std::time::SystemTime, String)> = None;
    if let Ok(entries) = std::fs::read_dir(&reflections_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        let is_newer = latest
                            .as_ref()
                            .map_or(true, |(t, _)| modified > *t);
                        if is_newer {
                            latest = Some((modified, path.to_string_lossy().to_string()));
                        }
                    }
                }
            }
        }
    }
    // Also check subdirectories
    if let Ok(entries) = std::fs::read_dir(&reflections_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(sub_entries) = std::fs::read_dir(&path) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if sub_path.extension().map(|e| e == "md").unwrap_or(false) {
                            if let Ok(meta) = sub_entry.metadata() {
                                if let Ok(modified) = meta.modified() {
                                    let is_newer = latest
                                        .as_ref()
                                        .map_or(true, |(t, _)| modified > *t);
                                    if is_newer {
                                        latest =
                                            Some((modified, sub_path.to_string_lossy().to_string()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    latest.map(|(_, p)| p)
}

/// Compute a heuristic quality score for an artifact.
/// Legacy heuristic quality scoring (retained for backward compatibility with existing tests).
#[allow(dead_code)]
fn compute_quality_score(content: &str, kind: &ArtifactKind) -> (i32, String) {
    let len = content.len();
    let mut notes = Vec::new();

    // Base score from content length
    let len_score = if len > 500 { 25 } else if len > 200 { 15 } else if len > 50 { 5 } else { 0 };

    // Structure score
    let mut struct_score = 0;
    match kind {
        ArtifactKind::Skill => {
            if content.contains("---") { struct_score += 15; }
            if content.contains("## ") { struct_score += 10; }
            if content.contains("- ") { struct_score += 5; }
        }
        ArtifactKind::Script => {
            if content.contains("#!/") { struct_score += 10; }
            if content.contains("echo ") || content.contains("test ") || content.contains("assert") {
                struct_score += 10;
            }
        }
        ArtifactKind::Mcp => {
            if content.contains("Server(") || content.contains("server") { struct_score += 10; }
            if content.contains("def ") || content.contains("func ") { struct_score += 10; }
        }
    }

    // Quality indicators
    let mut quality_score = 0;
    if content.contains("error") || content.contains("Error") || content.contains("handle") {
        quality_score += 5;
        notes.push("Has error handling".to_string());
    }
    if content.lines().count() > 10 {
        quality_score += 5;
    }

    let total = (len_score + struct_score + quality_score).min(100);
    if notes.is_empty() {
        notes.push(format!("Content: {} bytes, {} lines", len, content.lines().count()));
    }

    (total, notes.join("; "))
}

/// Increment a semver-like version string.
pub fn increment_version(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() < 2 {
        return format!("{}.1", version);
    }
    let last_idx = parts.len() - 1;
    if let Ok(mut minor) = parts[last_idx].parse::<u32>() {
        minor += 1;
        let mut result_parts: Vec<String> = parts[..last_idx].iter().map(|s| s.to_string()).collect();
        result_parts.push(minor.to_string());
        result_parts.join(".")
    } else {
        format!("{}.1", version)
    }
}

/// Save a version snapshot of an artifact file (synchronous).
///
/// Creates a `.versions/` subdirectory under the artifact's parent directory
/// and copies the artifact content to `{version}.bak`.
pub fn save_version_snapshot(artifact_path: &str, version: &str) -> std::io::Result<()> {
    let path = std::path::Path::new(artifact_path);
    let versions_dir = path
        .parent()
        .unwrap_or(path)
        .join(".versions");
    std::fs::create_dir_all(&versions_dir)?;

    let content = std::fs::read_to_string(path).unwrap_or_default();
    let snapshot_path = versions_dir.join(format!("{}.bak", version));
    std::fs::write(&snapshot_path, content)?;
    Ok(())
}

/// Load a version snapshot (synchronous).
///
/// Reads the snapshot content from `.versions/{version}.bak` under the
/// artifact's parent directory.
pub fn load_version_snapshot(artifact_path: &str, version: &str) -> std::io::Result<String> {
    let path = std::path::Path::new(artifact_path);
    let snapshot_path = path
        .parent()
        .unwrap_or(path)
        .join(".versions")
        .join(format!("{}.bak", version));
    std::fs::read_to_string(&snapshot_path)
}

/// Version snapshot operations.
pub mod version {
    use std::path::Path;

    /// Save a version snapshot of an artifact file.
    pub async fn save_snapshot(
        artifact_path: &Path,
        version: &str,
    ) -> std::io::Result<()> {
        let versions_dir = artifact_path
            .parent()
            .unwrap_or(artifact_path)
            .join(".versions");
        tokio::fs::create_dir_all(&versions_dir).await?;

        let content = tokio::fs::read_to_string(artifact_path).await.unwrap_or_default();
        let snapshot_path = versions_dir.join(format!("{}.bak", version));
        tokio::fs::write(&snapshot_path, content).await?;
        Ok(())
    }

    /// Load a version snapshot.
    pub async fn load_snapshot(
        artifact_path: &Path,
        version: &str,
    ) -> std::io::Result<String> {
        let snapshot_path = artifact_path
            .parent()
            .unwrap_or(artifact_path)
            .join(".versions")
            .join(format!("{}.bak", version));
        tokio::fs::read_to_string(&snapshot_path).await
    }
}

#[cfg(test)]
mod tests {
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

        let loaded = version::load_snapshot(&artifact_path, "1.0")
            .await
            .unwrap();
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
        let req_path = dir.path().join("forge").join("mcp").join("py-mcp").join("requirements.txt");
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

        let result = executor
            .execute("forge_list", &serde_json::json!({}))
            .await;
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
        let list_result = executor
            .execute("forge_list", &serde_json::json!({}))
            .await;
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
        assert_eq!(path, std::path::PathBuf::from("/tmp/forge/skills/my-skill/SKILL.md"));
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
        let go_mod_path = dir.path().join("forge").join("mcp").join("go-mcp").join("go.mod");
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
        assert!(create_result.success, "Create failed: {}", create_result.content);

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
        assert!(rollback_result.success, "Rollback failed: {}", rollback_result.content);
        assert!(rollback_result.content.contains("rolled back from 1.0"));
    }

    /// Edge case: forge_share with bridge and reflection data
    /// (matches Go's TestForgeShareTool_Execute_WithReflections)
    #[tokio::test]
    async fn test_execute_share_with_bridge() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let mut forge = Forge::new(ForgeConfig::default(), dir.path().to_path_buf());

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
        let list_all = executor
            .execute("forge_list", &serde_json::json!({}))
            .await;
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
            assert!(!def.description.is_empty(), "Tool {} missing description", def.name);
        }
    }

    #[test]
    fn test_forge_tool_definitions_have_parameters() {
        let defs = forge_tool_definitions();
        for def in &defs {
            assert!(def.parameters.is_object(), "Tool {} missing parameters", def.name);
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
        assert!(result.passed, "Valid skill should pass: {:?}", result.checks);
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
        let content = "#!/bin/bash\n# A script that does useful work\necho hello world\nexit 0\n# End of script";
        let result = static_validation(content, &ArtifactKind::Script);
        assert!(result.passed, "Valid script should pass: {:?}", result.checks);
    }

    #[test]
    fn test_static_validation_script_main_function() {
        let content = "# My Script\n\ndef main():\n    print('hello world')\n    print('goodbye')\n    return 0";
        let result = static_validation(content, &ArtifactKind::Script);
        assert!(result.passed, "Script with main function should pass: {:?}", result.checks);
    }

    #[test]
    fn test_static_validation_script_no_entry() {
        let content = "A ".repeat(30); // 60 chars but no shebang or main
        let result = static_validation(&content, &ArtifactKind::Script);
        assert!(!result.passed, "Script without entry point should fail");
    }

    #[test]
    fn test_static_validation_mcp_valid() {
        let content = "from mcp.server import Server\n\nclass MyServer:\n    def handle(self):\n        pass";
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
        assert!(result.score < 60, "Short content should have low score, got {}", result.score);
    }

    #[test]
    fn test_quality_assessment_empty_content() {
        let result = quality_assessment("", &ArtifactKind::Skill);
        assert!(result.score < 50, "Empty content should have very low score");
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
        let result = executor.execute("nonexistent_tool", &serde_json::json!({})).await;
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
        assert!(!result.success || result.content.contains("disabled") || result.content.contains("not"));
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
            .execute("forge_evaluate", &serde_json::json!({"id": "nonexistent-id"}))
            .await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_execute_update_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
        let executor = ForgeToolExecutor::new(forge);
        let result = executor
            .execute("forge_update", &serde_json::json!({"id": "nonexistent-id", "content": "new"}))
            .await;
        assert!(!result.success);
    }

    #[tokio::test]
    async fn test_execute_build_mcp_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let forge = Arc::new(Forge::new(ForgeConfig::default(), dir.path().to_path_buf()));
        let executor = ForgeToolExecutor::new(forge);
        let result = executor
            .execute("forge_build_mcp", &serde_json::json!({"id": "nonexistent-id"}))
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
        assert!(result.success, "Create with description failed: {}", result.content);
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
        assert!(result.success, "MCP python create failed: {}", result.content);

        // Check requirements.txt was created
        let req_path = dir.path().join("forge").join("mcp").join("py-mcp").join("requirements.txt");
        assert!(req_path.exists(), "requirements.txt should be created for Python MCP");
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
        let id = id_line.split("ID:").nth(1).unwrap().trim();

        // Create config.mcp.json so uninstall can remove the entry
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.mcp.json"),
            r#"{"mcpServers":{"forge-uninstall-test":{"command":"python","args":["server.py"]}}}"#,
        ).unwrap();

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
        assert!(score > 50, "Score should be high: {}, notes: {}", score, notes);
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
        assert!(result.passed, "Skill with lists should pass: {:?}", result.checks);
    }

    #[test]
    fn test_static_validation_script_shebang() {
        let content = "#!/usr/bin/env python3\nimport sys\nprint('hello')\nsys.exit(0)\n# End";
        let result = static_validation(content, &ArtifactKind::Script);
        assert!(result.passed, "Script with shebang should pass: {:?}", result.checks);
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
        assert_eq!(path, std::path::PathBuf::from("/tmp/forge/mcp/my-mcp/server.py"));
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
        let id = id_line.split("ID:").nth(1).unwrap().trim();

        // Evaluate the artifact
        let eval_result = executor
            .execute("forge_evaluate", &serde_json::json!({"id": id}))
            .await;
        assert!(eval_result.success, "Evaluate failed: {}", eval_result.content);
        assert!(eval_result.content.contains("score") || eval_result.content.contains("validation") || eval_result.content.contains("passed"));
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
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
            .execute("forge_list", &serde_json::json!({"status": "invalid_status"}))
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
        let id = id_line.split("ID:").nth(1).unwrap().trim();

        // Update without content or rollback
        let result = executor
            .execute(
                "forge_update",
                &serde_json::json!({"id": id}),
            )
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
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
        assert!(result.success, "Script with category failed: {}", result.content);
        // Check script is in the deploy category directory
        let script_path = dir.path().join("forge").join("scripts").join("deploy").join("my-script");
        assert!(script_path.exists(), "Script should be in deploy category dir");
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
        let error_dim = result.dimensions.iter().find(|d| d.name == "Error handling");
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
            .execute("forge_reflect", &serde_json::json!({"period": "week", "focus": "skill"}))
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
        assert!(result.success, "MCP with other language failed: {}", result.content);
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

        let id_line = create_result.content.lines().find(|l| l.contains("ID:")).unwrap();
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
}
