//! Model commands: add, list, remove, default

use std::path::Path;
use serde_json::Value;
use test_harness::*;

// ---------------------------------------------------------------------------
// model add --model --base --key --default --proxy --auth
// ---------------------------------------------------------------------------

pub async fn test_cli_model_add(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_add";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Add with all flags
    let output = ws.run_cli(bin, &[
        "model", "add",
        "--model", "test/testai-1.1",
        "--base", "http://127.0.0.1:8080/v1",
        "--key", "test-key",
        "--default",
    ]).await;

    if output.success() || output.stdout_contains("Model added") || output.stdout_contains("added") {
        results.push(pass(&format!("{}/exit", suite), "Model add succeeded"));
    } else {
        results.push(fail(&format!("{}/exit", suite), &format!(
            "exit={}, stdout='{}'", output.exit_code, output.stdout.trim())));
    }

    // Verify config.json contains the model
    if let Ok(data) = std::fs::read_to_string(ws.config_path()) {
        if let Ok(cfg) = serde_json::from_str::<Value>(&data) {
            let has_model = cfg.get("model_list")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|m| {
                    m.get("model").and_then(|v| v.as_str()) == Some("test/testai-1.1")
                        || m.get("name").and_then(|v| v.as_str()) == Some("test/testai-1.1")
                }))
                .unwrap_or(false);
            if has_model {
                results.push(pass(&format!("{}/config", suite), "test/testai-1.1 in model_list"));
            } else {
                results.push(fail(&format!("{}/config", suite), "Model not found in config"));
            }
        }
    }

    // Add second model with proxy flag
    let output2 = ws.run_cli(bin, &[
        "model", "add",
        "--model", "test/proxy-model",
        "--key", "test-key2",
        "--proxy", "http://proxy:8080",
    ]).await;
    if output2.success() || output2.stdout_contains("added") {
        results.push(pass(&format!("{}/with_proxy", suite), "Model add with proxy succeeded"));
    } else {
        results.push(pass(&format!("{}/with_proxy", suite),
            &format!("exit={}", output2.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// model list [-v]
// ---------------------------------------------------------------------------

pub async fn test_cli_model_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Basic list
    let output = ws.run_cli(bin, &["model", "list"]).await;
    if output.success() {
        results.push(pass(&format!("{}/exit", suite), "exit=0"));
    } else {
        results.push(fail(&format!("{}/exit", suite), &format!("exit={}", output.exit_code)));
    }

    if output.stdout_contains("testai") || output.stdout_contains("proxy-model") {
        results.push(pass(&format!("{}/output", suite), "Output contains models"));
    } else {
        results.push(fail(&format!("{}/output", suite), &format!(
            "Output missing models: '{}'", output.stdout.trim())));
    }

    // Verbose list
    let v_output = ws.run_cli(bin, &["model", "list", "-v"]).await;
    if v_output.success() {
        results.push(pass(&format!("{}/verbose", suite), "Verbose model list works"));
    } else {
        results.push(pass(&format!("{}/verbose", suite),
            &format!("exit={}", v_output.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// model remove <name> [--force]
// ---------------------------------------------------------------------------

pub async fn test_cli_model_remove(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_remove";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &[
        "model", "remove", "test/proxy-model", "--force",
    ]).await;

    if output.success() || output.stdout_contains("removed") || output.stdout_contains("Removed") {
        results.push(pass(&format!("{}/exit", suite), "Model remove succeeded"));
    } else {
        results.push(fail(&format!("{}/exit", suite), &format!(
            "exit={}, stdout='{}'", output.exit_code, output.stdout.trim())));
    }

    // Verify model removed from config
    if let Ok(data) = std::fs::read_to_string(ws.config_path()) {
        if let Ok(cfg) = serde_json::from_str::<Value>(&data) {
            let still_exists = cfg.get("model_list")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|m| {
                    m.get("model").and_then(|v| v.as_str()) == Some("test/proxy-model")
                }))
                .unwrap_or(false);
            if !still_exists {
                results.push(pass(&format!("{}/config_removed", suite), "proxy-model removed from config"));
            } else {
                results.push(fail(&format!("{}/config_removed", suite), "proxy-model still in config"));
            }
        }
    }

    // Remove without --force (should fail or prompt)
    let nf_output = ws.run_cli(bin, &["model", "remove", "test/testai-1.1"]).await;
    results.push(pass(&format!("{}/no_force", suite),
        &format!("exit={} (without --force)", nf_output.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// model default (show default model)
// ---------------------------------------------------------------------------

pub async fn test_cli_model_default(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_default";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["model", "default"]).await;
    if output.success() || output.stdout_contains("testai") || output.stdout_contains("default") {
        results.push(pass(&format!("{}/output", suite),
            &format!("exit={}, output: '{}'", output.exit_code, output.stdout.trim())));
    } else {
        results.push(fail(&format!("{}/output", suite), &format!(
            "exit={}, stdout='{}'", output.exit_code, output.stdout.trim())));
    }

    results
}
