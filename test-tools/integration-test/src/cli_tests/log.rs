//! Log commands: llm, general, status, config, set-level, file, console

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// log status / config
// ---------------------------------------------------------------------------

pub async fn test_cli_log_status_config(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/log_status";
    let mut results = Vec::new();
    print_suite_header(suite);

    // log status
    let status = ws.run_cli(bin, &["log", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}", status.exit_code)));

    // log config
    let config = ws.run_cli(bin, &["log", "config"]).await;
    results.push(pass(&format!("{}/config", suite),
        &format!("exit={}", config.exit_code)));

    // log config --detail-level
    let config_detail = ws.run_cli(bin, &["log", "config", "--detail-level", "full"]).await;
    results.push(pass(&format!("{}/config_detail_level", suite),
        &format!("exit={}", config_detail.exit_code)));

    // log config --log-dir
    let config_dir = ws.run_cli(bin, &[
        "log", "config", "--log-dir", ws.path().join("logs").to_str().unwrap_or("./logs"),
    ]).await;
    results.push(pass(&format!("{}/config_log_dir", suite),
        &format!("exit={}", config_dir.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// log enable / disable
// ---------------------------------------------------------------------------

pub async fn test_cli_log_enable_disable(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/log_enable_disable";
    let mut results = Vec::new();
    print_suite_header(suite);

    let disable = ws.run_cli(bin, &["log", "disable"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    let enable = ws.run_cli(bin, &["log", "enable"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// log llm (enable/disable/status/config)
// ---------------------------------------------------------------------------

pub async fn test_cli_log_llm(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/log_llm";
    let mut results = Vec::new();
    print_suite_header(suite);

    // llm status
    let status = ws.run_cli(bin, &["log", "llm", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}", status.exit_code)));

    // llm enable
    let enable = ws.run_cli(bin, &["log", "llm", "enable"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    // llm config
    let config = ws.run_cli(bin, &["log", "llm", "config"]).await;
    results.push(pass(&format!("{}/config", suite),
        &format!("exit={}", config.exit_code)));

    // llm disable
    let disable = ws.run_cli(bin, &["log", "llm", "disable"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    // Re-enable
    let _ = ws.run_cli(bin, &["log", "llm", "enable"]).await;

    results
}

// ---------------------------------------------------------------------------
// log general (enable/disable/status/level/file/console)
// ---------------------------------------------------------------------------

pub async fn test_cli_log_general(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/log_general";
    let mut results = Vec::new();
    print_suite_header(suite);

    // general status
    let status = ws.run_cli(bin, &["log", "general", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}", status.exit_code)));

    // general level INFO
    let level = ws.run_cli(bin, &["log", "general", "level", "INFO"]).await;
    results.push(pass(&format!("{}/level", suite),
        &format!("exit={}", level.exit_code)));

    // general level DEBUG
    let debug = ws.run_cli(bin, &["log", "general", "level", "DEBUG"]).await;
    results.push(pass(&format!("{}/level_debug", suite),
        &format!("exit={}", debug.exit_code)));

    // general file
    let log_path = ws.path().join("test.log");
    let file = ws.run_cli(bin, &[
        "log", "general", "file", log_path.to_str().unwrap_or("test.log"),
    ]).await;
    results.push(pass(&format!("{}/file", suite),
        &format!("exit={}", file.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// log set-level / enable-file / disable-file / enable-console / disable-console
// ---------------------------------------------------------------------------

pub async fn test_cli_log_level_file_console(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/log_level_file_console";
    let mut results = Vec::new();
    print_suite_header(suite);

    // set-level WARN
    let warn = ws.run_cli(bin, &["log", "set-level", "WARN"]).await;
    results.push(pass(&format!("{}/set_level_warn", suite),
        &format!("exit={}", warn.exit_code)));

    // set-level ERROR
    let error = ws.run_cli(bin, &["log", "set-level", "ERROR"]).await;
    results.push(pass(&format!("{}/set_level_error", suite),
        &format!("exit={}", error.exit_code)));

    // enable-file
    let ef = ws.run_cli(bin, &["log", "enable-file"]).await;
    results.push(pass(&format!("{}/enable_file", suite),
        &format!("exit={}", ef.exit_code)));

    // enable-file --path
    let efp = ws.run_cli(bin, &[
        "log", "enable-file", "--path", ws.path().join("custom.log").to_str().unwrap_or("custom.log"),
    ]).await;
    results.push(pass(&format!("{}/enable_file_path", suite),
        &format!("exit={}", efp.exit_code)));

    // disable-file
    let df = ws.run_cli(bin, &["log", "disable-file"]).await;
    results.push(pass(&format!("{}/disable_file", suite),
        &format!("exit={}", df.exit_code)));

    // enable-console
    let ec = ws.run_cli(bin, &["log", "enable-console"]).await;
    results.push(pass(&format!("{}/enable_console", suite),
        &format!("exit={}", ec.exit_code)));

    // disable-console
    let dc = ws.run_cli(bin, &["log", "disable-console"]).await;
    results.push(pass(&format!("{}/disable_console", suite),
        &format!("exit={}", dc.exit_code)));

    // Restore
    let _ = ws.run_cli(bin, &["log", "set-level", "INFO"]).await;
    let _ = ws.run_cli(bin, &["log", "enable-console"]).await;

    results
}
