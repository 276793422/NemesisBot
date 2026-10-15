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
    /// Add a new MCP server (stdio: --command；HTTP: --url [+ --header])
    ///
    /// HTTP 服务直连示例（无需桥接脚本，一行复制即可执行）：
    ///   nemesisbot mcp add -n pet --url http://127.0.0.1:8808/mcp -H "Authorization: vault:pet-token"
    ///
    /// header 值支持 vault:/env:/yaml: 凭据引用（整值前缀，每次连接现查；
    /// 不支持 'Bearer vault:<别名>' 内嵌写法）。vault 值存完整头值
    /// （如 'Bearer <token>'），需先交互式写入：nemesisbot vault set pet-token
    /// （exec 工具无 stdin，无法代写）。
    Add {
        /// Server name
        #[arg(short, long)]
        name: String,
        /// Command to start server (stdio 形态；与 --url 互斥，恰给其一)
        #[arg(short, long)]
        command: Option<String>,
        /// URL of the MCP server (HTTP 形态，如 'http://localhost:8808/mcp'；与 --command 互斥)
        #[arg(short, long)]
        url: Option<String>,
        /// Transport type override (http|stdio；缺省按 --command/--url 推断)
        #[arg(long)]
        transport: Option<String>,
        /// HTTP request header, "KEY: VALUE" (HTTP 专用，可多次；值支持 vault:/env: 引用)
        #[arg(short = 'H', long)]
        header: Vec<String>,
        /// Arguments for command (stdio 专用，comma-separated)
        #[arg(short, long)]
        args: Option<String>,
        /// Environment variables (stdio 专用，KEY=VALUE)
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
        /// HTTP request header, "KEY: VALUE" (HTTP 专用，可多次；带认证探测)
        #[arg(short = 'H', long)]
        header: Vec<String>,
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
    // W1（2026-10-15）：from_config 只认 stdio（HTTP 条目空 command 直接
    // 报错）；改用 from_server_config 按 transport_type 分派（与 manager
    // 注册路径同源），HTTP 型服务 test/tools/resources/prompts 直连。
    let mut client = nemesis_mcp::client::McpClient::from_server_config(&config)
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

// ---------------------------------------------------------------------------
// mcp add 的请求校验（W1 HTTP 化，2026-10-15）
// ---------------------------------------------------------------------------

/// `mcp add` 的一次请求（校验后的形态）。
///
/// stdio（`--command`）与 HTTP（`--url`）两形态互斥，恰给其一；`--header`
/// 仅 HTTP 合法，`--args`/`--env` 仅 stdio 合法。校验规则单一真相源 =
/// [`AddRequest::build`]（CLI 分发与单测共用）；本结构只承载语法校验，
/// 绝不解引用任何 `vault:`/`env:` 值（凭据真值只在连接期由
/// `nemesis-mcp` 现查）。
#[derive(Debug, Clone)]
struct AddRequest {
    name: String,
    /// "stdio" | "http"（已推断/已校验一致）
    transport: &'static str,
    command: Option<String>,
    url: Option<String>,
    headers: Vec<String>,
    args: Vec<String>,
    env: Vec<String>,
    timeout: u64,
}

impl AddRequest {
    #[allow(clippy::too_many_arguments)]
    fn build(
        name: &str,
        command: Option<&str>,
        url: Option<&str>,
        transport: Option<&str>,
        headers: &[String],
        args: Option<&str>,
        env: &[String],
        timeout: u64,
    ) -> Result<Self> {
        let command = command.map(str::trim).filter(|s| !s.is_empty());
        let url = url.map(str::trim).filter(|s| !s.is_empty());

        // 恰给其一
        match (command, url) {
            (Some(_), Some(_)) => anyhow::bail!(
                "--command 与 --url 只能给一个：stdio 服务用 --command，HTTP 服务用 --url"
            ),
            (None, None) => anyhow::bail!(
                "必须提供 --command <stdio 启动命令> 或 --url <HTTP 端点> 之一；\
                 HTTP 示例：nemesisbot mcp add -n pet --url http://127.0.0.1:8808/mcp \
                 -H \"Authorization: vault:pet-token\""
            ),
            _ => {}
        }

        // transport 显式值校验 + 与形态一致性；缺省按 command/url 推断
        let transport: &'static str = match transport.map(str::trim).filter(|s| !s.is_empty()) {
            None => {
                if url.is_some() {
                    "http"
                } else {
                    "stdio"
                }
            }
            Some(t) => match t.to_ascii_lowercase().as_str() {
                "http" => {
                    if command.is_some() {
                        anyhow::bail!(
                            "--transport http 与 --command 冲突（stdio 服务请用 --transport stdio 或省略）"
                        );
                    }
                    "http"
                }
                "stdio" => {
                    if url.is_some() {
                        anyhow::bail!(
                            "--transport stdio 与 --url 冲突（HTTP 服务请用 --transport http 或省略）"
                        );
                    }
                    "stdio"
                }
                other => anyhow::bail!("未知 transport '{}'（支持：http、stdio）", other),
            },
        };

        // 形态专属参数
        if transport == "http" {
            if args.map(str::trim).filter(|s| !s.is_empty()).is_some() {
                anyhow::bail!("--args 是 stdio 专用参数，HTTP 服务（--url）不支持");
            }
            if !env.is_empty() {
                anyhow::bail!("--env 是 stdio 专用参数，HTTP 服务（--url）不支持");
            }
        } else if !headers.is_empty() {
            anyhow::bail!("--header 是 HTTP 专用参数，stdio 服务（--command）不支持");
        }

        // header 行格式校验：只验语法与引用形态，绝不解析值
        for h in headers {
            validate_header_line(h)?;
        }

        // 与旧版逐字节一致：split(',') + trim，不过滤空段
        let args: Vec<String> = args
            .map(|a| a.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        Ok(Self {
            name: name.to_string(),
            transport,
            command: command.map(str::to_string),
            url: url.map(str::to_string),
            headers: headers.to_vec(),
            args,
            env: env.to_vec(),
            timeout,
        })
    }
}

/// 校验一行 `"KEY: VALUE"` 头（CLI 侧语法闸）。规则单一真相源 =
/// `nemesis_config::validate_mcp_header_line`（运行期 `resolved_header_pairs`
/// 与 Dashboard server.add/update 同源）：缺冒号 / 头名为空或含空白控制
/// 字符 / `Mcp-Session-Id` 保留头 / 整值 `vault:`/`env:`/`yaml:` 引用名为
/// 空或带首尾空格——只验语法，值原样落盘，绝不解引用。错误文案附加脱敏
/// 后的行（截掉值段），校验器本体不回显值段。
fn validate_header_line(line: &str) -> Result<()> {
    nemesis_config::validate_mcp_header_line(line)
        .map_err(|reason| anyhow::anyhow!("{reason}：{}", redact_header_for_error(line)))
}

/// 错误消息里的头行脱敏：截掉冒号后的值段（值可能含凭据字面量）。
fn redact_header_for_error(line: &str) -> String {
    match line.split_once(':') {
        Some((name, _)) => format!("{}: <redacted>", name.trim()),
        None => "<redacted>".to_string(),
    }
}

/// 收集 header 行中引用的 vault 别名（整值 `vault:<alias>` 前缀形态），
/// 用于 add 成功后的「凭据需交互式写入」提示与 test 401 诊断。
fn header_vault_aliases(headers: &[serde_json::Value]) -> Vec<String> {
    headers
        .iter()
        .filter_map(|v| v.as_str())
        .filter_map(|line| line.split_once(':').map(|(_, v)| v.trim()))
        .filter_map(|value| {
            value
                .strip_prefix("vault:")
                .filter(|alias| !alias.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// header 值内嵌引用前缀但非整值形态（如 `Bearer vault:x`）——运行期
/// `is_secret_reference` 只认整值前缀，内嵌形态按字面量整值发送，鉴权
/// 几乎必坏（外部设备实盘案例的隐性坑）。返回可疑行（仅用于提示；
/// 打印时只接头名，不回显值）。
fn embedded_ref_headers(headers: &[String]) -> Vec<String> {
    headers
        .iter()
        .filter(|h| match h.split_once(':') {
            Some((_, v)) => {
                let v = v.trim();
                ["vault:", "env:", "yaml:"]
                    .iter()
                    .any(|p| v.contains(p) && !v.starts_with(p))
            }
            None => false,
        })
        .cloned()
        .collect()
}

/// `mcp test` 的 HTTP 形态判定。
///
/// 与 [`nemesis_mcp::client::McpClient::from_server_config`] 的分派**同源**：
/// 同一别名集（`http` / `streamable_http` / `streamable-http`）、同大小写
/// 敏感语义，外加旧手写条目兼容（无 transport_type、无 command、有 url）。
/// 判定为 HTTP 才走直连测试分支；其余形态按 stdio 走 PATH 探测——与
/// 运行时分派一致，避免「test 一套口径、连接另一套口径」的断层。
/// `mcp list` 的命令显示值：与分派真相源的 stdio 回落同款（`command`
/// 空而 `url` 非空时显示 `url`——Dashboard stdio 约定命令存 url 字段），
/// 两者皆空回 "?"。
fn display_command_for(server: &serde_json::Value) -> String {
    let command = server.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if !command.is_empty() {
        return command.to_string();
    }
    let url = server.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if !url.is_empty() {
        url.to_string()
    } else {
        "?".to_string()
    }
}

fn server_entry_is_http(server: &serde_json::Value) -> bool {
    let transport_type = server
        .get("transport_type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let url = server.get("url").and_then(|v| v.as_str()).unwrap_or("");
    matches!(
        transport_type,
        "http" | "streamable_http" | "streamable-http"
    ) || (transport_type.is_empty()
        && !url.is_empty()
        && server
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty())
}

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
    // REL-002：统一原子写入（mcp 总开关在主配置）。
    nemesis_utils::write_file_atomic(
        &config_json_path.to_string_lossy(),
        serde_json::to_string_pretty(&cfg)
            .unwrap_or_default()
            .as_bytes(),
        0o600,
    )
    .map_err(anyhow::Error::msg)?;
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
        && cfg.get("enabled").and_then(|v| v.as_bool()) == Some(false)
    {
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
                    let command = display_command_for(server);
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
                    if server_entry_is_http(server) {
                        // W1（2026-10-15）：HTTP 条目显示 URL——无 command 键，
                        // 走旧分支会打出误导性的 "Command: ?"。
                        let url = server.get("url").and_then(|v| v.as_str()).unwrap_or("?");
                        println!("    Transport: HTTP");
                        println!("    URL: {}", url);
                        let header_count = server
                            .get("headers")
                            .and_then(|v| v.as_array())
                            .map(|a| a.len())
                            .unwrap_or(0);
                        if header_count > 0 {
                            println!("    Headers: {} line(s)", header_count);
                        }
                    } else {
                        println!("    Command: {} {}", command, args);
                    }
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
        println!(
            "  HTTP server example: nemesisbot mcp add -n <name> --url http://127.0.0.1:8808/mcp \
             -H \"Authorization: vault:<alias>\""
        );
    }
    Ok(())
}

fn cmd_add(
    mcp_cfg_path: &std::path::Path,
    config_json_path: &std::path::Path,
    req: &AddRequest,
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
            if s.get("name").and_then(|v| v.as_str()) == Some(req.name.as_str()) {
                println!("Error: Server '{}' already exists.", req.name);
                println!("Remove it first: nemesisbot mcp remove {}", req.name);
                return Ok(());
            }
        }
    }

    // W1（2026-10-15）：stdio 旧写盘形态逐字节不变；HTTP 新形态带
    // transport_type/url/headers（headers 值原样落盘，凭据引用在连接期现查）。
    let server = if req.transport == "http" {
        serde_json::json!({
            "name": req.name,
            "transport_type": "http",
            "url": req.url,
            "headers": req.headers,
            "timeout_secs": req.timeout,
        })
    } else {
        serde_json::json!({
            "name": req.name,
            "command": req.command,
            "args": req.args,
            "env": req.env,
            // 2026-08-31 收敛：写规范键 timeout_secs（读取侧双键兼容）
            "timeout_secs": req.timeout,
        })
    };

    // servers 数组缺失（旧 schema / 手工编辑）时补建，否则下面 push 静默
    // 跳过仍报成功 —— 用户以为已添加，实际文件里没有该服务器。
    if !cfg
        .get("servers")
        .map(serde_json::Value::is_array)
        .unwrap_or(false)
    {
        cfg["servers"] = serde_json::json!([]);
    }
    cfg["servers"]
        .as_array_mut()
        .expect("just normalized to array")
        .push(server);
    cfg["enabled"] = serde_json::Value::Bool(true);

    // REL-002：统一原子写入（config.mcp.json）。
    nemesis_utils::write_file_atomic(
        &mcp_cfg_path.to_string_lossy(),
        serde_json::to_string_pretty(&cfg)
            .unwrap_or_default()
            .as_bytes(),
        0o600,
    )
    .map_err(anyhow::Error::msg)?;

    // Sync master switch in the real home-root config.json: mcp.enabled = true
    sync_mcp_master_switch(config_json_path, true)?;

    println!("🔌 MCP server '{}' added.", req.name);
    println!("Configuration saved to: {}", mcp_cfg_path.display());
    if req.transport == "http" {
        println!("  Transport: HTTP");
        println!("  URL: {}", req.url.as_deref().unwrap_or("?"));
        if !req.headers.is_empty() {
            println!("  Headers: {} line(s)", req.headers.len());
        }
        println!("  Note: 运行中的 gateway 需重启后才会挂载新 server。");
        // 机制教训产品化（案例学费 → 内建提示）：vault 凭据只能交互式写入。
        let aliases = header_vault_aliases(
            &req.headers
                .iter()
                .map(|h| serde_json::json!(h))
                .collect::<Vec<_>>(),
        );
        if !aliases.is_empty() {
            println!();
            println!("🔐 Header 使用 vault 凭据引用（{}）。", aliases.join(", "));
            println!("   凭据需先交互式写入（exec 工具无 stdin，无法代写）：");
            for alias in &aliases {
                println!("     nemesisbot vault set {}", alias);
            }
            println!("   vault 值应为完整头值（如 'Bearer <token>'）——引用是整值形态，");
            println!("   不支持 'Bearer vault:<别名>' 内嵌写法（会按字面量发送）。");
        }
        // 内嵌引用形态 loud 提示（字面量发送，鉴权大概率失败）。
        for h in embedded_ref_headers(&req.headers) {
            let name = h.split_once(':').map(|(n, _)| n.trim()).unwrap_or("?");
            println!(
                "  ⚠ Header '{}' 的值内嵌 vault:/env: 引用——运行期按字面量发送，鉴权大概率失败；",
                name
            );
            println!(
                "    正确形态：'{}: vault:<别名>'，vault 值存完整头值（如 'Bearer <token>'）。",
                name
            );
        }
    }
    println!();
    println!("Next steps:");
    println!("  1. Test the connection: nemesisbot mcp test {}", req.name);
    println!("  2. List tools: nemesisbot mcp tools {}", req.name);
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
            // REL-002：统一原子写入（config.mcp.json）。
            nemesis_utils::write_file_atomic(
                &mcp_cfg_path.to_string_lossy(),
                serde_json::to_string_pretty(&cfg)
                    .unwrap_or_default()
                    .as_bytes(),
                0o600,
            )
            .map_err(anyhow::Error::msg)?;
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

    // W1（2026-10-15）：HTTP 型条目跳过 PATH 探测直连；stdio 条目保持原路径。
    // 2026-10-15 复查修复：别名集与大小写语义对齐 McpClient::from_server_config
    // （分派单一真相源）——此前只认 "http"（且大小写不敏感），手写
    // streamable_http 条目会掉进 stdio 分支被 which::which("?") 误判
    // 「NOT found in PATH」跳过测试。
    if server_entry_is_http(&server) {
        let url = server.get("url").and_then(|v| v.as_str()).unwrap_or("");
        println!("  Transport: HTTP");
        println!("  URL: {}", url);
    } else {
        // 与分派真相源 stdio 回落同源（R8）：web 形态 stdio 条目命令存
        // url 字段——直读 command 键会探测 which("?") 误导跳过连接测试。
        let command = display_command_for(&server);
        println!("  Command: {}", command);

        // Check if command exists
        if which::which(command).is_ok() {
            println!("  Command found in PATH: OK");
        } else {
            println!("  Command NOT found in PATH.");
            println!("  Skipping connection test.");
            return Ok(());
        }
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
            println!("  Error: {:#}", e);
            // 机制教训产品化（W1 连带小项 2）：401 且 header 含 vault 引用
            // → 指向 vault 条目检查（解析失败 WARN 跳过 = 头未注入，表现即 401）。
            let err_text = format!("{:#}", e);
            if err_text.contains("HTTP 401") {
                let aliases = header_vault_aliases(
                    server
                        .get("headers")
                        .and_then(|v| v.as_array())
                        .map(|a| a.as_slice())
                        .unwrap_or(&[]),
                );
                if !aliases.is_empty() {
                    println!(
                        "  诊断提示：服务端返回 401，且 header 使用 vault 凭据引用（{}）。",
                        aliases.join(", ")
                    );
                    println!("  检查凭据是否已写入：nemesisbot vault list；");
                    println!(
                        "  缺则交互式执行 nemesisbot vault set <别名>（exec 工具无 stdin，无法代写）；"
                    );
                    println!("  vault 值须为完整头值（如 'Bearer <token>'，引用不支持内嵌写法）。");
                }
            }
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
                && !desc.is_empty()
            {
                println!("   Description: {}", desc);
            }
            if let Some(mime) = res.mime_type.as_deref()
                && !mime.is_empty()
            {
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
                && !desc.is_empty()
            {
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
    headers: &[String],
    args: Option<&str>,
    timeout: u64,
) -> Result<()> {
    let result = match (url, command) {
        (Some(url), _) => {
            println!("Discovering MCP HTTP server: {}", url);
            // W1（2026-10-15）：--header 带认证探测（值可含 vault:/env: 引用，
            // 与运行时头消费化同源；CLI 侧只验语法不解引用）。
            for h in headers {
                validate_header_line(h)?;
            }
            nemesis_mcp::manager::discover_server_metadata_http_with_headers(
                url,
                headers.to_vec(),
                timeout,
            )
            .await
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
                        && !desc.is_empty()
                    {
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
                        && !desc.is_empty()
                    {
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
            url,
            transport,
            header,
            args,
            env,
            timeout,
        } => {
            let req = AddRequest::build(
                &name,
                command.as_deref(),
                url.as_deref(),
                transport.as_deref(),
                &header,
                args.as_deref(),
                &env,
                timeout,
            )?;
            cmd_add(&mcp_cfg_path, &common::config_path(&home), &req)?;
        }
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
            header,
            args,
            timeout,
        } => {
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(cmd_discover(
                    command.as_deref(),
                    url.as_deref(),
                    &header,
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
