//! 用户习惯画像 WSAPI（自进化升级 B 线，2026-10-14）。
//!
//! 命令：`profile.list` / `profile.veto` / `profile.forget` / `profile.pin`
//! / `profile.enable` / `profile.disable`。
//!
//! 语义裁决：习惯生成**免确认**自动生效；本 handler 只承担**后门动作**
//! （veto/forget/pin）与开关管理（enable/disable）。展示面走 `list`。
//! 开关真相源 = config.json `user_profile.enabled`（live 缓存优先）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::user_profile as up;
use serde_json::{Value, json};
use std::path::PathBuf;

pub struct ProfileHandler;

impl Default for ProfileHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileHandler {
    pub fn new() -> Self {
        Self
    }

    fn workspace(&self, ctx: &RequestContext) -> Result<PathBuf, String> {
        ctx.workspace
            .clone()
            .map(PathBuf::from)
            .ok_or_else(|| "workspace not configured".to_string())
    }

    fn home(&self, ctx: &RequestContext) -> Result<String, String> {
        ctx.home
            .clone()
            .ok_or_else(|| "home not configured".to_string())
    }

    fn enabled_now(&self, ctx: &RequestContext) -> bool {
        self.home(ctx)
            .ok()
            .and_then(|h| crate::handlers::config::load_config(&h).ok())
            .and_then(|cfg| cfg.user_profile)
            .map(|u| u.enabled)
            .unwrap_or(false)
    }

    fn list(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        let ws = self.workspace(ctx)?;
        let profile = up::load_profile(&ws);
        Ok(Some(json!({
            "enabled": self.enabled_now(ctx),
            "enabled_at": profile.enabled_at,
            "habits": profile.habits,
        })))
    }

    fn veto(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let ws = self.workspace(ctx)?;
        let hit = up::veto_habit(&ws, &sub)?;
        Ok(Some(json!({ "vetoed": hit })))
    }

    fn forget(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let ws = self.workspace(ctx)?;
        let hit = up::forget_habit(&ws, &sub)?;
        Ok(Some(json!({ "forgotten": hit })))
    }

    fn pin(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let pinned = data
            .and_then(|d| d.get("pinned"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let ws = self.workspace(ctx)?;
        let hit = up::set_pinned(&ws, &sub, pinned)?;
        Ok(Some(json!({ "updated": hit, "pinned": pinned })))
    }

    fn enable(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        let ws = self.workspace(ctx)?;
        let home = self.home(ctx)?;
        // 先写 config 开关（写锁在 update_config 内），锚点后置——失败顺序
        // 安全：开关失败 = 锚点不动；锚点成功后开关必已开。
        crate::handlers::config::update_config(&home, |cfg| {
            let entry = cfg
                .user_profile
                .get_or_insert_with(nemesis_config::UserProfileConfig::default);
            entry.enabled = true;
            Ok(())
        })?;
        // 截断上限与 sweep 同源：config 现读 max_entries（BUG-20261025-20）。
        let max_entries = crate::handlers::config::load_config(&home)
            .ok()
            .and_then(|cfg| cfg.user_profile)
            .map(|u| u.max_entries_or(up::MAX_ENTRIES))
            .unwrap_or(up::MAX_ENTRIES);
        let profile = up::enable_profile(&ws, max_entries)?;
        Ok(Some(json!({
            "enabled": true,
            "enabled_at": profile.enabled_at,
        })))
    }

    fn disable(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        let home = self.home(ctx)?;
        crate::handlers::config::update_config(&home, |cfg| {
            if let Some(entry) = cfg.user_profile.as_mut() {
                entry.enabled = false;
            }
            Ok(())
        })?;
        Ok(Some(json!({ "enabled": false })))
    }
}

fn content_sub(data: Option<&Value>) -> Result<String, String> {
    data.and_then(|d| d.get("content"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "missing or empty 'content'".to_string())
}

#[async_trait::async_trait]
impl ModuleHandler for ProfileHandler {
    fn module_name(&self) -> &str {
        "profile"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "veto", "forget", "pin", "enable", "disable"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "list" => self.list(ctx),
            "veto" => self.veto(ctx, data.as_ref()),
            "forget" => self.forget(ctx, data.as_ref()),
            "pin" => self.pin(ctx, data.as_ref()),
            "enable" => self.enable(ctx),
            "disable" => self.disable(ctx),
            _ => Err(format!("unknown command: profile.{cmd}")),
        }
    }
}
