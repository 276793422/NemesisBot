//! Gateway command - start the NemesisBot gateway server.
//!
//! Mirrors Go CmdGateway:
//! 1. Check config file exists
//! 2. Check home directory exists
//! 3. Load configuration
//! 4. Initialize logger from config
//! 5. Write PID file
//! 6. Create MessageBus
//! 7. Create LLM Provider via factory
//! 8. Create AgentLoop with bus integration
//! 9. Create WebServer with bus
//! 10. Create HealthServer
//! 11. Create HeartbeatService
//! 12. Start all services
//! 13. Print gateway banner
//! 14. Wait for shutdown signal
//! 15. Graceful shutdown

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use nemesis_services::LifecycleService;
use tracing::{info, warn, error};

use crate::adapters;
use crate::common;

// ---------------------------------------------------------------------------
// Global shutdown state
// ---------------------------------------------------------------------------

/// Global shutdown flag (replaces Go's globalShutdownChan).
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request global shutdown from any component.
#[cfg(not(target_os = "android"))]
pub fn trigger_global_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Check if global shutdown has been requested.
#[allow(dead_code)]
pub fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// Plugin window management via ProcessManager
// ---------------------------------------------------------------------------

/// Check if plugin-ui.dll exists in the `plugins/` directory next to the executable.
fn plugin_ui_dll_exists() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return false,
    };
    let exe_dir = match exe.parent() {
        Some(d) => d,
        None => return false,
    };
    exe_dir.join("plugins").join("plugin_ui.dll").exists()
        || exe_dir.join("plugins").join("plugin-ui.dll").exists()
}

/// Adapter connecting ProcessManager to the security auditor's ApprovalManager trait.
///
/// When a tool call triggers an "ask" security rule, the auditor calls
/// `request_approval_sync()` which spawns an approval popup child process
/// via ProcessManager and blocks until the user responds.
struct ApprovalPopupAdapter {
    process_manager: Arc<nemesis_desktop::process::ProcessManager>,
}

impl ApprovalPopupAdapter {
    fn new(pm: Arc<nemesis_desktop::process::ProcessManager>) -> Self {
        Self { process_manager: pm }
    }
}

impl nemesis_security::auditor::ApprovalManager for ApprovalPopupAdapter {
    fn is_running(&self) -> bool {
        true
    }

    fn request_approval_sync(
        &self,
        request_id: &str,
        operation: &str,
        target: &str,
        risk_level: &str,
        reason: &str,
        timeout_secs: u64,
    ) -> Result<bool, String> {
        // Check if plugin-ui.dll exists. If not, reject immediately —
        // we cannot show an approval popup without the UI DLL, and
        // allowing the operation without user confirmation is unsafe.
        if !plugin_ui_dll_exists() {
            warn!(
                "[Gateway] Approval rejected: plugin-ui.dll not found (operation={}, target={}, risk={}). \
                 Cannot show approval popup — denying by default.",
                operation, target, risk_level
            );
            return Ok(false);
        }

        let data = serde_json::json!({
            "request_id": request_id,
            "operation": operation,
            "operation_name": operation,
            "target": target,
            "risk_level": risk_level,
            "reason": reason,
            "timeout_seconds": timeout_secs.max(30),
            "context": {},
            "timestamp": chrono::Utc::now().timestamp(),
        });

        info!(
            "[Gateway] Requesting approval popup: operation={}, target={}, risk={}",
            operation, target, risk_level
        );

        let (_child_id, result_rx) = self.process_manager.spawn_child("approval", &data)
            .map_err(|e| format!("spawn_child failed: {}", e))?;

        let result_rx = result_rx.ok_or("no result channel")?;

        // The oneshot receiver is async but we're in a sync context.
        // Use a dedicated thread with its own tokio runtime to wait for the result.
        let (tx, rx) = std::sync::mpsc::channel::<Result<serde_json::Value, String>>();
        let wait_secs = timeout_secs + 10;
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(wait_secs),
                    result_rx,
                ).await
            });
            match result {
                Ok(Ok(value)) => { let _ = tx.send(Ok(value)); }
                Ok(Err(_)) => { let _ = tx.send(Err("channel closed".to_string())); }
                Err(_) => { let _ = tx.send(Err("timeout".to_string())); }
            }
        });

        // Block until the user responds or timeout
        match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs + 15)) {
            Ok(Ok(value)) => {
                let action = value.get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("rejected");
                info!("[Gateway] Approval result: action={} for request_id={}", action, request_id);
                Ok(action == "approved")
            }
            Ok(Err(e)) => {
                warn!("[Gateway] Approval channel error: {}", e);
                Ok(false)
            }
            Err(_) => {
                warn!("[Gateway] Approval timeout after {}s", timeout_secs);
                Ok(false) // timeout = rejected
            }
        }
    }
}

/// Bridge adapter connecting Cluster to Forge's ClusterForgeBridge trait.
///
/// Enables Forge to share reflections with and receive reflections from
/// cluster peers. Mirrors Go's `forge.NewClusterForgeBridge(cluster)`.
struct ClusterForgeBridgeAdapter {
    node_id: String,
}

impl ClusterForgeBridgeAdapter {
    fn new(node_id: String) -> Self {
        Self { node_id }
    }
}

#[async_trait::async_trait]
impl nemesis_forge::bridge::ClusterForgeBridge for ClusterForgeBridgeAdapter {
    async fn share_reflection(
        &self,
        report_json: serde_json::Value,
    ) -> Result<usize, String> {
        // TODO: When cluster has a share_reflection method, call it here.
        // For now, store locally only (matches Go's early implementation).
        let _ = report_json;
        Ok(0)
    }

    async fn get_remote_reflections(&self) -> Result<Vec<serde_json::Value>, String> {
        // TODO: When cluster has get_reflection_reports, call it here.
        Ok(Vec::new())
    }

    async fn get_online_peers(&self) -> Result<Vec<String>, String> {
        // TODO: When cluster has get_online_peers with node IDs, call it here.
        Ok(Vec::new())
    }

    fn local_node_id(&self) -> &str {
        &self.node_id
    }

    fn is_cluster_enabled(&self) -> bool {
        true
    }
}

/// Load security rules from `config.security.json` and apply to the SecurityPlugin.
///
/// Parses the JSON config file's `file_rules`, `dir_rules`, `process_rules`, etc.
/// and registers them as ABAC rules on the auditor. Also sets `default_action`.
fn load_security_rules(
    plugin: &Arc<nemesis_security::pipeline::SecurityPlugin>,
    config_path: &std::path::Path,
) {
    use nemesis_security::types::{OperationType, SecurityRule};

    if !config_path.exists() {
        info!("[Gateway] Security config file not found: {}, using defaults", config_path.display());
        return;
    }

    let data = match std::fs::read_to_string(config_path) {
        Ok(d) => d,
        Err(e) => {
            warn!("[Gateway] Failed to read security config: {}", e);
            return;
        }
    };

    let config: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            warn!("[Gateway] Failed to parse security config JSON: {}", e);
            return;
        }
    };

    // Set default_action
    if let Some(action) = config.get("default_action").and_then(|v| v.as_str()) {
        plugin.auditor().set_default_action(action);
        info!("[Gateway] Security default_action: {}", action);
    }

    // Helper: parse rules from JSON array of {pattern, action}
    fn parse_rules(value: &serde_json::Value) -> Vec<SecurityRule> {
        value.as_array()
            .map(|arr| {
                arr.iter().filter_map(|item| {
                    Some(SecurityRule {
                        pattern: item.get("pattern")?.as_str()?.to_string(),
                        action: item.get("action")?.as_str()?.to_string(),
                        comment: item.get("comment").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    })
                }).collect()
            })
            .unwrap_or_default()
    }

    // File rules
    if let Some(file_rules) = config.get("file_rules") {
        let read_rules = parse_rules(file_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let write_rules = parse_rules(file_rules.get("write").unwrap_or(&serde_json::Value::Null));
        let delete_rules = parse_rules(file_rules.get("delete").unwrap_or(&serde_json::Value::Null));
        let append_rules = parse_rules(file_rules.get("append").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::FileRead, read_rules);
        plugin.set_rules(OperationType::FileWrite, write_rules.clone());
        plugin.set_rules(OperationType::FileDelete, delete_rules);
        if !append_rules.is_empty() {
            // append uses FileWrite rules as well
            let mut combined = write_rules;
            combined.extend(append_rules);
            plugin.set_rules(OperationType::FileWrite, combined);
        }
        info!("[Gateway] Security file_rules loaded");
    }

    // Dir rules
    if let Some(dir_rules) = config.get("dir_rules") {
        let read_rules = parse_rules(dir_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let create_rules = parse_rules(dir_rules.get("create").unwrap_or(&serde_json::Value::Null));
        let delete_rules = parse_rules(dir_rules.get("delete").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::DirRead, read_rules);
        plugin.set_rules(OperationType::DirCreate, create_rules);
        plugin.set_rules(OperationType::DirDelete, delete_rules);
        info!("[Gateway] Security dir_rules loaded");
    }

    // Process rules
    if let Some(proc_rules) = config.get("process_rules") {
        let exec_rules = parse_rules(proc_rules.get("exec").unwrap_or(&serde_json::Value::Null));
        let spawn_rules = parse_rules(proc_rules.get("spawn").unwrap_or(&serde_json::Value::Null));
        let kill_rules = parse_rules(proc_rules.get("kill").unwrap_or(&serde_json::Value::Null));
        let suspend_rules = parse_rules(proc_rules.get("suspend").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::ProcessExec, exec_rules);
        plugin.set_rules(OperationType::ProcessSpawn, spawn_rules);
        plugin.set_rules(OperationType::ProcessKill, kill_rules);
        plugin.set_rules(OperationType::ProcessSuspend, suspend_rules);
        info!("[Gateway] Security process_rules loaded");
    }

    // Network rules
    if let Some(net_rules) = config.get("network_rules") {
        let request_rules = parse_rules(net_rules.get("request").unwrap_or(&serde_json::Value::Null));
        let download_rules = parse_rules(net_rules.get("download").unwrap_or(&serde_json::Value::Null));
        let upload_rules = parse_rules(net_rules.get("upload").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::NetworkRequest, request_rules);
        plugin.set_rules(OperationType::NetworkDownload, download_rules);
        plugin.set_rules(OperationType::NetworkUpload, upload_rules);
        info!("[Gateway] Security network_rules loaded");
    }

    // Hardware rules
    if let Some(hw_rules) = config.get("hardware_rules") {
        let i2c_rules = parse_rules(hw_rules.get("i2c").unwrap_or(&serde_json::Value::Null));
        let spi_rules = parse_rules(hw_rules.get("spi").unwrap_or(&serde_json::Value::Null));
        let gpio_rules = parse_rules(hw_rules.get("gpio").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::HardwareI2C, i2c_rules);
        plugin.set_rules(OperationType::HardwareSPI, spi_rules);
        plugin.set_rules(OperationType::HardwareGPIO, gpio_rules);
        info!("[Gateway] Security hardware_rules loaded");
    }

    // Registry rules
    if let Some(reg_rules) = config.get("registry_rules") {
        let read_rules = parse_rules(reg_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let write_rules = parse_rules(reg_rules.get("write").unwrap_or(&serde_json::Value::Null));
        let delete_rules = parse_rules(reg_rules.get("delete").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::RegistryRead, read_rules);
        plugin.set_rules(OperationType::RegistryWrite, write_rules);
        plugin.set_rules(OperationType::RegistryDelete, delete_rules);
        info!("[Gateway] Security registry_rules loaded");
    }

    info!("[Gateway] Security config loaded from {}", config_path.display());
}

/// Load scanner full config from `config.scanner.json`.
///
/// Returns None if the file doesn't exist or can't be parsed.
fn load_scanner_full_config(
    config_path: &std::path::Path,
) -> Option<nemesis_security::scanner::ScannerFullConfig> {
    let data = std::fs::read_to_string(config_path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;

    let enabled: Vec<String> = json
        .get("enabled")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let engines: std::collections::HashMap<String, serde_json::Value> = json
        .get("engines")
        .and_then(|v| v.as_object())
        .map(|map| map.clone().into_iter().collect())
        .unwrap_or_default();

    Some(nemesis_security::scanner::ScannerFullConfig { enabled, engines })
}

/// Open a URL in the default browser.
#[cfg(not(target_os = "android"))]
fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd")
            .raw_arg(format!("/c start {}", url))
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| format!("opening browser: {}", e))?;
        Ok(())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        Err("unsupported platform".to_string())
    }
}

/// Open a plugin window using ProcessManager for lifecycle and deduplication.
///
/// **Single-instance**: Only one window per type is allowed. If a window of
/// the same type already exists, a `window.bring_to_front` notification is
/// sent via WebSocket. If that fails (child dead or unresponsive), the stale
/// child is terminated and a new one is spawned.
///
/// Falls back to browser if the plugin-ui.dll is not found.
#[cfg(not(target_os = "android"))]
fn open_plugin_window(
    process_manager: &Arc<nemesis_desktop::process::ProcessManager>,
    window_type: &str,
    backend_url: &str,
    auth_token: &str,
) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("get exe path: {}", e))?;
    let exe_dir = exe.parent().ok_or("no parent dir")?;

    // Check if plugin-ui.dll exists
    let dll_path = exe_dir.join("plugins").join("plugin_ui.dll");
    let dll_path_alt = exe_dir.join("plugins").join("plugin-ui.dll");
    if !dll_path.exists() && !dll_path_alt.exists() {
        warn!("[Gateway] plugin-ui.dll not found, falling back to browser");
        return open_browser(backend_url);
    }

    // --- Dedup: check if a child of this type already exists ---
    if let Some(child_id) = process_manager.get_child_by_type(window_type) {
        info!(
            "[Gateway] Plugin window '{}' already running (child_id: {}), sending bring_to_front",
            window_type, child_id
        );
        // Try to notify the existing child to bring its window to front
        match process_manager.notify_child(
            &child_id,
            "window.bring_to_front",
            serde_json::json!({}),
        ) {
            Ok(()) => {
                info!("[Gateway] Sent bring_to_front notification to child {}", child_id);
                return Ok(());
            }
            Err(e) => {
                // Notification failed — child may be dead. Clean up and respawn.
                warn!(
                    "[Gateway] Failed to notify child {} ({}), cleaning up and respawning",
                    child_id, e
                );
                let _ = process_manager.terminate_child(&child_id);
                process_manager.cleanup_stale();
            }
        }
    }

    // Build window data
    let window_data = match window_type {
        "dashboard" => serde_json::json!({
            "token": auth_token,
            "web_port": backend_url.split(':').last().and_then(|p| p.parse::<u16>().ok()).unwrap_or(49000),
            "web_host": backend_url.split("://").nth(1).and_then(|s| s.split(':').next()).unwrap_or("127.0.0.1"),
        }),
        "approval" => serde_json::json!({}),
        _ => serde_json::json!({}),
    };

    // Spawn new child via ProcessManager (handles pipe handshake + WS key + window data)
    match process_manager.spawn_child(window_type, &window_data) {
        Ok((child_id, _result_rx)) => {
            info!("[Gateway] Plugin window '{}' spawned (child_id: {})", window_type, child_id);
            Ok(())
        }
        Err(e) => {
            warn!("[Gateway] Failed to spawn plugin window '{}': {}", window_type, e);
            Err(format!("spawn failed: {}", e))
        }
    }
}

// ---------------------------------------------------------------------------
// Gateway banner
// ---------------------------------------------------------------------------

/// Print the gateway startup banner.
fn print_gateway_banner(
    web_host: &str,
    web_port: i64,
    auth_token: &str,
    channels_enabled: usize,
    gateway_host: &str,
    gateway_port: i64,
) {
    println!();
    println!("{}", "=".repeat(50));
    println!("NemesisBot Gateway");
    println!("{}", "=".repeat(50));
    println!("  Web Interface: http://{}:{}", web_host, web_port);
    println!("  Auth Token: {}", common::format_token(auth_token));

    if channels_enabled > 0 {
        println!("  OK {} channel(s) enabled", channels_enabled);
    } else {
        println!("  WARNING: No channels enabled");
    }

    println!("  OK Gateway started on {}:{}", gateway_host, gateway_port);
    println!();
    println!("  Press Ctrl+C to stop");
    println!("{}", "=".repeat(50));
    println!();
}

#[cfg(test)]
mod tests;

/// Parse "host:port" string into (host, port).
fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some(idx) = addr.rfind(':') {
        let host = &addr[..idx];
        let port: u16 = addr[idx + 1..].parse().unwrap_or(0);
        (host.to_string(), port)
    } else {
        (addr.to_string(), 0)
    }
}

/// Count enabled channels.
fn count_enabled_channels(cfg: &nemesis_config::Config) -> usize {
    let mut count = 0;
    if cfg.channels.web.enabled { count += 1; }
    if cfg.channels.websocket.enabled { count += 1; }
    if cfg.channels.telegram.enabled { count += 1; }
    if cfg.channels.discord.enabled { count += 1; }
    if cfg.channels.feishu.enabled { count += 1; }
    if cfg.channels.slack.enabled { count += 1; }
    if cfg.channels.external.enabled { count += 1; }
    if cfg.channels.whatsapp.enabled { count += 1; }
    if cfg.channels.dingtalk.enabled { count += 1; }
    if cfg.channels.qq.enabled { count += 1; }
    if cfg.channels.line.enabled { count += 1; }
    if cfg.channels.onebot.enabled { count += 1; }
    if cfg.channels.maixcam.enabled { count += 1; }
    count
}

/// Print agent startup information.
fn print_agent_startup_info(home: &std::path::Path, total_tools: usize) {
    // Use register_default_tools just for counting display purposes
    let tools = nemesis_agent::register_default_tools();
    let default_count = tools.len();

    let skills_dir = home.join("workspace").join("skills");
    let skill_count = std::fs::read_dir(&skills_dir)
        .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);

    println!();
    println!("  Agent Status:");
    println!("    Tools: {} loaded ({} default + {} extended)", total_tools, default_count, total_tools - default_count);
    println!("    Skills: {} available", skill_count);
    info!("[Gateway] Agent initialized ({} tools, {} skills)", total_tools, skill_count);
}

// ---------------------------------------------------------------------------
// Cluster adapter types
// ---------------------------------------------------------------------------

/// Adapter: Cluster result store → TaskResultPersister trait.
///
// Bridges the cluster's TaskResultStore to PeerChatHandler's
/// TaskResultPersister interface.
struct ClusterResultPersisterAdapter {
    result_store: Arc<nemesis_cluster::task_result_store::TaskResultStore>,
    node_id: String,
}

impl nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister for ClusterResultPersisterAdapter {
    fn set_running(&self, task_id: &str, _source_node: &str) {
        // Mark as running with a placeholder result
        self.result_store.store_success(task_id, "peer_chat", serde_json::json!({
            "status": "running",
            "from": self.node_id,
        }));
    }

    fn set_result(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        error: &str,
        _source_node: &str,
    ) -> Result<(), String> {
        if status == "error" {
            self.result_store.store_failure(task_id, "peer_chat", error);
        } else {
            self.result_store.store_success(task_id, "peer_chat", serde_json::json!({
                "content": response,
                "from": self.node_id,
            }));
        }
        Ok(())
    }

    fn delete(&self, task_id: &str) -> Result<(), String> {
        // TaskResultStore doesn't have a delete method; this is a no-op.
        let _ = task_id;
        Ok(())
    }
}

/// Adapter: nemesis_bus::MessageBus → Cluster's MessageBus trait.
///
/// Translates Cluster's BusInboundMessage to nemesis_types::InboundMessage
/// and publishes on the real message bus.
struct BusToClusterAdapter {
    bus: Arc<nemesis_bus::MessageBus>,
}

impl nemesis_cluster::cluster::MessageBus for BusToClusterAdapter {
    fn publish_inbound(&self, msg: nemesis_cluster::cluster::BusInboundMessage) {
        let inbound = nemesis_types::channel::InboundMessage {
            channel: msg.channel,
            sender_id: msg.sender_id,
            chat_id: msg.chat_id,
            content: msg.content,
            media: vec![],
            session_key: String::new(),
            correlation_id: String::new(),
            metadata: std::collections::HashMap::new(),
            voice_playback: None,
        };
        self.bus.publish_inbound(inbound);
    }
}

// ---------------------------------------------------------------------------
// Gateway command
// ---------------------------------------------------------------------------

/// Run the gateway command.
pub async fn run(local: bool, extra_args: &[String]) -> Result<()> {
    // Step 1: Resolve home directory
    let home = common::resolve_home(local);

    // Step 2: Check configuration file exists
    let config_path = common::config_path(&home);
    if !config_path.exists() {
        eprintln!("Error: Configuration file not found: {}", config_path.display());
        eprintln!();
        eprintln!("  Gateway mode requires a configuration file.");
        eprintln!("  Run 'nemesisbot onboard default' to create one.");
        std::process::exit(1);
    }

    // Step 3: Check home directory exists
    if !home.exists() {
        eprintln!("Error: Configuration directory not found: {}", home.display());
        eprintln!("  Run 'nemesisbot onboard default' to create configuration.");
        std::process::exit(1);
    }

    // Step 3a: Ensure exe directory is in PATH so LLM shell tools can find nemesisbot
    if common::ensure_exe_in_path() {
        tracing::info!("[Gateway] Added exe directory to PATH for LLM shell access");
    }

    // Step 4: Load configuration
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("Error loading config: {}", e))?;

    // Step 5: Initialize logger from config
    let mut args: Vec<String> = std::env::args().skip(2).collect();
    args.extend(extra_args.iter().cloned());
    let _log_flags = common::init_logger_from_config(&config_path, &args);

    // Step 6: Write PID file
    let pid = std::process::id();
    let pid_path = home.join("gateway.pid");
    if let Err(e) = std::fs::write(&pid_path, pid.to_string()) {
        warn!("[Gateway] Failed to write PID file: {}", e);
    } else {
        info!("[Gateway] PID file written: {} (PID: {})", pid_path.display(), pid);
    }

    // Step 7: Resolve the default LLM model and create provider
    let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
    let resolution = nemesis_config::resolve_model_config(&cfg, &llm_ref)
        .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", llm_ref, e))?;

    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: home.join("workspace").to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode.clone(),
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    // Validate provider config by attempting creation (actual provider is built by the factory).
    nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
    info!("[Gateway] Provider config validated for {}", llm_ref);

    let model_name = resolution.model_name.clone();

    // Step 8: Create MessageBus
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    info!("[Gateway] Message bus created");

    // Step 9: Create AgentLoop with mpsc channels (bridge to broadcast bus)
    // The AgentLoop uses mpsc channels, while the bus uses broadcast.
    // We bridge: bus inbound (broadcast) → mpsc inbound → AgentLoop
    //            AgentLoop → mpsc outbound → bus outbound (broadcast)
    //
    // Capacity is 1024 (up from 256) to reduce message loss under load.
    // The inbound bridge is created inside AgentLoopServiceAdapter::start().
    let (agent_outbound_tx, mut agent_outbound_rx) = tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(1024);

    // Bridge: agent outbound mpsc → bus outbound broadcast
    let bus_out = bus.clone();
    let bridge_outbound_handle = tokio::spawn(async move {
        while let Some(msg) = agent_outbound_rx.recv().await {
            bus_out.publish_outbound(msg);
        }
    });

    // The AgentLoop is now created by the factory function (agent_factory.rs).
    // provider, system prompt, AgentConfig, AgentLoop::new_bus, session store,
    // state manager, SharedToolConfig, tool registration, MCP, cluster_rpc,
    // continuation manager — all handled inside build_agent_loop().
    // agent_outbound_tx will be stored in SharedResources later.

    // agent_outbound_tx is moved into SharedResources below.
    // For now, keep it as a local variable.
    // State manager injection into agent_loop is now handled by the factory function.

    // Register all tools (mirrors Go's bot_service.go initComponents):
    //   default tools + web + cluster + spawn + memory + skills + hardware + exec + cron
    let cron_store_path = common::cron_store_path(&home);
    let cron_service = std::sync::Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(
            &cron_store_path.to_string_lossy(),
        ),
    ));

    // C3: Wire CronService — set_on_job handler + start.
    // Mirrors Go's bot_service.go:392-399, 571-579.
    {
        let bus_for_cron = bus.clone();
        cron_service.lock().unwrap().set_on_job(move |job: &nemesis_cron::service::CronJob| {
            if !job.payload.message.is_empty() {
                let channel = job.payload.channel.clone().unwrap_or_else(|| "web".to_string());
                let to = job.payload.to.clone().unwrap_or_default();
                let inbound = nemesis_types::channel::InboundMessage {
                    channel: channel.clone(),
                    sender_id: format!("cron:{}", job.id),
                    chat_id: to,
                    content: job.payload.message.clone(),
                    media: vec![],
                    session_key: String::new(),
                    correlation_id: String::new(),
                    metadata: {
                        let mut m = std::collections::HashMap::new();
                        m.insert("cron_job_id".to_string(), job.id.clone());
                        m.insert("cron_job_name".to_string(), job.name.clone());
                        m
                    },
                    voice_playback: None,
                };
                bus_for_cron.publish_inbound(inbound);
                Ok(format!("Cron job '{}' triggered", job.name))
            } else {
                Ok("No message to deliver".to_string())
            }
        });
        info!("[Gateway] Cron service handler wired (publishes to bus)");
    }

    // Create Forge executor (always create instance for runtime toggle support).
    // M2 + M3 + L1 + L2 + M4 all wired here.
    let forge_enabled = cfg.forge.as_ref().map(|f| f.enabled).unwrap_or(false);
    let forge_for_web: Option<std::sync::Arc<nemesis_forge::forge::Forge>>;
    let forge_executor_for_tools: Option<std::sync::Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>;
    {
        // Load forge config from file, fall back to defaults if missing.
        let forge_config_path = home.join("workspace").join("config").join("config.forge.json");
        let forge_config = if forge_config_path.exists() {
            nemesis_forge::config::load_forge_config(&forge_config_path)
        } else {
            nemesis_forge::config::ForgeConfig::default()
        };
        let forge_workspace = home.join("workspace");
        let forge_dir = forge_workspace.join("forge");
        let mut forge = nemesis_forge::forge::Forge::new(
            forge_config.clone(),
            forge_workspace,
        );

        // Initialize Reflector (statistical analysis + report writing).
        forge.init_reflector(
            nemesis_forge::reflector::Reflector::with_reflections_dir(
                forge_dir.join("reflections"),
            ),
        );
        info!("[Gateway] Forge reflector initialized");

        // Initialize Pipeline (3-stage validation).
        // Create two instances: one owned by Forge, one Arc-shared with LearningEngine.
        let forge_pipeline_registry = std::sync::Arc::new(nemesis_forge::registry::Registry::new(
            nemesis_forge::types::RegistryConfig::default(),
        ));
        forge.init_pipeline(
            nemesis_forge::pipeline::Pipeline::new(
                forge_config.clone(),
                forge_pipeline_registry.clone(),
            ),
        );
        info!("[Gateway] Forge pipeline initialized");

        // Initialize trace collection (TraceCollector + TraceStore).
        {
            let trace_collector = nemesis_forge::trace::TraceCollector::new();
            let trace_store = nemesis_forge::trace_store::TraceStore::new(
                forge_dir.join("traces"),
            );
            forge.init_trace(trace_collector, trace_store);
            info!("[Gateway] Forge trace collection initialized");
        }

        // Initialize learning engine (Phase 6 closed-loop).
        let forge_monitor_registry = std::sync::Arc::new(nemesis_forge::registry::Registry::new(
            nemesis_forge::types::RegistryConfig::default(),
        ));
        {
            let registry = std::sync::Arc::new(nemesis_forge::registry::Registry::new(
                nemesis_forge::types::RegistryConfig::default(),
            ));
            let cycle_store = nemesis_forge::cycle_store::CycleStore::new(&forge_dir);
            let learning_engine = nemesis_forge::learning_engine::LearningEngine::with_forge_dir(
                forge_config.clone(),
                forge_dir.clone(),
                registry,
                cycle_store,
            );
            let cycle_store_for_init = nemesis_forge::cycle_store::CycleStore::new(&forge_dir);
            let forge_monitor = nemesis_forge::monitor::DeploymentMonitor::new(
                forge_config.clone(),
                forge_monitor_registry,
            );
            forge.init_learning(learning_engine, forge_monitor, cycle_store_for_init);
            info!("[Gateway] Forge learning engine initialized (Phase 6)");
        }

        // Set bridge → init syncer.
        forge.set_bridge(std::sync::Arc::new(nemesis_forge::bridge::NoOpBridge::new("local".to_string())));
        forge.init_syncer();
        info!("[Gateway] Forge syncer initialized");

        // Set LLM provider — now handled by the factory function (agent_factory.rs).

        let forge = std::sync::Arc::new(forge);

        // LearningEngine dependency injection is now handled by the factory function.

        let executor = std::sync::Arc::new(
            nemesis_forge::forge_tools::ForgeToolExecutor::new(forge.clone()),
        );
        info!("[Gateway] Forge executor created (8 tools will be registered)");

        // Forge injection into agent_loop is now handled by the factory function.

        // Start background tasks only if enabled in config.
        if forge_enabled {
            let forge_for_start = forge.clone();
            tokio::spawn(async move {
                forge_for_start.start().await;
            });
            info!("[Gateway] Forge started (background tasks running)");
        } else {
            info!("[Gateway] Forge created but not started (enabled=false in config)");
        }

        // Store for web server injection.
        forge_for_web = Some(forge);
        forge_executor_for_tools = Some(executor);
    }

    let mcp_enabled = cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false);

    let mut memory_manager_for_web: Option<std::sync::Arc<nemesis_memory::manager::MemoryManager>> = None;

    let skills_loader_arc: Option<std::sync::Arc<nemesis_skills::loader::SkillsLoader>> = {
        let workspace_str = home.join("workspace").to_string_lossy().to_string();
        let global_skills_str = home.join("workspace").join("skills").to_string_lossy().to_string();
        let loader = nemesis_skills::loader::SkillsLoader::new(
            &workspace_str,
            &global_skills_str,
            "",
        );
        info!("[Gateway] Skills loader created (workspace={}, global_skills={})", workspace_str, global_skills_str);
        Some(std::sync::Arc::new(loader))
    };

    let skills_registry_arc: Option<std::sync::Arc<nemesis_skills::registry::RegistryManager>> = {
        let skills_config_path = home.join("workspace").join("config").join("config.skills.json");
        if skills_config_path.exists() {
            match std::fs::read_to_string(&skills_config_path) {
                Ok(content) => {
                    match serde_json::from_str::<nemesis_skills::types::RegistryConfig>(&content) {
                        Ok(reg_config) => {
                            let rm = nemesis_skills::registry::RegistryManager::from_config(reg_config);
                            info!("[Gateway] Skills registry loaded from {}", skills_config_path.display());
                            Some(std::sync::Arc::new(rm))
                        }
                        Err(e) => {
                            warn!("[Gateway] Failed to parse skills config: {} — skills search/install disabled", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    warn!("[Gateway] Failed to read skills config: {} — skills search/install disabled", e);
                    None
                }
            }
        } else {
            info!("[Gateway] No skills config found at {} — skills search/install disabled", skills_config_path.display());
            None
        }
    };

    // Create MemoryManager (still needed for web server injection).
    // Memory tool executor creation is now handled by the factory function.
    if cfg.memory.as_ref().map(|m| m.enabled).unwrap_or(false) {
        let memory_data_dir = home.join("workspace").join("memory_vector");
        let config_dir = home.join("workspace").join("config");
        let mgr = std::sync::Arc::new(
            nemesis_memory::manager::MemoryManager::with_config_dir(
                &memory_data_dir, &config_dir,
            )
        );
        info!("[Gateway] Memory manager created (data_dir={})", memory_data_dir.display());
        memory_manager_for_web = Some(mgr);
    } else {
        info!("[Gateway] Enhanced memory disabled (config.json: memory.enabled = false)");
    }

    // Web search config: compute for reference, but tool registration is handled by factory.
    {
        let web = &cfg.tools.web;
        let any_enabled = web.brave.enabled
            || web.duckduckgo.enabled
            || web.perplexity.enabled;
        if any_enabled {
            info!("[Gateway] Web search enabled (brave={}, duckduckgo={}, perplexity={})",
                  web.brave.enabled, web.duckduckgo.enabled, web.perplexity.enabled);
        } else {
            info!("[Gateway] Web search disabled (no provider enabled in config.json: tools.web)");
        }
    }

    // SharedToolConfig construction, tool registration, and MCP reload are now handled
    // by the factory function (agent_factory.rs build_agent_loop()).

    if !mcp_enabled {
        info!("[Gateway] MCP disabled in config.json (mcp.enabled = false), skipping");
    }
    info!("[Gateway] Agent loop tools configured (default + memory + skills + hardware + exec + cron{})",
          if mcp_enabled { " + MCP" } else { "" });

    // Step 9a: Set up cluster if enabled.
    // Mirrors Go's bot_service.go initComponents → startCluster.
    // Master switch: config.json "cluster.enabled" (must be true).
    // Sub-config:    config.cluster.json "enabled" (also must be true).
    // Both must be enabled for cluster to activate.
    let cluster_master_enabled = cfg.cluster.as_ref().map(|c| c.enabled).unwrap_or(false);
    let cluster_app_cfg = nemesis_cluster::config_loader::load_app_config(&home.join("workspace"));

    // Cluster RPC resources — filled inside the cluster block below, consumed by SharedResources.
    let mut cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(&str, &str, serde_json::Value)
                -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                            Output = Result<serde_json::Value, String>,
                        > + Send,
                    >,
                > + Send
                + Sync,
        >,
    > = None;
    let mut cluster_rpc_config: Option<nemesis_agent::ClusterRpcConfig> = None;
    let mut cluster_peers_fn: Option<Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>> = None;
    // Cluster agent references — set inside the if block, used later for agent loop startup.
    let mut cluster_task_list_ref: Option<Arc<nemesis_cluster::ClusterTaskList>> = None;
    let mut cluster_work_queue_ref: Option<Arc<nemesis_cluster::ClusterWorkQueue>> = None;
    let mut cluster_arc_ref: Option<std::sync::Weak<nemesis_cluster::cluster::Cluster>> = None;
    let mut cluster_shutdown_ref: Option<Arc<nemesis_cluster::cluster::Cluster>> = None;
    let mut cluster_rpc_client_ref: Option<Arc<nemesis_cluster::rpc::client::RpcClient>> = None;
    if cluster_master_enabled && cluster_app_cfg.enabled {
        let cluster_cfg_path = common::cluster_config_path(&home);
        let cluster_json = std::fs::read_to_string(&cluster_cfg_path).unwrap_or_default();
        let cluster_data: serde_json::Value = serde_json::from_str(&cluster_json).unwrap_or_default();

        let node_id = cluster_data
            .get("node_id")
            .or_else(|| cluster_data.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let node_name = cluster_data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed")
            .to_string();
        let _role = cluster_data
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("worker")
            .to_string();
        let _category = cluster_data
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("development")
            .to_string();

        // Build ClusterConfig
        let cluster_config = nemesis_cluster::types::ClusterConfig {
            node_id: node_id.clone(),
            bind_address: format!("0.0.0.0:{}", cluster_app_cfg.rpc_port),
            peers: vec![],
        };

        let mut cluster = nemesis_cluster::cluster::Cluster::with_workspace(
            cluster_config,
            home.join("workspace"),
        );

        // Set ports and node info from app config
        cluster.set_ports(cluster_app_cfg.port, cluster_app_cfg.rpc_port);
        cluster.set_node_name(&node_name);
        cluster.set_node_type("agent");

        // Load static peers from peers.toml into the registry
        // The peers.toml uses [peers.Key] table format (not [[peers]] array),
        // so we parse it manually.
        let peers_toml_path = common::cluster_dir(&home).join("peers.toml");
        if peers_toml_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&peers_toml_path) {
                if let Ok(doc) = content.parse::<toml::Value>() {
                    if let Some(peers_table) = doc.get("peers").and_then(|v| v.as_table()) {
                        for (key, val) in peers_table {
                            let peer_id = key.replace('_', "-"); // Reverse TOML key sanitization
                            let addr = val.get("address").and_then(|v| v.as_str()).unwrap_or("");
                            let name = val.get("name").and_then(|v| v.as_str()).unwrap_or(&peer_id);
                            let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
                            let cat = val.get("category").and_then(|v| v.as_str()).unwrap_or("general");
                            if addr.is_empty() { continue; }
                            // The address field contains UDP host:port (e.g., "127.0.0.1:11950").
                            // Derive RPC port by convention: UDP port + 10000 (11949→21949).
                            let (host, udp_port) = parse_host_port(addr);
                            let rpc_port = if udp_port > 0 { udp_port + 10000 } else { 0 };
                            let addresses = if host.is_empty() { vec![] } else { vec![host] };
                            info!("[Gateway] Loading static peer: {} ({}) addr={} rpc_port={}", name, peer_id, addr, rpc_port);
                            cluster.handle_discovered_node(
                                &peer_id,
                                name,
                                addresses,
                                rpc_port,
                                role,
                                cat,
                                vec![],
                                vec![],
                                "unknown",
                            );
                        }
                    }
                }
            }
        }

        // --- Create and set RPC Server (before start, needs &mut self) ---
        let rpc_server_config = nemesis_cluster::rpc::server::RpcServerConfig {
            bind_address: format!("0.0.0.0:{}", cluster_app_cfg.rpc_port),
            ..Default::default()
        };
        cluster.set_rpc_server(Arc::new(nemesis_cluster::rpc::server::RpcServer::new(rpc_server_config)));

        // Start cluster (registers local node, creates RPC client, starts sync/recovery loops)
        cluster.start();
        info!("[Gateway] Cluster started (node_id: {}, name: {}, udp: {}, rpc: {})",
            node_id, node_name, cluster_app_cfg.port, cluster_app_cfg.rpc_port);

        // Diagnostic: list registry contents after start
        {
            let all_nodes = cluster.list_nodes();
            for n in &all_nodes {
                info!("[Gateway] Registry node: {} (id={}) status={:?} addr={}",
                    n.base.name, n.base.id, n.status, n.base.address);
            }
        }

        // Register RPC handlers on the server
        if let Err(e) = cluster.register_basic_handlers() {
            warn!("[Gateway] Failed to register basic RPC handlers: {}", e);        }

        // Start RPC server FIRST (register_default_handlers runs inside start(),
        // so we must call start() before registering our custom handlers to avoid
        // them being overwritten).
        let rpc_server_ref = cluster.rpc_server()
            .expect("rpc_server just set")
            .clone();
        info!("[Gateway] Starting RPC server on 0.0.0.0:{}", cluster_app_cfg.rpc_port);
        // Await start() synchronously — it binds the TCP listener and spawns the
        // accept loop, then returns. This ensures default handlers are registered
        // before we overwrite them below.
        if let Err(e) = rpc_server_ref.start().await {
            error!("[Gateway] RPC server error on port {}: {}", cluster_app_cfg.rpc_port, e);
        }
        info!("[Gateway] RPC server started on port {}", cluster_app_cfg.rpc_port);

        // Now register custom peer_chat handler using PeerChatHandler.
        // NOTE: We create the handler here but register it AFTER Arc::new(cluster)
        // so the closure can capture the Arc and register the remote node in the registry.
        let result_store = cluster.result_store().clone();
        let node_id_for_handler = node_id.clone();
        let _node_name_for_handler = node_name.clone();

        let mut handler = nemesis_cluster::rpc::peer_chat_handler::PeerChatHandler::new(
            node_id_for_handler.clone(),
        );
        let llm_timeout = if cluster_app_cfg.llm_timeout_secs > 0 {
            std::time::Duration::from_secs(cluster_app_cfg.llm_timeout_secs)
        } else {
            std::time::Duration::from_secs(24 * 3600)
        };
        handler.set_timeout(llm_timeout);

        // Create cluster agent work queue and task list.
        let cluster_data_dir = home.join("workspace").join("data");
        let cluster_task_list = Arc::new(nemesis_cluster::ClusterTaskList::new(&cluster_data_dir));
        let cluster_work_queue = Arc::new(nemesis_cluster::ClusterWorkQueue::new(64));
        handler.set_cluster_queue(cluster_task_list.clone(), cluster_work_queue.clone());

        // Set RPC client for callbacks (after cluster.start() creates the client).
        let rpc_client = cluster.rpc_client_arc();
        if let Some(client) = rpc_client.clone() {
            handler.set_rpc_client(client);
        }

        // Set result persister for fallback when callback fails.
        let persister = Arc::new(ClusterResultPersisterAdapter {
            result_store: result_store.clone(),
            node_id: node_id_for_handler.clone(),
        });
        handler.set_result_persister(persister);

        // We'll register the handler after Arc::new(cluster) below.
        let handler_arc = Arc::new(handler);
        // Register callback handler (placeholder — will be replaced after Arc::new below).
        // This placeholder just acknowledges receipt.
        {
            let _ = cluster.register_rpc_handler("peer_chat_callback", Box::new(move |payload| {
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(serde_json::json!({"status": "placeholder", "task_id": task_id}))
            }));
        }

        let cluster = Arc::new(cluster);

        // --- Inject cluster task queue into cluster for callback routing ---
        cluster.set_cluster_task_queue(cluster_task_list.clone(), cluster_work_queue.clone());

        // --- Register peer_chat handler (needs Arc<Cluster> to register remote nodes) ---
        {
            let handler_ref = handler_arc.clone();
            let cluster_ref = cluster.clone();
            let _ = cluster.register_rpc_handler("peer_chat", Box::new(move |mut payload| {
                // Extract source node ID from RPC metadata injected by the server.
                let source_node_id = payload
                    .get("_rpc")
                    .and_then(|r| r.get("from"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                if !source_node_id.is_empty() {
                    // Bridge: PeerChatHandler reads source_node_id from `_source.node_id`
                    if let Some(obj) = payload.as_object_mut() {
                        obj.insert(
                            "_source".to_string(),
                            serde_json::json!({"node_id": source_node_id.clone()}),
                        );
                    }

                    // Register the remote node in our registry so we can callback later.
                    // The remote node may not be known via UDP discovery yet (static peers
                    // use peer names, not node_ids). We use the RPC port from the payload
                    // (sent by the remote node's ClusterRpcTool).
                    if cluster_ref.get_peer(&source_node_id).is_none() {
                        let remote_rpc_port = payload
                            .get("_source_rpc_port")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(21949) as u16;

                        cluster_ref.handle_discovered_node(
                            &source_node_id,
                            &source_node_id,
                            vec!["127.0.0.1".to_string()],
                            remote_rpc_port,
                            "worker",
                            "general",
                            vec![],
                            vec![],
                            "unknown",
                        );
                    }
                }

                let h = handler_ref.clone();
                let ack = h.handle(payload, None);
                Ok(serde_json::to_value(&ack)
                    .unwrap_or_else(|_| serde_json::json!({"status": "error"})))
            }));
            info!("[Gateway] Registered PeerChatHandler (async LLM + callback) for peer_chat");
        }

        // --- Now that Cluster is Arc-wrapped, wire up the real callback handler ---
        // Routes callbacks to the correct destination:
        // 1. If the callback matches a ClusterAgent child task (nested cluster_rpc),
        //    inject it back into the ClusterAgent's work queue.
        // 2. Otherwise, publish to bus as cluster_continuation for the main AgentLoop.
        {
            let bus_for_cb = bus.clone();
            let task_list_for_cb = cluster_task_list.clone();
            let work_queue_for_cb = cluster_work_queue.clone();
            let _ = cluster.register_rpc_handler("peer_chat_callback", Box::new(move |payload| {
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("success");
                let response = payload
                    .get("response")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let source_node = payload
                    .get("source_node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                info!("[Gateway] peer_chat_callback received: task_id={}, status={}, from={}", task_id, status, source_node);

                // Route 1: Check if this callback belongs to a ClusterAgent child task.
                // When the ClusterAgent's LLM generates a nested cluster_rpc, the child
                // task's callback must be routed back to the ClusterAgent work queue,
                // not to the main AgentLoop's continuation system.
                if !task_id.is_empty() {
                    if let Some(parent_task_id) = task_list_for_cb.find_by_child_task_id(task_id) {
                        info!(
                            "[Gateway] Callback for child task {} matched ClusterAgent parent task {}, injecting result",
                            task_id, parent_task_id
                        );
                        task_list_for_cb.inject_callback(&parent_task_id, response);
                        if let Err(e) = work_queue_for_cb.submit(parent_task_id) {
                            warn!("[Gateway] Failed to submit resumed task to work queue: {}", e);
                        }
                        return Ok(serde_json::json!({"status": "received", "task_id": task_id}));
                    }
                }

                // Route 2: Main AgentLoop continuation — publish to bus.
                if !task_id.is_empty() {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("status".to_string(), status.to_string());
                    metadata.insert("source_node".to_string(), source_node.to_string());
                    if status == "error" {
                        metadata.insert("error".to_string(), response.to_string());
                    }

                    let inbound = nemesis_types::channel::InboundMessage {
                        channel: "system".to_string(),
                        sender_id: format!("cluster_continuation:{}", task_id),
                        chat_id: String::new(),
                        content: response.to_string(),
                        media: vec![],
                        session_key: String::new(),
                        correlation_id: String::new(),
                        metadata,
                        voice_playback: None,
                    };
                    bus_for_cb.publish_inbound(inbound);
                    info!("[Gateway] Published cluster_continuation for task_id={}", task_id);
                }

                Ok(serde_json::json!({"status": "received", "task_id": task_id}))
            }));
        }

        // --- Inject MessageBus into Cluster for continuation flow ---
        // Cluster.handle_task_complete() publishes cluster_continuation messages
        // on the bus, which AgentLoop intercepts to resume from snapshots.
        {
            let bus_adapter = Arc::new(BusToClusterAdapter {
                bus: bus.clone(),
            });
            cluster.set_message_bus(bus_adapter);
            info!("[Gateway] Cluster: message bus injected for continuation flow");
        }

        // --- Wire Forge-Cluster bridge ---
        // Replace the NoOpBridge with a real ClusterForgeBridgeAdapter so that
        // Forge can share reflections with cluster peers.
        if let Some(ref forge_arc) = forge_for_web {
            let cluster_bridge = ClusterForgeBridgeAdapter::new(node_id.clone());
            forge_arc.set_bridge(Arc::new(cluster_bridge));
            info!("[Gateway] Forge-Cluster bridge wired (node_id={})", node_id);
        }

        // --- Start UDP Discovery Service (managed by Cluster) ---
        cluster.start_discovery(cluster.clone());
        info!("[Gateway] UDP discovery started on port {}", cluster_app_cfg.port);

        // RPC server was already created and set above before start().
        // RPC client was already created by Cluster::start().

        // Create cluster_rpc config + RPC call function for SharedResources.
        // The factory function will create the ClusterRpcTool and register it.
        let rpc_cfg = nemesis_agent::ClusterRpcConfig {
            local_node_id: node_id.clone(),
            timeout_secs: 3600,
            local_rpc_port: cluster_app_cfg.rpc_port,
        };

        // Wire the RPC call function to use cluster.call_with_context_async
        let cluster_weak_for_rpc = Arc::downgrade(&cluster);
        let call_fn = std::sync::Arc::new(
            move |target: &str, action: &str, payload: serde_json::Value| {
                let c = match cluster_weak_for_rpc.upgrade() {
                    Some(arc) => arc,
                    None => {
                        return Box::pin(async move {
                            Err("Cluster已关闭，RPC调用不可用".to_string())
                        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>>;
                    }
                };
                let t = target.to_string();
                let a = action.to_string();
                Box::pin(async move {
                    let bytes = c.call_with_context_async(&t, &a, payload, std::time::Duration::from_secs(3600))
                        .await
                        .map_err(|e| e.to_string())?;
                    // Deserialize the response bytes to JSON Value
                    serde_json::from_slice::<serde_json::Value>(&bytes)
                        .map_err(|e| format!("Failed to parse RPC response: {}", e))
                }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>>
            },
        );

        // Store for SharedResources (factory function will register the tool).
        cluster_rpc_call_fn = Some(call_fn);
        cluster_rpc_config = Some(rpc_cfg);

        // cluster_rpc tool registration is now handled by the factory function.
        // The rpc_call_fn is stored here for SharedResources consumption.
        info!("[Gateway] cluster_rpc tool created (node: {}, peers loaded from peers.toml)", node_name);

        // Build peers_fn: closure that returns online peers with capabilities
        // from the Cluster registry. Used by ClusterRpcTool's dynamic tool description.
        {
            let cluster_weak_for_peers = Arc::downgrade(&cluster);
            cluster_peers_fn = Some(Arc::new(move || {
                match cluster_weak_for_peers.upgrade() {
                    Some(c) => c.get_online_peers()
                        .into_iter()
                        .map(|p| (p.base.id, p.base.name, p.capabilities))
                        .collect(),
                    None => Vec::new(),
                }
            }));
        }

        // ContinuationManager injection into agent_loop is now handled by the factory function.

        // Save references for cluster agent loop startup (used after SharedResources is created).
        cluster_task_list_ref = Some(cluster_task_list.clone());
        cluster_work_queue_ref = Some(cluster_work_queue.clone());
        cluster_arc_ref = Some(Arc::downgrade(&cluster));
        cluster_shutdown_ref = Some(cluster.clone());
        cluster_rpc_client_ref = rpc_client.clone();

        // cluster Arc stays alive via Arc cycle: Cluster → DiscoveryService → Arc<Cluster>.
        // The shutdown ref is used to call cluster.stop() on gateway shutdown.
    } else {
        info!("[Gateway] Cluster disabled in configuration");
    }

    // C1: Create ChannelManager and wire it.
    // Mirrors Go's bot_service.go:333-344: create ChannelManager, register channels,
    // start dispatch loop, call agentLoop.SetChannelManager().

    // Create WebServer early so we can inject SessionManager into WebChannel.
    let web_host = {
        let h = &cfg.channels.web.host;
        if h == "0.0.0.0" || h.is_empty() { "127.0.0.1".to_string() } else { h.clone() }
    };
    let web_port = cfg.channels.web.port;
    let cors_origins = {
        let cors_path = common::cors_config_path(&home);
        if cors_path.exists() {
            match nemesis_web::cors::CORSManager::new(&cors_path) {
                Ok(mgr) => {
                    let mgr_cfg = mgr.config();
                    if mgr_cfg.development_mode {
                        info!("[Gateway] CORS: development_mode enabled, allowing all origins");
                        vec![]
                    } else {
                        let origins = mgr.list_origins();
                        info!("[Gateway] CORS: loaded {} allowed origins from {}", origins.len(), cors_path.display());
                        origins
                    }
                }
                Err(e) => {
                    warn!("[Gateway] Failed to load CORS config: {}, using permissive defaults", e);
                    vec![]
                }
            }
        } else {
            vec![]
        }
    };
    let static_files = crate::embedded::resolve_static_files();
    let web_config = nemesis_web::server::WebServerConfig {
        listen_addr: format!("{}:{}", web_host, web_port),
        auth_token: cfg.channels.web.auth_token.clone(),
        cors_origins,
        ws_path: "/ws".to_string(),
        workspace: Some(home.join("workspace").to_string_lossy().to_string()),
        home: Some(home.to_string_lossy().to_string()),
        version: crate::common::VERSION_INFO.version.to_string(),
        static_dir: None,
        static_files: Some(static_files),
        index_file: "index.html".to_string(),
    };
    let mut web_server = nemesis_web::server::WebServer::new(web_config);
    let web_server_ops = std::sync::Arc::new(crate::adapters::WebServerOpsAdapter::new(
        web_server.session_manager().clone(),
    ));

    // Build list of enabled channels from config (needed by SharedResources + ChannelManager).
    let mut enabled_channels = Vec::new();
    if cfg.channels.web.enabled { enabled_channels.push("web".to_string()); }
    if cfg.channels.websocket.enabled { enabled_channels.push("websocket".to_string()); }
    if cfg.channels.telegram.enabled { enabled_channels.push("telegram".to_string()); }
    if cfg.channels.discord.enabled { enabled_channels.push("discord".to_string()); }
    if cfg.channels.feishu.enabled { enabled_channels.push("feishu".to_string()); }
    if cfg.channels.slack.enabled { enabled_channels.push("slack".to_string()); }
    if cfg.channels.whatsapp.enabled { enabled_channels.push("whatsapp".to_string()); }
    if cfg.channels.dingtalk.enabled { enabled_channels.push("dingtalk".to_string()); }
    if cfg.channels.qq.enabled { enabled_channels.push("qq".to_string()); }
    if cfg.channels.line.enabled { enabled_channels.push("line".to_string()); }
    if cfg.channels.onebot.enabled { enabled_channels.push("onebot".to_string()); }
    if cfg.channels.maixcam.enabled { enabled_channels.push("maixcam".to_string()); }
    if cfg.channels.external.enabled { enabled_channels.push("external".to_string()); }

    {
        let channel_manager = Arc::new(nemesis_channels::manager::ChannelManager::with_allowed_channels(
            enabled_channels.clone(),
        ));

        // Build ChannelInitConfig from gateway config (web channel is always available).
        let init_config = nemesis_channels::manager::ChannelInitConfig {
            web: if cfg.channels.web.enabled {
                Some(nemesis_channels::web::WebChannelConfig {
                    host: cfg.channels.web.host.clone(),
                    port: cfg.channels.web.port as u16,
                    ws_path: cfg.channels.web.path.clone(),
                    auth_token: cfg.channels.web.auth_token.clone(),
                    session_timeout_secs: cfg.channels.web.session_timeout as u64,
                    allow_from: cfg.channels.web.allow_from.clone(),
                })
            } else {
                None
            },
            web_server_ops: Some(web_server_ops),
            external: if cfg.channels.external.enabled {
                Some(nemesis_channels::external::ExternalConfig {
                    input_exe: cfg.channels.external.input_exe.clone(),
                    output_exe: cfg.channels.external.output_exe.clone(),
                    chat_id: cfg.channels.external.chat_id.clone(),
                    sync_to: cfg.channels.external.sync_to.clone(),
                    allow_from: cfg.channels.external.allow_from.clone(),
                })
            } else {
                None
            },
            maixcam: if cfg.channels.maixcam.enabled {
                Some(nemesis_channels::maixcam::MaixCamConfig {
                    host: cfg.channels.maixcam.host.clone(),
                    port: cfg.channels.maixcam.port as u16,
                    allow_from: cfg.channels.maixcam.allow_from.clone(),
                })
            } else {
                None
            },
            line: if cfg.channels.line.enabled {
                Some(nemesis_channels::line::LineConfig {
                    channel_access_token: cfg.channels.line.channel_access_token.clone(),
                    channel_secret: cfg.channels.line.channel_secret.clone(),
                    webhook_port: cfg.channels.line.webhook_port as u16,
                    allow_from: cfg.channels.line.allow_from.clone(),
                })
            } else {
                None
            },
            websocket: if cfg.channels.websocket.enabled {
                Some(nemesis_channels::websocket::WebSocketChannelConfig {
                    host: cfg.channels.websocket.host.clone(),
                    port: cfg.channels.websocket.port as u16,
                    path: cfg.channels.websocket.path.clone(),
                    auth_token: cfg.channels.websocket.auth_token.clone(),
                    allow_from: cfg.channels.websocket.allow_from.clone(),
                    sync_to: cfg.channels.websocket.sync_to.clone(),
                })
            } else {
                None
            },
            // Feature-gated channels (telegram/discord/feishu/slack/etc.) are mapped
            // when the corresponding feature is enabled in nemesisbot's Cargo.toml:
            //   nemesis-channels = { workspace = true, features = ["telegram"] }
            ..Default::default()
        };

        // Initialize channels from config (registers them in the manager).
        let bus_inbound_sender = bus.inbound_sender();
        if let Err(e) = channel_manager.init_channels(&init_config, bus_inbound_sender).await {
            warn!("[Gateway] ChannelManager init_channels note: {} (non-fatal)", e);
        }

        // Setup sync targets — reads each channel's sync_to config and calls add_sync_target().
        // Mirrors Go's manager.go: m.setupSyncTargets() called after initChannels().
        {
            let mut sync_map = std::collections::HashMap::new();
            // Collect sync_to from all channel configs that are enabled
            macro_rules! add_sync {
                ($cfg:expr, $name:expr) => {
                    if $cfg.enabled && !$cfg.sync_to.is_empty() {
                        sync_map.insert($name.to_string(), $cfg.sync_to.clone());
                    }
                };
            }
            add_sync!(cfg.channels.websocket, "websocket");
            add_sync!(cfg.channels.external, "external");
            add_sync!(cfg.channels.web, "web");
            add_sync!(cfg.channels.telegram, "telegram");
            add_sync!(cfg.channels.discord, "discord");
            add_sync!(cfg.channels.feishu, "feishu");
            add_sync!(cfg.channels.dingtalk, "dingtalk");
            add_sync!(cfg.channels.slack, "slack");
            add_sync!(cfg.channels.whatsapp, "whatsapp");
            add_sync!(cfg.channels.qq, "qq");
            add_sync!(cfg.channels.line, "line");
            add_sync!(cfg.channels.maixcam, "maixcam");
            add_sync!(cfg.channels.onebot, "onebot");
            let sync_config = nemesis_channels::manager::ChannelSyncConfig { targets: sync_map };
            channel_manager.setup_sync_targets(&sync_config).await;
        }

        // Bridge: bus outbound broadcast → ChannelManager mpsc.
        // Mirrors Go's manager.go: dispatchOutbound reading from bus.OutboundChannel().
        // Without this, non-web channel outbound is silently dropped.
        let bus_for_cm = bus.clone();
        let cm_outbound_tx = channel_manager.outbound_sender();
        let _cm_bridge_handle = tokio::spawn(async move {
            let mut rx = bus_for_cm.subscribe_outbound();
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if cm_outbound_tx.send(msg).await.is_err() {
                            break; // ChannelManager dispatch loop stopped
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("[Gateway] ChannelManager outbound bridge lagged {} messages", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });
        info!("[Gateway] Bus outbound → ChannelManager bridge connected");

        // Start the outbound dispatch loop (reads from internal mpsc, dispatches to channels).
        if let Err(e) = channel_manager.start_dispatch_loop() {
            warn!("[Gateway] ChannelManager start_dispatch_loop note: {} (non-fatal)", e);
        }

        // Start all registered channels.
        if let Err(e) = channel_manager.start_all().await {
            warn!("[Gateway] ChannelManager start_all note: {} (non-fatal)", e);
        }

        // Keep the ChannelManager alive.
        std::mem::forget(channel_manager);
        info!("[Gateway] ChannelManager created with {} enabled channel(s)", enabled_channels.len());
        // Channel manager injection into agent_loop is now handled by the factory function.
    }

    // Step 9b: Create and inject SecurityPlugin if enabled
    // Mirrors Go's SecurityPlugin registered via PluginManager in instance.go.
    // Keep a reference to the auditor so we can wire up the approval manager later.
    let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
    let security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>> = if security_enabled {
        let security_config = nemesis_security::pipeline::SecurityPluginConfig::default();
        let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(security_config));

        // Load security rules from config.security.json (mirrors Go's config loading)
        let sec_config_path = common::security_config_path(&home);
        load_security_rules(&plugin, &sec_config_path);

        // Initialize audit log file.
        // The JSON config field is "audit_log_file_enabled"; default is true.
        // Log directory is always `{home}/workspace/logs/security_logs/`.
        let audit_dir = format!("{}/workspace/logs/security_logs", home.display());
        if let Err(e) = plugin.init_audit_log_file(&audit_dir) {
            warn!("[Gateway] Failed to initialize security audit log: {}", e);
        } else {
            info!("[Gateway] Security audit log initialized: {}", audit_dir);
        }

        // Security plugin injection into agent_loop is now handled by the factory function.
        info!("[Gateway] Security plugin enabled (injection handled by factory)");

        // Step 9c: Initialize scanner chain from config.scanner.json
        // Mirrors Go's initScannerChain() which calls LoadFromConfig() + chain.Start()
        let scanner_config_path = common::scanner_config_path(&home);
        if scanner_config_path.exists() {
            if let Some(full_config) = load_scanner_full_config(&scanner_config_path) {
                if !full_config.enabled.is_empty() {
                    info!("[Gateway] Initializing scanner chain from config...");
                    plugin.init_scanner_from_config(&full_config).await;
                }
            }
        } else {
            info!("[Gateway] Scanner config file not found: {}, scanner chain not initialized", scanner_config_path.display());
        }

        Some(plugin)
    } else {
        info!("[Gateway] Security plugin disabled by configuration");
        None
    };

    // Step 9d: Setup Observer Manager for conversation lifecycle events.
    // Mirrors Go's bot_service.go Phase 5: observerMgr creation + RequestLogger registration.
    let observer_manager: Option<Arc<nemesis_observer::Manager>> = {
        let observer_mgr = Arc::new(nemesis_observer::Manager::new());

        // Register RequestLogger as Observer (if logging.llm.enabled)
        if let Some(ref logging_cfg) = cfg.logging {
            if let Some(llm_cfg) = &logging_cfg.llm {
                if llm_cfg.enabled {
                    let rl_logging_config = nemesis_agent::request_logger::LoggingConfig {
                        enabled: true,
                        detail_level: match llm_cfg.detail_level.as_str() {
                            "truncated" => nemesis_agent::request_logger::DetailLevel::Truncated,
                            _ => nemesis_agent::request_logger::DetailLevel::Full,
                        },
                        log_dir: if llm_cfg.log_dir.is_empty() {
                            "logs/request_logs".to_string()
                        } else {
                            llm_cfg.log_dir.clone()
                        },
                        save_raw: llm_cfg.save_raw,
                    };
                    let workspace_path = home.join("workspace");
                    let rl_observer = Arc::new(
                        nemesis_agent::request_logger_observer::RequestLoggerObserver::new(
                            rl_logging_config,
                            &workspace_path,
                        ),
                    );
                    // We need an async context to register, but observer_mgr.register is async
                    // and we're not in an async block here. Use tokio runtime handle directly.
                    let mgr = observer_mgr.clone();
                    tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(async {
                            mgr.register(rl_observer).await;
                        })
                    });
                    info!("[Gateway] RequestLoggerObserver registered (logging.llm.enabled = true)");
                }
            }
        }

        // Check if any observers were registered.
        let mgr_check = observer_mgr.clone();
        let has_observers = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                mgr_check.has_observers().await
            })
        });
        if has_observers {
            info!("[Gateway] Observer manager initialized (injection handled by factory)");
            Some(observer_mgr)
        } else {
            None
        }
    };

    // Step 9b: Create DataStore for usage statistics
    let data_store = {
        let data_dir = home.join("workspace").join("data");
        let db_path = data_dir.join("nemesisbot_data.db");
        match nemesis_data::DataStore::open(&db_path) {
            Ok(store) => {
                info!("[Gateway] DataStore opened at {}", db_path.display());
                Some(Arc::new(store))
            }
            Err(e) => {
                warn!("[Gateway] Failed to open DataStore: {e}, usage statistics disabled");
                None
            }
        }
    };

    // Note: DataStore injection into agent_loop is now handled by the factory function.

    // Note: Forge injection into agent_loop is now handled at creation time above.

    // Build SharedResources and use the factory to create the AgentLoop.
    let shared_resources = crate::agent_factory::SharedResources {
        home: home.clone(),
        bus: bus.clone(),
        agent_outbound_tx,
        forge: forge_for_web.clone(),
        forge_executor: forge_executor_for_tools.clone(),
        cron_service: cron_service.clone(),
        security_plugin: security_plugin.clone(),
        observer_manager: observer_manager.clone(),
        data_store: data_store.clone(),
        skills_loader: skills_loader_arc.clone(),
        skills_registry: skills_registry_arc.clone(),
        memory_manager: memory_manager_for_web.clone(),
        enabled_channels: enabled_channels.clone(),
        cluster_rpc_call_fn,
        cluster_rpc_config,
        cluster_peers_fn,
        cluster_rpc_enabled: parking_lot::RwLock::new(None::<Arc<std::sync::atomic::AtomicBool>>),
        mcp_config_path: common::mcp_config_path(&home),
        mcp_enabled,
    };

    let shared_resources = Arc::new(shared_resources);
    let agent_loop = crate::agent_factory::build_agent_loop(&shared_resources)
        .map_err(|e| anyhow::anyhow!("Failed to build agent loop: {}", e))?;
    let initial_tool_count = agent_loop.tool_count();
    info!("[Gateway] AgentLoop built via factory ({} tools)", initial_tool_count);

    // --- Inject tool capabilities into cluster for discovery broadcast ---
    if let Some(ref cluster_weak) = cluster_arc_ref {
        if let Some(cluster) = cluster_weak.upgrade() {
            let tool_names = agent_loop.tool_names();
            cluster.set_capabilities(tool_names);
            info!(
                "[Gateway] Cluster capabilities injected ({} tools)",
                initial_tool_count
            );
        }
    }

    // --- Start cluster agent event loop (if cluster is enabled) ---
    if let (Some(cluster_task_list), Some(cluster_work_queue), Some(cluster_weak)) =
        (cluster_task_list_ref, cluster_work_queue_ref, cluster_arc_ref)
    {
        if let Some(cluster_arc) = cluster_weak.upgrade() {
            // --- Crash recovery: restore incomplete tasks from previous run ---
            //
            // If the process crashed or was killed while cluster tasks were in progress,
            // their state was persisted to disk by save_async_state(). On restart we:
            //   1. restore_from_disk() — load task entries + conversation snapshots into DashMap
            //   2. recover_task_ids()  — collect Pending tasks AND reset WaitingRemote → Pending
            //   3. re-submit all recovered tasks into the work queue
            //
            // WaitingRemote tasks are reset to Pending because the callback they were waiting
            // for is lost — the remote node already sent it while we were down. Re-executing
            // is the safest fallback. See ClusterTaskList::recover_task_ids() for details.
            if let Err(e) = cluster_task_list.restore_from_disk() {
                warn!("[Gateway] Failed to restore cluster tasks from disk: {}", e);
            }
            let recovered = cluster_task_list.recover_task_ids();
            for task_id in &recovered {
                if let Err(e) = cluster_work_queue.submit(task_id.clone()) {
                    warn!(
                        task_id = %task_id,
                        "[Gateway] Failed to re-submit recovered task: {}", e
                    );
                }
            }
            if !recovered.is_empty() {
                info!(
                    count = recovered.len(),
                    "[Gateway] Recovered {} cluster tasks from previous run",
                    recovered.len()
                );
            }

            match crate::agent_factory::build_cluster_agent_loop(&shared_resources, cluster_arc) {
                Ok((cluster_agent, cluster_config)) => {
                    tokio::spawn(async move {
                        crate::cluster_agent::cluster_agent_loop(
                            cluster_agent,
                            cluster_config,
                            cluster_work_queue,
                            cluster_task_list,
                            cluster_rpc_client_ref,
                        ).await;
                    });
                    info!("[Gateway] Cluster agent event loop started");
                }
                Err(e) => {
                    warn!("[Gateway] Failed to build cluster agent: {}", e);
                }
            }
        } else {
            warn!("[Gateway] Cluster已关闭，跳过cluster agent启动");
        }
    }

    // Create shared reference for WebServer model switching
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));

    // Create AgentLoopServiceAdapter for tray start/stop control.
    // Passes the initial AgentLoop directly — no double construction.
    // The adapter manages the inbound bridge + agent spawn internally.
    let agent_adapter = Arc::new(adapters::AgentLoopServiceAdapter::new(
        agent_loop,
        shared_resources.clone(),
        bus.clone(),
        agent_loop_ref.clone(),
    ));

    // Step 10: Wire up WebServer (created early for WebChannel injection)
    web_server.set_message_bus(bus.clone());
    web_server.set_model_info(&model_name, &resolution.api_base, !resolution.api_key.is_empty());

    // Wire streaming provider for SSE chat endpoint.
    // Create an HttpProvider from the same config used for the main provider.
    // This enables /api/chat/stream for token-by-token streaming.
    {
        let streaming_cfg = nemesis_providers::http_provider::HttpProviderConfig {
            name: "streaming".to_string(),
            base_url: resolution.api_base.clone(),
            api_key: resolution.api_key.clone(),
            default_model: resolution.model_name.clone(),
            timeout_secs: 120,
            headers: std::collections::HashMap::new(),
            proxy: None,
            preserve_prefix: false,
        };
        web_server.set_streaming_provider(Arc::new(nemesis_providers::http_provider::HttpProvider::new(streaming_cfg)));
        info!("[Gateway] SSE streaming provider configured for /api/chat/stream");
    }

    info!("[Gateway] Web server created for {}:{}", web_host, web_port);

    // Inject agent service into web server for start/stop control
    web_server.set_agent_service(agent_adapter.clone());
    info!("[Gateway] Agent service injected into web server");

    // Inject DataStore into web server for usage statistics API
    if let Some(ref ds) = data_store {
        web_server.set_data_store(ds.clone());
        info!("[Gateway] DataStore injected into web server");
    }

    // Inject MemoryManager into web server for runtime vector store control
    if let Some(mgr) = memory_manager_for_web {
        web_server.set_memory_manager(mgr);
        info!("[Gateway] MemoryManager injected into web server");
    }

    // Inject Forge into web server for runtime start/stop control
    if let Some(forge) = forge_for_web {
        web_server.set_forge(forge);
        info!("[Gateway] Forge instance injected into web server");
    }

    // Inject AgentLoop ref into web server for runtime model switching
    web_server.set_agent_loop(agent_loop_ref.clone());
    info!("[Gateway] AgentLoop ref injected into web server for model switching");

    info!("[Gateway] Web server components injected");

    // Step 11: Create HealthServer
    let health_port = cfg.gateway.port;
    let health_config = nemesis_health::server::HealthServerConfig {
        listen_addr: format!("{}:{}", &cfg.gateway.host, health_port),
        version: Some(crate::common::VERSION_INFO.version.to_string()),
    };
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(health_config));
    info!("[Gateway] Health server created for {}:{}", &cfg.gateway.host, health_port);

    // Step 12: Create HeartbeatService
    let heartbeat_interval_secs = if cfg.heartbeat.interval > 0 {
        (cfg.heartbeat.interval * 60) as u64
    } else {
        300
    };
    let heartbeat_config = nemesis_heartbeat::service::HeartbeatConfig {
        interval: std::time::Duration::from_secs(heartbeat_interval_secs),
        enabled: cfg.heartbeat.enabled,
        workspace: Some(common::workspace_path(&home).to_string_lossy().to_string()),
        min_interval_minutes: 5,
        default_interval_minutes: 30,
    };
    let heartbeat_service = Arc::new(nemesis_heartbeat::service::HeartbeatService::new(heartbeat_config));
    info!("[Gateway] Heartbeat service created (enabled: {})", cfg.heartbeat.enabled);

    // C2: Wire HeartbeatService — bus + handler + skip file.
    // Mirrors Go's bot_service.go:403-406:
    //   heartbeatSvc.SetBus(msgBus)
    //   heartbeatSvc.SetHandler(createHeartbeatHandler(agentLoop))
    {
        // Adapter: nemesis_bus::MessageBus → heartbeat::MessageBus
        struct HeartbeatBusAdapter {
            bus: Arc<nemesis_bus::MessageBus>,
        }
        impl nemesis_heartbeat::service::MessageBus for HeartbeatBusAdapter {
            fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
                let msg = nemesis_types::channel::OutboundMessage {
                    channel,
                    chat_id,
                    content,
                    message_type: String::new(),
                };
                self.bus.publish_outbound(msg);
            }
        }
        heartbeat_service.set_bus(Arc::new(HeartbeatBusAdapter { bus: bus.clone() }));

        // Handler: calls agent_loop.process_heartbeat() synchronously via block_in_place.
        // Mirrors Go's `createHeartbeatHandler()` in bot_service.go:
        //   1. Check BOOTSTRAP.md → skip heartbeat
        //   2. Fallback channel = "cli", chat_id = "direct"
        //   3. Call ProcessHeartbeat(prompt, channel, chatID)
        //   4. Always return SilentResult (agent sends messages via tools, not via handler)
        let bootstrap_path = common::workspace_path(&home).join("BOOTSTRAP.md");
        let adapter_for_hb = agent_adapter.clone();
        heartbeat_service.set_handler(Box::new(move |prompt: String, mut channel: String, mut chat_id: String| {
            // Check BOOTSTRAP.md — if exists, skip heartbeat entirely.
            if bootstrap_path.exists() {
                tracing::info!("[Gateway] BOOTSTRAP.md exists, skipping heartbeat LLM call");
                return Some(nemesis_heartbeat::service::HeartbeatResult {
                    is_error: false,
                    is_async: false,
                    silent: true,
                    for_user: String::new(),
                    for_llm: "HEARTBEAT_OK".to_string(),
                });
            }

            // Get the current AgentLoop via adapter (may be None if stopped).
            let agent_loop_for_hb = match adapter_for_hb.current() {
                Some(al) => al,
                None => {
                    tracing::debug!("[Gateway] Agent not running, skipping heartbeat");
                    return Some(nemesis_heartbeat::service::HeartbeatResult {
                        is_error: false,
                        is_async: false,
                        silent: true,
                        for_user: String::new(),
                        for_llm: "HEARTBEAT_OK".to_string(),
                    });
                }
            };

            // Use cli:direct as fallback (matching Go).
            if channel.is_empty() || chat_id.is_empty() {
                channel = "cli".to_string();
                chat_id = "direct".to_string();
            }

            tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                match rt.block_on(agent_loop_for_hb.process_heartbeat(&prompt, &channel, &chat_id)) {
                    Ok(response) if response.is_empty() => None,
                    Ok(response) => {
                        let is_heartbeat_ok = response.trim() == "HEARTBEAT_OK";
                        Some(nemesis_heartbeat::service::HeartbeatResult {
                            is_error: false,
                            is_async: false,
                            silent: true, // Go always returns SilentResult
                            for_user: String::new(),
                            for_llm: if is_heartbeat_ok { "HEARTBEAT_OK".to_string() } else { response },
                        })
                    }
                    Err(e) => Some(nemesis_heartbeat::service::HeartbeatResult {
                        is_error: true,
                        is_async: false,
                        silent: false,
                        for_user: String::new(),
                        for_llm: format!("Heartbeat error: {}", e),
                    }),
                }
            })
        }));

        // Set skip file (BOOTSTRAP.md) — if present, heartbeat is deferred.
        let skip_file = common::workspace_path(&home).join("BOOTSTRAP.md");
        if skip_file.exists() {
            heartbeat_service.set_skip_file(skip_file.to_string_lossy().to_string());
        }

        info!("[Gateway] Heartbeat service wired (bus + handler + skip_file)");
    }

    // M1: Create and wire DeviceService.
    // Mirrors Go's bot_service.go:409-413: devices.NewService(Config{Enabled, MonitorUSB}).
    if cfg.devices.enabled {
        let device_config = nemesis_devices::service::DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 30,
            monitor_usb: cfg.devices.monitor_usb,
        };
        let device_service = nemesis_devices::service::DeviceService::with_config(device_config);
        // Wire bus sender: device events → outbound messages via bus
        let bus_for_devices = bus.clone();
        device_service.set_bus_sender(Box::new(move |channel: &str, chat_id: &str, content: &str| {
            let msg = nemesis_types::channel::OutboundMessage {
                channel: channel.to_string(),
                chat_id: chat_id.to_string(),
                content: content.to_string(),
                message_type: String::new(),
            };
            bus_for_devices.publish_outbound(msg);
        }));
        // Start monitoring (USB hotplug, etc.) — async, fire-and-forget
        if let Err(e) = device_service.start().await {
            warn!("[Gateway] Device service start note: {} (non-fatal)", e);
        } else {
            info!("[Gateway] Device service started (USB hotplug monitoring)");
        }
    } else {
        info!("[Gateway] Device service disabled (config.json: devices.enabled = false)");
    }

    // Step 13: Create ServiceManager with config
    let bot_config = nemesis_services::BotServiceConfig {
        security_enabled: cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true),
        config_path: config_path.clone(),
        workspace: home.join("workspace"),
        heartbeat_interval_secs,
        heartbeat_enabled: cfg.heartbeat.enabled,
        gateway_host: cfg.gateway.host.clone(),
        gateway_port: cfg.gateway.port as u16,
        llm_logging_enabled: cfg.logging.as_ref()
            .and_then(|l| l.llm.as_ref())
            .map(|l| l.enabled)
            .unwrap_or(false),
        ..Default::default()
    };
    let svc_mgr = Arc::new(nemesis_services::ServiceManager::with_config(bot_config));

    // Inject adapted services into BotService
    {
        let bot = svc_mgr.get_bot_service();
        bot.inject_health(Arc::new(adapters::HealthServerAdapter::new(health_server.clone())));
        bot.inject_heartbeat(Arc::new(adapters::HeartbeatServiceAdapter::new(heartbeat_service.clone())));
        // Agent is NOT injected into BotService — its lifecycle is managed directly
        // by AgentLoopServiceAdapter (tray start/stop, gateway shutdown).
    }

    // Step 14: Start basic services
    svc_mgr.start_basic_services().map_err(|e| {
        anyhow::anyhow!("Error starting basic services: {}", e)
    })?;

    // Start cron scheduler (after on_job handler is wired).
    // Mirrors Go's bot_service.go:571-579 cronSvc.Start().
    {
        let cron = cron_service.lock().unwrap();
        if let Err(e) = cron.start().await {
            warn!("[Gateway] Cron service start note: {}", e);
        } else {
            info!("[Gateway] Cron scheduler started");
        }
    }

    // Step 15: Print agent startup info
    print_agent_startup_info(&home, initial_tool_count);

    // L3: Bridge logger → SSE EventHub for real-time log streaming to Dashboard.
    // Mirrors Go's bot_service.go:674-688: logger.SetLogHook() → eventHub.
    if let Some(logger) = nemesis_logger::global() {
        let event_hub = web_server.event_hub().clone();
        logger.set_hook(Box::new(move |entry: nemesis_logger::logger::LogEntry| {
            let data = serde_json::json!({
                "level": entry.level,
                "timestamp": entry.timestamp,
                "component": entry.component,
                "message": entry.message,
            });
            event_hub.publish(nemesis_web::events::EVENT_LOG, data);
        }));
        info!("[Gateway] Logger → SSE EventHub bridge connected");
    }

    // Step 16: Start outbound dispatch (bus outbound → WebSocket sessions)
    let dispatch_bus = bus.clone();
    let dispatch_session_mgr = web_server.session_manager().clone();
    let dispatch_handle = tokio::spawn(async move {
        nemesis_web::server::dispatch_outbound(dispatch_bus, dispatch_session_mgr).await;
    });
    info!("[Gateway] Outbound dispatch started");

    // Step 17: Start WebServer in background
    let web_shutdown_rx = svc_mgr.subscribe_shutdown();
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = web_server.start_with_shutdown(web_shutdown_rx, Some(bound_tx)).await {
            error!("[Gateway] Web server error: {}", e);
        }
    });
    info!("[Gateway] Web server starting on {}:{}", web_host, web_port);

    // Wait for the actual bound address (sent immediately after TcpListener::bind)
    let real_port: i64 = match bound_rx.await {
        Ok(addr) => {
            info!("[Gateway] Web server bound to {}", addr);
            addr.port() as i64
        }
        Err(_) => {
            warn!("[Gateway] Failed to receive web server bound address, using config port");
            web_port
        }
    };

    // Step 17: HealthServer is started by BotService (svc_mgr.start_bot() below)
    // via start_services() → services.health.start(). No separate spawn needed here.
    info!("[Gateway] Health server will be started by bot service on {}:{}", &cfg.gateway.host, health_port);

    // Step 18: Start AgentLoop's bus processing via adapter
    if let Err(e) = agent_adapter.start() {
        warn!("[Gateway] Agent adapter start note: {}", e);
    }
    info!("[Gateway] Agent loop started via adapter, listening on bus");

    // Step 19: Start bot service (for state tracking)
    if let Err(e) = svc_mgr.start_bot() {
        warn!("[Gateway] Bot service start note: {}", e);
        // Non-fatal: the real services are already started above
    }

    // Step 20: Compute display URLs (real_port already resolved via oneshot in Step 17)
    let _web_url = format!("http://{}:{}", web_host, real_port);
    let _chat_url = format!("http://{}:{}/chat/", web_host, real_port);

    // Step 21: Print startup banner
    let enabled_channels = count_enabled_channels(&cfg);
    print_gateway_banner(
        &web_host,
        real_port,
        &cfg.channels.web.auth_token,
        enabled_channels,
        &cfg.gateway.host,
        cfg.gateway.port,
    );

    // Verify web server is listening
    let listen_addr = format!("{}:{}", web_host, real_port);
    println!("  Checking web server on {}...", listen_addr);
    match tokio::net::TcpStream::connect(&listen_addr).await {
        Ok(_) => println!("  OK Web server is listening"),
        Err(e) => println!("  WARNING: Web server not yet listening: {}", e),
    }

    // Mark as ready (mirrors Go's automatic readiness after HTTP server starts)
    health_server.set_ready(true);

    // Create and start ProcessManager for plugin window lifecycle + dedup
    let process_manager = Arc::new(nemesis_desktop::process::ProcessManager::new());
    if let Err(e) = process_manager.start().await {
        warn!("[Gateway] ProcessManager start note: {} (non-fatal, plugin windows will use fallback)", e);
    } else {
        info!("[Gateway] ProcessManager started (WS server on port {})", process_manager.ws_port());
    }

    // Wire up ApprovalManager: ProcessManager → SecurityPlugin auditor
    // When a tool call triggers an "ask" rule, the auditor will call
    // request_approval_sync() which spawns an approval popup child process.
    if let Some(ref plugin) = security_plugin {
        let auditor = plugin.auditor();
        let adapter = Arc::new(ApprovalPopupAdapter::new(process_manager.clone()));
        auditor.set_approval_manager(adapter);
        info!("[Gateway] Approval manager wired (popup via ProcessManager)");
    }

    // Step 22: Configure system tray (desktop only)
    #[cfg(not(target_os = "android"))]
    {
        use nemesis_desktop::PlatformTray;

        let mut tray = PlatformTray::new();

        let start_adapter = Arc::clone(&agent_adapter);
        tray.set_on_start(Box::new(move || {
            if let Err(e) = start_adapter.start() {
                tracing::warn!("[Gateway] Tray: failed to start agent: {}", e);
            }
        }));

        let stop_adapter = Arc::clone(&agent_adapter);
        tray.set_on_stop(Box::new(move || {
            if let Err(e) = stop_adapter.stop() {
                tracing::warn!("[Gateway] Tray: failed to stop agent: {}", e);
            }
        }));

        let pm = Arc::clone(&process_manager);
        let dashboard_url = _web_url.clone();
        let dashboard_token = cfg.channels.web.auth_token.clone();
        tray.set_on_open_dashboard(Box::new(move || {
            let _ = open_plugin_window(&pm, "dashboard", &dashboard_url, &dashboard_token);
        }));

        let chat_url = _chat_url.clone();
        tray.set_on_open_chat(Box::new(move || {
            let _ = open_browser(&chat_url);
        }));

        let shutdown_svc = Arc::clone(&svc_mgr);
        tray.set_on_quit(Box::new(move || {
            trigger_global_shutdown();
            shutdown_svc.shutdown();
        }));

        // Start tray on a dedicated thread (runs winit EventLoop)
        let _tray_handle = tray.run();
        info!("[Gateway] System tray started");
        println!("  OK System tray started");
    }

    // Step 23: Wait for shutdown signal
    svc_mgr.wait_for_shutdown().await;

    // Step 24: Graceful shutdown
    println!();
    println!("Shutting down...");
    svc_mgr.shutdown();

    // Cancel active voice sessions and release ONNX engines
    // so spawn_blocking tasks exit before Runtime drop.
    nemesis_web::handlers::voice::voice_shutdown().await;

    // Stop ProcessManager (terminates all child processes)
    if let Err(e) = process_manager.stop() {
        warn!("[Gateway] ProcessManager stop note: {}", e);
    }

    // Close the message bus
    bus.close();

    // Abort background tasks
    web_handle.abort();
    agent_adapter.stop().ok();
    bridge_outbound_handle.abort();
    dispatch_handle.abort();

    // Stop cluster (joins discovery threads, breaks Arc cycle)
    if let Some(cluster) = cluster_shutdown_ref.take() {
        cluster.stop();
    }

    // Clean up PID file
    let _ = std::fs::remove_file(home.join("gateway.pid"));

    println!("  OK Gateway stopped");

    Ok(())
}

