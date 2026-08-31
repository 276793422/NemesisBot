//! 自定义 slash 命令表管理（`config.commands.json`）。
//!
//! Dashboard「命令」页（CommandsView）的 CRUD 后端：`commands.list` 读全表、
//! `commands.save` 整表原子保存（tmp+rename）。AgentLoop 侧经 mtime 热重载
//! 同一份文件（`rewrite_custom_command`），无需重启 Bot。
//!
//! 校验（保存时）：name 非空 / 无空白 / 唯一；prompt 非空。`argument_hint`
//! 可空。`$ARGUMENTS` 占位符由 AgentLoop 改写时替换，此处不校验。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_config::{CommandsConfig, CommandEntry, load_commands_config, save_commands_config};
use nemesis_path::resolve_commands_config_path_in_workspace;
use std::path::{Path, PathBuf};

pub struct CommandsHandler;

impl Default for CommandsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandsHandler {
    pub fn new() -> Self {
        Self
    }

    fn config_path(&self, workspace: &str) -> PathBuf {
        resolve_commands_config_path_in_workspace(Path::new(workspace))
    }

    fn commands_list(&self, workspace: &str) -> Result<serde_json::Value, String> {
        let cfg = load_commands_config(&self.config_path(workspace));
        Ok(serde_json::json!({
            "commands": cfg.commands,
            "total": cfg.commands.len(),
        }))
    }

    fn commands_save(
        &self,
        workspace: &str,
        commands: Vec<CommandEntry>,
    ) -> Result<serde_json::Value, String> {
        // 校验：name 非空/无空白/唯一；prompt 非空。错误带序号定位。
        let mut seen = std::collections::HashSet::new();
        for (i, c) in commands.iter().enumerate() {
            let at = format!("第 {} 条", i + 1);
            let name = c.name.trim();
            if name.is_empty() {
                return Err(format!("{at}：命令名称不能为空"));
            }
            if name.split_whitespace().count() != 1 {
                return Err(format!("{at}：命令名称不能包含空格（{name:?}）"));
            }
            if !seen.insert(name.to_string()) {
                return Err(format!("{at}：命令名称重复（/{name}）"));
            }
            if c.prompt.trim().is_empty() {
                return Err(format!("{at}（/{name}）：命令提示词不能为空"));
            }
        }

        let cfg = CommandsConfig { commands };
        save_commands_config(&self.config_path(workspace), &cfg)?;
        Ok(serde_json::json!({
            "saved": true,
            "total": cfg.commands.len(),
        }))
    }
}

#[async_trait::async_trait]
impl ModuleHandler for CommandsHandler {
    fn module_name(&self) -> &str {
        "commands"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = crate::handlers::require_workspace(ctx)?;

        match cmd {
            // 路由按 module_name 分发后 cmd 是裸名（模块前缀已被剥掉）——
            // 不要写成 "commands.list"（2026-08-29 曾因此 100% 未知命令）。
            "list" => Ok(Some(self.commands_list(workspace)?)),
            "save" => {
                let data = data.ok_or("missing data")?;
                let commands: Vec<CommandEntry> = serde_json::from_value(
                    data.get("commands").cloned().unwrap_or(serde_json::json!([])),
                )
                .map_err(|e| format!("invalid commands payload: {e}"))?;
                Ok(Some(self.commands_save(workspace, commands)?))
            }
            _ => Err(format!("unknown command: commands.{}", cmd)),
        }
    }
}

#[cfg(test)]
mod tests;
