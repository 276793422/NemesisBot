//! MCP Manager — centralized MCP server configuration and tool discovery.
//!
//! Manages the MCP config file (`config.mcp.json`), provides CRUD operations
//! for server entries, discovers tools from MCP servers, and tracks file
//! modification time for hot-reload support.

use std::path::PathBuf;
use std::time::SystemTime;

use nemesis_config::McpConfig;
use tracing::{info, warn};

use crate::adapter::{self, Tool};
use crate::client::{Client, McpClient};
use crate::http_transport::{HttpTransport, sanitize_url_for_log};
use crate::stdio_transport::StdioTransport;
use crate::types::{McpTool, Resource, ServerConfig, ServerInfo};

// ---------------------------------------------------------------------------
// McpManager
// ---------------------------------------------------------------------------

/// Centralized manager for MCP server configuration and tool discovery.
///
/// Reads/writes `config.mcp.json`, discovers tools from MCP servers,
/// and tracks file modification time for automatic hot-reload.
///
/// 2026-08-31 单一真相源收敛：私有 `McpFileConfig`（与 nemesis-config 的
/// `McpConfig` 字段分叉的同族重复定义）已删除，配置层直接复用
/// [`McpConfig`]——与 Dashboard/CLI 解析同一类型，宽容反序列化统一生效。
pub struct McpManager {
    config_path: PathBuf,
    config: McpConfig,
    last_mtime: Option<SystemTime>,
    /// G2（2026-10-18 批四）：per-agent 浏览器 profile 显式覆盖（gateway
    /// agent loop 装配时注入；None = 连接时回落进程 env——执行体子进程
    /// 上下文/共享形态零变化）。`discover_tools` 是单一消费点（启动全量
    /// 挂载与热重载补挂都经它，manager 实例贯穿两者）。
    browser_profile: Option<String>,
}

impl McpManager {
    /// Create a new manager bound to the given config file path.
    ///
    /// Loads the config if the file exists; otherwise starts with empty defaults.
    pub fn new(config_path: PathBuf) -> Self {
        let mut mgr = Self {
            config_path,
            config: McpConfig::default(),
            last_mtime: None,
            browser_profile: None,
        };
        if let Err(e) = mgr.load_config() {
            warn!("[McpManager] Failed to load config on init: {}", e);
        }
        mgr.last_mtime = Self::read_mtime(&mgr.config_path);
        mgr
    }

    /// G2：设置 per-agent 浏览器 profile（须在 `discover_tools` 之前调用；
    /// None = 清除回落 env 语义）。
    pub fn set_browser_profile(&mut self, profile: Option<String>) {
        self.browser_profile = profile;
    }

    /// G2：当前生效的 per-agent 浏览器 profile（可观测面）。
    pub fn browser_profile(&self) -> Option<&str> {
        self.browser_profile.as_deref()
    }

    // -----------------------------------------------------------------------
    // Config I/O
    // -----------------------------------------------------------------------

    /// Load config from disk. Returns Ok(()) even if the file doesn't exist.
    pub fn load_config(&mut self) -> Result<(), String> {
        if !self.config_path.exists() {
            return Ok(());
        }
        let data = std::fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read MCP config: {}", e))?;
        self.config = serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse MCP config: {}", e))?;
        // 第十四轮缺陷 17：sanitize 前缀碰撞可观测（加载面单一 warn 点，
        // 启动 + 热重载都经此处）。挂载语义：热重载只挂首个（其余被
        // find_new_servers 静默过滤），启动全量挂载但工具键后到者覆盖。
        for (prefix, names) in crate::adapter::sanitize_prefix_collisions(
            self.config.servers.iter().map(|s| s.name.as_str()),
        ) {
            warn!(
                "[McpManager] Server names {} collide on tool prefix '{}' — hot-reload mounts only the first, startup mount overwrites tool keys; rename one (config: {})",
                names.join(", "),
                prefix,
                self.config_path.display()
            );
        }
        Ok(())
    }

    /// Save current config to disk.
    ///
    /// 2026-10-25 BUG-20261025-47：写路径收编到 `nemesis_config::save_mcp_config`
    /// （write_file_atomic 原子写，与 Dashboard/CLI 写同一实现）；此前本地手搓
    /// tmp+rename 无 sync/cleanup，与同文件 web 写路径漂移（同族 BUG-45 形态）。
    /// 错误保留 "Failed to write MCP config" 前缀（S1 Windows 钉住依赖）。
    pub fn save_config(&self) -> Result<(), String> {
        nemesis_config::save_mcp_config(&self.config_path, &self.config)
            .map_err(|e| format!("Failed to write MCP config: {}", e))
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Whether MCP is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Return the config file path.
    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    /// List all configured servers.
    pub fn list_servers(&self) -> &[ServerConfig] {
        &self.config.servers
    }

    /// Find a server by name.
    pub fn get_server(&self, name: &str) -> Option<&ServerConfig> {
        self.config.servers.iter().find(|s| s.name == name)
    }

    // -----------------------------------------------------------------------
    // CRUD
    // -----------------------------------------------------------------------

    /// Add a new server. Returns Err if a server with the same name exists.
    pub fn add_server(&mut self, config: ServerConfig) -> Result<(), String> {
        // 写串行锁：config.mcp.json 的生产写者（web handlers::mcp 的
        // update_mcp_config）持同把 config_write_lock——本 CRUD 虽当前无
        // 生产接线（横扫六期记档的死码地雷），锁取在函数内使任何未来
        // 接线者结构性安全（横扫六期，2026-10-13）。
        let _cfg_serial = nemesis_config::config_write_lock();
        if self.config.servers.iter().any(|s| s.name == config.name) {
            return Err(format!("Server '{}' already exists", config.name));
        }
        self.config.servers.push(config);
        self.config.enabled = true;
        self.save_config()?;
        Ok(())
    }

    /// Remove a server by name. Returns true if a server was removed.
    pub fn remove_server(&mut self, name: &str) -> Result<bool, String> {
        // 同 add_server：与 web 写家族同锁（横扫六期，2026-10-13）。
        let _cfg_serial = nemesis_config::config_write_lock();
        let before = self.config.servers.len();
        self.config.servers.retain(|s| s.name != name);
        let removed = self.config.servers.len() < before;
        if removed {
            self.save_config()?;
        }
        Ok(removed)
    }

    // -----------------------------------------------------------------------
    // Hot-reload
    // -----------------------------------------------------------------------

    /// Check if the config file has been modified since the last check.
    /// Updates the internal mtime and returns true if changed.
    pub fn check_config_changed(&mut self) -> bool {
        let current = Self::read_mtime(&self.config_path);
        if current != self.last_mtime {
            if let Err(e) = self.load_config() {
                warn!("[McpManager] Failed to reload config: {}", e);
                // Do NOT update last_mtime — will retry next round
                return false;
            }
            self.last_mtime = current;
            info!("[McpManager] Config file changed, reloaded");
            return true;
        }
        false
    }

    /// Return servers that are not yet represented in the registered tools list.
    ///
    /// `registered_tool_prefixes` contains the sanitized server-name prefixes
    /// of already-registered MCP tools (e.g., `["mcp_test_server_"]`).
    pub fn find_new_servers(&self, registered_tool_prefixes: &[String]) -> Vec<&ServerConfig> {
        self.config
            .servers
            .iter()
            .filter(|server| {
                let prefix = format!("mcp_{}_", adapter::sanitize_name(&server.name));
                !registered_tool_prefixes.contains(&prefix)
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Discovery
    // -----------------------------------------------------------------------

    /// Connect to an MCP server, discover its tools, and return adapter Tool objects.
    ///
    /// Uses the config name (not the server's self-reported name) for tool prefixing.
    pub async fn discover_tools(
        &self,
        server: &ServerConfig,
    ) -> Result<Vec<Box<dyn Tool>>, String> {
        // 按 transport_type 分派：stdio（默认）| http（Streamable HTTP）。
        // HTTP 型 MCP 服务器（如 desktop-pet2 的 8808/mcp 端点）不再需要
        // stdio bridge 脚本——直接注册（2026-08-30）。
        // 2026-10-15 收敛：分派逻辑上移到 McpClient::from_server_config
        // （单一真相源），与 CLI `mcp test/tools/...` 连接路径共用。
        // G2：per-agent 浏览器 profile 随此处注入（opt-in 闸在 transport
        // 侧按 server 配置执行——未 opt-in 的 server 目录在场也不注入）。
        let mut client: Box<dyn Client> = match McpClient::from_server_config_with_profile(
            server,
            self.browser_profile.as_deref(),
        ) {
            Ok(c) => Box::new(c),
            Err(e) => return Err(e.to_string()),
        };

        let timeout = std::time::Duration::from_secs(if server.timeout_secs > 0 {
            server.timeout_secs
        } else {
            30
        });

        // Initialize with timeout
        let init_result = tokio::time::timeout(timeout, client.initialize()).await;
        match init_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                // Init failed — client dropped here, killing the subprocess
                drop(client);
                return Err(format!(
                    "MCP server '{}' initialization failed: {}",
                    server.name, e
                ));
            }
            Err(_) => {
                // Timeout — client dropped here, killing the subprocess
                drop(client);
                return Err(format!(
                    "MCP server '{}' initialization timed out",
                    server.name
                ));
            }
        }

        info!(
            server = %server.name,
            "[McpManager] MCP server initialized"
        );

        // Discover tools using config name
        adapter::create_tools_from_client_named(client, &server.name, server.timeout_secs)
            .await
            .map_err(|e| format!("Failed to discover tools from '{}': {}", server.name, e))
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn read_mtime(path: &PathBuf) -> Option<SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }
}

// ---------------------------------------------------------------------------
// One-shot discovery (for mcp_discover tool)
// ---------------------------------------------------------------------------

/// Result of one-shot MCP server discovery.
///
/// Contains raw metadata (not adapter objects) for formatting/display.
pub struct DiscoveryResult {
    /// Server name and version from the initialize handshake.
    pub server_info: Option<ServerInfo>,
    /// Tools exposed by the server.
    pub tools: Vec<McpTool>,
    /// Resources exposed by the server.
    pub resources: Vec<Resource>,
    /// Prompts exposed by the server.
    pub prompts: Vec<crate::types::Prompt>,
}

/// One-shot discovery: connect to an MCP server, collect metadata, close.
///
/// This is intended for the `mcp_discover` agent tool. It does NOT keep
/// the client alive — the subprocess is killed after discovery.
pub async fn discover_server_metadata(
    command: &str,
    args: Vec<String>,
    env: Vec<String>,
    timeout_secs: u64,
) -> Result<DiscoveryResult, String> {
    let transport = StdioTransport::new(command, args, env);
    let mut client: Box<dyn Client> = Box::new(McpClient::new(Box::new(transport)));

    let duration = std::time::Duration::from_secs(if timeout_secs > 0 { timeout_secs } else { 15 });

    // Initialize with timeout
    let init_result = tokio::time::timeout(duration, client.initialize()).await;
    match init_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP server '{}' initialization failed: {}. \
                 If this is an HTTP-based MCP server, stdio discovery will not work.",
                command, e
            ));
        }
        Err(_) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP server '{}' timed out after {}s. \
                 If this is an HTTP-based MCP server, use the 'url' parameter instead.",
                command, timeout_secs
            ));
        }
    }

    let server_info = client.server_info().cloned();

    // Collect metadata — each call is best-effort
    let tools = client.list_tools().await.unwrap_or_else(|e| {
        warn!("[McpManager] list_tools failed: {}", e);
        Vec::new()
    });
    let resources = client.list_resources().await.unwrap_or_else(|e| {
        warn!("[McpManager] list_resources failed: {}", e);
        Vec::new()
    });
    let prompts = client.list_prompts().await.unwrap_or_else(|e| {
        warn!("[McpManager] list_prompts failed: {}", e);
        Vec::new()
    });

    // Always close to kill the subprocess
    let _ = client.close().await;

    Ok(DiscoveryResult {
        server_info,
        tools,
        resources,
        prompts,
    })
}

/// One-shot HTTP discovery: connect to an MCP server via Streamable HTTP,
/// collect metadata, close.
///
/// This is the HTTP counterpart of [`discover_server_metadata`].
pub async fn discover_server_metadata_http(
    url: &str,
    timeout_secs: u64,
) -> Result<DiscoveryResult, String> {
    discover_server_metadata_http_with_headers(url, Vec::new(), timeout_secs).await
}

/// One-shot HTTP discovery with configured header lines（"KEY: VALUE"，
/// 值可含 vault:/env:/yaml: 引用——带认证探测与运行时头消费化同源，
/// 消除「探测能通、安装后 401」的断层；2026-10-15 W1）。
pub async fn discover_server_metadata_http_with_headers(
    url: &str,
    header_lines: Vec<String>,
    timeout_secs: u64,
) -> Result<DiscoveryResult, String> {
    let transport = HttpTransport::with_headers(url, header_lines);
    let mut client: Box<dyn Client> = Box::new(McpClient::new(Box::new(transport)));

    let duration = std::time::Duration::from_secs(if timeout_secs > 0 { timeout_secs } else { 15 });

    let init_result = tokio::time::timeout(duration, client.initialize()).await;
    match init_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP HTTP server '{}' initialization failed: {}",
                sanitize_url_for_log(url),
                e
            ));
        }
        Err(_) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP HTTP server '{}' timed out after {}s",
                sanitize_url_for_log(url),
                timeout_secs
            ));
        }
    }

    let server_info = client.server_info().cloned();

    let tools = client.list_tools().await.unwrap_or_else(|e| {
        warn!("[McpManager] HTTP list_tools failed: {}", e);
        Vec::new()
    });
    let resources = client.list_resources().await.unwrap_or_else(|e| {
        warn!("[McpManager] HTTP list_resources failed: {}", e);
        Vec::new()
    });
    let prompts = client.list_prompts().await.unwrap_or_else(|e| {
        warn!("[McpManager] HTTP list_prompts failed: {}", e);
        Vec::new()
    });

    let _ = client.close().await;

    Ok(DiscoveryResult {
        server_info,
        tools,
        resources,
        prompts,
    })
}

#[cfg(test)]
mod tests;
