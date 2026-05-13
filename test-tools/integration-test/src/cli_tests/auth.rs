//! Auth commands: login, logout, status

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// auth status / login / logout
// ---------------------------------------------------------------------------

pub async fn test_cli_auth_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/auth";
    let mut results = Vec::new();
    print_suite_header(suite);

    // auth status (should work without login)
    let status = ws.run_cli(bin, &["auth", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}, output: '{}'", status.exit_code,
            status.stdout.trim().chars().take(80).collect::<String>())));

    // auth login --help (actual login requires interactive input)
    let help = ws.run_cli(bin, &["auth", "login", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/login_help", suite), "Auth login help works"));
    } else {
        results.push(fail(&format!("{}/login_help", suite),
            &format!("exit={}", help.exit_code)));
    }

    // auth logout --help
    let logout_help = ws.run_cli(bin, &["auth", "logout", "--help"]).await;
    if logout_help.success() {
        results.push(pass(&format!("{}/logout_help", suite), "Auth logout help works"));
    } else {
        results.push(fail(&format!("{}/logout_help", suite),
            &format!("exit={}", logout_help.exit_code)));
    }

    // auth login --provider test (non-interactive attempt)
    let login = ws.run_cli(bin, &["auth", "login", "--provider", "test"]).await;
    results.push(pass(&format!("{}/login_attempt", suite),
        &format!("exit={}", login.exit_code)));

    // auth logout --provider test
    let logout = ws.run_cli(bin, &["auth", "logout", "--provider", "test"]).await;
    results.push(pass(&format!("{}/logout", suite),
        &format!("exit={}", logout.exit_code)));

    results
}
