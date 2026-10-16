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
use crate::mcp_installer::MCPInstaller;

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
    if !non_empty {
        all_passed = false;
    }

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
    if !min_len {
        all_passed = false;
    }

    // Check 3: Type-specific structure
    match kind {
        ArtifactKind::Skill => {
            // Skills should have markdown structure
            let has_sections = content.contains("## ") || content.contains("# ");
            checks.push(StaticCheck {
                name: "Skill has headings".into(),
                passed: has_sections,
                detail: if has_sections {
                    Some("Contains markdown headings".into())
                } else {
                    Some("Missing markdown headings".into())
                },
            });
            if !has_sections {
                all_passed = false;
            }

            let has_lists = content.contains("- ") || content.contains("* ");
            checks.push(StaticCheck {
                name: "Skill has list items".into(),
                passed: has_lists,
                detail: if has_lists {
                    Some("Contains list items".into())
                } else {
                    None
                },
            });
            // Lists are recommended but not required
        }
        ArtifactKind::Script => {
            // Scripts should have a shebang or main entry
            let has_shebang = content.starts_with("#!/");
            let has_entry = content.contains("def main")
                || content.contains("function main")
                || content.contains("func main");
            let has_valid_entry = has_shebang || has_entry;
            checks.push(StaticCheck {
                name: "Script has entry point".into(),
                passed: has_valid_entry,
                detail: if has_shebang {
                    Some("Has shebang line".into())
                } else if has_entry {
                    Some("Has main function".into())
                } else {
                    Some("No shebang or main function found".into())
                },
            });
            if !has_valid_entry {
                all_passed = false;
            }
        }
        ArtifactKind::Mcp => {
            // MCP servers should have server-related code
            let has_server = content.contains("Server")
                || content.contains("server")
                || content.contains("tool(")
                || content.contains("@tool");
            checks.push(StaticCheck {
                name: "MCP has server setup".into(),
                passed: has_server,
                detail: if has_server {
                    Some("Contains server initialization".into())
                } else {
                    Some("No server setup detected".into())
                },
            });
            if !has_server {
                all_passed = false;
            }

            let has_handler = content.contains("def ")
                || content.contains("func ")
                || content.contains("async def ");
            checks.push(StaticCheck {
                name: "MCP has handler functions".into(),
                passed: has_handler,
                detail: if has_handler {
                    Some("Contains handler functions".into())
                } else {
                    Some("No handler functions detected".into())
                },
            });
            if !has_handler {
                all_passed = false;
            }
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
    if !has_reasonable_lines {
        all_passed = false;
    }

    // Check 5: Security scan（F-S1 同类扫，2026-10-16 R16）——危险命令 +
    // 硬编码密钥拦在晋升闸前。本函数是 forge_evaluate 的 Stage 1，此前纯
    // 结构检查：手工编辑/历史落盘/未设闸时期写入的危险内容可 Draft→Active
    // 晋升为 agent 指令。规则与写盘闸同源（StaticValidator::security_errors）。
    let sec_errors = crate::validator::StaticValidator::new().security_errors(content);
    checks.push(StaticCheck {
        name: "Security scan".into(),
        passed: sec_errors.is_empty(),
        detail: if sec_errors.is_empty() {
            None
        } else {
            Some(sec_errors.join("; "))
        },
    });
    if !sec_errors.is_empty() {
        all_passed = false;
    }

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
        if len > 500 {
            s += 10;
        } else if len > 200 {
            s += 5;
        }
        if content.lines().count() > 15 {
            s += 8;
        } else if content.lines().count() > 5 {
            s += 4;
        }
        match kind {
            ArtifactKind::Skill => {
                if content.contains("## ") {
                    s += 4;
                }
                if content.contains("- ") || content.contains("* ") {
                    s += 3;
                }
            }
            ArtifactKind::Script => {
                if content.contains("#!") {
                    s += 4;
                }
                if content.contains("echo ") || content.contains("print(") {
                    s += 3;
                }
            }
            ArtifactKind::Mcp => {
                if content.contains("Server") || content.contains("server") {
                    s += 4;
                }
                if content.contains("if __name__") || content.contains("func main") {
                    s += 3;
                }
            }
        }
        s.min(25)
    };
    dimensions.push(QualityDimension {
        name: "Content completeness".into(),
        score: completeness_score,
        max_score: 25,
        note: Some(format!(
            "{} bytes, {} lines",
            content.len(),
            content.lines().count()
        )),
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
        if content.contains("return")
            && (content.contains("error") || content.contains("nil") || content.contains("None"))
        {
            s += 4;
        }
        s.min(25)
    };
    dimensions.push(QualityDimension {
        name: "Error handling".into(),
        score: error_handling_score,
        max_score: 25,
        note: if error_handling_score > 0 {
            Some("Has error handling patterns".into())
        } else {
            Some("No error handling detected".into())
        },
    });
    total_score += error_handling_score;

    // Dimension 3: Documentation (0-25)
    let documentation_score = {
        let mut s = 0i32;
        let comment_lines = content
            .lines()
            .filter(|l| {
                l.trim().starts_with('#')
                    || l.trim().starts_with("//")
                    || l.trim().starts_with("/*")
                    || l.trim().starts_with("* ")
                    || l.trim().starts_with("'''")
                    || l.trim().starts_with("\"\"\"")
            })
            .count();
        if comment_lines >= 3 {
            s += 10;
        } else if comment_lines >= 1 {
            s += 5;
        }
        if content.contains("README") || content.contains("Usage") || content.contains("usage") {
            s += 5;
        }
        if content.contains("description")
            || content.contains("Description")
            || content.contains("docstring")
        {
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
        if spaces > 0 || tabs > 0 {
            s += 5;
        }

        // Function/method definitions
        let fn_count = content.matches("def ").count()
            + content.matches("func ").count()
            + content.matches("fn ").count();
        if fn_count > 0 {
            s += 8;
        }

        // Type hints or type annotations
        if content.contains(": str")
            || content.contains(": int")
            || content.contains("-> ")
            || content.contains(": String")
            || content.contains(": i32")
        {
            s += 7;
        }

        // Return statements
        if content.contains("return ") {
            s += 5;
        }

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
    pub async fn execute(&self, tool_name: &str, args: &serde_json::Value) -> ForgeToolResult {
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

        let mut output = "## Reflection Report\n\n".to_string();
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
        let description = args["description"].as_str().unwrap_or("").to_string();

        if artifact_type.is_empty() || name.is_empty() || content.is_empty() {
            return ForgeToolResult::err("type, name, content are required fields");
        }

        // Validate: scripts and MCP require test_cases
        if (artifact_type == "script" || artifact_type == "mcp")
            && args.get("test_cases").is_none_or(|v| v.is_null())
        {
            return ForgeToolResult::err(
                "Script and MCP types require test_cases. Provide test cases and retry.",
            );
        }

        // Validate artifact type
        match artifact_type.as_str() {
            "skill" | "script" | "mcp" => {}
            _ => {
                return ForgeToolResult::err(
                    "type must be 'skill', 'script', or 'mcp'".to_string(),
                );
            }
        }

        // Sanitize name（2026-10-16 R18 路径穿越修复）：旧实现只
        // `to_lowercase().replace(' ', "-")`——`../`、`..\`、`C:x` 全部透传，
        // 下游 `forge/skills/{name}`、`scripts/{category}/{name}`、
        // `mcp/{name}` 三处 join 全是可穿越写面（agent 可直接调本工具）。
        // 与 learning_engine 蒸馏名同一规则（slugify_skill_name 单一真相
        // 源）：折叠为安全 kebab 段，全非法名诚实报错。
        let name = crate::forge::slugify_skill_name(&name);
        if name.is_empty() {
            return ForgeToolResult::err(
                "name contains no usable characters after sanitization (need [a-z0-9_-])",
            );
        }

        // Use shared CreateSkill method for skill type (matching Go's behavior:
        // t.forge.CreateSkill(ctx, name, content, description, nil))
        if artifact_type == "skill" {
            match self
                .forge
                .create_skill(&name, &content, &description, Vec::new())
            {
                Ok(artifact) => {
                    let status = format!("{:?}", artifact.status);
                    let mut validation_info = String::new();
                    if self.forge.config().validation.auto_validate {
                        validation_info
                            .push_str(&format!("\n- Auto-validation: enabled (status={})", status));
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

        // Security gate (F-S1): block dangerous commands / hardcoded secrets
        // BEFORE writing script/mcp content to disk.
        let sec_errors = crate::validator::StaticValidator::new().security_errors(&content);
        if !sec_errors.is_empty() {
            return ForgeToolResult::err(format!(
                "Content failed security validation: {}",
                sec_errors.join("; ")
            ));
        }

        let forge_dir = self.forge.workspace().join("forge");
        let artifact_path = match artifact_type.as_str() {
            "script" => {
                // category 同样是路径段（R18：此前完全未消毒，`../` 可越出
                // scripts/ 目录），同一 slug 规则。
                let category =
                    crate::forge::slugify_skill_name(args["category"].as_str().unwrap_or("utils"));
                let category = if category.is_empty() {
                    "utils"
                } else {
                    &category
                };
                let category = category.to_string();
                forge_dir.join("scripts").join(category).join(&name)
            }
            "mcp" => {
                let language = args["language"].as_str().unwrap_or("python");
                let ext = if language == "go" { "go" } else { "py" };
                let entry = if language == "go" { "main" } else { "server" };
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
                        let _ =
                            tokio::fs::write(mcp_dir.join("requirements.txt"), requirements).await;
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
        if let Some(test_cases) = args.get("test_cases")
            && !test_cases.is_null()
            && let Ok(test_data) = serde_json::to_string_pretty(test_cases)
            && let Some(parent) = artifact_path.parent()
        {
            let test_dir = parent.join("tests");
            let _ = tokio::fs::create_dir_all(&test_dir).await;
            let _ = tokio::fs::write(test_dir.join("test_cases.json"), test_data).await;
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
            created_at: chrono::Local::now().to_rfc3339(),
            updated_at: chrono::Local::now().to_rfc3339(),
            usage_count: 0,
            last_degraded_at: None,
            success_rate: 0.0,
            consecutive_observing_rounds: 0,
            deployed_at: None,
            last_efficacy: None,
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
            if let Some(ref a) = final_artifact
                && matches!(a.status, nemesis_types::forge::ArtifactStatus::Active)
                && let Some(installer) = self.forge.mcp_installer()
            {
                let _mcp_dir = artifact_path.parent().unwrap_or(&artifact_path);
                let command = if args["language"].as_str() == Some("go") {
                    "go".to_string()
                } else {
                    "uv".to_string()
                };
                // `go run <file>` 和 `uv run <file>` share the same args;
                // the command itself (see above) carries the difference.
                // 2026-10-16 复查（FT1）：必须传**绝对路径**——此前传裸文件名，
                // MCP server 进程以 gateway 的 CWD 启动，`uv run server.py`
                // 只有在 CWD 恰好等于 mcp 目录时才能命中；与 execute_build_mcp
                // 安装臂（同文件下方，绝对路径）口径对齐。
                let cmd_args = vec![
                    "run".to_string(),
                    artifact_path.to_string_lossy().to_string(),
                ];
                match installer.install(&name, &command, cmd_args) {
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

        // R18 诚实化：installer 未注入时 Active MCP 不会落 config.mcp.json。
        // 此前静默——create 报告「成功」而用户等一个不存在的注册。当前
        // init_mcp_installer 无生产接线点（gateway 不构造），该臂是常态
        // 而非异常，必须让调用方（LLM/用户）知道要手动注册。
        if artifact_type == "mcp"
            && self
                .forge
                .registry()
                .get(&artifact_id)
                .is_some_and(|a| matches!(a.status, nemesis_types::forge::ArtifactStatus::Active))
            && self.forge.mcp_installer().is_none()
        {
            validation_info.push_str(
                "\n- MCP auto-registration unavailable: no installer wired in this context (register manually in config.mcp.json)",
            );
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
            artifact_type,
            name,
            artifact_path.display(),
            final_status,
            artifact_id,
            validation_info
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
        let rollback_version = args["rollback_version"].as_str().unwrap_or("").to_string();

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
                    return ForgeToolResult::err(format!("Failed to load version snapshot: {}", e));
                }
            }
        }

        if content.is_empty() {
            return ForgeToolResult::err("content or rollback_version is required");
        }

        // F-S1 安全闸（2026-10-16 R16 同类扫）：agent 可调的更新面在写盘前
        // 过 security_errors——content 直写 forge 文件且 Skill 类同步到活
        // workspace/skills/（agent 指令面），比 create 路径更需要闸。回滚
        // 快照同样过闸（快照可能出自未设闸时期的落盘内容）。
        let sec_errors = crate::validator::StaticValidator::new().security_errors(&content);
        if !sec_errors.is_empty() {
            return ForgeToolResult::err(format!(
                "security gate rejected update: {}",
                sec_errors.join("; ")
            ));
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
            // 2026-10-16 复查（FT2）：目录段必须 slug——registry 可能装着
            // 历史污染名（`../x` 等，R18 的 resolve_artifact_path 只护了
            // forge/ 主副本），这里拼 workspace/skills/ 同样是名字当路径段。
            // slugify 对合法 kebab 名恒等，对穿越序列消解。
            let skill_slug = crate::forge::slugify_skill_name(&artifact.name);
            if skill_slug.is_empty() {
                return ForgeToolResult::err(format!(
                    "Artifact {} has an unusable name (slugifies to empty); skill copy skipped",
                    id
                ));
            }
            let skills_dir = self
                .forge
                .workspace()
                .join("skills")
                .join(format!("{}-forge", skill_slug));
            let _ = tokio::fs::create_dir_all(&skills_dir).await;
            let _ = tokio::fs::write(skills_dir.join("SKILL.md"), &content).await;
        }

        // Re-register MCP if active after update (mirrors Go behavior)
        if artifact.kind == ArtifactKind::Mcp {
            let updated = self.forge.registry().get(&id);
            if let Some(ref a) = updated
                && matches!(a.status, nemesis_types::forge::ArtifactStatus::Active)
            {
                match self.forge.mcp_installer() {
                    Some(installer) => {
                        // FT1 同类收口（2026-10-16 全文复查）：与 create 臂同
                        // 口径——绝对路径 + 按入口扩展名选 command（main.go →
                        // go，server.py → uv）+ 安装失败诚实 warn。此前传裸
                        // file_name（`uv run server.py` 只有 gateway CWD 恰好
                        // 等于 mcp 目录才命中）且 `let _ =` 吞掉写盘失败。
                        let command = if artifact_path
                            .extension()
                            .map(|e| e == "go")
                            .unwrap_or(false)
                        {
                            "go"
                        } else {
                            "uv"
                        };
                        let cmd_args = vec![
                            "run".to_string(),
                            artifact_path.to_string_lossy().to_string(),
                        ];
                        if let Err(e) = installer.install(&artifact.name, command, cmd_args) {
                            tracing::warn!(
                                "[Forge] MCP '{}' re-registration failed: {} (register manually)",
                                artifact.name,
                                e
                            );
                        }
                    }
                    // R18 诚实化：installer 未注入 = 热注册跳过，warn 不静默
                    //（与 create 侧「unavailable」注记同语义）。
                    None => tracing::warn!(
                        "[Forge] MCP '{}' is Active but no installer is wired; auto re-registration skipped",
                        artifact.name
                    ),
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
                    ));
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
        } else {
            // Stage 1 failed, or later stages failed: stay as Draft either way.
            ArtifactStatus::Draft
        };

        // Update registry with validation results
        self.forge.registry().update(&id, |a| {
            a.status = new_status;
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
                dim.name,
                dim.score,
                dim.max_score,
                if dim.score >= dim.max_score / 2 {
                    "(good)"
                } else {
                    "(needs improvement)"
                }
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
        let action = args["action"].as_str().unwrap_or("build").to_string();

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
                    a.status = new_status;
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
                    (
                        "python".to_string(),
                        vec![mcp_dir.join("server.py").to_string_lossy().to_string()],
                    )
                } else if mcp_dir.join("main.go").exists() {
                    (
                        "go".to_string(),
                        vec![
                            "run".to_string(),
                            mcp_dir.join("main.go").to_string_lossy().to_string(),
                        ],
                    )
                } else {
                    (
                        "python".to_string(),
                        vec![artifact_path.to_string_lossy().to_string()],
                    )
                };

                // P1 单一真相源对齐（2026-10-16）：注册走 MCPInstaller（标准
                // `servers` 形状 + 写串行锁 + 更新保字段）。此前裸写
                // `mcpServers` map 形状——若文件已是标准 `servers` 数组形态，
                // 宽容解析只认 `servers` 键，这份注册整段隐形（装了等于没装）。
                let installer = MCPInstaller::new(self.forge.workspace());
                match installer.install(&entry_name, &command, args_vec.clone()) {
                    Ok(()) => ForgeToolResult::ok(format!(
                        "MCP server '{}' installed to config.mcp.json\n- Command: {}\n- Args: {:?}\n- Path: {}",
                        entry_name,
                        command,
                        args_vec,
                        installer.config_path().display()
                    )),
                    Err(e) => ForgeToolResult::err(format!("Failed to write MCP config: {}", e)),
                }
            }
            "uninstall" => {
                // Remove MCP from config.mcp.json（同走 MCPInstaller 单一真相源）。
                let entry_name = format!("forge-{}", artifact.name);
                let installer = MCPInstaller::new(self.forge.workspace());

                if !installer.config_path().exists() {
                    return ForgeToolResult::ok(format!(
                        "MCP server '{}' was not installed (config.mcp.json does not exist)",
                        entry_name
                    ));
                }

                // 配置文件在但条目不在（含文件只有无 `servers` 键的顶层字段）：
                // 宽容解析得空表 → 按已卸载诚实回话。装载/写盘失败 loud 报错，
                // 不静默当成功。
                let was_installed = installer.is_installed(&entry_name);
                match installer.uninstall(&entry_name) {
                    Ok(()) if was_installed => ForgeToolResult::ok(format!(
                        "MCP server '{}' removed from config.mcp.json",
                        entry_name
                    )),
                    Ok(()) => ForgeToolResult::ok(format!(
                        "MCP server '{}' was not in config.mcp.json (already uninstalled)",
                        entry_name
                    )),
                    Err(e) => ForgeToolResult::err(format!("Failed to update MCP config: {}", e)),
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
                            );
                        }
                    }
                } else {
                    // Validate report_path is within reflections directory
                    let reflections_dir = self.forge.workspace().join("forge").join("reflections");
                    let abs_path = PathBuf::from(&report_path);
                    // 2026-09-01 fail-open 修复：旧逻辑 `if let Ok(canonicalize)`
                    // 在路径不存在 / 表示差异（8.3 短名）时**整体跳过守卫**，
                    // reflections 目录外的任意路径都能流进 share 上报给集群
                    // 对端。canonicalize_for_compare 无条件解析（不存在路径借
                    // 最长存在祖先对齐表示），守卫恒生效。
                    let canonical = nemesis_path::paths::canonicalize_for_compare(&abs_path);
                    let refl_canonical =
                        nemesis_path::paths::canonicalize_for_compare(&reflections_dir);
                    if !canonical.starts_with(&refl_canonical) {
                        return ForgeToolResult::err(
                            "report_path must be within forge reflections directory",
                        );
                    }
                    report_path
                };

                let report = serde_json::json!({
                    "source": bridge.local_node_id(),
                    "report_path": report_path,
                    "timestamp": chrono::Local::now().to_rfc3339(),
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
                output.push_str(&format!("- Patterns found: {}\n", cycle.patterns_found));
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
                    format!("{:.0}%", a.usage_count as f64 / total.max(1) as f64 * 100.0)
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
///
/// 2026-10-16 R18：name 在使用点再次 slug——注册表是磁盘态数据，历史
/// 遗留的中毒名（旧 create 弱消毒时期写入的 `../x` 等）从这里流进
/// execute_update/evaluate/build_mcp 的读写路径仍可越出 forge 目录。
/// 合法名（slug 产物）经 slugify 后逐字节不变，零行为差异。
fn resolve_artifact_path(
    forge_dir: &std::path::Path,
    artifact: &nemesis_types::forge::Artifact,
) -> PathBuf {
    let name = crate::forge::slugify_skill_name(&artifact.name);
    // 空 slug = 纯符号中毒名（`..`、`../..` 等穿越序列，旧 create 弱消毒
    // 时期可落库）——此处回落原始名等于把穿越序列原样交还给 join，正是
    // 本函数要堵的洞。改用受控占位段：读路径诚实未命中，写路径关在 forge
    // 子目录内（合法名 slugify 恒非空，占位段永不命中正常产物）。
    let name = if name.is_empty() {
        "_unnamed_".to_string()
    } else {
        name
    };
    match artifact.kind {
        ArtifactKind::Skill => forge_dir.join("skills").join(name).join("SKILL.md"),
        ArtifactKind::Script => forge_dir.join("scripts").join(name),
        ArtifactKind::Mcp => {
            // Try both Python and Go entry points
            let mcp_dir = forge_dir.join("mcp").join(name);
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
            if path.extension().map(|e| e == "md").unwrap_or(false)
                && let Ok(meta) = entry.metadata()
                && let Ok(modified) = meta.modified()
            {
                let is_newer = latest.as_ref().is_none_or(|(t, _)| modified > *t);
                if is_newer {
                    latest = Some((modified, path.to_string_lossy().to_string()));
                }
            }
        }
    }
    // Also check subdirectories
    if let Ok(entries) = std::fs::read_dir(&reflections_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Ok(sub_entries) = std::fs::read_dir(&path)
            {
                for sub_entry in sub_entries.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.extension().map(|e| e == "md").unwrap_or(false)
                        && let Ok(meta) = sub_entry.metadata()
                        && let Ok(modified) = meta.modified()
                    {
                        let is_newer = latest.as_ref().is_none_or(|(t, _)| modified > *t);
                        if is_newer {
                            latest = Some((modified, sub_path.to_string_lossy().to_string()));
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
    let len_score = if len > 500 {
        25
    } else if len > 200 {
        15
    } else if len > 50 {
        5
    } else {
        0
    };

    // Structure score
    let mut struct_score = 0;
    match kind {
        ArtifactKind::Skill => {
            if content.contains("---") {
                struct_score += 15;
            }
            if content.contains("## ") {
                struct_score += 10;
            }
            if content.contains("- ") {
                struct_score += 5;
            }
        }
        ArtifactKind::Script => {
            if content.contains("#!/") {
                struct_score += 10;
            }
            if content.contains("echo ") || content.contains("test ") || content.contains("assert")
            {
                struct_score += 10;
            }
        }
        ArtifactKind::Mcp => {
            if content.contains("Server(") || content.contains("server") {
                struct_score += 10;
            }
            if content.contains("def ") || content.contains("func ") {
                struct_score += 10;
            }
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
        notes.push(format!(
            "Content: {} bytes, {} lines",
            len,
            content.lines().count()
        ));
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
        let mut result_parts: Vec<String> =
            parts[..last_idx].iter().map(|s| s.to_string()).collect();
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
    let versions_dir = path.parent().unwrap_or(path).join(".versions");
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
    pub async fn save_snapshot(artifact_path: &Path, version: &str) -> std::io::Result<()> {
        let versions_dir = artifact_path
            .parent()
            .unwrap_or(artifact_path)
            .join(".versions");
        tokio::fs::create_dir_all(&versions_dir).await?;

        let content = tokio::fs::read_to_string(artifact_path)
            .await
            .unwrap_or_default();
        let snapshot_path = versions_dir.join(format!("{}.bak", version));
        tokio::fs::write(&snapshot_path, content).await?;
        Ok(())
    }

    /// Load a version snapshot.
    pub async fn load_snapshot(artifact_path: &Path, version: &str) -> std::io::Result<String> {
        let snapshot_path = artifact_path
            .parent()
            .unwrap_or(artifact_path)
            .join(".versions")
            .join(format!("{}.bak", version));
        tokio::fs::read_to_string(&snapshot_path).await
    }
}

#[cfg(test)]
mod tests;
