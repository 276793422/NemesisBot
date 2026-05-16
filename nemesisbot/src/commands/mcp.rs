//! MCP command - manage MCP (Model Context Protocol) servers.
//!
//! Provides full client connectivity via nemesis_mcp crate for
//! listing tools/resources/prompts and testing server connections.

use anyhow::Result;
use crate::common;
use nemesis_mcp::client::Client;

#[derive(clap::Subcommand)]
pub enum McpAction {
    /// List configured MCP servers
    List,
    /// Add a new MCP server
    Add {
        /// Server name
        #[arg(short, long)]
        name: String,
        /// Command to start server
        #[arg(short, long)]
        command: String,
        /// Arguments for command (comma-separated)
        #[arg(short, long)]
        args: Option<String>,
        /// Environment variables (KEY=VALUE)
        #[arg(short, long)]
        env: Vec<String>,
        /// Timeout in seconds
        #[arg(short, long, default_value_t = 30)]
        timeout: u64,
    },
    /// Remove a MCP server
    Remove {
        /// Server name
        name: String,
    },
    /// Test a MCP server connection
    Test {
        /// Server name
        name: String,
    },
    /// Inspect MCP server details
    Inspect {
        /// Server name
        name: String,
    },
    /// List available tools from a server
    Tools {
        /// Server name
        name: String,
    },
    /// List available resources from a server
    Resources {
        /// Server name
        name: String,
    },
    /// List available prompts from a server
    Prompts {
        /// Server name
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_server(mcp_cfg_path: &std::path::Path, name: &str) -> Result<Option<serde_json::Value>> {
    if !mcp_cfg_path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(mcp_cfg_path)?;
    let cfg: serde_json::Value = serde_json::from_str(&data)?;
    if let Some(servers) = cfg.get("servers").and_then(|v| v.as_array()) {
        for s in servers {
            if s.get("name").and_then(|v| v.as_str()) == Some(name) {
                return Ok(Some(s.clone()));
            }
        }
    }
    Ok(None)
}

/// Build a ServerConfig from the JSON stored in mcp config.
fn json_to_server_config(server: &serde_json::Value) -> nemesis_mcp::types::ServerConfig {
    let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let command = server.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let args: Vec<String> = server.get("args")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let env: Option<Vec<String>> = server.get("env")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        });

    let timeout = server.get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(30);

    nemesis_mcp::types::ServerConfig {
        name,
        command,
        args,
        env,
        timeout_secs: timeout,
    }
}

/// Connect to an MCP server, initialize, and return a client for use.
async fn connect_to_server(server: &serde_json::Value) -> Result<nemesis_mcp::client::McpClient> {
    let config = json_to_server_config(server);
    let mut client = nemesis_mcp::client::McpClient::from_config(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create MCP client: {}", e))?;
    client.initialize().await
        .map_err(|e| anyhow::anyhow!("Failed to initialize MCP connection: {}", e))?;
    Ok(client)
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_list(mcp_cfg_path: &std::path::Path) -> Result<()> {
    // Check if MCP is explicitly disabled (even if config file doesn't exist yet)
    if let Ok(data) = std::fs::read_to_string(mcp_cfg_path) {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
            if cfg.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                println!("MCP is disabled in config.");
                return Ok(());
            }
        }
    }

    if mcp_cfg_path.exists() {
        let data = std::fs::read_to_string(mcp_cfg_path)?;
        let cfg: serde_json::Value = serde_json::from_str(&data)?;

        let enabled = cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);

        if let Some(servers) = cfg.get("servers").and_then(|v| v.as_array()) {
            println!("Configured MCP Servers ({}):", servers.len());
            println!("-------------------------");

            if servers.is_empty() {
                println!("  No servers configured.");
            } else {
                for server in servers {
                    let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let command = server.get("command").and_then(|v| v.as_str()).unwrap_or("?");
                    let args = server.get("args").and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" "))
                        .unwrap_or_default();
                    let timeout = server.get("timeout").and_then(|v| v.as_u64()).unwrap_or(0);
                    let env_count = server.get("env").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

                    println!("  {}", name);
                    println!("    Command: {} {}", command, args);
                    if timeout > 0 {
                        println!("    Timeout: {} seconds", timeout);
                    }
                    if env_count > 0 {
                        println!("    Environment: {} variable(s)", env_count);
                    }
                    println!();
                }
            }
            println!("  MCP enabled: {}", enabled);
        } else {
            println!("Configured MCP Servers (0):");
            println!("  No servers configured.");
        }
    } else {
        println!("Configured MCP Servers (0):");
        println!("  No MCP configuration found.");
        println!("  Add a server with: nemesisbot mcp add -n <name> -c <command>");
    }
    Ok(())
}

fn cmd_add(mcp_cfg_path: &std::path::Path, name: &str, command: &str, args: Option<&str>, env: &[String], timeout: u64) -> Result<()> {
    let dir = mcp_cfg_path.parent().unwrap();
    let _ = std::fs::create_dir_all(dir);

    let mut cfg = if mcp_cfg_path.exists() {
        serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(mcp_cfg_path)?)?
    } else {
        serde_json::json!({"enabled": true, "servers": []})
    };

    // Check for duplicate
    if let Some(servers) = cfg.get("servers").and_then(|v| v.as_array()) {
        for s in servers {
            if s.get("name").and_then(|v| v.as_str()) == Some(name) {
                println!("Error: Server '{}' already exists.", name);
                println!("Remove it first: nemesisbot mcp remove {}", name);
                return Ok(());
            }
        }
    }

    let args_array: Vec<String> = args
        .map(|a| a.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let server = serde_json::json!({
        "name": name,
        "command": command,
        "args": args_array,
        "env": env,
        "timeout": timeout,
    });

    if let Some(servers) = cfg.get_mut("servers").and_then(|v| v.as_array_mut()) {
        servers.push(server);
    }
    cfg["enabled"] = serde_json::Value::Bool(true);

    std::fs::write(mcp_cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
    println!("🔌 MCP server '{}' added.", name);
    println!("Configuration saved to: {}", mcp_cfg_path.display());
    println!();
    println!("Next steps:");
    println!("  1. Test the connection: nemesisbot mcp test {}", name);
    println!("  2. List tools: nemesisbot mcp tools {}", name);
    Ok(())
}

fn cmd_remove(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    if mcp_cfg_path.exists() {
        let mut cfg: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(mcp_cfg_path)?)?;
        let mut found = false;
        if let Some(servers) = cfg.get_mut("servers").and_then(|v| v.as_array_mut()) {
            let before = servers.len();
            servers.retain(|s| s.get("name").and_then(|v| v.as_str()) != Some(name));
            found = servers.len() < before;
        }
        if found {
            std::fs::write(mcp_cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
            println!("MCP server '{}' removed.", name);
            println!("Restart agent/gateway to apply changes.");
        } else {
            println!("Server '{}' not found.", name);
        }
    } else {
        println!("Server '{}' not found.", name);
    }
    Ok(())
}

async fn cmd_test(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    println!("🔌 Testing MCP server '{}'...", name);

    let server = match find_server(mcp_cfg_path, name)? {
        Some(s) => s,
        None => {
            println!("  Server '{}' not found in configuration.", name);
            return Ok(());
        }
    };

    let command = server.get("command").and_then(|v| v.as_str()).unwrap_or("?");
    println!("  Command: {}", command);

    // Check if command exists
    if which::which(command).is_ok() {
        println!("  Command found in PATH: OK");
    } else {
        println!("  Command NOT found in PATH.");
        println!("  Skipping connection test.");
        return Ok(());
    }

    println!("  Connecting...");
    match connect_to_server(&server).await {
        Ok(mut client) => {
            println!("✅ Connection: OK");

            if let Some(info) = client.server_info() {
                println!("  Server: {} v{}", info.name, info.version);
            }

            // Try listing tools
            match client.list_tools().await {
                Ok(tools) => println!("  Tools: {} available", tools.len()),
                Err(e) => println!("  Tools: error - {}", e),
            }

            client.close().await.map_err(|e| anyhow::anyhow!("close error: {}", e))?;
            println!("  Disconnected: OK");
            println!();
            println!("✅ Test passed.");
        }
        Err(e) => {
            println!("❌ Connection: FAILED");
            println!("  Error: {}", e);
        }
    }
    Ok(())
}

async fn cmd_tools(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    println!("Fetching tools from MCP server '{}'...", name);

    let server = match find_server(mcp_cfg_path, name)? {
        Some(s) => s,
        None => {
            println!("  Server '{}' not found.", name);
            return Ok(());
        }
    };

    let mut client = connect_to_server(&server).await?;
    let tools = client.list_tools().await.map_err(|e| anyhow::anyhow!("list_tools failed: {}", e))?;

    if tools.is_empty() {
        println!("  No tools available.");
    } else {
        println!();
        println!("Found {} tool(s):", tools.len());
        println!("-------------------");
        for (i, tool) in tools.iter().enumerate() {
            let desc = tool.description.as_deref().unwrap_or("(no description)");
            println!("{}. {}", i + 1, tool.name);
            println!("   Description: {}", desc);

            // Extract parameters from input_schema
            if let Some(properties) = tool.input_schema.get("properties").and_then(|v| v.as_object()) {
                let required: Vec<&str> = tool.input_schema.get("required")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let param_names: Vec<String> = properties.keys().map(|k| {
                    if required.contains(&k.as_str()) {
                        format!("{}*", k)
                    } else {
                        k.clone()
                    }
                }).collect();

                if !param_names.is_empty() {
                    println!("   Parameters: {}", param_names.join(", "));
                }
            }
        }
    }

    client.close().await.map_err(|e| anyhow::anyhow!("close error: {}", e))?;
    Ok(())
}

async fn cmd_resources(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    println!("Fetching resources from MCP server '{}'...", name);

    let server = match find_server(mcp_cfg_path, name)? {
        Some(s) => s,
        None => {
            println!("  Server '{}' not found.", name);
            return Ok(());
        }
    };

    let mut client = connect_to_server(&server).await?;
    let resources = client.list_resources().await.map_err(|e| anyhow::anyhow!("list_resources failed: {}", e))?;

    if resources.is_empty() {
        println!("  No resources available.");
    } else {
        println!();
        println!("Found {} resource(s):", resources.len());
        println!("-------------------");
        for (i, res) in resources.iter().enumerate() {
            println!("{}. {}", i + 1, res.name);
            println!("   URI: {}", res.uri);
            if let Some(desc) = res.description.as_deref() {
                if !desc.is_empty() {
                    println!("   Description: {}", desc);
                }
            }
            if let Some(mime) = res.mime_type.as_deref() {
                if !mime.is_empty() {
                    println!("   MIME Type: {}", mime);
                }
            }
        }
    }

    client.close().await.map_err(|e| anyhow::anyhow!("close error: {}", e))?;
    Ok(())
}

async fn cmd_prompts(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    println!("Fetching prompts from MCP server '{}'...", name);

    let server = match find_server(mcp_cfg_path, name)? {
        Some(s) => s,
        None => {
            println!("  Server '{}' not found.", name);
            return Ok(());
        }
    };

    let mut client = connect_to_server(&server).await?;
    let prompts = client.list_prompts().await.map_err(|e| anyhow::anyhow!("list_prompts failed: {}", e))?;

    if prompts.is_empty() {
        println!("  No prompts available.");
    } else {
        println!();
        println!("Found {} prompt(s):", prompts.len());
        println!("-------------------");
        for (i, p) in prompts.iter().enumerate() {
            println!("{}. {}", i + 1, p.name);
            if let Some(desc) = p.description.as_deref() {
                if !desc.is_empty() {
                    println!("   Description: {}", desc);
                }
            }
            if !p.arguments.is_empty() {
                println!("   Arguments:");
                for arg in &p.arguments {
                    let required_marker = if arg.required.unwrap_or(false) { "*" } else { "" };
                    if let Some(arg_desc) = arg.description.as_deref() {
                        if !arg_desc.is_empty() {
                            println!("     - {}{}: {}", arg.name, required_marker, arg_desc);
                        } else {
                            println!("     - {}{}", arg.name, required_marker);
                        }
                    } else {
                        println!("     - {}{}", arg.name, required_marker);
                    }
                }
                println!("   (* = required)");
            }
        }
    }

    client.close().await.map_err(|e| anyhow::anyhow!("close error: {}", e))?;
    Ok(())
}

fn cmd_inspect(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    println!("Inspecting MCP server '{}'...", name);
    if let Some(server) = find_server(mcp_cfg_path, name)? {
        println!("{}", serde_json::to_string_pretty(&server).unwrap_or_default());
    } else {
        println!("  Server '{}' not found.", name);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub fn run(action: McpAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let mcp_cfg_path = common::mcp_config_path(&home);

    match action {
        McpAction::List => cmd_list(&mcp_cfg_path)?,
        McpAction::Add { name, command, args, env, timeout } => {
            cmd_add(&mcp_cfg_path, &name, &command, args.as_deref(), &env, timeout)?
        }
        McpAction::Remove { name } => cmd_remove(&mcp_cfg_path, &name)?,
        McpAction::Test { name } => {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_test(&mcp_cfg_path, &name))
            })?;
            result
        }
        McpAction::Inspect { name } => cmd_inspect(&mcp_cfg_path, &name)?,
        McpAction::Tools { name } => {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_tools(&mcp_cfg_path, &name))
            })?;
            result
        }
        McpAction::Resources { name } => {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_resources(&mcp_cfg_path, &name))
            })?;
            result
        }
        McpAction::Prompts { name } => {
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_prompts(&mcp_cfg_path, &name))
            })?;
            result
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_mcp_config(tmp: &TempDir) -> std::path::PathBuf {
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [
                {
                    "name": "test-server",
                    "command": "node",
                    "args": ["server.js"],
                    "env": ["KEY=value"],
                    "timeout": 30
                }
            ]
        });
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg_path
    }

    fn make_empty_mcp_config(tmp: &TempDir) -> std::path::PathBuf {
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.mcp.json");
        let config = serde_json::json!({"enabled": true, "servers": []});
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cfg_path
    }

    #[test]
    fn test_find_server_found() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        let server = find_server(&cfg, "test-server").unwrap();
        assert!(server.is_some());
        assert_eq!(server.unwrap()["command"], "node");
    }

    #[test]
    fn test_find_server_not_found() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        let server = find_server(&cfg, "nonexistent").unwrap();
        assert!(server.is_none());
    }

    #[test]
    fn test_find_server_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        let server = find_server(&cfg, "test").unwrap();
        assert!(server.is_none());
    }

    #[test]
    fn test_json_to_server_config_full() {
        let json = serde_json::json!({
            "name": "my-server",
            "command": "python",
            "args": ["-m", "server"],
            "env": ["API_KEY=secret"],
            "timeout": 60
        });
        let config = json_to_server_config(&json);
        assert_eq!(config.name, "my-server");
        assert_eq!(config.command, "python");
        assert_eq!(config.args, vec!["-m", "server"]);
        assert_eq!(config.env, Some(vec!["API_KEY=secret".to_string()]));
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn test_json_to_server_config_minimal() {
        let json = serde_json::json!({"name": "minimal", "command": "echo"});
        let config = json_to_server_config(&json);
        assert_eq!(config.name, "minimal");
        assert_eq!(config.command, "echo");
        assert!(config.args.is_empty());
        assert!(config.env.is_none());
        assert_eq!(config.timeout_secs, 30); // default
    }

    #[test]
    fn test_cmd_add_new_server() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);

        cmd_add(&cfg, "new-server", "python", Some("-m,server"), &[], 30).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "new-server");
        assert_eq!(servers[0]["command"], "python");
    }

    #[test]
    fn test_cmd_add_duplicate_server() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);

        // Should succeed but not add duplicate
        cmd_add(&cfg, "test-server", "node", None, &[], 30).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1); // still just one
    }

    #[test]
    fn test_cmd_add_creates_new_config() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");

        cmd_add(&cfg, "fresh-server", "npx", Some("some,mcp"), &["KEY=val".to_string()], 60).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["enabled"], true);
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers[0]["name"], "fresh-server");
        assert_eq!(servers[0]["timeout"], 60);
    }

    #[test]
    fn test_cmd_remove_existing_server() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);

        cmd_remove(&cfg, "test-server").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert!(servers.is_empty());
    }

    #[test]
    fn test_cmd_remove_nonexistent_server() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);

        cmd_remove(&cfg, "nonexistent").unwrap();
        // Should succeed without error, server count unchanged
        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["servers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_cmd_remove_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        cmd_remove(&cfg, "test").unwrap();
    }

    #[test]
    fn test_cmd_list_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        cmd_list(&cfg).unwrap();
    }

    #[test]
    fn test_cmd_list_disabled() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.mcp.json");
        std::fs::write(&cfg, r#"{"enabled": false, "servers": []}"#).unwrap();
        cmd_list(&cfg).unwrap();
    }

    #[test]
    fn test_cmd_list_with_servers() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        cmd_list(&cfg).unwrap();
    }

    #[test]
    fn test_cmd_inspect_found() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        cmd_inspect(&cfg, "test-server").unwrap();
    }

    #[test]
    fn test_cmd_inspect_not_found() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        cmd_inspect(&cfg, "nonexistent").unwrap();
    }

    #[test]
    fn test_cmd_add_args_parsing() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);

        cmd_add(&cfg, "test", "cmd", Some("arg1,arg2,arg3"), &[], 30).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let args = data["servers"][0]["args"].as_array().unwrap();
        assert_eq!(args.len(), 3);
        assert_eq!(args[0], "arg1");
        assert_eq!(args[2], "arg3");
    }

    // -------------------------------------------------------------------------
    // json_to_server_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_json_to_server_config_empty_args() {
        let json = serde_json::json!({
            "name": "test",
            "command": "echo",
            "args": []
        });
        let config = json_to_server_config(&json);
        assert!(config.args.is_empty());
    }

    #[test]
    fn test_json_to_server_config_empty_env() {
        let json = serde_json::json!({
            "name": "test",
            "command": "echo",
            "env": []
        });
        let config = json_to_server_config(&json);
        assert!(config.env.is_some());
        assert!(config.env.as_ref().unwrap().is_empty());
    }

    #[test]
    fn test_json_to_server_config_multiple_env() {
        let json = serde_json::json!({
            "name": "test",
            "command": "python",
            "env": ["KEY1=val1", "KEY2=val2", "KEY3=val3"]
        });
        let config = json_to_server_config(&json);
        assert_eq!(config.env.unwrap().len(), 3);
    }

    #[test]
    fn test_json_to_server_config_zero_timeout() {
        let json = serde_json::json!({
            "name": "test",
            "command": "echo",
            "timeout": 0
        });
        let config = json_to_server_config(&json);
        assert_eq!(config.timeout_secs, 0);
    }

    #[test]
    fn test_json_to_server_config_no_name_or_command() {
        let json = serde_json::json!({});
        let config = json_to_server_config(&json);
        assert!(config.name.is_empty());
        assert!(config.command.is_empty());
    }

    // -------------------------------------------------------------------------
    // find_server edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_find_server_empty_servers() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.mcp.json");
        std::fs::write(&cfg_path, r#"{"enabled": true, "servers": []}"#).unwrap();

        let result = find_server(&cfg_path, "anything").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_find_server_invalid_json() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        std::fs::write(&cfg_path, "not valid json").unwrap();

        let result = find_server(&cfg_path, "test");
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------------
    // cmd_add with environment variables
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_add_with_env_vars() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);

        let env = vec!["API_KEY=secret123".to_string(), "DEBUG=true".to_string()];
        cmd_add(&cfg, "env-server", "python", None, &env, 60).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let server = &data["servers"][0];
        assert_eq!(server["name"], "env-server");
        let env_arr = server["env"].as_array().unwrap();
        assert_eq!(env_arr.len(), 2);
        assert_eq!(env_arr[0], "API_KEY=secret123");
    }

    // -------------------------------------------------------------------------
    // cmd_list with empty config
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_list_empty_servers() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);
        cmd_list(&cfg).unwrap();
    }

    #[test]
    fn test_cmd_list_with_timeout_and_env() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [
                {
                    "name": "full-server",
                    "command": "python",
                    "args": ["-m", "server"],
                    "env": ["KEY=val"],
                    "timeout": 60
                }
            ]
        });
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
        cmd_list(&cfg_path).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_inspect edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_inspect_no_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("nonexistent.json");
        cmd_inspect(&cfg, "anything").unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_remove edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_remove_preserves_other_servers() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.mcp.json");
        let config = serde_json::json!({
            "enabled": true,
            "servers": [
                {"name": "keep-me", "command": "echo"},
                {"name": "remove-me", "command": "rm"}
            ]
        });
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        cmd_remove(&cfg_path, "remove-me").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "keep-me");
    }

    // -------------------------------------------------------------------------
    // Additional coverage tests for mcp
    // -------------------------------------------------------------------------

    #[test]
    fn test_mcp_config_read_no_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.mcp.json");
        cmd_list(&path).unwrap();
    }

    #[test]
    fn test_mcp_config_invalid_json_find() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.mcp.json");
        std::fs::write(&path, "bad json").unwrap();
        let result = find_server(&path, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_mcp_config_save_and_read() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("nested").join("config");
        let path = dir.join("config.mcp.json");

        let config = serde_json::json!({
            "enabled": true,
            "servers": [
                {"name": "server1", "command": "cmd1"},
                {"name": "server2", "command": "cmd2", "args": ["--flag"]}
            ]
        });
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let loaded: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded["enabled"], true);
        assert_eq!(loaded["servers"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_mcp_cmd_list_empty_config() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);
        cmd_list(&cfg).unwrap();
    }

    #[test]
    fn test_mcp_cmd_add_with_env_vars() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);

        cmd_add(&cfg, "env-server", "cmd", None, &["KEY=VALUE".to_string()], 30).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "env-server");
    }

    #[test]
    fn test_mcp_cmd_add_with_args_and_env() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_empty_mcp_config(&tmp);

        cmd_add(&cfg, "full-server", "cmd", Some("a,b"), &["K=V".to_string()], 60).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let server = &data["servers"][0];
        assert_eq!(server["args"].as_array().unwrap().len(), 2);
        assert_eq!(server["timeout"], 60);
    }

    #[test]
    fn test_mcp_cmd_remove_nonexistent_v2() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);

        cmd_remove(&cfg, "nonexistent").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let servers = data["servers"].as_array().unwrap();
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn test_mcp_cmd_remove_all() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);

        cmd_remove(&cfg, "test-server").unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert!(data["servers"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_mcp_json_to_server_config_all_fields() {
        let json = serde_json::json!({
            "name": "full",
            "command": "python",
            "args": ["-m", "server"],
            "env": ["KEY=val"],
            "timeout": 120
        });
        let config = json_to_server_config(&json);
        assert_eq!(config.name, "full");
        assert_eq!(config.command, "python");
        assert_eq!(config.args.len(), 2);
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_mcp_find_server_found_v2() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        let result = find_server(&cfg, "test-server").unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_mcp_find_server_not_found_v2() {
        let tmp = TempDir::new().unwrap();
        let cfg = make_mcp_config(&tmp);
        let result = find_server(&cfg, "other-server").unwrap();
        assert!(result.is_none());
    }
}
