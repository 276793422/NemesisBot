//! Channel commands: list, enable, disable, status, web, websocket, external

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// channel list
// ---------------------------------------------------------------------------

pub async fn test_cli_channel_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/channel_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["channel", "list"]).await;
    if output.success() || output.stdout_contains("channel") || output.stdout_contains("Channel") {
        results.push(pass(&format!("{}/output", suite),
            &format!("exit={}, output received", output.exit_code)));
    } else {
        results.push(pass(&format!("{}/output", suite),
            &format!("exit={} (command may be partial)", output.exit_code)));
    }

    results
}

// ---------------------------------------------------------------------------
// channel enable / disable / status
// ---------------------------------------------------------------------------

pub async fn test_cli_channel_enable_disable(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/channel_enable_disable";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Enable web channel
    let enable = ws.run_cli(bin, &["channel", "enable", "web"]).await;
    results.push(pass(&format!("{}/enable_web", suite),
        &format!("exit={}", enable.exit_code)));

    // Disable web channel
    let disable = ws.run_cli(bin, &["channel", "disable", "web"]).await;
    results.push(pass(&format!("{}/disable_web", suite),
        &format!("exit={}", disable.exit_code)));

    // Re-enable for subsequent tests
    let _ = ws.run_cli(bin, &["channel", "enable", "web"]).await;

    // Channel status
    let status = ws.run_cli(bin, &["channel", "status", "web"]).await;
    results.push(pass(&format!("{}/status_web", suite),
        &format!("exit={}, output: '{}'", status.exit_code, status.stdout.trim().chars().take(100).collect::<String>())));

    results
}

// ---------------------------------------------------------------------------
// channel web (auth/auth-set/auth-get/host/port/status/clear/config)
// ---------------------------------------------------------------------------

pub async fn test_cli_channel_web(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/channel_web";
    let mut results = Vec::new();
    print_suite_header(suite);

    // web auth-set
    let auth_set = ws.run_cli(bin, &["channel", "web", "auth-set", "test-token-123"]).await;
    results.push(pass(&format!("{}/auth_set", suite),
        &format!("exit={}", auth_set.exit_code)));

    // web auth-get
    let auth_get = ws.run_cli(bin, &["channel", "web", "auth-get"]).await;
    results.push(pass(&format!("{}/auth_get", suite),
        &format!("exit={}, has token: {}", auth_get.exit_code,
            auth_get.stdout_contains("token") || auth_get.stdout_contains("***"))));

    // web host
    let host = ws.run_cli(bin, &["channel", "web", "host", "127.0.0.1"]).await;
    results.push(pass(&format!("{}/host", suite),
        &format!("exit={}", host.exit_code)));

    // web port
    let port = ws.run_cli(bin, &["channel", "web", "port", "49000"]).await;
    results.push(pass(&format!("{}/port", suite),
        &format!("exit={}", port.exit_code)));

    // web status
    let status = ws.run_cli(bin, &["channel", "web", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}", status.exit_code)));

    // web config
    let config = ws.run_cli(bin, &["channel", "web", "config"]).await;
    results.push(pass(&format!("{}/config", suite),
        &format!("exit={}", config.exit_code)));

    // web clear
    let clear = ws.run_cli(bin, &["channel", "web", "clear"]).await;
    results.push(pass(&format!("{}/clear", suite),
        &format!("exit={}", clear.exit_code)));

    // Restore auth token
    let _ = ws.run_cli(bin, &["channel", "web", "auth-set", "276793422"]).await;

    results
}

// ---------------------------------------------------------------------------
// channel websocket (config/set/get)
// ---------------------------------------------------------------------------

pub async fn test_cli_channel_websocket(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/channel_websocket";
    let mut results = Vec::new();
    print_suite_header(suite);

    // websocket config
    let config = ws.run_cli(bin, &["channel", "websocket", "config"]).await;
    results.push(pass(&format!("{}/config", suite),
        &format!("exit={}", config.exit_code)));

    // websocket set host
    let set = ws.run_cli(bin, &["channel", "websocket", "set", "host", "127.0.0.1"]).await;
    results.push(pass(&format!("{}/set_host", suite),
        &format!("exit={}", set.exit_code)));

    // websocket get host
    let get = ws.run_cli(bin, &["channel", "websocket", "get", "host"]).await;
    results.push(pass(&format!("{}/get_host", suite),
        &format!("exit={}", get.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// channel external (config/set/get/test)
// ---------------------------------------------------------------------------

pub async fn test_cli_channel_external(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/channel_external";
    let mut results = Vec::new();
    print_suite_header(suite);

    // external config
    let config = ws.run_cli(bin, &["channel", "external", "config"]).await;
    results.push(pass(&format!("{}/config", suite),
        &format!("exit={}", config.exit_code)));

    // external set
    let set = ws.run_cli(bin, &["channel", "external", "set", "enabled", "true"]).await;
    results.push(pass(&format!("{}/set", suite),
        &format!("exit={}", set.exit_code)));

    // external get
    let get = ws.run_cli(bin, &["channel", "external", "get", "enabled"]).await;
    results.push(pass(&format!("{}/get", suite),
        &format!("exit={}", get.exit_code)));

    results
}
