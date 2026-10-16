//! MCP installer - manages MCP server registration in config.mcp.json.
//!
//! 单一真相源对齐（2026-10-16 P1 整改）：本模块不再持有本地影子 schema。
//! 旧 `MCPConfig/MCPServerConfig {name, command, args}` 三字段形态的 RMW
//! 会把盘上既有条目的 url/transport_type/headers/env 等字段整体剥掉，
//! 且与 `nemesis-config`（Dashboard/CLI/运行期同一真相源）三套形状并存。
//! 现在直接复用 `nemesis_config::{McpConfig, McpServerConfig}`（宽容解析
//! 与 typed round-trip 保未知键），读写走 nemesis-config 的
//! `load_mcp_config`/`save_mcp_config`（原子写 0600），写路径持
//! [`nemesis_config::config_write_lock`] 写串行锁覆盖整段 RMW（防与
//! WSAPI/Dashboard 并发写丢更新）。

use std::path::PathBuf;

pub use nemesis_config::{McpConfig, McpServerConfig};

/// nemesis-config 错误 → io::Error 的诚实映射：Io 保留原始 kind，
/// 解析/校验类归 InvalidData。
fn io_err(e: nemesis_config::ConfigError) -> std::io::Error {
    match e {
        nemesis_config::ConfigError::Io(io) => io,
        other => std::io::Error::new(std::io::ErrorKind::InvalidData, other.to_string()),
    }
}

/// Manages MCP server registration.
pub struct MCPInstaller {
    workspace: PathBuf,
}

impl MCPInstaller {
    /// Create a new MCP installer for the given workspace.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    /// Get the config file path.
    pub fn config_path(&self) -> PathBuf {
        nemesis_path::resolve_mcp_config_path_in_workspace(&self.workspace)
    }

    /// Install (add or update) an MCP server.
    ///
    /// 更新臂只覆盖本模块拥有的字段（transport_type/command/args），其余
    /// 字段（url/description/headers/env/provider/tags/extra/timeout）原样
    /// 保留——旧影子 schema 的整体替换会把这些剥掉。
    /// 写路径持 `config_write_lock` 串行锁覆盖整段 load→save（锁非重入，
    /// 勿在本函数内再嵌套取锁）。
    pub fn install(&self, name: &str, command: &str, args: Vec<String>) -> std::io::Result<()> {
        let path = self.config_path();
        let _serial = nemesis_config::config_write_lock();
        let mut config = nemesis_config::load_mcp_config(&path).map_err(io_err)?;

        match config.servers.iter_mut().find(|s| s.name == name) {
            Some(existing) => {
                existing.transport_type = "stdio".to_string();
                existing.command = command.to_string();
                existing.args = args;
            }
            None => config.servers.push(McpServerConfig {
                name: name.to_string(),
                transport_type: "stdio".to_string(),
                command: command.to_string(),
                args,
                ..Default::default()
            }),
        }
        config.enabled = true;

        nemesis_config::save_mcp_config(&path, &config).map_err(io_err)
    }

    /// Uninstall (remove) an MCP server. 幂等：目标不存在时照常成功，且
    /// **不重写文件**（P1-b，2026-10-16 复查）：McpConfig 顶层无 flatten
    /// extra，no-op 重写会把宽容解析不认识的外来顶层键整体剥掉（旧实现
    /// 只在确实移除时才写盘，本函数恢复该语义；形状归一化只在真实卸载时
    /// 发生）。
    pub fn uninstall(&self, name: &str) -> std::io::Result<()> {
        let path = self.config_path();
        let _serial = nemesis_config::config_write_lock();
        let mut config = nemesis_config::load_mcp_config(&path).map_err(io_err)?;
        let before = config.servers.len();
        config.servers.retain(|s| s.name != name);
        if config.servers.len() == before {
            return Ok(());
        }
        nemesis_config::save_mcp_config(&path, &config).map_err(io_err)
    }

    /// Check if an MCP server is installed. 读路径不入写锁（save 侧 rename
    /// 原子性保证读到完整旧值或完整新值）；装载失败按「未安装」处理。
    pub fn is_installed(&self, name: &str) -> bool {
        nemesis_config::load_mcp_config(&self.config_path())
            .map(|config| config.servers.iter().any(|s| s.name == name))
            .unwrap_or(false)
    }

    /// Load the MCP config from disk（宽容解析，缺失文件回落默认）。
    pub fn load_config(&self) -> std::io::Result<McpConfig> {
        nemesis_config::load_mcp_config(&self.config_path()).map_err(io_err)
    }

    /// Save the MCP config to disk（原子写）。写路径持串行锁。
    pub fn save_config(&self, config: &McpConfig) -> std::io::Result<()> {
        let _serial = nemesis_config::config_write_lock();
        nemesis_config::save_mcp_config(&self.config_path(), config).map_err(io_err)
    }
}

#[cfg(test)]
mod tests;
