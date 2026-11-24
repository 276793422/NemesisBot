//! 用户习惯画像 WSAPI（Forge 化重构，2026-11-24：API 面迁
//! `nemesis-forge::userprofile`——存储/挖掘/反思/遥测同族于 Forge）。
//!
//! 命令：`profile.list` / `profile.veto` / `profile.forget` / `profile.pin`
//! / `profile.enable` / `profile.disable` / `profile.route`
//! / `profile.telemetry_get` / `profile.telemetry_set` / `profile.stats`
//! / `profile.observations` / `profile.observation_respond`
//! / `profile.prune_queue`。
//!
//! 语义裁决：习惯生成**免确认**自动生效；本 handler 只承担**后门动作**
//! （veto/forget/pin）与开关管理（enable/disable）。展示面走 `list`
//! （HabitEntry 序列化自带 steps/evidence/auto 结构化字段）。
//! 开关权威 = config.json `user_profile.enabled`（live 缓存优先）。
//!
//! `profile.observations` / `profile.observation_respond`：接口二·待确认
//! 观察（2026-11-24）——反思通道低于置信度闸的边缘结论交人工裁决；
//! `observation_respond {id, approved}` 确认 = `ingest_external_entry`
//! （source=observation）入画像，忽略 = 负样本事件回流。
//!
//! `profile.prune_queue {older_than_days}`：队列清理**底层接口**（UI 按钮
//! 后接；显式用户动作，不挂 sweep 自动删数据）——清理教训队列（acked 且
//! 早于 cutoff）与观察队列（已裁决且早于 cutoff），pending 永不动。
//!
//! `profile.route`：前端 router.afterEach 页面路由遥测（例 2「使用集群前
//! 先去诊断页」的页面足迹信号），经 telemetry::record_wsapi_call 以
//! `module="route"` 落 wsapi_usage.jsonl。前端恒发、后端按 feature/开关
//! 门控落盘——发送失败不影响导航。
//!
//! `profile.telemetry_get/set`：config `user_profile.ui_telemetry` 开关
//! （缺省 true；存储即隐私闸——关 = 不落盘，不止不挖掘）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_forge::userprofile::{store, surface, telemetry};
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

    fn ui_telemetry_now(&self, ctx: &RequestContext) -> bool {
        self.home(ctx)
            .ok()
            .and_then(|h| crate::handlers::config::load_config(&h).ok())
            .and_then(|cfg| cfg.user_profile)
            .map(|u| u.ui_telemetry_enabled_or(true))
            .unwrap_or(true)
    }

    fn list(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        let ws = self.workspace(ctx)?;
        let profile = surface::list(&ws);
        Ok(Some(json!({
            "enabled": self.enabled_now(ctx),
            "ui_telemetry": self.ui_telemetry_now(ctx),
            "enabled_at": profile.enabled_at,
            "habits": profile.habits,
        })))
    }

    fn veto(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let ws = self.workspace(ctx)?;
        let hit = surface::veto(&ws, &sub)?;
        Ok(Some(json!({ "vetoed": hit })))
    }

    fn forget(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let ws = self.workspace(ctx)?;
        let hit = surface::forget(&ws, &sub)?;
        Ok(Some(json!({ "forgotten": hit })))
    }

    fn pin(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let pinned = data
            .and_then(|d| d.get("pinned"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let ws = self.workspace(ctx)?;
        let hit = surface::pin(&ws, &sub, pinned)?;
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
        // 截断上限与 sweep 同源：config 现读 max_entries。
        let max_entries = crate::handlers::config::load_config(&home)
            .ok()
            .and_then(|cfg| cfg.user_profile)
            .map(|u| u.max_entries_or(store::MAX_ENTRIES))
            .unwrap_or(store::MAX_ENTRIES);
        let profile = surface::enable(&ws, max_entries)?;
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

    /// 页面路由遥测（前端 router.afterEach 上报；fire-and-forget）。
    fn route(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let path = data
            .and_then(|d| d.get("path"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing or empty 'path'".to_string())?;
        // 主开关关 = 页面足迹一并停录（隐私闸统一，不止挖掘停）。
        if !self.enabled_now(ctx) {
            return Ok(Some(json!({ "recorded": false, "reason": "disabled" })));
        }
        telemetry::record_wsapi_call(
            ctx.workspace.as_deref(),
            ctx.home.as_deref(),
            "route",
            path,
            true,
        );
        Ok(Some(json!({ "recorded": true })))
    }

    /// 挖掘/分流统计面（批 D Dashboard 展示；只读，永不注入）。
    fn stats(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        let ws = self.workspace(ctx)?;
        Ok(Some(json!({
            "enabled": self.enabled_now(ctx),
            "stats": surface::collect_stats(&ws),
        })))
    }

    fn telemetry_get(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        Ok(Some(json!({
            "ui_telemetry": self.ui_telemetry_now(ctx),
            "enabled": self.enabled_now(ctx),
        })))
    }

    fn telemetry_set(
        &self,
        ctx: &RequestContext,
        data: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        let enabled = data
            .and_then(|d| d.get("enabled"))
            .and_then(|v| v.as_bool())
            .ok_or_else(|| "missing boolean 'enabled'".to_string())?;
        let home = self.home(ctx)?;
        crate::handlers::config::update_config(&home, |cfg| {
            let entry = cfg
                .user_profile
                .get_or_insert_with(nemesis_config::UserProfileConfig::default);
            entry.ui_telemetry = Some(enabled);
            Ok(())
        })?;
        Ok(Some(json!({ "ui_telemetry": enabled })))
    }

    /// 待确认观察列表（接口二：pending 提名 + 三态计数）。
    fn observations(&self, ctx: &RequestContext) -> Result<Option<Value>, String> {
        let ws = self.workspace(ctx)?;
        let pending = surface::pending_observations(&ws, 50);
        let (p, c, d) = nemesis_forge::userprofile::bridge::observation_stats(&ws);
        Ok(Some(json!({
            "observations": pending,
            "counts": { "pending": p, "confirmed": c, "dismissed": d },
        })))
    }

    /// 裁决观察提名：`{id, approved: bool}`——确认入画像 / 忽略负样本回流。
    fn observation_respond(
        &self,
        ctx: &RequestContext,
        data: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        let id = data
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing or empty 'id'".to_string())?;
        let approved = data
            .and_then(|d| d.get("approved"))
            .and_then(|v| v.as_bool())
            .ok_or_else(|| "missing boolean 'approved'".to_string())?;
        let ws = self.workspace(ctx)?;
        let resolved = if approved {
            surface::confirm_observation(&ws, id)?
        } else {
            surface::dismiss_observation(&ws, id)?.map(|_| ())
        };
        Ok(Some(json!({
            "resolved": resolved.is_some(),
            "approved": approved,
        })))
    }

    /// 队列清理（底层接口；显式用户动作——UI 按钮后接，不挂 sweep）。
    /// `older_than_days ≥ 1`：清理两个队列中早于 cutoff 的**已消费/已裁决**
    /// 条目（pending 永不动）。
    fn prune_queue(
        &self,
        ctx: &RequestContext,
        data: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        let days = data
            .and_then(|d| d.get("older_than_days"))
            .and_then(|v| v.as_i64())
            .ok_or_else(|| "missing integer 'older_than_days'".to_string())?;
        if days < 1 {
            return Err("older_than_days must be >= 1".to_string());
        }
        let ws = self.workspace(ctx)?;
        let cutoff = chrono::Utc::now().timestamp_millis() - days * 24 * 60 * 60 * 1000;
        let pruned_lessons =
            nemesis_forge::userprofile::bridge::prune_lessons_older_than(&ws, cutoff)?;
        let pruned_observations =
            nemesis_forge::userprofile::bridge::prune_observations_older_than(&ws, cutoff)?;
        Ok(Some(json!({
            "pruned_lessons": pruned_lessons,
            "pruned_observations": pruned_observations,
        })))
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
        &[
            "list",
            "veto",
            "forget",
            "pin",
            "enable",
            "disable",
            "route",
            "telemetry_get",
            "telemetry_set",
            "stats",
            "observations",
            "observation_respond",
            "prune_queue",
        ]
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
            "route" => self.route(ctx, data.as_ref()),
            "telemetry_get" => self.telemetry_get(ctx),
            "telemetry_set" => self.telemetry_set(ctx, data.as_ref()),
            "stats" => self.stats(ctx),
            "observations" => self.observations(ctx),
            "observation_respond" => self.observation_respond(ctx, data.as_ref()),
            "prune_queue" => self.prune_queue(ctx, data.as_ref()),
            _ => Err(format!("unknown command: profile.{cmd}")),
        }
    }
}
