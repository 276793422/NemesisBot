//! Workflow command - manage DAG workflows.
//!
//! Uses nemesis_workflow crate for parsing, validation, and execution.

use anyhow::Result;
use crate::common;

#[derive(clap::Subcommand)]
pub enum WorkflowAction {
    /// List workflows
    List,
    /// Run a workflow
    Run {
        /// Workflow ID or name
        name: String,
        /// Input parameters as positional key=value pairs
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        input: Vec<String>,
    },
    /// Show workflow status
    Status {
        /// Execution ID (omit for recent executions)
        id: Option<String>,
    },
    /// Manage workflow templates
    Template {
        #[command(subcommand)]
        action: Option<TemplateAction>,
    },
    /// Validate a workflow definition
    Validate {
        /// Path to workflow file (YAML or JSON)
        path: String,
    },
}

#[derive(clap::Subcommand)]
pub enum TemplateAction {
    /// List available templates
    List,
    /// Show template details
    Show {
        /// Template name
        name: String,
    },
    /// Create from template
    Create {
        /// Template name
        template: String,
        #[arg(long)]
        output: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse positional key=value arguments into a HashMap.
/// If an argument has no '=', it's stored under key "input" (only first such arg).
fn parse_positional_input(args: &[String]) -> std::collections::HashMap<String, serde_json::Value> {
    let mut map = std::collections::HashMap::new();
    let mut used_input_key = false;
    for arg in args {
        if let Some((key, value)) = arg.split_once('=') {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            let v = if value == "true" {
                serde_json::Value::Bool(true)
            } else if value == "false" {
                serde_json::Value::Bool(false)
            } else if let Ok(n) = value.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(n) = value.parse::<f64>() {
                serde_json::from_str(&format!("{}", n)).unwrap_or(serde_json::Value::String(value))
            } else {
                serde_json::Value::String(value)
            };
            map.insert(key, v);
        } else if !used_input_key {
            map.insert("input".to_string(), serde_json::Value::String(arg.clone()));
            used_input_key = true;
        }
    }
    map
}

/// Recursively scan workspace for workflow files (.yaml/.yml/.json), skipping "executions/" dir.
fn scan_workflow_files(workflow_dir: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let mut files = Vec::new();
    if !workflow_dir.exists() {
        return files;
    }

    fn scan_recursive(dir: &std::path::Path, files: &mut Vec<(String, std::path::PathBuf)>) {
        let extensions = [".yaml", ".yml", ".json"];
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_dir() {
                    // Skip executions directory
                    let dir_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    if dir_name != "executions" {
                        scan_recursive(&path, files);
                    }
                } else {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    if extensions.iter().any(|ext| name.ends_with(ext)) {
                        let display_name = name
                            .trim_end_matches(".yaml")
                            .trim_end_matches(".yml")
                            .trim_end_matches(".json")
                            .to_string();
                        files.push((display_name, path));
                    }
                }
            }
        }
    }

    scan_recursive(workflow_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// A parsed template: (name, description, definition JSON).
type Template = (String, String, serde_json::Value);

/// Search for template files on disk and return them.
///
/// Looks in two locations:
/// 1. `exe_dir/templates/` - bundled templates shipped with the binary
/// 2. `workspace/workflow/templates/` - user-defined templates
///
/// Files must be YAML (.yaml/.yml) or JSON (.json) and parse into a Workflow
/// definition matching the nemesis_workflow crate's format.
fn load_templates_from_disk() -> Vec<Template> {
    let mut templates = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    let search_dirs = {
        let mut dirs = Vec::new();

        // exe_dir/templates/
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                dirs.push(exe_dir.join("templates"));
            }
        }

        // workspace/workflow/templates/ (resolve using common helper)
        let home = common::resolve_home(false);
        dirs.push(common::workspace_path(&home).join("workflow").join("templates"));

        dirs
    };

    for search_dir in &search_dirs {
        if !search_dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(search_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let ext = path.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
                if ext != "yaml" && ext != "yml" && ext != "json" {
                    continue;
                }

                let name = path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();

                if name.is_empty() || seen_names.contains(&name) {
                    continue;
                }

                // Try to parse the file as a Workflow
                match nemesis_workflow::parser::parse_file(&path) {
                    Ok(wf) => {
                        seen_names.insert(name.clone());
                        let desc = wf.description.clone();
                        let definition = serde_json::to_value(&wf).unwrap_or(serde_json::Value::Null);
                        templates.push((name, desc, definition));
                    }
                    Err(e) => {
                        eprintln!("  Warning: Failed to parse template {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    templates.sort_by(|a, b| a.0.cmp(&b.0));
    templates
}

/// Get the hardcoded default template definitions (fallback when no disk templates found).
fn get_default_templates() -> Vec<Template> {
    vec![
        ("researcher".to_string(), "Research and summarize a topic".to_string(), serde_json::json!({
            "name": "researcher",
            "description": "Research and summarize a topic",
            "version": "1.0.0",
            "nodes": [
                {"id": "search", "node_type": "tool", "config": {"tool_name": "web_search"}, "depends_on": []},
                {"id": "analyze", "node_type": "llm", "config": {"prompt": "Analyze and summarize the research findings"}, "depends_on": ["search"]},
                {"id": "report", "node_type": "tool", "config": {"tool_name": "file_write"}, "depends_on": ["analyze"]}
            ],
            "edges": [
                {"from_node": "search", "to_node": "analyze"},
                {"from_node": "analyze", "to_node": "report"}
            ]
        })),
        ("coder".to_string(), "Code generation with review".to_string(), serde_json::json!({
            "name": "coder",
            "description": "Code generation with review",
            "version": "1.0.0",
            "nodes": [
                {"id": "generate", "node_type": "llm", "config": {"prompt": "Generate code based on requirements"}, "depends_on": []},
                {"id": "review", "node_type": "llm", "config": {"prompt": "Review the generated code for quality and correctness"}, "depends_on": ["generate"]},
                {"id": "save", "node_type": "tool", "config": {"tool_name": "file_write"}, "depends_on": ["review"]}
            ],
            "edges": [
                {"from_node": "generate", "to_node": "review"},
                {"from_node": "review", "to_node": "save", "condition": "approved"}
            ]
        })),
        ("monitor".to_string(), "Monitor a system or service".to_string(), serde_json::json!({
            "name": "monitor",
            "description": "Monitor a system or service",
            "version": "1.0.0",
            "nodes": [
                {"id": "check", "node_type": "tool", "config": {"tool_name": "http_request"}, "depends_on": []},
                {"id": "evaluate", "node_type": "condition", "config": {"expression": "status != 200"}, "depends_on": ["check"]},
                {"id": "alert", "node_type": "tool", "config": {"tool_name": "send_alert"}, "depends_on": ["evaluate"]}
            ],
            "edges": [
                {"from_node": "check", "to_node": "evaluate"},
                {"from_node": "evaluate", "to_node": "alert", "condition": "true"}
            ]
        })),
        ("collector".to_string(), "Collect and process data".to_string(), serde_json::json!({
            "name": "collector",
            "description": "Collect and process data",
            "version": "1.0.0",
            "nodes": [
                {"id": "fetch", "node_type": "tool", "config": {"tool_name": "http_request"}, "depends_on": []},
                {"id": "transform", "node_type": "transform", "config": {"expression": "data.items"}, "depends_on": ["fetch"]},
                {"id": "store", "node_type": "tool", "config": {"tool_name": "file_write"}, "depends_on": ["transform"]}
            ],
            "edges": [
                {"from_node": "fetch", "to_node": "transform"},
                {"from_node": "transform", "to_node": "store"}
            ]
        })),
        ("translator".to_string(), "Translate content between languages".to_string(), serde_json::json!({
            "name": "translator",
            "description": "Translate content between languages",
            "version": "1.0.0",
            "nodes": [
                {"id": "translate", "node_type": "llm", "config": {"prompt": "Translate the content"}, "depends_on": []},
                {"id": "review", "node_type": "llm", "config": {"prompt": "Review translation quality"}, "depends_on": ["translate"]}
            ],
            "edges": [
                {"from_node": "translate", "to_node": "review"}
            ]
        })),
    ]
}

/// Get templates: first try loading from disk, fall back to hardcoded defaults.
fn get_templates() -> Vec<Template> {
    let disk_templates = load_templates_from_disk();
    if !disk_templates.is_empty() {
        return disk_templates;
    }
    get_default_templates()
}

/// Count execution files in the executions directory.
fn count_executions(workflow_dir: &std::path::Path) -> usize {
    let exec_dir = workflow_dir.join("executions");
    if !exec_dir.exists() {
        return 0;
    }
    std::fs::read_dir(&exec_dir)
        .map(|d| d.filter_map(|e| e.ok()).filter(|e| {
            e.file_type().map(|t| t.is_file()).unwrap_or(false)
                && e.file_name().to_string_lossy().ends_with(".json")
        }).count())
        .unwrap_or(0)
}

/// Format a DateTime<Utc> for display.
fn format_datetime(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_list(workflow_dir: &std::path::Path) -> Result<()> {
    println!("Workflow Engine");
    println!();

    let files = scan_workflow_files(workflow_dir);
    let total_execs = count_executions(workflow_dir);

    if files.is_empty() {
        println!("  Registered Workflows (0):");
        println!();
        println!("  No workflows defined.");
        println!("  Create one from a template: nemesisbot workflow template create <name>");
    } else {
        println!("  Registered Workflows ({}):", files.len());
        println!();
        println!("  {:<22} {:<8} {:<40} {}", "Name", "Version", "Description", "Triggers");
        println!("  {}{}{}{}", "-".repeat(22), "-".repeat(8), "-".repeat(40), "-".repeat(15));

        for (name, path) in &files {
            // Try to load metadata
            if let Ok(wf) = nemesis_workflow::parser::parse_file(path) {
                let desc = if wf.description.len() > 37 {
                    format!("{}...", &wf.description[..34])
                } else if wf.description.is_empty() {
                    "-".to_string()
                } else {
                    wf.description.clone()
                };
                let triggers: Vec<&str> = wf.triggers.iter().map(|t| t.trigger_type.as_str()).collect();
                let trigger_str = if triggers.is_empty() { "none".to_string() } else { triggers.join(", ") };
                println!("  {:<22} {:<8} {:<40} {}", name, wf.version, desc, trigger_str);
            } else {
                println!("  {:<22} {:<8} {:<40} {}", name, "?", "(parse error)", "none");
            }
        }
    }

    println!();
    println!("  Total executions: {}", total_execs);
    Ok(())
}

async fn cmd_run(workflow_dir: &std::path::Path, name: &str, input_args: &[String]) -> Result<()> {
    println!("Running workflow: {}", name);

    // Find the workflow file
    let wf_path = if std::path::Path::new(name).exists() {
        std::path::PathBuf::from(name)
    } else {
        // Search in workflow dir
        let candidates = [
            workflow_dir.join(format!("{}.yaml", name)),
            workflow_dir.join(format!("{}.yml", name)),
            workflow_dir.join(format!("{}.json", name)),
        ];
        let mut found = candidates.into_iter().find(|p| p.exists());

        // If not found in workspace, try exe_dir/templates/ as fallback
        if found.is_none() {
            if let Ok(exe_path) = std::env::current_exe() {
                if let Some(exe_dir) = exe_path.parent() {
                    let template_dir = exe_dir.join("templates");
                    for ext in &["yaml", "yml", "json"] {
                        let p = template_dir.join(format!("{}.{}", name, ext));
                        if p.exists() {
                            found = Some(p);
                            break;
                        }
                    }
                }
            }
        }

        found.ok_or_else(|| anyhow::anyhow!("Workflow '{}' not found", name))?
    };

    // Parse the workflow
    let workflow = nemesis_workflow::parser::parse_file(&wf_path)
        .map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;
    println!("  Loaded: {} (v{}, {} nodes)", workflow.name, workflow.version, workflow.nodes.len());

    // Validate
    if let Err(e) = nemesis_workflow::parser::validate(&workflow) {
        println!("  Validation error: {}", e);
        return Err(anyhow::anyhow!("Workflow validation failed"));
    }
    println!("  Validation: OK");

    // Parse input from positional args
    let input_map = parse_positional_input(input_args);
    if !input_map.is_empty() {
        println!("  Input: {:?}", input_map);
    }

    // Create engine and run
    let engine = nemesis_workflow::engine::WorkflowEngine::new();
    engine.register_workflow(workflow)
        .map_err(|e| anyhow::anyhow!("Registration error: {}", e))?;

    println!("  Executing...");
    let result = engine.run(name, input_map).await
        .map_err(|e| anyhow::anyhow!("Execution error: {}", e))?;

    // Display result with timestamps and duration
    println!();
    println!("  Execution ID:    {}", result.id);
    println!("  State:           {}", result.state);
    println!("  Started:         {}", format_datetime(&result.started_at));
    if let Some(ended) = result.ended_at {
        println!("  Ended:           {}", format_datetime(&ended));
        let duration = ended.signed_duration_since(result.started_at);
        let millis = duration.num_milliseconds();
        if millis < 1000 {
            println!("  Duration:        {}ms", millis);
        } else {
            println!("  Duration:        {:.3}s", millis as f64 / 1000.0);
        }
    }
    if let Some(ref error) = result.error {
        println!("  Error:           {}", error);
    }

    // Show node results
    if !result.node_results.is_empty() {
        println!();
        println!("  Node Results:");
        for (node_id, nr) in &result.node_results {
            let error_str = nr.error.as_deref().unwrap_or("");
            if error_str.is_empty() {
                println!("    [{}] {}", node_id, nr.state);
            } else {
                println!("    [{}] {} - {}", node_id, nr.state, error_str);
            }
        }
    }

    // Save execution record
    let exec_dir = workflow_dir.join("executions");
    let _ = std::fs::create_dir_all(&exec_dir);
    let exec_path = exec_dir.join(format!("{}.json", result.id));
    std::fs::write(&exec_path, serde_json::to_string_pretty(&result).unwrap_or_default())?;
    println!();
    println!("  Execution saved: {}", exec_path.display());

    Ok(())
}

fn cmd_status(workflow_dir: &std::path::Path, id: Option<&str>) -> Result<()> {
    let exec_dir = workflow_dir.join("executions");

    match id {
        Some(exec_id) => {
            let exec_path = exec_dir.join(format!("{}.json", exec_id));
            if !exec_path.exists() {
                println!("Execution '{}' not found.", exec_id);
                return Ok(());
            }
            let data = std::fs::read_to_string(&exec_path)?;
            let exec: serde_json::Value = serde_json::from_str(&data)?;

            // Formatted detail view
            println!("Workflow Execution Detail");
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
            println!("  Execution ID:    {}", exec.get("id").and_then(|v| v.as_str()).unwrap_or("?"));
            println!("  Workflow:        {}", exec.get("workflow_name").and_then(|v| v.as_str()).unwrap_or("?"));
            println!("  State:           {}", exec.get("state").and_then(|v| v.as_str()).unwrap_or("?"));

            if let Some(started) = exec.get("started_at").and_then(|v| v.as_str()) {
                println!("  Started:         {}", started);
            }
            if let Some(ended) = exec.get("ended_at").and_then(|v| v.as_str()) {
                println!("  Ended:           {}", ended);
                // Try to calculate duration
                if let Some(started) = exec.get("started_at").and_then(|v| v.as_str()) {
                    let start_ok = chrono::DateTime::parse_from_rfc3339(started)
                        .map(|dt| dt.with_timezone(&chrono::Utc));
                    let end_ok = chrono::DateTime::parse_from_rfc3339(ended)
                        .map(|dt| dt.with_timezone(&chrono::Utc));
                    if let (Ok(start_dt), Ok(end_dt)) = (start_ok, end_ok) {
                        let duration = end_dt.signed_duration_since(start_dt);
                        let millis = duration.num_milliseconds();
                        if millis < 1000 {
                            println!("  Duration:        {}ms", millis);
                        } else {
                            println!("  Duration:        {:.3}s", millis as f64 / 1000.0);
                        }
                    }
                }
            }
            if let Some(error) = exec.get("error").and_then(|v| v.as_str()) {
                if !error.is_empty() {
                    println!("  Error:           {}", error);
                }
            }

            // Show input
            if let Some(input) = exec.get("input") {
                if let Some(obj) = input.as_object() {
                    if !obj.is_empty() {
                        println!();
                        println!("  Input:");
                        for (k, v) in obj {
                            println!("    {}: {}", k, v);
                        }
                    }
                }
            }

            // Show variables
            if let Some(vars) = exec.get("variables") {
                if let Some(obj) = vars.as_object() {
                    if !obj.is_empty() {
                        println!();
                        println!("  Variables:");
                        for (k, v) in obj {
                            println!("    {}: {}", k, v);
                        }
                    }
                }
            }

            // Show node results
            if let Some(node_results) = exec.get("node_results").and_then(|v| v.as_object()) {
                if !node_results.is_empty() {
                    println!();
                    println!("  Node Results:");
                    for (node_id, nr) in node_results {
                        let state = nr.get("state").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("    [{}] {}", node_id, state);
                        if let Some(started) = nr.get("started_at").and_then(|v| v.as_str()) {
                            if let Some(ended) = nr.get("ended_at").and_then(|v| v.as_str()) {
                                println!("      Started: {}  Ended: {}", started, ended);
                            }
                        }
                        if let Some(error) = nr.get("error").and_then(|v| v.as_str()) {
                            if !error.is_empty() {
                                println!("      Error: {}", error);
                            }
                        }
                        if let Some(output) = nr.get("output") {
                            let output_str = output.to_string();
                            if output_str != "null" && !output_str.is_empty() {
                                let truncated = if output_str.len() > 200 {
                                    format!("{}...", &output_str[..197])
                                } else {
                                    output_str
                                };
                                println!("      Output: {}", truncated);
                            }
                        }
                    }
                }
            }
            println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        }
        None => {
            println!("Workflow Executions");
            println!("===================");

            if !exec_dir.exists() {
                println!("  No executions found.");
                return Ok(());
            }

            let mut entries: Vec<_> = std::fs::read_dir(&exec_dir)?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type().map(|t| t.is_file()).unwrap_or(false)
                        && e.file_name().to_string_lossy().ends_with(".json")
                })
                .collect();

            entries.sort_by_key(|e| std::fs::metadata(e.path()).ok().and_then(|m| m.modified().ok()).unwrap_or(std::time::SystemTime::UNIX_EPOCH));
            entries.reverse();

            if entries.is_empty() {
                println!("  No executions found.");
            } else {
                println!("  {:<38} {:<20} {:<12} {}", "ID", "Workflow", "State", "Started");
                println!("  {}{}{}{}", "-".repeat(38), "-".repeat(20), "-".repeat(12), "-".repeat(20));
                for entry in entries.iter().take(20) {
                    let data = std::fs::read_to_string(entry.path())?;
                    if let Ok(exec) = serde_json::from_str::<serde_json::Value>(&data) {
                        let id = exec.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                        let wf = exec.get("workflow_name").and_then(|v| v.as_str()).unwrap_or("?");
                        let state = exec.get("state").and_then(|v| v.as_str()).unwrap_or("?");
                        let started = exec.get("started_at").and_then(|v| v.as_str()).unwrap_or("?");
                        // Trim the timestamp for display
                        let started_short = if started.len() > 19 { &started[..19] } else { started };
                        println!("  {:<38} {:<20} {:<12} {}", id, wf, state, started_short);
                    }
                }
                println!();
                println!("  Total: {} execution(s)", entries.len());
            }
        }
    }
    Ok(())
}

fn cmd_template_show(name: &str) -> Result<()> {
    let templates = get_templates();
    let found = templates.iter().find(|(n, _, _)| *n == name);

    match found {
        Some((name, desc, definition)) => {
            println!("Template: {}", name);
            println!("Description: {}", desc);
            // Show node count and trigger count
            if let Some(nodes) = definition.get("nodes").and_then(|v| v.as_array()) {
                println!("Nodes: {}", nodes.len());
            }
            if let Some(triggers) = definition.get("triggers").and_then(|v| v.as_array()) {
                println!("Triggers: {}", triggers.len());
            }
            println!();
            println!("{}", serde_json::to_string_pretty(definition).unwrap_or_default());
        }
        None => {
            println!("Template '{}' not found.", name);
            println!("Available templates:");
            for (n, desc, _) in &templates {
                println!("  {} - {}", n, desc);
            }
        }
    }
    Ok(())
}

fn cmd_template_create(workflow_dir: &std::path::Path, template: &str, output: Option<&str>) -> Result<()> {
    let templates = get_templates();
    let found = templates.iter().find(|(n, _, _)| *n == template);

    match found {
        Some((_, _, definition)) => {
            let out = output.unwrap_or(template);
            let out_path = if out.ends_with(".yaml") || out.ends_with(".yml") || out.ends_with(".json") {
                workflow_dir.join(out)
            } else {
                workflow_dir.join(format!("{}.yaml", out))
            };

            let _ = std::fs::create_dir_all(workflow_dir);

            if out_path.extension().map(|e| e == "json").unwrap_or(false) {
                std::fs::write(&out_path, serde_json::to_string_pretty(definition).unwrap_or_default())?;
            } else {
                // Write as YAML
                let yaml = serde_yaml::to_string(definition).unwrap_or_default();
                std::fs::write(&out_path, yaml)?;
            }

            println!("Workflow created from template '{}' -> {}", template, out_path.display());
            println!("Edit the file to customize, then run: nemesisbot workflow run {}", out);
        }
        None => {
            println!("Template '{}' not found.", template);
            println!("Available templates:");
            for (n, desc, _) in &templates {
                println!("  {} - {}", n, desc);
            }
        }
    }
    Ok(())
}

fn cmd_validate(path: &str) -> Result<()> {
    println!("Validating workflow: {}", path);
    let wf_path = std::path::Path::new(path);
    if !wf_path.exists() {
        println!("  Error: File not found.");
        return Ok(());
    }

    // Parse
    match nemesis_workflow::parser::parse_file(wf_path) {
        Ok(wf) => {
            println!("  Valid format: yes");
            println!("  Name: {}", wf.name);
            println!("  Version: {}", wf.version);
            println!("  Nodes: {}", wf.nodes.len());
            println!("  Edges: {}", wf.edges.len());
            println!("  Triggers: {}", wf.triggers.len());

            // Validate
            match nemesis_workflow::parser::validate(&wf) {
                Ok(()) => {
                    println!("  Validation: PASSED");
                }
                Err(e) => {
                    println!("  Validation: FAILED");
                    println!("    {}", e);
                }
            }
        }
        Err(e) => {
            println!("  Error: Parse failed - {}", e);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub fn run(action: WorkflowAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let workflow_dir = common::workspace_path(&home).join("workflow");

    match action {
        WorkflowAction::List => cmd_list(&workflow_dir)?,
        WorkflowAction::Run { name, input } => {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_run(&workflow_dir, &name, &input))
            })?;
            result
        }
        WorkflowAction::Status { id } => cmd_status(&workflow_dir, id.as_deref())?,
        WorkflowAction::Template { action } => {
            match action {
                None => {
                    // Default: list templates (Go behavior)
                    println!("Workflow Templates");
                    println!("==================");
                    let templates = get_templates();
                    for (name, desc, def) in &templates {
                        let nodes = def.get("nodes").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                        let triggers = def.get("triggers").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                        println!("  {} - {} ({} nodes, {} triggers)", name, desc, nodes, triggers);
                    }
                    println!();
                    println!("Show details: nemesisbot workflow template show <name>");
                    println!("Create: nemesisbot workflow template create <name>");
                }
                Some(TemplateAction::List) => {
                    println!("Workflow Templates");
                    println!("==================");
                    let templates = get_templates();
                    for (name, desc, def) in &templates {
                        let nodes = def.get("nodes").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                        let triggers = def.get("triggers").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                        println!("  {} - {} ({} nodes, {} triggers)", name, desc, nodes, triggers);
                    }
                    println!();
                    println!("Show details: nemesisbot workflow template show <name>");
                    println!("Create: nemesisbot workflow template create <name>");
                }
                Some(TemplateAction::Show { name }) => cmd_template_show(&name)?,
                Some(TemplateAction::Create { template, output }) => {
                    cmd_template_create(&workflow_dir, &template, output.as_deref())?
                }
            }
        }
        WorkflowAction::Validate { path } => cmd_validate(&path)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_positional_input_key_value() {
        let args = vec!["name=hello".to_string(), "count=42".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["name"], serde_json::Value::String("hello".to_string()));
        assert_eq!(map["count"], serde_json::Value::Number(42i64.into()));
    }

    #[test]
    fn test_parse_positional_input_boolean() {
        let args = vec!["enabled=true".to_string(), "disabled=false".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["enabled"], serde_json::Value::Bool(true));
        assert_eq!(map["disabled"], serde_json::Value::Bool(false));
    }

    #[test]
    fn test_parse_positional_input_float() {
        let args = vec!["rate=3.14".to_string()];
        let map = parse_positional_input(&args);
        // Float should be a number
        assert!(map["rate"].is_number());
    }

    #[test]
    fn test_parse_positional_input_string_no_equals() {
        let args = vec!["hello world".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["input"], serde_json::Value::String("hello world".to_string()));
    }

    #[test]
    fn test_parse_positional_input_no_equals_only_first() {
        let args = vec!["first".to_string(), "second".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map.len(), 1); // Only first gets "input" key
        assert_eq!(map["input"], serde_json::Value::String("first".to_string()));
    }

    #[test]
    fn test_parse_positional_input_mixed() {
        let args = vec!["some input".to_string(), "key=value".to_string(), "num=10".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map.len(), 3);
        assert_eq!(map["input"], "some input");
        assert_eq!(map["key"], "value");
        assert_eq!(map["num"], 10);
    }

    #[test]
    fn test_parse_positional_input_empty() {
        let args: Vec<String> = vec![];
        let map = parse_positional_input(&args);
        assert!(map.is_empty());
    }

    #[test]
    fn test_scan_workflow_files_nonexistent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("nonexistent");
        let files = scan_workflow_files(&dir);
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_workflow_files_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        let files = scan_workflow_files(&dir);
        assert!(files.is_empty());
    }

    #[test]
    fn test_scan_workflow_files_finds_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("test.yaml"), "name: test").unwrap();
        std::fs::write(dir.join("test2.yml"), "name: test2").unwrap();
        std::fs::write(dir.join("data.txt"), "not a workflow").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 2);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"test"));
        assert!(names.contains(&"test2"));
    }

    #[test]
    fn test_scan_workflow_files_finds_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workflow.json"), r#"{"name": "test"}"#).unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "workflow");
    }

    #[test]
    fn test_scan_workflow_files_skips_executions_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        let exec_dir = dir.join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(dir.join("real.yaml"), "name: real").unwrap();
        std::fs::write(exec_dir.join("exec1.json"), "{}").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "real");
    }

    #[test]
    fn test_scan_workflow_files_sorted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("z_last.yaml"), "name: z").unwrap();
        std::fs::write(dir.join("a_first.yaml"), "name: a").unwrap();
        std::fs::write(dir.join("m_middle.yaml"), "name: m").unwrap();

        let files = scan_workflow_files(&dir);
        assert_eq!(files[0].0, "a_first");
        assert_eq!(files[1].0, "m_middle");
        assert_eq!(files[2].0, "z_last");
    }

    #[test]
    fn test_count_executions_no_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(count_executions(tmp.path()), 0);
    }

    #[test]
    fn test_count_executions_with_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let exec_dir = tmp.path().join("executions");
        std::fs::create_dir_all(&exec_dir).unwrap();
        std::fs::write(exec_dir.join("exec1.json"), "{}").unwrap();
        std::fs::write(exec_dir.join("exec2.json"), "{}").unwrap();
        std::fs::write(exec_dir.join("not_json.txt"), "text").unwrap();

        assert_eq!(count_executions(tmp.path()), 2);
    }

    #[test]
    fn test_format_datetime() {
        use chrono::TimeZone;
        let dt = chrono::Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 45).unwrap();
        let formatted = format_datetime(&dt);
        assert_eq!(formatted, "2026-01-15 10:30:45");
    }

    #[test]
    fn test_get_default_templates_count() {
        let templates = get_default_templates();
        assert_eq!(templates.len(), 5); // researcher, coder, monitor, collector, translator
    }

    #[test]
    fn test_get_default_templates_names() {
        let templates = get_default_templates();
        let names: Vec<&str> = templates.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"researcher"));
        assert!(names.contains(&"coder"));
        assert!(names.contains(&"monitor"));
        assert!(names.contains(&"collector"));
        assert!(names.contains(&"translator"));
    }

    #[test]
    fn test_get_default_templates_have_nodes() {
        let templates = get_default_templates();
        for (name, _, def) in &templates {
            let nodes = def.get("nodes").and_then(|v| v.as_array());
            assert!(nodes.is_some(), "Template '{}' should have nodes", name);
            assert!(!nodes.unwrap().is_empty(), "Template '{}' should have non-empty nodes", name);
        }
    }

    #[test]
    fn test_get_default_templates_have_edges() {
        let templates = get_default_templates();
        for (name, _, def) in &templates {
            let edges = def.get("edges").and_then(|v| v.as_array());
            assert!(edges.is_some(), "Template '{}' should have edges", name);
            assert!(!edges.unwrap().is_empty(), "Template '{}' should have non-empty edges", name);
        }
    }

    #[test]
    fn test_cmd_list_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflow");
        std::fs::create_dir_all(&dir).unwrap();
        cmd_list(&dir).unwrap();
    }

    #[test]
    fn test_cmd_status_no_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        cmd_status(tmp.path(), None).unwrap();
    }

    #[test]
    fn test_cmd_status_specific_id_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        cmd_status(tmp.path(), Some("nonexistent-id")).unwrap();
    }

    #[test]
    fn test_cmd_template_show_not_found() {
        cmd_template_show("nonexistent_template").unwrap();
    }

    #[test]
    fn test_cmd_template_show_found() {
        cmd_template_show("researcher").unwrap();
    }

    #[test]
    fn test_cmd_validate_nonexistent() {
        cmd_validate("/nonexistent/file.yaml").unwrap();
    }

    #[test]
    fn test_parse_positional_input_whitespace() {
        let args = vec!["  key  =  value  ".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["key"], "value");
    }

    #[test]
    fn test_parse_positional_input_negative_number() {
        let args = vec!["offset=-5".to_string()];
        let map = parse_positional_input(&args);
        assert_eq!(map["offset"], -5);
    }
}
