//! 自定义 slash 命令表（`<workspace>/config/config.commands.json`）。
//!
//! 命令 = 快捷提示词发送器（类 CC `/命令`）：用户在任意通道输入 `/name args`，
//! AgentLoop 入口把模板中的 `$ARGUMENTS` 替换为 `args` 后作为用户消息进入正常
//! LLM 轮次（改写型，区别于内置命令的短路型）。四维度：name / description /
//! argument_hint / prompt —— schema 与作用域层级无关（单 workspace 起步，
//! 多级合并为后续扩展，见 docs/PLAN/2026-08-29_slash-commands.md）。
//!
//! 单一真相源 = 本文件；AgentLoop 改写与 Dashboard 管理页（CommandsView）都
//! 经由这里读写。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 一条自定义命令（四维度）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandEntry {
    /// 命令名（`/name` 的 name；无空格，唯一）。
    pub name: String,
    /// 补全菜单里展示的描述。
    #[serde(default)]
    pub description: String,
    /// 菜单里的参数格式提示（如 `<文件路径>`；可空）。
    #[serde(default)]
    pub argument_hint: String,
    /// 提示词模板；`$ARGUMENTS` 替换为命令后追加的文字。
    pub prompt: String,
}

/// config.commands.json 顶层结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandsConfig {
    #[serde(default)]
    pub commands: Vec<CommandEntry>,
}

impl CommandsConfig {
    pub fn get(&self, name: &str) -> Option<&CommandEntry> {
        self.commands.iter().find(|c| c.name == name)
    }
}

/// `<workspace>/config/config.commands.json`（调用方经 nemesis-path 取路径）。
pub fn load_commands_config(path: &Path) -> CommandsConfig {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!(
                "[Commands] config 解析失败（{}），按空表处理: {}",
                path.display(),
                e
            );
            CommandsConfig::default()
        }),
        Err(_) => CommandsConfig::default(),
    }
}

/// 原子-ish 保存（tmp + rename，与 catalog.rs save_cache 同款）。
pub fn save_commands_config(path: &Path, cfg: &CommandsConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let body = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("write: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}
