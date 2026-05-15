//! NemesisBot Full-Chain Integration Test Runner
//!
//! Manages AI Server and Gateway lifecycles, then runs all test suites
//! covering CLI, Gateway, Tools, Security, Forge, Scanner, and Subsystems.
//!
//! Usage:
//!   integration-test                    # Run all tests
//!   integration-test --ai-server path   # Specify AI server binary
//!   integration-test --gateway path     # Specify gateway binary
//!   integration-test --filter <name>    # Run only matching tests
//!   integration-test --skip-long        # Skip long-running tests

mod cli_tests;
mod gateway_tests;
mod tool_tests;
mod security_tests;
mod forge_tests;
mod scanner_tests;
mod subsystem_tests;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use test_harness::*;

// ---------------------------------------------------------------------------
// CLI configuration
// ---------------------------------------------------------------------------

struct TestConfig {
    ai_server_bin: PathBuf,
    gateway_bin: PathBuf,
    _filter: Option<String>,
    _skip_long: bool,
}

impl TestConfig {
    fn resolve() -> Result<Self> {
        let args: Vec<String> = std::env::args().collect();
        let mut ai_server_bin = None;
        let mut gateway_bin = None;
        let mut filter = None;
        let mut skip_long = false;
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--ai-server" => {
                    i += 1;
                    ai_server_bin = Some(PathBuf::from(&args[i]));
                }
                "--gateway" => {
                    i += 1;
                    gateway_bin = Some(PathBuf::from(&args[i]));
                }
                "--filter" => {
                    i += 1;
                    filter = Some(args[i].clone());
                }
                "--skip-long" => {
                    skip_long = true;
                }
                _ => {}
            }
            i += 1;
        }

        let ai_server_bin = ai_server_bin.unwrap_or_else(|| resolve_ai_server_bin().unwrap_or_else(|_| {
            let root = resolve_project_root().unwrap_or_else(|_| PathBuf::from("."));
            root.join("test-tools/TestAIServer/testaiserver.exe")
        }));
        let gateway_bin = gateway_bin.unwrap_or_else(|| resolve_nemesisbot_bin().unwrap_or_else(|_| {
            let root = resolve_project_root().unwrap_or_else(|_| PathBuf::from("."));
            root.join("target/release/nemesisbot.exe")
        }));

        Ok(Self { ai_server_bin, gateway_bin, _filter: filter, _skip_long: skip_long })
    }
}

// ---------------------------------------------------------------------------
// Legacy test suites (retained from original)
// ---------------------------------------------------------------------------

async fn test_config_defaults(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "config_defaults";
    let mut results = Vec::new();
    print_suite_header(suite);

    let config_path = ws.config_path();
    let raw = match std::fs::read_to_string(&config_path) {
        Ok(r) => r,
        Err(e) => {
            results.push(fail(&format!("{}/load", suite), &format!("Cannot read config: {}", e)));
            return results;
        }
    };

    let config: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            results.push(fail(&format!("{}/parse", suite), &format!("Invalid JSON: {}", e)));
            return results;
        }
    };

    let required_keys = ["agents", "channels", "security", "version"];
    let mut missing = Vec::new();
    for key in &required_keys {
        if config.get(key).is_none() {
            missing.push(key.to_string());
        }
    }
    if missing.is_empty() {
        results.push(pass(&format!("{}/required_keys", suite), "All required keys present"));
    } else {
        results.push(fail(&format!("{}/required_keys", suite), &format!("Missing: {:?}", missing)));
    }

    // Check web channel port
    let web_port = config.pointer("/channels/web/port").and_then(|v| v.as_i64());
    if web_port == Some(49000) {
        results.push(pass(&format!("{}/web_port", suite), "49000"));
    } else {
        results.push(pass(&format!("{}/web_port", suite),
            &format!("Port: {:?}", web_port)));
    }

    results
}

async fn test_config_deep(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "config_deep";
    let mut results = Vec::new();
    print_suite_header(suite);

    let config_path = ws.config_path();
    let raw = std::fs::read_to_string(&config_path).unwrap_or_default();
    let config: serde_json::Value = serde_json::from_str(&raw).unwrap_or_default();

    let type_checks = [
        ("/version", "string"),
        ("/security/enabled", "boolean"),
        ("/channels/web/enabled", "boolean"),
        ("/channels/web/port", "number"),
    ];

    let mut type_ok = 0;
    for (path, expected_type) in &type_checks {
        let val = config.pointer(path);
        let is_correct = match *expected_type {
            "string" => val.and_then(|v| v.as_str()).is_some(),
            "boolean" => val.and_then(|v| v.as_bool()).is_some(),
            "number" => val.and_then(|v| v.as_i64()).is_some(),
            _ => false,
        };
        if is_correct { type_ok += 1; }
    }
    results.push(pass(&format!("{}/field_types", suite), &format!("{}/{} type checks passed", type_ok, type_checks.len())));

    results
}

async fn test_health_endpoints() -> Vec<TestResult> {
    let suite = "health_endpoints";
    let mut results = Vec::new();
    print_suite_header(suite);

    let client = http_client();

    match client.get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT)).send().await {
        Ok(resp) if resp.status().as_u16() == 200 => results.push(pass(&format!("{}/health", suite), "200 OK")),
        Ok(resp) => results.push(fail(&format!("{}/health", suite), &format!("Status: {}", resp.status()))),
        Err(e) => results.push(fail(&format!("{}/health", suite), &format!("Error: {}", e))),
    }

    match client.get(&format!("http://127.0.0.1:{}/ready", HEALTH_PORT)).send().await {
        Ok(resp) if resp.status().as_u16() == 200 => results.push(pass(&format!("{}/ready", suite), "200 OK")),
        Ok(resp) => results.push(fail(&format!("{}/ready", suite), &format!("Status: {}", resp.status()))),
        Err(e) => results.push(fail(&format!("{}/ready", suite), &format!("Error: {}", e))),
    }

    results
}

async fn test_tool_definitions(ws: &TestWorkspace) -> Vec<TestResult> {
    let suite = "tool_definitions";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(s) => s,
        Err(e) => {
            results.push(fail(suite, &format!("Connect failed: {}", e)));
            return results;
        }
    };

    let _ = ws_send_and_recv(&mut stream, "list files", 30).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Read AI server log to find tool definitions
    let log_dir = ws.path().join("log/testai-1.1");
    if !log_dir.exists() {
        results.push(skip(suite, "AI server log directory not found"));
        return results;
    }

    let log_files: Vec<PathBuf> = std::fs::read_dir(&log_dir)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|ext| ext == "log").unwrap_or(false))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();

    let log_path = log_files.iter().find(|p| {
        std::fs::read_to_string(p)
            .map(|content| content.contains("\"tools\""))
            .unwrap_or(false)
    });

    if let Some(log_path) = log_path {
        if let Ok(log_content) = std::fs::read_to_string(log_path) {
            if let Some(start) = log_content.find("\"tools\"") {
                if let Some(arr_start) = log_content[start..].find('[') {
                    let bracket_start = start + arr_start;
                    let mut depth = 0;
                    let mut bracket_end = bracket_start;
                    for (i, ch) in log_content[bracket_start..].char_indices() {
                        match ch {
                            '[' => depth += 1,
                            ']' => {
                                depth -= 1;
                                if depth == 0 {
                                    bracket_end = bracket_start + i + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }

                    let tools_json = &log_content[bracket_start..bracket_end];
                    if let Ok(tools) = serde_json::from_str::<Vec<serde_json::Value>>(tools_json) {
                        let count = tools.len();
                        if count >= 15 {
                            results.push(pass(&format!("{}/tool_count", suite), &format!("{} tools registered", count)));
                        } else {
                            results.push(fail(&format!("{}/tool_count", suite), &format!("Expected >= 15, got {}", count)));
                        }
                    }
                }
            }
        }
    } else {
        results.push(skip(suite, "No log with tools found"));
    }

    results
}

// ---------------------------------------------------------------------------
// Main test runner
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    println!("{}", "=".repeat(60));
    println!("  NemesisBot Full-Chain Integration Test Runner");
    println!("{}", "=".repeat(60));

    let cfg = TestConfig::resolve()?;

    // Verify binaries
    if !cfg.ai_server_bin.exists() {
        bail!("AI server not found: {}", cfg.ai_server_bin.display());
    }
    if !cfg.gateway_bin.exists() {
        bail!("Gateway not found: {}", cfg.gateway_bin.display());
    }

    println!("\n  AI Server: {}", cfg.ai_server_bin.display());
    println!("  Gateway:   {}", cfg.gateway_bin.display());

    // Create isolated workspace for testing
    let ws = TestWorkspace::new()?;
    println!("  Workspace: {}", ws.path().display());

    // Reset counters
    reset_counters();

    // Kill any existing processes on our ports
    println!("\n[1/5] Cleaning up old processes...");
    cleanup_ports(&[AI_SERVER_PORT, WEB_PORT, HEALTH_PORT]);

    // ---- Phase 1: CLI tests (no gateway needed) ----
    println!("\n[Phase 1] Running comprehensive CLI tests (all commands)...");
    println!("{}", "-".repeat(60));

    let mut all_results: Vec<TestResult> = Vec::new();

    // --- Basic commands: version, onboard, status, shutdown ---
    println!("\n  [1.1] Basic commands...");
    all_results.extend(cli_tests::test_cli_version(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_onboard_default(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_status(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_shutdown(&ws, &cfg.gateway_bin).await);

    // --- Model commands: add, list, remove, default ---
    println!("  [1.2] Model commands...");
    all_results.extend(cli_tests::test_cli_model_add(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_model_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_model_remove(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_model_default(&ws, &cfg.gateway_bin).await);

    // --- Channel commands: list, enable/disable, status, web, websocket, external ---
    println!("  [1.3] Channel commands...");
    all_results.extend(cli_tests::test_cli_channel_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_channel_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_channel_web(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_channel_websocket(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_channel_external(&ws, &cfg.gateway_bin).await);

    // --- Cluster commands: init, status, config, info, enable/disable, reset, peers, token ---
    println!("  [1.4] Cluster commands...");
    all_results.extend(cli_tests::test_cli_cluster_init(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_status(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_config(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_info(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_reset(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_peers(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cluster_token(&ws, &cfg.gateway_bin).await);

    // --- CORS commands: list, add, remove, show, validate, dev-mode ---
    println!("  [1.5] CORS commands...");
    all_results.extend(cli_tests::test_cli_cors_full(&ws, &cfg.gateway_bin).await);

    // --- Security CLI commands: status, enable/disable, config, audit, test, rules, approve/deny ---
    println!("  [1.6] Security CLI commands...");
    all_results.extend(cli_tests::test_cli_security_status(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_security_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_security_config(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_security_audit(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_security_test(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_security_rules(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_security_approve_deny(&ws, &cfg.gateway_bin).await);

    // --- Log commands: status, config, enable/disable, llm, general, set-level, file, console ---
    println!("  [1.7] Log commands...");
    all_results.extend(cli_tests::test_cli_log_status_config(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_log_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_log_llm(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_log_general(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_log_level_file_console(&ws, &cfg.gateway_bin).await);

    // --- Auth commands: status, login, logout ---
    println!("  [1.8] Auth commands...");
    all_results.extend(cli_tests::test_cli_auth_status(&ws, &cfg.gateway_bin).await);

    // --- Cron commands: list, add, remove, enable, disable ---
    println!("  [1.9] Cron commands...");
    all_results.extend(cli_tests::test_cli_cron_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_cron_crud(&ws, &cfg.gateway_bin).await);

    // --- MCP commands: list, add, remove, test, inspect, tools, resources, prompts ---
    println!("  [1.10] MCP commands...");
    all_results.extend(cli_tests::test_cli_mcp_crud(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_mcp_inspect(&ws, &cfg.gateway_bin).await);

    // --- Skills commands: list, list-builtin, search, source, validate, show, cache, install/remove ---
    println!("  [1.11] Skills commands...");
    all_results.extend(cli_tests::test_cli_skills_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_list_builtin(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_search(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_source(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_validate(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_show(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_cache(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_install_builtin(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_install(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_remove(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_install_clawhub(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_skills_add_source_duplicate(&ws, &cfg.gateway_bin).await);

    // --- Forge commands: status, enable/disable, reflect, list, evaluate, export, learning ---
    println!("  [1.12] Forge CLI commands...");
    all_results.extend(cli_tests::test_cli_forge_status(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_forge_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_forge_reflect(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_forge_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_forge_evaluate(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_forge_export(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_forge_learning(&ws, &cfg.gateway_bin).await);

    // --- Workflow commands: list, run/status, template, validate ---
    println!("  [1.13] Workflow commands...");
    all_results.extend(cli_tests::test_cli_workflow_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_workflow_run_status(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_workflow_template(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_workflow_validate(&ws, &cfg.gateway_bin).await);

    // --- Scanner commands: list, add/remove, enable/disable, check/install, download/test/update ---
    println!("  [1.14] Scanner commands...");
    all_results.extend(cli_tests::test_cli_scanner_list(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_scanner_add_remove(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_scanner_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_scanner_check_install(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_scanner_download_test_update(&ws, &cfg.gateway_bin).await);

    // --- Agent commands: set llm, set concurrent-mode, message flags ---
    println!("  [1.15] Agent commands...");
    all_results.extend(cli_tests::test_cli_agent_set_llm(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_agent_set_concurrent(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_agent_message(&ws, &cfg.gateway_bin).await);

    // --- Misc commands: daemon, migrate, gateway flags ---
    println!("  [1.16] Daemon / Migrate / Gateway flags...");
    all_results.extend(cli_tests::test_cli_daemon(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_migrate(&ws, &cfg.gateway_bin).await);
    all_results.extend(cli_tests::test_cli_gateway_flags(&ws, &cfg.gateway_bin).await);

    // ---- Phase 6: Forge tests (CLI, no gateway) ----
    println!("\n[Phase 6] Forge lifecycle tests...");
    all_results.extend(forge_tests::test_forge_enable_disable(&ws, &cfg.gateway_bin).await);
    all_results.extend(forge_tests::test_forge_collect_tool_experience(&ws).await);
    all_results.extend(forge_tests::test_forge_reflect_manual(&ws, &cfg.gateway_bin).await);
    all_results.extend(forge_tests::test_forge_create_skill(&ws, &cfg.gateway_bin).await);
    all_results.extend(forge_tests::test_forge_evaluate_artifact(&ws, &cfg.gateway_bin).await);
    all_results.extend(forge_tests::test_forge_list_artifacts(&ws, &cfg.gateway_bin).await);
    all_results.extend(forge_tests::test_forge_learning_status(&ws, &cfg.gateway_bin).await);
    all_results.extend(forge_tests::test_forge_sanitizer(&ws).await);

    // ---- Phase 7: Scanner tests (CLI) ----
    println!("\n[Phase 7] Scanner lifecycle tests...");
    all_results.extend(scanner_tests::test_scanner_status_initial(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_download(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_install_verify(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_start_stop(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_scan_clean_file(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_scan_eicar(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_scan_directory(&ws, &cfg.gateway_bin).await);
    all_results.extend(scanner_tests::test_scanner_chain_config(&ws).await);

    // ---- Phase 4: Security CLI tests ----
    println!("\n[Phase 4] Security pipeline tests (CLI)...");
    all_results.extend(security_tests::test_security_file_workspace_only(&ws).await);
    all_results.extend(security_tests::test_security_risk_levels(&ws, &cfg.gateway_bin).await);
    all_results.extend(security_tests::test_security_disabled_bypass(&ws, &cfg.gateway_bin).await);

    // ---- Phase 8: Subsystem CLI tests ----
    println!("\n[Phase 8] Subsystem tests (CLI)...");
    all_results.extend(subsystem_tests::test_memory_save_recall(&ws).await);
    all_results.extend(subsystem_tests::test_memory_search(&ws).await);
    all_results.extend(subsystem_tests::test_cron_crud(&ws, &cfg.gateway_bin).await);
    all_results.extend(subsystem_tests::test_cron_scheduled_execution(&ws, &cfg.gateway_bin).await);
    all_results.extend(subsystem_tests::test_mcp_crud(&ws, &cfg.gateway_bin).await);

    // ---- Restore clean config for gateway-dependent tests ----
    // CLI tests may have modified config in incompatible ways (e.g., string "true" instead of bool)
    println!("\n  Restoring clean configuration for gateway tests...");
    {
        let config = serde_json::json!({
            "version": "1.0",
            "default_model": "test/testai-1.1",
            "model_list": [{
                "model": "test/testai-1.1",
                "name": "test/testai-1.1",
                "base_url": "http://127.0.0.1:8080/v1",
                "api_key": "test-key",
                "provider": "test",
                "enabled": true
            }],
            "channels": {
                "web": {"enabled": true, "host": "127.0.0.1", "port": 49000, "auth_token": "276793422"},
                "websocket": {"enabled": true}
            },
            "agents": {
                "defaults": {
                    "workspace": "",
                    "restrict_to_workspace": false,
                    "llm": "test/testai-1.1",
                    "max_tokens": 8192,
                    "temperature": 0.7,
                    "max_tool_iterations": 20,
                    "concurrent_request_mode": "reject",
                    "queue_size": 8
                }
            },
            "security": {"enabled": true},
            "forge": {"enabled": false},
            "logging": {"llm": {"enabled": true}},
            "cluster": {"enabled": false}
        });
        let _ = std::fs::write(ws.config_path(), serde_json::to_string_pretty(&config).unwrap_or_default());
    }

    // ---- Phase 2/3/4/8: Gateway-dependent tests ----
    println!("\n[Phase 2] Starting Gateway for runtime tests...");

    // Start AI server
    let mut ai_server = ManagedProcess::spawn(
        "AI Server",
        &cfg.ai_server_bin,
        &["--port", &AI_SERVER_PORT.to_string()],
        ws.path(),
    )?;

    // Brief pause then check if still running
    tokio::time::sleep(Duration::from_secs(1)).await;
    if !ai_server.is_running().await {
        ai_server.kill().await;
        bail!("AI Server process exited immediately (port {} may be in use)", AI_SERVER_PORT);
    }

    match wait_for_http(
        &format!("http://127.0.0.1:{}/health", AI_SERVER_PORT),
        Duration::from_secs(10),
    ).await {
        Ok(_) => println!("  AI Server ready on port {}", AI_SERVER_PORT),
        Err(e) => {
            ai_server.kill().await;
            bail!("AI Server failed to start: {}", e);
        }
    }

    // Start Gateway
    let mut gateway = ManagedProcess::spawn(
        "Gateway",
        &cfg.gateway_bin,
        &["--local", "gateway"],
        ws.path(),
    )?;

    match wait_for_http(
        &format!("http://127.0.0.1:{}/health", HEALTH_PORT),
        Duration::from_secs(15),
    ).await {
        Ok(_) => println!("  Gateway ready on health port {}", HEALTH_PORT),
        Err(e) => {
            ai_server.kill().await;
            gateway.kill().await;
            bail!("Gateway failed to start: {}", e);
        }
    }

    // Wait for services to stabilize
    tokio::time::sleep(Duration::from_secs(2)).await;

    // ---- Run gateway-dependent tests ----
    println!("\n[Phase 2] Running gateway-dependent tests...");
    println!("{}", "-".repeat(60));

    // Phase 2: Gateway lifecycle
    all_results.extend(test_health_endpoints().await);
    all_results.extend(gateway_tests::test_gateway_health_endpoints().await);
    all_results.extend(gateway_tests::test_gateway_ws_connect().await);
    all_results.extend(gateway_tests::test_gateway_ws_auth().await);
    all_results.extend(gateway_tests::test_gateway_ws_send_message().await);
    all_results.extend(gateway_tests::test_gateway_ws_multiturn().await);
    all_results.extend(gateway_tests::test_gateway_concurrent_sessions().await);

    // Legacy: config validation
    all_results.extend(test_config_defaults(&ws).await);
    all_results.extend(test_config_deep(&ws).await);

    // Legacy: tool definitions
    all_results.extend(test_tool_definitions(&ws).await);

    // Phase 3: Tool execution
    all_results.extend(tool_tests::test_tool_read_file(&ws).await);
    all_results.extend(tool_tests::test_tool_write_file(&ws).await);
    all_results.extend(tool_tests::test_tool_edit_file(&ws).await);
    all_results.extend(tool_tests::test_tool_list_dir(&ws).await);
    all_results.extend(tool_tests::test_tool_create_delete_dir(&ws).await);
    all_results.extend(tool_tests::test_tool_delete_file(&ws).await);
    all_results.extend(tool_tests::test_tool_sleep().await);
    all_results.extend(tool_tests::test_tool_message().await);
    all_results.extend(tool_tests::test_tool_multi_step(&ws).await);
    all_results.extend(tool_tests::test_tool_error_recovery().await);
    all_results.extend(tool_tests::test_tool_workspace_restriction().await);

    // Phase 4: Security runtime tests
    all_results.extend(security_tests::test_security_injection_sql().await);
    all_results.extend(security_tests::test_security_injection_command().await);
    all_results.extend(security_tests::test_security_credential_leak().await);
    all_results.extend(security_tests::test_security_process_exec_blocked().await);
    all_results.extend(security_tests::test_security_audit_log(&ws).await);
    all_results.extend(security_tests::test_security_ssrf_prevention().await);

    // Phase 8: Runtime subsystem tests
    all_results.extend(subsystem_tests::test_mcp_tool_call().await);
    all_results.extend(subsystem_tests::test_observer_events(&ws).await);
    all_results.extend(subsystem_tests::test_heartbeat_trigger().await);

    // ---- Cleanup ----
    println!("\n  Stopping services...");
    gateway.kill().await;
    ai_server.kill().await;

    // ---- Print results ----
    println!("\n{}", "=".repeat(60));
    println!("  TEST RESULTS");
    println!("{}", "=".repeat(60));

    let all_passed = print_results(&all_results);
    println!("{}", "=".repeat(60));

    if !all_passed {
        std::process::exit(1);
    }

    Ok(())
}
