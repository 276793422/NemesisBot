//! 客户端驱动委派（2026-09-28 角色目录与分档供给）：WSAPI `chat.spawn` 的
//! loop 侧入口。
//!
//! 客户端（Dashboard 委派卡）直接把任务派给指定角色的子代理——同一
//! detached 通道与模型侧 `spawn` 工具共用（同一 SpawnFn 闭包），工具档位/
//! 深度/角色模板/安全管线全部同源。区别只在入口：模型的 spawn 走工具派发
//! 闸（角色目录闸在 dispatch），客户端 spawn 的角色合法性由 handler 侧
//! [`Self::check_client_role`] 用同一 [`Self::visible_roles`] 裁决——两处
//! 口径永不漂移。

use super::*;

impl AgentLoop {
    /// 注入 spawn 共享槽镜像（factory 组装后调用；与 SpawnTool 持同一 Arc）。
    pub fn set_spawn_slot(
        &self,
        slot: std::sync::Arc<std::sync::OnceLock<crate::loop_tools::SpawnFn>>,
    ) {
        *self.spawn_slot.write() = Some(slot);
    }

    /// 客户端 spawn 前的角色合法性检查（handler 侧裁决）：未知 slug /
    /// 分档外 / `agents.roles.hidden` 隐藏 = `Err`（错误串与 dispatch 闸
    /// 同文案形态）。空 role = `Ok`（缺省角色，档位推导）。
    pub fn check_client_role(&self, role: &str) -> Result<(), String> {
        let role = role.trim();
        if role.is_empty() {
            return Ok(());
        }
        let visible = self.visible_roles();
        if visible.contains(&role) {
            return Ok(());
        }
        let known = nemesis_prompts::subagents::SubagentRole::from_slug(role).is_some();
        Err(if known {
            format!(
                "role '{role}' is not available for the current model tier, or it is hidden via agents.roles.hidden. Available roles: {}.",
                visible.join(", ")
            )
        } else {
            format!(
                "Unknown role '{role}'. Valid roles: {}.",
                nemesis_prompts::subagents::SubagentRole::catalog()
                    .iter()
                    .map(|(slug, _, _)| *slug)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    /// WSAPI `roles.list` 的数据面：当前 tier + 目录全量（slug/职责/最低
    /// 档/当前可见性/配置隐藏）。可见性裁决与 dispatch 闸共用
    /// [`Self::visible_roles`]——前端下拉看到的集合就是模型能用的集合。
    pub fn roles_surface(&self) -> serde_json::Value {
        let tier_str = match *self.tier.read() {
            nemesis_types::capability::ModelTier::Mini => "mini",
            nemesis_types::capability::ModelTier::Normal => "normal",
            nemesis_types::capability::ModelTier::Auto
            | nemesis_types::capability::ModelTier::Big => "big",
        };
        let visible = self.visible_roles();
        let hidden = self.current_hidden_roles();
        let roles: Vec<serde_json::Value> = nemesis_prompts::subagents::SubagentRole::catalog()
            .iter()
            .map(|(slug, desc, min)| {
                serde_json::json!({
                    "slug": slug,
                    "description": desc,
                    "min_tier": min,
                    "visible": visible.contains(slug),
                    "hidden": hidden.iter().any(|h| h == slug),
                })
            })
            .collect();
        serde_json::json!({
            "tier": tier_str,
            "visible_count": visible.len(),
            "roles": roles,
        })
    }

    /// 客户端驱动委派：走与模型 spawn 同一 SpawnFn（detached 子代理，
    /// 前台等待最终回复）。`role` 空串 = 缺省（档位推导）；`tools_profile`
    /// 按 SpawnTool 同款值域（"readonly"|"full"，空 = readonly 缺省）。
    /// 槽未注入 / 闭包未 set = 诚实 `Err`。
    pub async fn client_spawn(
        &self,
        task: &str,
        role: &str,
        tools_profile: &str,
    ) -> Result<String, String> {
        let slot = self.spawn_slot.read().clone();
        let spawn_fn = slot.as_ref().and_then(|s| s.get()).ok_or_else(|| {
            "sub-agent spawning is not available on this loop (no spawn channel configured)"
                .to_string()
        })?;
        spawn_fn(
            "client",
            task,
            "", // model：空 = SpawnConfig 缺省模型（detached 沿用主模型）
            "web",
            "", // chat_id：detached 不经通道路由，占位统一
            tools_profile,
            1, // 深度 = 直接子代理（与主代理 spawn 同级；SpawnFn 侧 DetachedOpts.depth）
            false,
            role,
        )
        .await
    }
}
