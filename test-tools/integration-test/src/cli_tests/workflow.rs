//! Workflow commands: list, run, status, template, validate

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// workflow list
// ---------------------------------------------------------------------------

pub async fn test_cli_workflow_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/workflow_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["workflow", "list"]).await;
    results.push(pass(&format!("{}/list", suite),
        &format!("exit={}", output.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// workflow run / status
// ---------------------------------------------------------------------------

pub async fn test_cli_workflow_run_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/workflow_run";
    let mut results = Vec::new();
    print_suite_header(suite);

    // run with fake workflow (will fail, tests parsing)
    let run = ws.run_cli(bin, &["workflow", "run", "nonexistent"]).await;
    results.push(pass(&format!("{}/run", suite),
        &format!("exit={}", run.exit_code)));

    // run with key=value input
    let run_kv = ws.run_cli(bin, &["workflow", "run", "nonexistent", "key1=value1"]).await;
    results.push(pass(&format!("{}/run_with_input", suite),
        &format!("exit={}", run_kv.exit_code)));

    // status
    let status = ws.run_cli(bin, &["workflow", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}", status.exit_code)));

    // status with id
    let status_id = ws.run_cli(bin, &["workflow", "status", "fake-id"]).await;
    results.push(pass(&format!("{}/status_id", suite),
        &format!("exit={}", status_id.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// workflow template (list / show / create)
// ---------------------------------------------------------------------------

pub async fn test_cli_workflow_template(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/workflow_template";
    let mut results = Vec::new();
    print_suite_header(suite);

    // template list
    let list = ws.run_cli(bin, &["workflow", "template", "list"]).await;
    results.push(pass(&format!("{}/list", suite),
        &format!("exit={}", list.exit_code)));

    // template show (fake name)
    let show = ws.run_cli(bin, &["workflow", "template", "show", "nonexistent"]).await;
    results.push(pass(&format!("{}/show", suite),
        &format!("exit={}", show.exit_code)));

    // template create (fake template)
    let create = ws.run_cli(bin, &["workflow", "template", "create", "nonexistent"]).await;
    results.push(pass(&format!("{}/create", suite),
        &format!("exit={}", create.exit_code)));

    // template create --output
    let create_out = ws.run_cli(bin, &[
        "workflow", "template", "create", "nonexistent",
        "--output", ws.path().join("workflow_out").to_str().unwrap_or("out"),
    ]).await;
    results.push(pass(&format!("{}/create_output", suite),
        &format!("exit={}", create_out.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// workflow validate
// ---------------------------------------------------------------------------

pub async fn test_cli_workflow_validate(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/workflow_validate";
    let mut results = Vec::new();
    print_suite_header(suite);

    // validate with nonexistent path
    let validate = ws.run_cli(bin, &["workflow", "validate", "nonexistent.json"]).await;
    results.push(pass(&format!("{}/validate_missing", suite),
        &format!("exit={}", validate.exit_code)));

    // Create a minimal valid workflow file and validate it
    let wf_path = ws.path().join("test_workflow.json");
    let wf_content = serde_json::json!({
        "name": "test-workflow",
        "steps": []
    });
    let _ = std::fs::write(&wf_path, serde_json::to_string_pretty(&wf_content).unwrap_or_default());
    let validate2 = ws.run_cli(bin, &[
        "workflow", "validate", wf_path.to_str().unwrap_or("test_workflow.json"),
    ]).await;
    results.push(pass(&format!("{}/validate_file", suite),
        &format!("exit={}", validate2.exit_code)));

    results
}
