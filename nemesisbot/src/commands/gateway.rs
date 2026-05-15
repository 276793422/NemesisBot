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
use tracing::{info, warn, error};

use crate::adapters;
use crate::common;

// ---------------------------------------------------------------------------
// Global shutdown state
// ---------------------------------------------------------------------------

/// Global shutdown flag (replaces Go's globalShutdownChan).
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request global shutdown from any component.
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
                "Approval rejected: plugin-ui.dll not found (operation={}, target={}, risk={}). \
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
            "Requesting approval popup: operation={}, target={}, risk={}",
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
                info!("Approval result: action={} for request_id={}", action, request_id);
                Ok(action == "approved")
            }
            Ok(Err(e)) => {
                warn!("Approval channel error: {}", e);
                Ok(false)
            }
            Err(_) => {
                warn!("Approval timeout after {}s", timeout_secs);
                Ok(false) // timeout = rejected
            }
        }
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
        info!("Security config file not found: {}, using defaults", config_path.display());
        return;
    }

    let data = match std::fs::read_to_string(config_path) {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to read security config: {}", e);
            return;
        }
    };

    let config: serde_json::Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse security config JSON: {}", e);
            return;
        }
    };

    // Set default_action
    if let Some(action) = config.get("default_action").and_then(|v| v.as_str()) {
        plugin.auditor().set_default_action(action);
        info!("Security default_action: {}", action);
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
        info!("Security file_rules loaded");
    }

    // Dir rules
    if let Some(dir_rules) = config.get("dir_rules") {
        let read_rules = parse_rules(dir_rules.get("read").unwrap_or(&serde_json::Value::Null));
        let create_rules = parse_rules(dir_rules.get("create").unwrap_or(&serde_json::Value::Null));
        let delete_rules = parse_rules(dir_rules.get("delete").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::DirRead, read_rules);
        plugin.set_rules(OperationType::DirCreate, create_rules);
        plugin.set_rules(OperationType::DirDelete, delete_rules);
        info!("Security dir_rules loaded");
    }

    // Process rules
    if let Some(proc_rules) = config.get("process_rules") {
        let exec_rules = parse_rules(proc_rules.get("exec").unwrap_or(&serde_json::Value::Null));
        let spawn_rules = parse_rules(proc_rules.get("spawn").unwrap_or(&serde_json::Value::Null));
        let kill_rules = parse_rules(proc_rules.get("kill").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::ProcessExec, exec_rules);
        plugin.set_rules(OperationType::ProcessSpawn, spawn_rules);
        plugin.set_rules(OperationType::ProcessKill, kill_rules);
        info!("Security process_rules loaded");
    }

    // Network rules
    if let Some(net_rules) = config.get("network_rules") {
        let request_rules = parse_rules(net_rules.get("request").unwrap_or(&serde_json::Value::Null));
        let download_rules = parse_rules(net_rules.get("download").unwrap_or(&serde_json::Value::Null));
        let upload_rules = parse_rules(net_rules.get("upload").unwrap_or(&serde_json::Value::Null));

        plugin.set_rules(OperationType::NetworkRequest, request_rules);
        plugin.set_rules(OperationType::NetworkDownload, download_rules);
        plugin.set_rules(OperationType::NetworkUpload, upload_rules);
        info!("Security network_rules loaded");
    }

    info!("Security config loaded from {}", config_path.display());
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
fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
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
        warn!("plugin-ui.dll not found, falling back to browser");
        return open_browser(backend_url);
    }

    // --- Dedup: check if a child of this type already exists ---
    if let Some(child_id) = process_manager.get_child_by_type(window_type) {
        info!(
            "Plugin window '{}' already running (child_id: {}), sending bring_to_front",
            window_type, child_id
        );
        // Try to notify the existing child to bring its window to front
        match process_manager.notify_child(
            &child_id,
            "window.bring_to_front",
            serde_json::json!({}),
        ) {
            Ok(()) => {
                info!("Sent bring_to_front notification to child {}", child_id);
                return Ok(());
            }
            Err(e) => {
                // Notification failed — child may be dead. Clean up and respawn.
                warn!(
                    "Failed to notify child {} ({}), cleaning up and respawning",
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
            info!("Plugin window '{}' spawned (child_id: {})", window_type, child_id);
            Ok(())
        }
        Err(e) => {
            warn!("Failed to spawn plugin window '{}': {}", window_type, e);
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
    if cfg.channels.telegram.enabled { count += 1; }
    if cfg.channels.discord.enabled { count += 1; }
    if cfg.channels.feishu.enabled { count += 1; }
    if cfg.channels.slack.enabled { count += 1; }
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
    info!("Agent initialized ({} tools, {} skills)", total_tools, skill_count);
}

// ---------------------------------------------------------------------------
// Provider adapter (shared with agent.rs)
// ---------------------------------------------------------------------------

use async_trait::async_trait;
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};
use nemesis_agent::types::{AgentConfig, ToolCallInfo as AgentToolCallInfo};

/// Adapter wrapping a `nemesis_providers::router::LLMProvider` to implement
/// the `nemesis_agent::LlmProvider` trait expected by `AgentLoop`.
struct ProviderAdapter {
    inner: Arc<dyn nemesis_providers::router::LLMProvider>,
    default_model: String,
}

impl ProviderAdapter {
    fn new(inner: Arc<dyn nemesis_providers::router::LLMProvider>, default_model: String) -> Self {
        Self { inner, default_model }
    }
}

#[async_trait]
impl LlmProvider for ProviderAdapter {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let model_to_use = if model.is_empty() { &self.default_model } else { model };

        let provider_messages: Vec<nemesis_providers::types::Message> = messages
            .into_iter()
            .map(|m| nemesis_providers::types::Message {
                role: m.role,
                content: m.content,
                tool_calls: m.tool_calls.unwrap_or_default().into_iter().map(|tc| {
                    nemesis_providers::types::ToolCall {
                        id: tc.id,
                        call_type: Some("function".to_string()),
                        function: Some(nemesis_providers::types::FunctionCall {
                            name: tc.name,
                            arguments: tc.arguments,
                        }),
                        name: None,
                        arguments: None,
                    }
                }).collect(),
                tool_call_id: m.tool_call_id,
                timestamp: None,
            })
            .collect();

        let provider_options = match options {
            Some(opts) => nemesis_providers::types::ChatOptions {
                temperature: opts.temperature.map(|t| t as f64),
                max_tokens: opts.max_tokens.map(|t| t as i64),
                top_p: opts.top_p.map(|p| p as f64),
                stop: opts.stop,
                extra: std::collections::HashMap::new(),
            },
            None => nemesis_providers::types::ChatOptions {
                temperature: Some(0.7),
                max_tokens: Some(8192),
                top_p: None,
                stop: None,
                extra: std::collections::HashMap::new(),
            },
        };

        // Convert agent tool definitions to provider tool definitions.
        let provider_tools: Vec<nemesis_providers::types::ToolDefinition> = tools
            .into_iter()
            .map(|t| nemesis_providers::types::ToolDefinition {
                tool_type: t.tool_type,
                function: nemesis_providers::types::ToolFunctionDefinition {
                    name: t.function.name,
                    description: t.function.description,
                    parameters: t.function.parameters,
                },
            })
            .collect();

        match self.inner.chat(&provider_messages, &provider_tools, model_to_use, &provider_options).await {
            Ok(resp) => {
                let tool_calls: Vec<AgentToolCallInfo> = resp.tool_calls
                    .into_iter()
                    .filter_map(|tc| {
                        let func = tc.function?;
                        Some(AgentToolCallInfo {
                            id: tc.id,
                            name: func.name,
                            arguments: func.arguments,
                        })
                    })
                    .collect();
                let finished = tool_calls.is_empty() || resp.finish_reason == "stop";
                Ok(LlmResponse {
                    content: resp.content,
                    tool_calls,
                    finished,
                })
            }
            Err(e) => {
                warn!("LLM provider error: {}", e);
                Err(format!("{}", e))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cluster adapter types
// ---------------------------------------------------------------------------

/// Direct LLM channel for PeerChatHandler.
///
/// Calls the local LLM provider directly (via HTTP), bypassing the
/// AgentLoop/Bus pipeline. This is the Phase 1 approach: B-side gets
/// real LLM processing without requiring a full Bus→AgentLoop integration.
struct DirectLlmChannel {
    base_url: String,
    api_key: String,
    model: String,
}

impl DirectLlmChannel {
    fn new(base_url: String, api_key: String, model: String) -> Self {
        Self { base_url, api_key, model }
    }
}

impl nemesis_cluster::rpc::peer_chat_handler::LlmChannel for DirectLlmChannel {
    fn submit(
        &self,
        _session_key: &str,
        content: &str,
        _correlation_id: &str,
    ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let base_url = self.base_url.clone();
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let content = content.to_string();

        tokio::spawn(async move {
            // Call the LLM HTTP API directly
            let client = reqwest::Client::new();
            let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

            let body = serde_json::json!({
                "model": model,
                "messages": [{"role": "user", "content": content}],
                "temperature": 0.7,
            });

            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .timeout(std::time::Duration::from_secs(120))
                .send()
                .await;

            let result = match response {
                Ok(resp) => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(json) => {
                            json.get("choices")
                                .and_then(|c| c.get(0))
                                .and_then(|c| c.get("message"))
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.as_str())
                                .unwrap_or("")
                                .to_string()
                        }
                        Err(e) => {
                            tracing::error!("DirectLlmChannel: failed to parse response: {}", e);
                            format!("[LLM error: {}]", e)
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("DirectLlmChannel: HTTP request failed: {}", e);
                    format!("[LLM error: {}]", e)
                }
            };

            let _ = tx.send(result);
        });

        Ok(rx)
    }
}

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
        warn!("Failed to write PID file: {}", e);
    } else {
        info!("PID file written: {} (PID: {})", pid_path.display(), pid);
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
        connect_mode: resolution.connect_mode,
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    let provider = nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
    info!("Provider created for {}", llm_ref);

    let model_name = resolution.model_name.clone();

    // Step 8: Create MessageBus
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    info!("Message bus created");

    // Step 9: Create AgentLoop with mpsc channels (bridge to broadcast bus)
    // The AgentLoop uses mpsc channels, while the bus uses broadcast.
    // We bridge: bus inbound (broadcast) → mpsc inbound → AgentLoop
    //            AgentLoop → mpsc outbound → bus outbound (broadcast)
    //
    // Capacity is 1024 (up from 256) to reduce message loss under load.
    // The inbound bridge also tracks dropped messages for observability.
    let (agent_inbound_tx, agent_inbound_rx) = tokio::sync::mpsc::channel::<nemesis_types::channel::InboundMessage>(1024);
    let (agent_outbound_tx, mut agent_outbound_rx) = tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(1024);

    // Bridge: bus inbound broadcast → agent inbound mpsc
    let bus_inbound = bus.subscribe_inbound();
    let bridge_inbound_handle = tokio::spawn(async move {
        let mut rx = bus_inbound;
        let mut total_dropped: u64 = 0;
        loop {
            match rx.recv().await {
                Ok(msg) => {
                    if agent_inbound_tx.send(msg).await.is_err() {
                        break; // Agent receiver dropped
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    total_dropped += n as u64;
                    tracing::warn!(
                        "Agent inbound bridge lagged by {} messages (total dropped: {})",
                        n, total_dropped
                    );
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    if total_dropped > 0 {
                        tracing::warn!(
                            "Agent inbound bridge closing with {} total dropped messages",
                            total_dropped
                        );
                    }
                    break;
                }
            }
        }
    });

    // Bridge: agent outbound mpsc → bus outbound broadcast
    let bus_out = bus.clone();
    let bridge_outbound_handle = tokio::spawn(async move {
        while let Some(msg) = agent_outbound_rx.recv().await {
            bus_out.publish_outbound(msg);
        }
    });

    let adapter = ProviderAdapter::new(provider, model_name.clone());
    let agent_config = AgentConfig {
        model: model_name.clone(),
        system_prompt: None,
        max_turns: cfg.agents.defaults.max_tool_iterations.max(1) as u32,
        tools: Vec::new(),
    };

    let mut agent_loop = AgentLoop::new_bus(
        Box::new(adapter),
        agent_config,
        agent_outbound_tx,
        nemesis_agent::r#loop::ConcurrentMode::Reject,
        8,
    );

    // Register all tools (mirrors Go's bot_service.go initComponents):
    //   default tools + web + cluster + spawn + memory + skills + hardware + exec + cron
    let cron_store_path = common::cron_store_path(&home);
    let cron_service = std::sync::Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(
            &cron_store_path.to_string_lossy(),
        ),
    ));

    // Create Forge executor if forge.enabled = true (mirrors Go's bot_service.go initComponents)
    let forge_executor = if cfg.forge.as_ref().map(|f| f.enabled).unwrap_or(false) {
        let forge_config = nemesis_forge::config::ForgeConfig::default();
        let forge = std::sync::Arc::new(nemesis_forge::forge::Forge::new(
            forge_config,
            home.join("workspace"),
        ));
        let executor = std::sync::Arc::new(
            nemesis_forge::forge_tools::ForgeToolExecutor::new(forge),
        );
        info!("Forge executor created (8 tools will be registered)");
        Some(executor)
    } else {
        None
    };

    let shared_config = nemesis_agent::SharedToolConfig {
        workspace: Some(home.join("workspace").to_string_lossy().to_string()),
        cron_service: Some(cron_service),
        forge_executor,
        ..Default::default()
    };
    let all_tools = nemesis_agent::register_shared_tools(&shared_config);
    for (name, tool) in all_tools {
        agent_loop.register_tool(name, tool);
    }
    info!("Agent loop created with shared tools (default + memory + skills + hardware + exec + cron)");

    // Step 9a: Set up cluster if enabled.
    // Mirrors Go's bot_service.go initComponents → startCluster.
    // Loads cluster config from workspace/config/config.cluster.json,
    // creates a Cluster instance, starts UDP discovery + RPC server,
    // and registers the cluster_rpc tool.
    let cluster_app_cfg = nemesis_cluster::config_loader::load_app_config(&home.join("workspace"));
    if cluster_app_cfg.enabled {
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
                            info!("Loading static peer: {} ({}) addr={} rpc_port={}", name, peer_id, addr, rpc_port);
                            cluster.handle_discovered_node(
                                &peer_id,
                                name,
                                addresses,
                                rpc_port,
                                role,
                                cat,
                                vec![],
                                vec![],
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
        info!("Cluster started (node_id: {}, name: {}, udp: {}, rpc: {})",
            node_id, node_name, cluster_app_cfg.port, cluster_app_cfg.rpc_port);

        // Diagnostic: list registry contents after start
        {
            let all_nodes = cluster.list_nodes();
            for n in &all_nodes {
                info!("Registry node: {} (id={}) status={:?} addr={}",
                    n.base.name, n.base.id, n.status, n.base.address);
            }
        }

        // Register RPC handlers on the server
        if let Err(e) = cluster.register_basic_handlers() {
            warn!("Failed to register basic RPC handlers: {}", e);
        }

        // Start RPC server FIRST (register_default_handlers runs inside start(),
        // so we must call start() before registering our custom handlers to avoid
        // them being overwritten).
        let rpc_server_ref = cluster.rpc_server()
            .expect("rpc_server just set")
            .clone();
        info!("Starting RPC server on 0.0.0.0:{}", cluster_app_cfg.rpc_port);
        // Await start() synchronously — it binds the TCP listener and spawns the
        // accept loop, then returns. This ensures default handlers are registered
        // before we overwrite them below.
        if let Err(e) = rpc_server_ref.start().await {
            error!("RPC server error on port {}: {}", cluster_app_cfg.rpc_port, e);
        }
        info!("RPC server started on port {}", cluster_app_cfg.rpc_port);

        // Now register custom peer_chat handler using PeerChatHandler.
        // Phase 1: B-side uses DirectLlmChannel to call LLM directly.
        // Phase 2: ACK → async LLM → callback → continuation.
        // NOTE: We create the handler here but register it AFTER Arc::new(cluster)
        // so the closure can capture the Arc and register the remote node in the registry.
        let result_store = cluster.result_store().clone();
        let node_id_for_handler = node_id.clone();
        let _node_name_for_handler = node_name.clone();

        // Create DirectLlmChannel: calls the local TestAIServer LLM directly.
        // This replaces the echo-closure with real LLM processing on the B-side.
        let llm_channel = Arc::new(DirectLlmChannel::new(
            resolution.api_base.clone(),
            resolution.api_key.clone(),
            model_name.clone(),
        ));

        let mut handler = nemesis_cluster::rpc::peer_chat_handler::PeerChatHandler::new(
            node_id_for_handler.clone(),
        );
        // Use user-configured LLM timeout (default 2 hours) for B-side API request.
        let llm_timeout = if cluster_app_cfg.llm_timeout_secs > 0 {
            std::time::Duration::from_secs(cluster_app_cfg.llm_timeout_secs)
        } else {
            // 0 means no timeout — use a very large value as safety net
            std::time::Duration::from_secs(24 * 3600)
        };
        handler.set_timeout(llm_timeout);
        handler.set_llm_channel(llm_channel);

        // Set RPC client for callbacks (after cluster.start() creates the client).
        let rpc_client = cluster.rpc_client_arc();
        if let Some(client) = rpc_client {
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
        let peer_chat_handler_node_id = node_id_for_handler.clone();

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
                        );
                    }
                }

                let h = handler_ref.clone();
                let ack = h.handle(payload, None);
                Ok(serde_json::to_value(&ack)
                    .unwrap_or_else(|_| serde_json::json!({"status": "error"})))
            }));
            info!("Registered PeerChatHandler (async LLM + callback) for peer_chat");
        }

        // --- Now that Cluster is Arc-wrapped, wire up the real callback handler ---
        // This replaces the placeholder with a handler that publishes a
        // cluster_continuation message on the bus for AgentLoop to resume.
        {
            let bus_for_cb = bus.clone();
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

                info!("peer_chat_callback received: task_id={}, status={}, from={}", task_id, status, source_node);

                // Publish directly to bus as a cluster_continuation message.
                // AgentLoop's bus loop intercepts this prefix, loads the continuation
                // snapshot, and resumes the LLM session with the B-side response.
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
                    };
                    bus_for_cb.publish_inbound(inbound);
                    info!("Published cluster_continuation for task_id={}", task_id);
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
            info!("Cluster: message bus injected for continuation flow");
        }

        // --- Start UDP Discovery Service ---
        // Mirrors Go's discovery.Start() call
        let discovery_config = nemesis_cluster::discovery::DiscoveryConfig::with_encryption(
            cluster_app_cfg.port,
            std::time::Duration::from_secs(cluster_app_cfg.broadcast_interval),
            "", // No encryption token for now
        );
        match nemesis_cluster::discovery::DiscoveryService::new(
            cluster.clone(),
            discovery_config,
        ) {
            Ok(discovery) => {
                match discovery.start() {
                    Ok(_) => info!("UDP discovery started on port {}", cluster_app_cfg.port),
                    Err(e) => warn!("Failed to start UDP discovery: {}", e),
                }
                // Keep discovery alive — prevent Drop which would stop it
                std::mem::forget(discovery);
            }
            Err(e) => warn!("Failed to create discovery service: {}", e),
        }

        // RPC server was already created and set above before start().
        // RPC client was already created by Cluster::start().

        // Create the cluster_rpc tool with an RPC call function
        let cluster_rpc_config = nemesis_agent::ClusterRpcConfig {
            local_node_id: node_id.clone(),
            timeout_secs: 3600,
            local_rpc_port: cluster_app_cfg.rpc_port,
        };
        let mut cluster_rpc_tool = nemesis_agent::ClusterRpcTool::new(cluster_rpc_config);

        // Wire the RPC call function to use cluster.call_with_context_async
        let cluster_for_rpc = cluster.clone();
        let rpc_call_fn = std::sync::Arc::new(
            move |target: &str, action: &str, payload: serde_json::Value| {
                let c = cluster_for_rpc.clone();
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
        cluster_rpc_tool.set_rpc_call_fn(rpc_call_fn);

        agent_loop.register_tool("cluster_rpc".to_string(), Box::new(cluster_rpc_tool));
        info!("cluster_rpc tool registered (node: {}, peers loaded from peers.toml)", node_name);

        // --- Inject ContinuationManager into AgentLoop ---
        // When a cluster_rpc tool returns an ACK (async), the AgentLoop saves
        // a continuation snapshot. When the callback arrives, the bus loop
        // detects the cluster_continuation message and resumes the LLM session.
        {
            let cont_mgr = Arc::new(nemesis_agent::ContinuationManager::with_disk_store(
                &home.join("workspace"),
            ));
            agent_loop.set_continuation_manager(cont_mgr);
            info!("ContinuationManager injected into AgentLoop (with disk persistence)");
        }

        // Keep cluster alive until gateway shuts down
        std::mem::forget(cluster);
    } else {
        info!("Cluster disabled in configuration");
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

        let auditor = plugin.auditor();
        agent_loop.set_security_plugin(plugin.clone());
        info!("Security plugin enabled and injected into agent loop");

        // Step 9c: Initialize scanner chain from config.scanner.json
        // Mirrors Go's initScannerChain() which calls LoadFromConfig() + chain.Start()
        let scanner_config_path = common::scanner_config_path(&home);
        if scanner_config_path.exists() {
            if let Some(full_config) = load_scanner_full_config(&scanner_config_path) {
                if !full_config.enabled.is_empty() {
                    info!("Initializing scanner chain from config...");
                    plugin.init_scanner_from_config(&full_config).await;
                }
            }
        } else {
            info!("Scanner config file not found: {}, scanner chain not initialized", scanner_config_path.display());
        }

        drop(auditor);
        Some(plugin)
    } else {
        info!("Security plugin disabled by configuration");
        None
    };

    // Step 10: Create WebServer
    let web_host = {
        let h = &cfg.channels.web.host;
        if h == "0.0.0.0" || h.is_empty() { "127.0.0.1".to_string() } else { h.clone() }
    };
    let web_port = cfg.channels.web.port;

    let static_dir = crate::embedded::resolve_embedded_static_dir();
    let web_config = nemesis_web::server::WebServerConfig {
        listen_addr: format!("{}:{}", web_host, web_port),
        auth_token: cfg.channels.web.auth_token.clone(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: Some(home.join("workspace").to_string_lossy().to_string()),
        version: crate::common::VERSION_INFO.version.to_string(),
        static_dir: static_dir.clone(),
        index_file: "index.html".to_string(),
    };
    let mut web_server = nemesis_web::server::WebServer::new(web_config);
    web_server.set_message_bus(bus.clone());
    web_server.set_model_name(&model_name);
    if let Some(dir) = static_dir {
        web_server.set_workspace(std::path::PathBuf::from(dir));
    }

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
        info!("SSE streaming provider configured for /api/chat/stream");
    }

    info!("Web server created for {}:{}", web_host, web_port);

    // Step 11: Create HealthServer
    let health_port = cfg.gateway.port;
    let health_config = nemesis_health::server::HealthServerConfig {
        listen_addr: format!("{}:{}", &cfg.gateway.host, health_port),
        version: Some(crate::common::VERSION_INFO.version.to_string()),
    };
    let health_server = Arc::new(nemesis_health::server::HealthServer::new(health_config));
    info!("Health server created for {}:{}", &cfg.gateway.host, health_port);

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
    info!("Heartbeat service created (enabled: {})", cfg.heartbeat.enabled);

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
    }

    // Step 14: Start basic services
    svc_mgr.start_basic_services().map_err(|e| {
        anyhow::anyhow!("Error starting basic services: {}", e)
    })?;

    // Step 15: Print agent startup info
    print_agent_startup_info(&home, agent_loop.tool_count());

    // Step 16: Start outbound dispatch (bus outbound → WebSocket sessions)
    let dispatch_bus = bus.clone();
    let dispatch_session_mgr = web_server.session_manager().clone();
    let dispatch_handle = tokio::spawn(async move {
        nemesis_web::server::dispatch_outbound(dispatch_bus, dispatch_session_mgr).await;
    });
    info!("Outbound dispatch started");

    // Step 17: Start WebServer in background
    let web_shutdown_rx = svc_mgr.subscribe_shutdown();
    let web_handle = tokio::spawn(async move {
        if let Err(e) = web_server.start_with_shutdown(web_shutdown_rx).await {
            error!("Web server error: {}", e);
        }
    });
    info!("Web server starting on {}:{}", web_host, web_port);

    // Give the web server a moment to bind
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Step 17: HealthServer is started by BotService (svc_mgr.start_bot() below)
    // via start_services() → services.health.start(). No separate spawn needed here.
    info!("Health server will be started by bot service on {}:{}", &cfg.gateway.host, health_port);

    // Step 18: Start AgentLoop's bus processing in background
    let agent_handle = tokio::spawn(async move {
        agent_loop.run_bus_owned(agent_inbound_rx).await
    });
    info!("Agent loop started, listening on bus");

    // Step 19: Start bot service (for state tracking)
    if let Err(e) = svc_mgr.start_bot() {
        warn!("Bot service start note: {}", e);
        // Non-fatal: the real services are already started above
    }

    // Step 20: Compute display URLs
    let web_url = format!("http://{}:{}", web_host, web_port);
    let _chat_url = format!("http://{}:{}/chat/", web_host, web_port);

    // Step 21: Print startup banner
    let enabled_channels = count_enabled_channels(&cfg);
    print_gateway_banner(
        &web_host,
        web_port,
        &cfg.channels.web.auth_token,
        enabled_channels,
        &cfg.gateway.host,
        cfg.gateway.port,
    );

    // Verify web server is listening
    let listen_addr = format!("{}:{}", web_host, web_port);
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
        warn!("ProcessManager start note: {} (non-fatal, plugin windows will use fallback)", e);
    } else {
        info!("ProcessManager started (WS server on port {})", process_manager.ws_port());
    }

    // Wire up ApprovalManager: ProcessManager → SecurityPlugin auditor
    // When a tool call triggers an "ask" rule, the auditor will call
    // request_approval_sync() which spawns an approval popup child process.
    if let Some(ref plugin) = security_plugin {
        let auditor = plugin.auditor();
        let adapter = Arc::new(ApprovalPopupAdapter::new(process_manager.clone()));
        auditor.set_approval_manager(adapter);
        info!("Approval manager wired (popup via ProcessManager)");
    }

    // Step 22: Configure system tray
    {
        use nemesis_desktop::PlatformTray;

        let mut tray = PlatformTray::new();

        let start_svc = Arc::clone(&svc_mgr);
        tray.set_on_start(Box::new(move || {
            let _ = start_svc.start_bot();
        }));

        let stop_svc = Arc::clone(&svc_mgr);
        tray.set_on_stop(Box::new(move || {
            stop_svc.shutdown();
        }));

        let pm = Arc::clone(&process_manager);
        let dashboard_url = web_url.clone();
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
        info!("System tray started");
        println!("  OK System tray started");
    }

    // Step 23: Wait for shutdown signal
    svc_mgr.wait_for_shutdown().await;

    // Step 24: Graceful shutdown
    println!();
    println!("Shutting down...");
    svc_mgr.shutdown();

    // Stop ProcessManager (terminates all child processes)
    if let Err(e) = process_manager.stop() {
        warn!("ProcessManager stop note: {}", e);
    }

    // Close the message bus
    bus.close();

    // Abort background tasks
    web_handle.abort();
    agent_handle.abort();
    bridge_inbound_handle.abort();
    bridge_outbound_handle.abort();
    dispatch_handle.abort();

    // Clean up PID file
    let _ = std::fs::remove_file(home.join("gateway.pid"));

    println!("  OK Gateway stopped");

    Ok(())
}
