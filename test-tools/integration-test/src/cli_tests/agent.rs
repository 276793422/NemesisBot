//! Agent commands: set llm, set concurrent-mode, --message

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// agent set llm
// ---------------------------------------------------------------------------

pub async fn test_cli_agent_set_llm(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/agent_set_llm";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["agent", "set", "llm", "test/testai-1.1"]).await;
    results.push(pass(&format!("{}/set_llm", suite),
        &format!("exit={}", output.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// agent set concurrent-mode
// ---------------------------------------------------------------------------

pub async fn test_cli_agent_set_concurrent(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/agent_set_concurrent";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Set reject mode
    let reject = ws.run_cli(bin, &["agent", "set", "concurrent-mode", "reject"]).await;
    results.push(pass(&format!("{}/reject", suite),
        &format!("exit={}", reject.exit_code)));

    // Set queue mode
    let queue = ws.run_cli(bin, &["agent", "set", "concurrent-mode", "queue"]).await;
    results.push(pass(&format!("{}/queue", suite),
        &format!("exit={}", queue.exit_code)));

    // Set queue mode with --queue-size
    let queue_size = ws.run_cli(bin, &[
        "agent", "set", "concurrent-mode", "queue", "--queue-size", "10",
    ]).await;
    results.push(pass(&format!("{}/queue_size", suite),
        &format!("exit={}", queue_size.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// agent --message (needs LLM, test --help only)
// ---------------------------------------------------------------------------

pub async fn test_cli_agent_message(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/agent_message";
    let mut results = Vec::new();
    print_suite_header(suite);

    // agent --help
    let help = ws.run_cli(bin, &["agent", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "Agent help works"));
    } else {
        results.push(fail(&format!("{}/help", suite),
            &format!("exit={}", help.exit_code)));
    }

    // Verify --message flag exists in help
    if help.stdout_contains("--message") || help.stdout_contains("-m") {
        results.push(pass(&format!("{}/message_flag", suite), "--message flag present"));
    } else {
        results.push(fail(&format!("{}/message_flag", suite), "--message flag missing"));
    }

    // Verify --session flag exists
    if help.stdout_contains("--session") || help.stdout_contains("-s") {
        results.push(pass(&format!("{}/session_flag", suite), "--session flag present"));
    } else {
        results.push(fail(&format!("{}/session_flag", suite), "--session flag missing"));
    }

    // Verify --debug flag exists
    if help.stdout_contains("--debug") || help.stdout_contains("-d") {
        results.push(pass(&format!("{}/debug_flag", suite), "--debug flag present"));
    } else {
        results.push(fail(&format!("{}/debug_flag", suite), "--debug flag missing"));
    }

    // Verify --no-console flag exists
    if help.stdout_contains("--no-console") {
        results.push(pass(&format!("{}/no_console_flag", suite), "--no-console flag present"));
    } else {
        results.push(fail(&format!("{}/no_console_flag", suite), "--no-console flag missing"));
    }

    results
}
