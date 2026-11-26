//! 用户习惯画像 WSAPI（Forge 化重构，2026-11-24：API 面迁
//! `nemesis-forge::userprofile`——存储/挖掘/反思/遥测同族于 Forge）。
//!
//! 命令：`profile.list` / `profile.veto` / `profile.forget` / `profile.pin`
//! / `profile.enable` / `profile.disable` / `profile.route`
//! / `profile.telemetry_get` / `profile.telemetry_set` / `profile.stats`
//! / `profile.observations` / `profile.observation_respond`
//! / `profile.prune_queue` / `profile.manual_get` / `profile.manual_save`
//! / `profile.preview`。
//!
//! scope 参数化（双 scope 拆分批三，2026-11-25）：卡片类命令
//! （list/veto/forget/pin/observation_respond 确认臂/manual_get/
//! manual_save/preview）接受 `data.scope`（`"dialogue"`|`"project"`，
//! 缺省 dialogue 兼容旧前端；显式未知值 loud 拒绝）。enable/disable/
//! stats/route/telemetry_*/prune_queue/observations 无 scope——开关与
//! 统计是全局面（stats 双画像合并聚合）。
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
//! 早于 cutoff）与观察队列（已裁决且早于 cutoff），pending 永不动；
//! 2026-11-25 批 D 起同 cutoff 一并清理 signals/events/reflections 三个
//! 旁路证据文件（响应带 `pruned_signals`/`pruned_events`/
//! `pruned_reflections` 三个计数）。
//!
//! `profile.route`：前端 router.afterEach 页面路由遥测（例 2「使用集群前
//! 先去诊断页」的页面足迹信号），经 telemetry::record_wsapi_call 以
//! `module="route"` 落 wsapi_usage.jsonl。前端恒发、后端按 feature/开关
//! 门控落盘——发送失败不影响导航。
//!
//! `profile.telemetry_get/set`：config `user_profile.ui_telemetry` 开关
//! （缺省 true；存储即隐私闸——关 = 不落盘，不止不挖掘）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_forge::userprofile::store::ProfileScope;
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

    fn list(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let scope = scope_from(data)?;
        let ws = self.workspace(ctx)?;
        let profile = surface::list(&ws, scope);
        // 逐条投影 source_kind（话术 AI 化批三 §4.5：五态来源徽标的单一
        // 派生在 surface::source_kind，与 profile.stats 五桶统计同口径——
        // 前端只认这个字段，不再自行从 auto/source 推导）。
        let habits: Vec<Value> = profile
            .habits
            .iter()
            .map(|h| {
                let mut v = serde_json::to_value(h).unwrap_or_else(|_| serde_json::json!({}));
                v["source_kind"] = serde_json::json!(surface::source_kind(h));
                v
            })
            .collect();
        Ok(Some(json!({
            "scope": scope_name(scope),
            "enabled": self.enabled_now(ctx),
            "ui_telemetry": self.ui_telemetry_now(ctx),
            "enabled_at": profile.enabled_at,
            "habits": habits,
        })))
    }

    fn veto(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let scope = scope_from(data)?;
        let ws = self.workspace(ctx)?;
        // home = append_event 总闸（复查 2026-11-25 批 F3：学习关闭不落）。
        let hit = surface::veto(&ws, scope, Some(&self.home(ctx)?), &sub)?;
        Ok(Some(json!({ "vetoed": hit })))
    }

    fn forget(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let scope = scope_from(data)?;
        let ws = self.workspace(ctx)?;
        let hit = surface::forget(&ws, scope, Some(&self.home(ctx)?), &sub)?;
        Ok(Some(json!({ "forgotten": hit })))
    }

    fn pin(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let sub = content_sub(data)?;
        let scope = scope_from(data)?;
        let pinned = data
            .and_then(|d| d.get("pinned"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let ws = self.workspace(ctx)?;
        let hit = surface::pin(&ws, scope, &sub, pinned)?;
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
        let scope = scope_from(data)?;
        let resolved = if approved {
            // 确认臂带 scope（入哪份画像）；忽略臂无 scope（观察队列/
            // 负样本事件是全局面，D7）。
            surface::confirm_observation(&ws, scope, id)?
        } else {
            surface::dismiss_observation(&ws, Some(&self.home(ctx)?), id)?.map(|_| ())
        };
        Ok(Some(json!({
            "resolved": resolved.is_some(),
            "approved": approved,
        })))
    }

    /// 队列清理（底层接口；显式用户动作——UI 按钮后接，不挂 sweep）。
    /// `older_than_days ≥ 1`：清理两个队列中早于 cutoff 的**已消费/已裁决**
    /// 条目（pending 永不动）；复查 2026-11-25 批 D 扩展——同 cutoff 一并
    /// 清理 signals/events/reflections 三个无状态旁路文件（纯 append 证据
    /// 面，无 pending 概念，早于 cutoff 的行整行删除；挖掘/反思只消费
    /// 尾部窗口，清理不伤语义）。
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
        // 饱和算术（复查 2026-11-24，配置面 i64 纪律）：极端大天数不 panic
        // ——乘法饱和到 i64::MAX、减法饱和到 i64::MIN，cutoff 退化为极早时
        // 间 = 什么都不清（诚实降级，不崩 handler）。
        let cutoff = prune_cutoff_ms(days);
        let pruned_lessons =
            nemesis_forge::userprofile::bridge::prune_lessons_older_than(&ws, cutoff)?;
        let pruned_observations =
            nemesis_forge::userprofile::bridge::prune_observations_older_than(&ws, cutoff)?;
        let pruned_signals =
            nemesis_forge::userprofile::collection::prune_signals_older_than(&ws, cutoff)?;
        let pruned_events =
            nemesis_forge::userprofile::collection::prune_events_older_than(&ws, cutoff)?;
        let pruned_reflections =
            nemesis_forge::userprofile::reflection::prune_ledger_older_than(&ws, cutoff)?;
        Ok(Some(json!({
            "pruned_lessons": pruned_lessons,
            "pruned_observations": pruned_observations,
            "pruned_signals": pruned_signals,
            "pruned_events": pruned_events,
            "pruned_reflections": pruned_reflections,
        })))
    }

    /// 手动习惯回显（TAB1 上半编辑器；双 scope D8/D9）。返回
    /// `[类别] 内容` 行数组，前端 textarea 直接按行回显——行语法解析
    /// （含裁行/折叠/类别缺省）的单一真相源在 store 侧，前端零复制。
    fn manual_get(
        &self,
        ctx: &RequestContext,
        data: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        let scope = scope_from(data)?;
        let ws = self.workspace(ctx)?;
        let profile = store::try_load_profile_scope(&ws, scope)?;
        Ok(Some(json!({
            "scope": scope_name(scope),
            "lines": store::manual_lines(&profile),
        })))
    }

    /// 手动习惯保存（TAB1 上半编辑器整体替换语义；`store::manual_save`
    /// 按行 diff：新增 upsert / 消失删除，只动 source=manual 条目）。
    /// 损坏行 = Err（报行号 fail-loud，前端 toast）；撞文机器条目经
    /// `conflicts` 诚实回报（不重复建）。
    fn manual_save(
        &self,
        ctx: &RequestContext,
        data: Option<&Value>,
    ) -> Result<Option<Value>, String> {
        let scope = scope_from(data)?;
        let lines: Vec<String> = data
            .and_then(|d| d.get("lines"))
            .and_then(|v| v.as_array())
            .ok_or_else(|| "missing array 'lines'".to_string())?
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        let line_refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let ws = self.workspace(ctx)?;
        let now = chrono::Local::now().to_rfc3339();
        let out = store::manual_save(&ws, scope, &line_refs, &now)?;
        Ok(Some(json!({
            "scope": scope_name(scope),
            "added": out.added,
            "removed": out.removed,
            "removed_contents": out.removed_contents,
            "conflicts": out.conflicts,
        })))
    }

    /// 注入内容预览（TAB1 下半只读编辑器；D10）。后端渲染——安全过滤/
    /// pinned 排序/单行折叠的单一真相源在 `store::render_section`，前端
    /// 不复制渲染规则。预览只渲染**自动条目**（手动条目在上半编辑器自己
    /// 看得到），手动条数以注记行附尾（「另有手动 N 条一并注入」）。
    fn preview(&self, ctx: &RequestContext, data: Option<&Value>) -> Result<Option<Value>, String> {
        let scope = scope_from(data)?;
        let ws = self.workspace(ctx)?;
        let profile = store::try_load_profile_scope(&ws, scope)?;
        let mut auto_profile = profile.clone();
        auto_profile
            .habits
            .retain(|h| h.source != store::SOURCE_MANUAL);
        let mut text = store::render_section(&auto_profile);
        // 手动注记计数口径 = 会被注入的手动条目（active/decaying——vetoed
        // /expired 不进注入面，render_section 同款过滤）。
        let manual_count = profile
            .habits
            .iter()
            .filter(|h| h.source == store::SOURCE_MANUAL)
            .filter(|h| h.status == store::STATUS_ACTIVE || h.status == store::STATUS_DECAYING)
            .count();
        if manual_count > 0 {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("（另有手动 {manual_count} 条一并注入）"));
        }
        Ok(Some(json!({
            "scope": scope_name(scope),
            "preview": text,
            "manual_count": manual_count,
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

/// 卡片类命令的 scope 解析（批三）：`data.scope` 缺失 = 对话（兼容旧
/// 前端）；空串 = 对话；`"dialogue"`/`"project"`（大小写不敏感）；显式
/// 未知值/非字符串 loud 拒绝（不静默纠偏——前端拼错 scope 名要当场红，
/// 不是悄悄读错画像）。
fn scope_from(data: Option<&Value>) -> Result<ProfileScope, String> {
    let Some(v) = data.and_then(|d| d.get("scope")) else {
        return Ok(ProfileScope::Dialogue);
    };
    let Some(s) = v.as_str() else {
        return Err("'scope' 必须是字符串（dialogue|project）".to_string());
    };
    match s.trim().to_lowercase().as_str() {
        "" | "dialogue" => Ok(ProfileScope::Dialogue),
        "project" => Ok(ProfileScope::Project),
        other => Err(format!(
            "unknown scope '{other}'（合法值：dialogue|project，缺省 dialogue）"
        )),
    }
}

/// scope → wire 名（响应投影/前端回显用）。
fn scope_name(scope: ProfileScope) -> &'static str {
    match scope {
        ProfileScope::Dialogue => "dialogue",
        ProfileScope::Project => "project",
    }
}

/// `profile.prune_queue` 的 cutoff 计算（复查 2026-11-24 抽出纯函数便于单
/// 测）：`now − days×86400×1000`，两级饱和（BUG-20261125-006；同族纪律
/// BUG-20261107-025 decay_sweep——入参来源 i64 裸乘溢出在 debug panic /
/// release 环绕）——乘法饱和到 i64::MAX、减法饱和到 i64::MIN，极端大天数
/// 不 panic，cutoff 退化为极早时间 = 什么都不清（诚实降级；配置面 i64
/// 算术纪律同 decay_sweep/stale 安全网族）。
pub(crate) fn prune_cutoff_ms(days: i64) -> i64 {
    chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(days.saturating_mul(24 * 60 * 60 * 1000))
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
            "manual_get",
            "manual_save",
            "preview",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "list" => self.list(ctx, data.as_ref()),
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
            "manual_get" => self.manual_get(ctx, data.as_ref()),
            "manual_save" => self.manual_save(ctx, data.as_ref()),
            "preview" => self.preview(ctx, data.as_ref()),
            _ => Err(format!("unknown command: profile.{cmd}")),
        }
    }
}

#[cfg(all(test, feature = "forge"))]
mod tests;
