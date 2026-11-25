//! 自定义 slash 命令表（`<workspace>/config/config.commands.json`）。
//!
//! 命令 = 快捷提示词发送器（slash `/命令` 形态）：用户在任意通道输入 `/name args`，
//! AgentLoop 入口把模板中的 `$ARGUMENTS` 替换为 `args` 后作为用户消息进入正常
//! LLM 轮次（改写型，区别于内置命令的短路型）。四维度：name / description /
//! argument_hint / prompt —— schema 与作用域层级无关（单 workspace 起步，
//! 多级合并为后续扩展，见 slash-commands）。
//!
//! 单一权威 = 本文件；AgentLoop 改写与 Dashboard 管理页（CommandsView）都
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
///
/// HotLoadFn 契约（nemesis-agent loop/fs_watch.rs）：签名必须是
/// `fn(&Path) -> CommandsConfig`——必须容忍缺失文件、错误不外泄，违形
/// （`[]` 等非 object 文档）只能走 WARN 降级臂。形状闸前置
/// （`ensure_object`，BUG-20261125-008）：CommandsConfig 全字段 default
/// 无 flatten，`[]` 曾走 serde visit_seq 零元素臂 = 静默「空表」零诊断
/// （全部自定义 slash 命令无声消失）；object 闸后违形进既有 WARN 臂。
pub fn load_commands_config(path: &Path) -> CommandsConfig {
    match nemesis_utils::fsutil::read_config_string(path) {
        Ok(raw) => {
            let parsed = serde_json::from_str::<serde_json::Value>(&raw)
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    nemesis_utils::json_shape::ensure_object(&v, "config.commands.json")?;
                    serde_json::from_value::<CommandsConfig>(v).map_err(|e| e.to_string())
                });
            match parsed {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::warn!(
                        "[Commands] config 解析失败/形状非法（{}），按空表处理: {}",
                        path.display(),
                        e
                    );
                    CommandsConfig::default()
                }
            }
        }
        Err(_) => CommandsConfig::default(),
    }
}

/// 保存（现走统一原子 helper，取代本文件自制
/// tmp+rename——唯一临时名 + sync_all + 失败清理）。
pub fn save_commands_config(path: &Path, cfg: &CommandsConfig) -> Result<(), String> {
    let body = serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize: {e}"))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), body.as_bytes(), 0o600)
}
