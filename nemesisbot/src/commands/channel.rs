//! Channel command - manage communication channels.

use anyhow::Result;
use crate::common;

const KNOWN_CHANNELS: &[&str] = &[
    "web", "websocket", "telegram", "discord", "whatsapp",
    "feishu", "slack", "line", "onebot", "qq", "dingtalk",
    "maixcam", "external",
];

#[derive(clap::Subcommand)]
pub enum ChannelAction {
    /// List all channels and their status
    List,
    /// Enable a channel
    Enable {
        /// Channel name
        name: String,
    },
    /// Disable a channel
    Disable {
        /// Channel name
        name: String,
    },
    /// Show channel detailed status
    Status {
        /// Channel name
        name: String,
    },
    /// Web channel specific commands
    Web {
        #[command(subcommand)]
        action: WebAction,
    },
    /// WebSocket channel specific commands
    WebSocket {
        #[command(subcommand)]
        action: WebSocketAction,
    },
    /// External channel specific commands
    External {
        #[command(subcommand)]
        action: ExternalAction,
    },
}

#[derive(clap::Subcommand)]
pub enum WebAction {
    /// Set authentication token interactively
    Auth,
    /// Set authentication token directly
    AuthSet { token: String },
    /// Show current token (masked)
    AuthGet,
    /// Set web server host
    Host { host: String },
    /// Set web server port
    Port { port: u16 },
    /// Show web channel status
    Status,
    /// Clear authentication token
    Clear,
    /// Show detailed configuration
    Config,
}

#[derive(clap::Subcommand)]
pub enum WebSocketAction {
    /// Interactive setup
    Setup,
    /// Show configuration
    Config,
    /// Set a configuration parameter
    Set {
        /// Parameter name (host, port, path, token, sync, session)
        key: String,
        /// Parameter value
        value: String,
    },
    /// Get a configuration parameter
    Get {
        /// Parameter name
        key: String,
    },
}

#[derive(clap::Subcommand)]
pub enum ExternalAction {
    /// Interactive setup
    Setup,
    /// Show configuration
    Config,
    /// Test external programs
    Test,
    /// Set a configuration parameter
    Set { key: String, value: String },
    /// Get a configuration parameter
    Get { key: String },
}

pub fn run(action: ChannelAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);

    match action {
        ChannelAction::List => {
            println!("NemesisBot Channel Status");
            println!("========================");
            println!();
            println!("{:<12} {:<12}", "Channel", "Status");
            println!("{}", "-".repeat(30));

            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let cfg: serde_json::Value = serde_json::from_str(&data)?;
                for ch in KNOWN_CHANNELS {
                    let enabled = cfg.get("channels")
                        .and_then(|c| c.get(*ch))
                        .and_then(|c| c.get("enabled"))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    println!("{:<12} {:<12}", ch, if enabled { "enabled" } else { "disabled" });
                }
            } else {
                for ch in KNOWN_CHANNELS {
                    println!("{:<12} {:<12}", ch, "unknown");
                }
            }
            println!();
            println!("Note: 'Running' status is only accurate when gateway is running.");
        }
        ChannelAction::Enable { name } => {
            if !KNOWN_CHANNELS.contains(&name.as_str()) {
                println!("Unknown channel: {}", name);
                println!("Available: {}", KNOWN_CHANNELS.join(", "));
                return Ok(());
            }
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                // Set channels.<name>.enabled = true
                if let Some(ch) = cfg.pointer_mut(&format!("/channels/{}", name)) {
                    if let Some(obj) = ch.as_object_mut() {
                        obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                    }
                } else if let Some(channels) = cfg.as_object_mut().and_then(|o| o.get_mut("channels")).and_then(|v| v.as_object_mut()) {
                    channels.insert(name.clone(), serde_json::json!({"enabled": true}));
                }
                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
            }
            println!("Channel '{}' enabled.", name);
            println!("Restart gateway for changes to take effect.");
        }
        ChannelAction::Disable { name } => {
            if !KNOWN_CHANNELS.contains(&name.as_str()) {
                println!("Unknown channel: {}", name);
                println!("Available: {}", KNOWN_CHANNELS.join(", "));
                return Ok(());
            }
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(ch) = cfg.pointer_mut(&format!("/channels/{}", name)) {
                    if let Some(obj) = ch.as_object_mut() {
                        obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
                    }
                }
                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
            }
            println!("Channel '{}' disabled.", name);
        }
        ChannelAction::Status { name } => {
            println!("  {} Channel Status", name);
            println!("  {}", "=".repeat(30));
            if cfg_path.exists() {
                let data = std::fs::read_to_string(&cfg_path)?;
                let cfg: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(ch) = cfg.get("channels").and_then(|c| c.get(&name)) {
                    let enabled = ch.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    match name.as_str() {
                        "web" => {
                            let host = ch.get("host").and_then(|v| v.as_str()).unwrap_or("0.0.0.0");
                            let port = ch.get("port").and_then(|v| v.as_u64()).unwrap_or(8080);
                            let auth = ch.get("auth_token").and_then(|v| v.as_str()).unwrap_or("");
                            let has_auth = !auth.is_empty();
                            println!("  Enabled: {}", enabled);
                            println!("  Host: {}", host);
                            println!("  Port: {}", port);
                            println!("  Authentication: {}", if has_auth { "enabled" } else { "disabled" });
                            if has_auth {
                                let last4 = if auth.len() > 4 { &auth[auth.len()-4..] } else { auth };
                                println!("  Auth Token: ****{} (length: {})", last4, auth.len());
                            }
                            println!("  Access URL: http://{}:{}", host, port);
                        }
                        "websocket" => {
                            let host = ch.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1");
                            let port = ch.get("port").and_then(|v| v.as_u64()).unwrap_or(49001);
                            let path = ch.get("path").and_then(|v| v.as_str()).unwrap_or("/ws");
                            let auth = ch.get("auth_token").and_then(|v| v.as_str()).unwrap_or("");
                            let has_auth = !auth.is_empty();
                            println!("  Enabled: {}", enabled);
                            println!("  Host: {}", host);
                            println!("  Port: {}", port);
                            println!("  Path: {}", path);
                            println!("  Authentication: {}", if has_auth { "enabled" } else { "disabled" });
                            println!("  Connection URL: ws://{}:{}{}", host, port, path);
                        }
                        _ => {
                            println!("  Enabled: {}", enabled);
                            if let Some(obj) = ch.as_object() {
                                for (k, v) in obj {
                                    if k != "enabled" {
                                        println!("  {}: {}", k, v);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    println!("  Not configured.");
                }
            } else {
                println!("  No configuration found.");
            }
        }
        ChannelAction::Web { action } => {
            match action {
                WebAction::Auth => {
                    use std::io::{self, Write};
                    println!("Set Web Authentication Token");
                    println!("  Note: The auth token protects access to the Web UI.");
                    print!("Token: ");
                    io::stdout().flush().ok();
                    let mut token = String::new();
                    io::stdin().read_line(&mut token).ok();
                    let token = token.trim().to_string();
                    if token.is_empty() {
                        println!("  Error: Token cannot be empty.");
                        return Ok(());
                    }
                    if token.len() < 8 {
                        println!("  Warning: Token is short (less than 8 characters). Consider using a longer token.");
                    }
                    print!("  Save this token? (y/N): ");
                    io::stdout().flush().ok();
                    let mut answer = String::new();
                    io::stdin().read_line(&mut answer).ok();
                    if answer.trim().to_lowercase() != "y" {
                        println!("  Cancelled.");
                        return Ok(());
                    }
                    set_channel_config(&cfg_path, "web", "auth_token", &token)?;
                    println!("  Web auth token set: {}****",
                        if token.len() > 4 { &token[..4] } else { "***" });
                }
                WebAction::AuthSet { token } => {
                    if token.is_empty() {
                        println!("  Error: Token cannot be empty.");
                        return Ok(());
                    }
                    if token.len() < 8 {
                        println!("  Warning: Token is short. Consider using a longer token for better security.");
                    }
                    set_channel_config(&cfg_path, "web", "auth_token", &token)?;
                    println!("  Web auth token set: {}****",
                        if token.len() > 4 { &token[..4] } else { "***" });
                }
                WebAction::AuthGet => {
                    let masked = get_channel_config(&cfg_path, "web", "auth_token")
                        .map(|v| common::format_token(&v))
                        .unwrap_or_else(|| "(not set)".to_string());
                    println!("Web auth token: {}", masked);
                }
                WebAction::Host { host } => {
                    set_channel_config(&cfg_path, "web", "host", &host)?;
                    println!("Web host set to: {}", host);
                }
                WebAction::Port { port } => {
                    set_channel_config(&cfg_path, "web", "port", &port.to_string())?;
                    println!("Web port set to: {}", port);
                }
                WebAction::Status => {
                    println!("  Web Channel Status");
                    println!("  ==================");
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let cfg: serde_json::Value = serde_json::from_str(&data)?;
                        if let Some(web) = cfg.get("channels").and_then(|c| c.get("web")) {
                            let enabled = web.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                            let host = web.get("host").and_then(|v| v.as_str()).unwrap_or("0.0.0.0");
                            let port = web.get("port").and_then(|v| v.as_u64()).unwrap_or(8080);
                            let auth = web.get("auth_token").and_then(|v| v.as_str()).unwrap_or("");
                            let ws_path = web.get("path").and_then(|v| v.as_str()).unwrap_or("/ws");
                            let has_auth = !auth.is_empty();
                            println!("  Enabled: {}", enabled);
                            println!("  Host: {}", host);
                            println!("  Port: {}", port);
                            println!("  Authentication: {}", if has_auth { "enabled" } else { "disabled" });
                            if has_auth {
                                let last4 = if auth.len() > 4 { &auth[auth.len()-4..] } else { auth };
                                println!("  Auth Token: ****{} (length: {})", last4, auth.len());
                            }
                            println!("  WebSocket Path: {}", ws_path);
                            println!("  Access URL: http://{}:{}", host, port);
                        } else {
                            println!("  (not configured)");
                        }
                    } else {
                        println!("  No configuration found.");
                    }
                }
                WebAction::Clear => {
                    use std::io::{self, Write};
                    println!("  WARNING: Clearing the auth token means anyone can access the Web UI.");
                    print!("  Continue? (y/N): ");
                    io::stdout().flush().ok();
                    let mut answer = String::new();
                    io::stdin().read_line(&mut answer).ok();
                    if answer.trim().to_lowercase() != "y" {
                        println!("  Cancelled.");
                        return Ok(());
                    }
                    remove_channel_config(&cfg_path, "web", "auth_token")?;
                    println!("  Web auth token cleared.");
                }
                WebAction::Config => {
                    println!("  Web Channel Configuration");
                    println!("  =========================");
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let cfg: serde_json::Value = serde_json::from_str(&data)?;
                        if let Some(web) = cfg.get("channels").and_then(|c| c.get("web")) {
                            let enabled = web.get("enabled").and_then(|v| v.as_bool());
                            let host = web.get("host").and_then(|v| v.as_str()).unwrap_or("0.0.0.0");
                            let port = web.get("port").and_then(|v| v.as_u64()).unwrap_or(8080);
                            let auth = web.get("auth_token").and_then(|v| v.as_str()).unwrap_or("");
                            let ws_path = web.get("path").and_then(|v| v.as_str()).unwrap_or("/ws");
                            let tls_cert = web.get("tls_cert").and_then(|v| v.as_str());
                            let tls_key = web.get("tls_key").and_then(|v| v.as_str());
                            let cors = web.get("cors").and_then(|v| v.as_bool());
                            let max_connections = web.get("max_connections").and_then(|v| v.as_u64());

                            println!("  Enabled: {}", enabled.map(|b| b.to_string()).unwrap_or("(not set)".to_string()));
                            println!("  Host: {}", host);
                            println!("  Port: {}", port);
                            println!("  Authentication: {}", if !auth.is_empty() { "enabled" } else { "disabled" });
                            if !auth.is_empty() {
                                let last4 = if auth.len() > 4 { &auth[auth.len()-4..] } else { auth };
                                println!("  Auth Token: ****{} (length: {})", last4, auth.len());
                            }
                            println!("  WebSocket Path: {}", ws_path);
                            println!("  Access URL: http://{}:{}", host, port);
                            if let Some(cert) = tls_cert {
                                println!("  TLS Certificate: {}", cert);
                            }
                            if let Some(key) = tls_key {
                                println!("  TLS Key: {}", key);
                            }
                            if let Some(c) = cors {
                                println!("  CORS: {}", c);
                            }
                            if let Some(mc) = max_connections {
                                println!("  Max Connections: {}", mc);
                            }
                            // Show any other fields not covered above
                            if let Some(obj) = web.as_object() {
                                let covered = ["enabled", "host", "port", "auth_token", "path", "tls_cert", "tls_key", "cors", "max_connections"];
                                for (k, v) in obj {
                                    if !covered.contains(&k.as_str()) {
                                        println!("  {}: {}", k, v);
                                    }
                                }
                            }
                        } else {
                            println!("  (not configured)");
                        }
                    } else {
                        println!("  No configuration found.");
                    }
                }
            }
        }
        ChannelAction::WebSocket { action } => {
            match action {
                WebSocketAction::Setup => {
                    use std::io::{self, Write};
                    println!("WebSocket Channel Setup");
                    println!("{}", "-".repeat(40));

                    let mut host = get_channel_config(&cfg_path, "websocket", "host")
                        .unwrap_or_else(|| "127.0.0.1".to_string());
                    let mut port = get_channel_config(&cfg_path, "websocket", "port")
                        .unwrap_or_else(|| "49001".to_string());
                    let mut path = get_channel_config(&cfg_path, "websocket", "path")
                        .unwrap_or_else(|| "/ws".to_string());
                    let mut token = get_channel_config(&cfg_path, "websocket", "auth_token")
                        .unwrap_or_default();

                    print!("Host [{}]: ", host);
                    io::stdout().flush().ok();
                    let mut input = String::new();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { host = v; }

                    print!("Port [{}]: ", port);
                    io::stdout().flush().ok();
                    input.clear();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { port = v; }

                    print!("Path [{}]: ", path);
                    io::stdout().flush().ok();
                    input.clear();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { path = v; }

                    print!("Auth token [{}]: ", if token.is_empty() { "none" } else { &token[..4.min(token.len())] });
                    io::stdout().flush().ok();
                    input.clear();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { token = v; }

                    // Save all values
                    set_channel_config(&cfg_path, "websocket", "host", &host)?;
                    set_channel_config(&cfg_path, "websocket", "port", &port)?;
                    set_channel_config(&cfg_path, "websocket", "path", &path)?;
                    if token.is_empty() {
                        remove_channel_config(&cfg_path, "websocket", "auth_token")?;
                    } else {
                        set_channel_config(&cfg_path, "websocket", "auth_token", &token)?;
                    }

                    // Enable the websocket channel
                    if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                        if let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(ws) = cfg.pointer_mut("/channels/websocket") {
                                if let Some(obj) = ws.as_object_mut() {
                                    obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                                }
                            }
                            let _ = std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
                        }
                    }

                    // Sync to Web channel question
                    print!("  Sync to Web channel? (Y/n): ");
                    io::stdout().flush().ok();
                    input.clear();
                    io::stdin().read_line(&mut input).ok();
                    let sync_answer = input.trim().to_lowercase();
                    if sync_answer != "n" {
                        // Read or generate session ID
                        let default_session = uuid_session();
                        print!("  Session ID [{}]: ", default_session);
                        io::stdout().flush().ok();
                        input.clear();
                        io::stdin().read_line(&mut input).ok();
                        let session = if input.trim().is_empty() { default_session } else { input.trim().to_string() };

                        // Sync host, port, path, and token to web channel
                        set_channel_config(&cfg_path, "web", "host", &host)?;
                        set_channel_config(&cfg_path, "web", "port", &port)?;
                        set_channel_config(&cfg_path, "web", "path", &path)?;
                        if token.is_empty() {
                            remove_channel_config(&cfg_path, "web", "auth_token")?;
                        } else {
                            set_channel_config(&cfg_path, "web", "auth_token", &token)?;
                        }
                        set_channel_config(&cfg_path, "web", "session_id", &session)?;
                        println!("  Synced to Web channel (session: {}).", session);
                    }

                    println!("WebSocket channel configured and enabled.");
                    println!("  Host: {}", host);
                    println!("  Port: {}", port);
                    println!("  Path: {}", path);
                }
                WebSocketAction::Config => {
                    println!("WebSocket Channel Configuration");
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let cfg: serde_json::Value = serde_json::from_str(&data)?;
                        if let Some(ws) = cfg.get("channels").and_then(|c| c.get("websocket")) {
                            println!("{}", serde_json::to_string_pretty(ws).unwrap_or_default());
                        } else {
                            println!("  (not configured)");
                        }
                    }
                }
                WebSocketAction::Set { key, mut value } => {
                    // Validate port range
                    if key == "port" {
                        let port: u16 = value.parse().map_err(|_| anyhow::anyhow!("Invalid port number"))?;
                        if port == 0 {
                            anyhow::bail!("Port cannot be 0");
                        }
                    }
                    // Ensure path starts with "/"
                    if key == "path" && !value.starts_with('/') {
                        value = format!("/{}", value);
                    }
                    set_channel_config(&cfg_path, "websocket", &key, &value)?;
                    println!("  Set websocket.{} = {}", key, value);
                }
                WebSocketAction::Get { key } => {
                    let val = get_channel_config(&cfg_path, "websocket", &key)
                        .unwrap_or_else(|| "(not set)".to_string());
                    println!("websocket.{} = {}", key, val);
                }
            }
        }
        ChannelAction::External { action } => {
            match action {
                ExternalAction::Setup => {
                    use std::io::{self, Write};
                    println!("External Channel Setup");
                    println!("{}", "-".repeat(40));

                    let mut input_exe = get_channel_config(&cfg_path, "external", "input_exe")
                        .unwrap_or_default();
                    let mut output_exe = get_channel_config(&cfg_path, "external", "output_exe")
                        .unwrap_or_default();
                    let mut chat_id = get_channel_config(&cfg_path, "external", "chat_id")
                        .unwrap_or_else(|| "external:main".to_string());

                    print!("Input executable [{}]: ", if input_exe.is_empty() { "none" } else { &input_exe });
                    io::stdout().flush().ok();
                    let mut input = String::new();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { input_exe = v; }

                    print!("Output executable [{}]: ", if output_exe.is_empty() { "none" } else { &output_exe });
                    io::stdout().flush().ok();
                    input.clear();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { output_exe = v; }

                    print!("Chat ID [{}]: ", chat_id);
                    io::stdout().flush().ok();
                    input.clear();
                    io::stdin().read_line(&mut input).ok();
                    let v = input.trim().to_string();
                    if !v.is_empty() { chat_id = v; }

                    // Save all values
                    if input_exe.is_empty() {
                        remove_channel_config(&cfg_path, "external", "input_exe")?;
                    } else {
                        set_channel_config(&cfg_path, "external", "input_exe", &input_exe)?;
                    }
                    if output_exe.is_empty() {
                        remove_channel_config(&cfg_path, "external", "output_exe")?;
                    } else {
                        set_channel_config(&cfg_path, "external", "output_exe", &output_exe)?;
                    }
                    set_channel_config(&cfg_path, "external", "chat_id", &chat_id)?;

                    // Enable the external channel
                    if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                        if let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                            if let Some(ext) = cfg.pointer_mut("/channels/external") {
                                if let Some(obj) = ext.as_object_mut() {
                                    obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                                }
                            }
                            let _ = std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
                        }
                    }

                    println!("External channel configured and enabled.");
                    println!("  Input exe:  {}", if input_exe.is_empty() { "(none)" } else { &input_exe });
                    println!("  Output exe: {}", if output_exe.is_empty() { "(none)" } else { &output_exe });
                    println!("  Chat ID:    {}", chat_id);
                }
                ExternalAction::Config => {
                    println!("External Channel Configuration");
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let cfg: serde_json::Value = serde_json::from_str(&data)?;
                        if let Some(ext) = cfg.get("channels").and_then(|c| c.get("external")) {
                            println!("{}", serde_json::to_string_pretty(ext).unwrap_or_default());
                        } else {
                            println!("  (not configured)");
                        }
                    }
                }
                ExternalAction::Test => {
                    println!("Testing External Channel Programs");
                    println!("{}", "-".repeat(40));

                    let input_exe = get_channel_config(&cfg_path, "external", "input_exe")
                        .unwrap_or_default();
                    let output_exe = get_channel_config(&cfg_path, "external", "output_exe")
                        .unwrap_or_default();

                    if input_exe.is_empty() && output_exe.is_empty() {
                        println!("  No external programs configured.");
                        println!("  Run 'nemesisbot channel external setup' first.");
                        return Ok(());
                    }

                    // Test input program
                    if !input_exe.is_empty() {
                        print!("  Input program ({}): ", input_exe);
                        let path = std::path::Path::new(&input_exe);
                        if path.exists() {
                            // Try to run it briefly
                            match std::process::Command::new(&input_exe)
                                .stdin(std::process::Stdio::piped())
                                .stdout(std::process::Stdio::piped())
                                .stderr(std::process::Stdio::piped())
                                .spawn()
                            {
                                Ok(mut child) => {
                                    // Kill immediately, we just wanted to verify it starts
                                    let _ = child.kill();
                                    let _ = child.wait();
                                    println!("OK (starts successfully)");
                                }
                                Err(e) => println!("FAILED ({})", e),
                            }
                        } else {
                            println!("NOT FOUND");
                        }
                    } else {
                        println!("  Input program: not configured");
                    }

                    // Test output program
                    if !output_exe.is_empty() {
                        print!("  Output program ({}): ", output_exe);
                        let path = std::path::Path::new(&output_exe);
                        if path.exists() {
                            match std::process::Command::new(&output_exe)
                                .stdout(std::process::Stdio::piped())
                                .stderr(std::process::Stdio::piped())
                                .spawn()
                            {
                                Ok(mut child) => {
                                    let _ = child.kill();
                                    let _ = child.wait();
                                    println!("OK (starts successfully)");
                                }
                                Err(e) => println!("FAILED ({})", e),
                            }
                        } else {
                            println!("NOT FOUND");
                        }
                    } else {
                        println!("  Output program: not configured");
                    }

                    println!();
                    println!("External channel test complete.");
                }
                ExternalAction::Set { key, value } => {
                    set_channel_config(&cfg_path, "external", &key, &value)?;
                    println!("Set external.{} = {}", key, value);
                }
                ExternalAction::Get { key } => {
                    let val = get_channel_config(&cfg_path, "external", &key)
                        .unwrap_or_else(|| "(not set)".to_string());
                    println!("external.{} = {}", key, val);
                }
            }
        }
    }
    Ok(())
}

/// Generate a simple session ID using timestamp and random suffix.
fn uuid_session() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    // Use low bits as a simple random-ish suffix
    let suffix = ts % 100000;
    format!("ws-{:05}", suffix)
}

/// Set a value at `channels.<channel>.<key>` in the JSON config file.
fn set_channel_config(cfg_path: &std::path::Path, channel: &str, key: &str, value: &str) -> Result<()> {
    if !cfg_path.exists() {
        anyhow::bail!("Config file not found: {}", cfg_path.display());
    }
    let data = std::fs::read_to_string(cfg_path)?;
    let mut cfg: serde_json::Value = serde_json::from_str(&data)?;

    // Ensure the path channels.<channel> exists as an object
    let channels_obj = cfg
        .as_object_mut()
        .and_then(|o| o.get_mut("channels"))
        .and_then(|v| v.as_object_mut());

    if let Some(channels) = channels_obj {
        let ch_entry = channels
            .entry(channel)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(obj) = ch_entry.as_object_mut() {
            obj.insert(key.to_string(), serde_json::Value::String(value.to_string()));
        }
    } else {
        // No "channels" key at all; create the full path
        let mut ch_map = serde_json::Map::new();
        ch_map.insert(key.to_string(), serde_json::Value::String(value.to_string()));
        let mut channels_map = serde_json::Map::new();
        channels_map.insert(channel.to_string(), serde_json::Value::Object(ch_map));
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("channels".to_string(), serde_json::Value::Object(channels_map));
        }
    }

    std::fs::write(cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
    Ok(())
}

/// Get a value from `channels.<channel>.<key>` in the JSON config file.
fn get_channel_config(cfg_path: &std::path::Path, channel: &str, key: &str) -> Option<String> {
    if !cfg_path.exists() {
        return None;
    }
    let data = std::fs::read_to_string(cfg_path).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&data).ok()?;
    cfg.get("channels")
        .and_then(|c| c.get(channel))
        .and_then(|c| c.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Remove a key from `channels.<channel>` in the JSON config file.
fn remove_channel_config(cfg_path: &std::path::Path, channel: &str, key: &str) -> Result<()> {
    if !cfg_path.exists() {
        return Ok(());
    }
    let data = std::fs::read_to_string(cfg_path)?;
    let mut cfg: serde_json::Value = serde_json::from_str(&data)?;

    if let Some(ch) = cfg.pointer_mut(&format!("/channels/{}", channel)) {
        if let Some(obj) = ch.as_object_mut() {
            obj.remove(key);
        }
    }

    std::fs::write(cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_config(tmp: &TempDir) -> std::path::PathBuf {
        let cfg_path = tmp.path().join("config.json");
        let config = serde_json::json!({
            "channels": {
                "web": {
                    "enabled": true,
                    "host": "0.0.0.0",
                    "port": 8080,
                    "auth_token": "mysecrettoken123"
                },
                "websocket": {
                    "enabled": false,
                    "host": "127.0.0.1",
                    "port": 49001,
                    "path": "/ws"
                },
                "telegram": {
                    "enabled": false
                }
            }
        });
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg_path
    }

    fn make_empty_config(tmp: &TempDir) -> std::path::PathBuf {
        let cfg_path = tmp.path().join("config.json");
        let config = serde_json::json!({"channels": {}});
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg_path
    }

    fn make_no_channels_config(tmp: &TempDir) -> std::path::PathBuf {
        let cfg_path = tmp.path().join("config.json");
        let config = serde_json::json!({});
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg_path
    }

    #[test]
    fn test_set_channel_config_existing_channel() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        set_channel_config(&cfg, "web", "host", "127.0.0.1").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["channels"]["web"]["host"], "127.0.0.1");
        // Other fields should remain
        assert_eq!(data["channels"]["web"]["port"], 8080);
    }

    #[test]
    fn test_set_channel_config_new_channel() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        set_channel_config(&cfg, "discord", "enabled", "true").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["channels"]["discord"]["enabled"], "true");
    }

    #[test]
    fn test_set_channel_config_no_channels_key() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_no_channels_config(&tmp);

        set_channel_config(&cfg, "web", "host", "0.0.0.0").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["channels"]["web"]["host"], "0.0.0.0");
    }

    #[test]
    fn test_set_channel_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");

        let result = set_channel_config(&cfg, "web", "host", "0.0.0.0");
        assert!(result.is_err());
    }

    #[test]
    fn test_get_channel_config_existing() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        let val = get_channel_config(&cfg, "web", "host");
        assert_eq!(val, Some("0.0.0.0".to_string()));
    }

    #[test]
    fn test_get_channel_config_missing_key() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        let val = get_channel_config(&cfg, "web", "nonexistent_key");
        assert!(val.is_none());
    }

    #[test]
    fn test_get_channel_config_missing_channel() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        let val = get_channel_config(&cfg, "discord", "host");
        assert!(val.is_none());
    }

    #[test]
    fn test_get_channel_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");

        let val = get_channel_config(&cfg, "web", "host");
        assert!(val.is_none());
    }

    #[test]
    fn test_remove_channel_config_existing_key() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        remove_channel_config(&cfg, "web", "auth_token").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(data["channels"]["web"].get("auth_token").is_none());
        // Other keys remain
        assert_eq!(data["channels"]["web"]["host"], "0.0.0.0");
    }

    #[test]
    fn test_remove_channel_config_nonexistent_key() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        // Should succeed even if key doesn't exist
        remove_channel_config(&cfg, "web", "nonexistent").unwrap();
    }

    #[test]
    fn test_remove_channel_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");

        // Should succeed (no-op)
        remove_channel_config(&cfg, "web", "host").unwrap();
    }

    #[test]
    fn test_uuid_session_format() {
        let session = uuid_session();
        assert!(session.starts_with("ws-"));
        assert_eq!(session.len(), 8); // "ws-" + 5 digits
    }

    #[test]
    fn test_uuid_session_numeric_suffix() {
        let session = uuid_session();
        let suffix = &session[3..];
        assert!(suffix.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn test_known_channels_contains_web() {
        assert!(KNOWN_CHANNELS.contains(&"web"));
    }

    #[test]
    fn test_known_channels_contains_telegram() {
        assert!(KNOWN_CHANNELS.contains(&"telegram"));
    }

    #[test]
    fn test_known_channels_count() {
        assert_eq!(KNOWN_CHANNELS.len(), 13);
    }

    #[test]
    fn test_set_and_get_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_config(&tmp);

        set_channel_config(&cfg, "web", "port", "9090").unwrap();
        set_channel_config(&cfg, "web", "host", "192.168.1.1").unwrap();

        assert_eq!(get_channel_config(&cfg, "web", "port"), Some("9090".to_string()));
        assert_eq!(get_channel_config(&cfg, "web", "host"), Some("192.168.1.1".to_string()));
    }

    #[test]
    fn test_set_overwrite_value() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        set_channel_config(&cfg, "web", "port", "3000").unwrap();
        assert_eq!(get_channel_config(&cfg, "web", "port"), Some("3000".to_string()));
    }

    #[test]
    fn test_set_remove_then_get() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_config(&tmp);

        // auth_token exists
        assert!(get_channel_config(&cfg, "web", "auth_token").is_some());

        remove_channel_config(&cfg, "web", "auth_token").unwrap();
        assert!(get_channel_config(&cfg, "web", "auth_token").is_none());
    }
}
