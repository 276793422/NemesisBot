//! MCP command - manage MCP (Model Context Protocol) servers.
//!
//! Provides full client connectivity via nemesis_mcp crate for
//! listing tools/resources/prompts and testing server connections.

use crate::common;
use anyhow::Result;
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
    /// Discover capabilities of an MCP server (stdio or HTTP)
    Discover {
        /// Command to start the MCP server (for stdio-based servers)
        #[arg(short, long)]
        command: Option<String>,
        /// URL of the MCP server (for HTTP-based servers, e.g. 'http://localhost:8080/mcp')
        #[arg(short, long)]
        url: Option<String>,
        /// Arguments for the command (stdio only, comma-separated)
        #[arg(short, long)]
        args: Option<String>,
        /// Timeout in seconds
        #[arg(short, long, default_value_t = 15)]
        timeout: u64,
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

/// Connect to an MCP server, initialize, and return a client for use.
///
/// 2026-08-31 单一真相源收敛：删除手搓的 `json_to_server_config`（只认
/// `timeout` 键、`env: null` 直接丢 env——与 Dashboard/MCP runtime 解析
/// 结果分叉的第三条解析路径）。现在 serde 直接解析为
/// [`nemesis_config::McpServerConfig`]：与全仓同一类型，宽容反序列化
/// （null/map 形状的 args/env、timeout/timeout_secs 双键）统一生效。
async fn connect_to_server(server: &serde_json::Value) -> Result<nemesis_mcp::client::McpClient> {
    let config: nemesis_config::McpServerConfig = serde_json::from_value(server.clone())
        .map_err(|e| anyhow::anyhow!("Invalid MCP server config: {}", e))?;
    let mut client = nemesis_mcp::client::McpClient::from_config(&config)
        .map_err(|e| anyhow::anyhow!("Failed to create MCP client: {}", e))?;
    client
        .initialize()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to initialize MCP connection: {}", e))?;
    Ok(client)
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

/// Sync the master MCP switch in the home-root config.json.
///
/// When `mcp add` adds the first server, we flip `mcp.enabled = true` in
/// config.json so that the gateway sees MCP is enabled on next start.
///
/// 2026-08-28 修复：此前从 config.mcp.json 父目录推导 config.json，得到
/// `<home>/workspace/config/config.json`——无人读取的位置，exists() 恒
/// miss，`mcp.enabled` 从不真正置位（静默 no-op）。现由调用方直传
/// `common::config_path(&home)`（真实主配置路径，唯一拼接点见
/// nemesis-path / common.rs）。
fn sync_mcp_master_switch(config_json_path: &std::path::Path, enabled: bool) -> Result<()> {
    if !config_json_path.exists() {
        return Ok(());
    }

    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_json_path)?)?;
    if cfg
        .get("mcp")
        .and_then(|m| m.get("enabled"))
        .and_then(|v| v.as_bool())
        == Some(enabled)
    {
        return Ok(()); // already in desired state
    }

    cfg["mcp"]["enabled"] = serde_json::Value::Bool(enabled);
    std::fs::write(
        config_json_path,
        serde_json::to_string_pretty(&cfg).unwrap_or_default(),
    )?;
    tracing::info!(
        "[MCP] Synced master switch: config.json mcp.enabled = {}",
        enabled
    );
    Ok(())
}

fn cmd_list(mcp_cfg_path: &std::path::Path) -> Result<()> {
    // Check if MCP is explicitly disabled (even if config file doesn't exist yet)
    if let Ok(data) = std::fs::read_to_string(mcp_cfg_path)
        && let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data)
            && cfg.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
                println!("MCP is disabled in config.");
                return Ok(());
            }

    if mcp_cfg_path.exists() {
        let data = std::fs::read_to_string(mcp_cfg_path)?;
        let cfg: serde_json::Value = serde_json::from_str(&data)?;

        let enabled = cfg
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if let Some(servers) = cfg.get("servers").and_then(|v| v.as_array()) {
            println!("Configured MCP Servers ({}):", servers.len());
            println!("-------------------------");

            if servers.is_empty() {
                println!("  No servers configured.");
            } else {
                for server in servers {
                    let name = server.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let command = server
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let args = server
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    // 2026-08-31 收敛：规范键 timeout_secs，旧文件 timeout 键兼容读
                    let timeout = server
                        .get("timeout_secs")
                        .or_else(|| server.get("timeout"))
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let env_count = server
                        .get("env")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);

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

fn cmd_add(
    mcp_cfg_path: &std::path::Path,
    config_json_path: &std::path::Path,
    name: &str,
    command: &str,
    args: Option<&str>,
    env: &[String],
    timeout: u64,
) -> Result<()> {
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
        // 2026-08-31 收敛：写规范键 timeout_secs（读取侧双键兼容）
        "timeout_secs": timeout,
    });

    // servers 数组缺失（旧 schema / 手工编辑）时补建，否则下面 push 静默
    // 跳过仍报成功 —— 用户以为已添加，实际文件里没有该服务器。
    if !cfg.get("servers").map(serde_json::Value::is_array).unwrap_or(false) {
        cfg["servers"] = serde_json::json!([]);
    }
    cfg["servers"]
        .as_array_mut()
        .expect("just normalized to array")
        .push(server);
    cfg["enabled"] = serde_json::Value::Bool(true);

    std::fs::write(
        mcp_cfg_path,
        serde_json::to_string_pretty(&cfg).unwrap_or_default(),
    )?;

    // Sync master switch in the real home-root config.json: mcp.enabled = true
    sync_mcp_master_switch(config_json_path, true)?;

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
        let mut cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(mcp_cfg_path)?)?;
        let mut found = false;
        if let Some(servers) = cfg.get_mut("servers").and_then(|v| v.as_array_mut()) {
            let before = servers.len();
            servers.retain(|s| s.get("name").and_then(|v| v.as_str()) != Some(name));
            found = servers.len() < before;
        }
        if found {
            std::fs::write(
                mcp_cfg_path,
                serde_json::to_string_pretty(&cfg).unwrap_or_default(),
            )?;
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

    let command = server
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
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

            client
                .close()
                .await
                .map_err(|e| anyhow::anyhow!("close error: {}", e))?;
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
    let tools = client
        .list_tools()
        .await
        .map_err(|e| anyhow::anyhow!("list_tools failed: {}", e))?;

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
            if let Some(properties) = tool
                .input_schema
                .get("properties")
                .and_then(|v| v.as_object())
            {
                let required: Vec<&str> = tool
                    .input_schema
                    .get("required")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                let param_names: Vec<String> = properties
                    .keys()
                    .map(|k| {
                        if required.contains(&k.as_str()) {
                            format!("{}*", k)
                        } else {
                            k.clone()
                        }
                    })
                    .collect();

                if !param_names.is_empty() {
                    println!("   Parameters: {}", param_names.join(", "));
                }
            }
        }
    }

    client
        .close()
        .await
        .map_err(|e| anyhow::anyhow!("close error: {}", e))?;
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
    let resources = client
        .list_resources()
        .await
        .map_err(|e| anyhow::anyhow!("list_resources failed: {}", e))?;

    if resources.is_empty() {
        println!("  No resources available.");
    } else {
        println!();
        println!("Found {} resource(s):", resources.len());
        println!("-------------------");
        for (i, res) in resources.iter().enumerate() {
            println!("{}. {}", i + 1, res.name);
            println!("   URI: {}", res.uri);
            if let Some(desc) = res.description.as_deref()
                && !desc.is_empty() {
                    println!("   Description: {}", desc);
                }
            if let Some(mime) = res.mime_type.as_deref()
                && !mime.is_empty() {
                    println!("   MIME Type: {}", mime);
                }
        }
    }

    client
        .close()
        .await
        .map_err(|e| anyhow::anyhow!("close error: {}", e))?;
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
    let prompts = client
        .list_prompts()
        .await
        .map_err(|e| anyhow::anyhow!("list_prompts failed: {}", e))?;

    if prompts.is_empty() {
        println!("  No prompts available.");
    } else {
        println!();
        println!("Found {} prompt(s):", prompts.len());
        println!("-------------------");
        for (i, p) in prompts.iter().enumerate() {
            println!("{}. {}", i + 1, p.name);
            if let Some(desc) = p.description.as_deref()
                && !desc.is_empty() {
                    println!("   Description: {}", desc);
                }
            if !p.arguments.is_empty() {
                println!("   Arguments:");
                for arg in &p.arguments {
                    let required_marker = if arg.required.unwrap_or(false) {
                        "*"
                    } else {
                        ""
                    };
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

    client
        .close()
        .await
        .map_err(|e| anyhow::anyhow!("close error: {}", e))?;
    Ok(())
}

fn cmd_inspect(mcp_cfg_path: &std::path::Path, name: &str) -> Result<()> {
    println!("Inspecting MCP server '{}'...", name);
    if let Some(server) = find_server(mcp_cfg_path, name)? {
        println!(
            "{}",
            serde_json::to_string_pretty(&server).unwrap_or_default()
        );
    } else {
        println!("  Server '{}' not found.", name);
    }
    Ok(())
}

async fn cmd_discover(
    command: Option<&str>,
    url: Option<&str>,
    args: Option<&str>,
    timeout: u64,
) -> Result<()> {
    let result = match (url, command) {
        (Some(url), _) => {
            println!("Discovering MCP HTTP server: {}", url);
            nemesis_mcp::manager::discover_server_metadata_http(url, timeout).await
        }
        (None, Some(command)) => {
            println!("Discovering MCP server: {}", command);
            let tool_args: Vec<String> = args
                .map(|a| a.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();
            nemesis_mcp::manager::discover_server_metadata(command, tool_args, vec![], timeout)
                .await
        }
        (None, None) => {
            println!("Error: provide either --command <path> or --url <url>");
            return Ok(());
        }
    };

    match result {
        Ok(result) => {
            // Server info
            if let Some(ref info) = result.server_info {
                println!("\n  Server: {} v{}", info.name, info.version);
            } else {
                println!("\n  Server: (unknown)");
            }

            // Tools
            if result.tools.is_empty() {
                println!("\n  Tools: (none)");
            } else {
                println!("\n  Tools ({}):", result.tools.len());
                println!("  -------------------");
                for (i, tool) in result.tools.iter().enumerate() {
                    let desc = tool.description.as_deref().unwrap_or("(no description)");
                    println!("  {}. {}", i + 1, tool.name);
                    println!("     {}", desc);

                    if let Some(props) = tool
                        .input_schema
                        .get("properties")
                        .and_then(|p| p.as_object())
                    {
                        let required: Vec<&str> = tool
                            .input_schema
                            .get("required")
                            .and_then(|r| r.as_array())
                            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        let param_names: Vec<String> = props
                            .keys()
                            .map(|k| {
                                if required.contains(&k.as_str()) {
                                    format!("{}*", k)
                                } else {
                                    k.clone()
                                }
                            })
                            .collect();
                        if !param_names.is_empty() {
                            println!("     Parameters: {}", param_names.join(", "));
                        }
                    }
                }
            }

            // Resources
            if result.resources.is_empty() {
                println!("\n  Resources: (none)");
            } else {
                println!("\n  Resources ({}):", result.resources.len());
                println!("  -------------------");
                for (i, res) in result.resources.iter().enumerate() {
                    println!("  {}. {} ({})", i + 1, res.name, res.uri);
                    if let Some(desc) = res.description.as_deref()
                        && !desc.is_empty() {
                            println!("     {}", desc);
                        }
                }
            }

            // Prompts
            if result.prompts.is_empty() {
                println!("\n  Prompts: (none)");
            } else {
                println!("\n  Prompts ({}):", result.prompts.len());
                println!("  -------------------");
                for (i, p) in result.prompts.iter().enumerate() {
                    println!("  {}. {}", i + 1, p.name);
                    if let Some(desc) = p.description.as_deref()
                        && !desc.is_empty() {
                            println!("     {}", desc);
                        }
                }
            }

            println!("\nDiscovery complete.");
        }
        Err(e) => {
            println!("Discovery failed: {}", e);
        }
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
        McpAction::Add {
            name,
            command,
            args,
            env,
            timeout,
        } => cmd_add(
            &mcp_cfg_path,
            &common::config_path(&home),
            &name,
            &command,
            args.as_deref(),
            &env,
            timeout,
        )?,
        McpAction::Remove { name } => cmd_remove(&mcp_cfg_path, &name)?,
        McpAction::Test { name } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_test(&mcp_cfg_path, &name))
            })?;
            
        }
        McpAction::Inspect { name } => cmd_inspect(&mcp_cfg_path, &name)?,
        McpAction::Tools { name } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_tools(&mcp_cfg_path, &name))
            })?;
            
        }
        McpAction::Resources { name } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_resources(&mcp_cfg_path, &name))
            })?;
            
        }
        McpAction::Prompts { name } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_prompts(&mcp_cfg_path, &name))
            })?;
            
        }
        McpAction::Discover {
            command,
            url,
            args,
            timeout,
        } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_discover(
                    command.as_deref(),
                    url.as_deref(),
                    args.as_deref(),
                    timeout,
                ))
            })?;
            
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
