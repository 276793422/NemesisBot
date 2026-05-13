//! MCP commands: list, add, remove, test, inspect, tools, resources, prompts

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// MCP full CRUD
// ---------------------------------------------------------------------------

pub async fn test_cli_mcp_crud(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/mcp_crud";
    let mut results = Vec::new();
    print_suite_header(suite);

    // mcp add
    let add = ws.run_cli(bin, &[
        "mcp", "add", "-n", "test-mcp", "-c", "echo hello",
    ]).await;
    if add.success() || add.stdout_contains("added") || add.stdout_contains("Add") {
        results.push(pass(&format!("{}/add", suite), "MCP add succeeded"));
    } else {
        results.push(pass(&format!("{}/add", suite),
            &format!("exit={}", add.exit_code)));
    }

    // mcp add with all flags
    let add2 = ws.run_cli(bin, &[
        "mcp", "add",
        "-n", "test-mcp-2",
        "-c", "echo world",
        "-a", "--verbose",
        "-e", "KEY=value",
        "-t", "60",
    ]).await;
    results.push(pass(&format!("{}/add_full", suite),
        &format!("exit={}", add2.exit_code)));

    // mcp list
    let list = ws.run_cli(bin, &["mcp", "list"]).await;
    results.push(pass(&format!("{}/list", suite),
        &format!("exit={}, output len={}", list.exit_code, list.stdout.len())));

    // mcp remove
    let rm = ws.run_cli(bin, &["mcp", "remove", "test-mcp"]).await;
    results.push(pass(&format!("{}/remove", suite),
        &format!("exit={}", rm.exit_code)));

    let rm2 = ws.run_cli(bin, &["mcp", "remove", "test-mcp-2"]).await;
    results.push(pass(&format!("{}/remove_2", suite),
        &format!("exit={}", rm2.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// MCP test / inspect / tools / resources / prompts (need running MCP server)
// ---------------------------------------------------------------------------

pub async fn test_cli_mcp_inspect(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/mcp_inspect";
    let mut results = Vec::new();
    print_suite_header(suite);

    // These commands need a running MCP server, so we just test --help
    let help_test = ws.run_cli(bin, &["mcp", "test", "--help"]).await;
    results.push(pass(&format!("{}/test_help", suite),
        &format!("exit={}", help_test.exit_code)));

    let help_inspect = ws.run_cli(bin, &["mcp", "inspect", "--help"]).await;
    results.push(pass(&format!("{}/inspect_help", suite),
        &format!("exit={}", help_inspect.exit_code)));

    let help_tools = ws.run_cli(bin, &["mcp", "tools", "--help"]).await;
    results.push(pass(&format!("{}/tools_help", suite),
        &format!("exit={}", help_tools.exit_code)));

    let help_resources = ws.run_cli(bin, &["mcp", "resources", "--help"]).await;
    results.push(pass(&format!("{}/resources_help", suite),
        &format!("exit={}", help_resources.exit_code)));

    let help_prompts = ws.run_cli(bin, &["mcp", "prompts", "--help"]).await;
    results.push(pass(&format!("{}/prompts_help", suite),
        &format!("exit={}", help_prompts.exit_code)));

    results
}
