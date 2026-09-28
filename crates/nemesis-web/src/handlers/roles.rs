//! 角色目录 WSAPI handler（2026-09-28 角色目录与分档供给）—
//! `roles.list`。
//!
//! 前端委派卡（DelegateDialog）的角色下拉数据源：目录全量 17 项 +
//! 当前 tier + 每项可见性。可见性裁决走 AgentLoop 的单一函数
//! （`visible_roles`）——与 spawn dispatch 闸同源，前端看到的集合就是
//! 模型派发能通过的集合。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct RolesHandler;

#[async_trait::async_trait]
impl ModuleHandler for RolesHandler {
    fn module_name(&self) -> &str {
        "roles"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "list" => {
                let agent_loop = ctx
                    .state
                    .agent_loop
                    .read()
                    .clone()
                    .ok_or_else(|| "agent loop not running".to_string())?;
                Ok(Some(agent_loop.roles_surface()))
            }
            _ => Err(format!("unknown command: roles.{}", cmd)),
        }
    }
}
