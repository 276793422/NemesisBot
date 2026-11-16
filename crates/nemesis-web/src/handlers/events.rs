//! 事件驱动唤醒 handler — `events.status` / `events.rules.get` /
//! `events.rules.save`（项1 P2，2026-11-16 质变计划）。
//!
//! 读 `ctx.state.events_snapshot`（gateway 启动时注入的泵观测快照句柄——
//! 泵任务每周期写入，本 handler 读）+ workspace config/events.json（规则
//! 真相源文件；save 走 nemesis-events 校验 + 原子落盘，泵 mtime 检测 ≤250ms
//! 热重载）。None = 泵未装配（feature 关 / headless / 测试装配）→ 诚实报错。
//!
//! estop 语义：投递冻结在泵侧判定（冻结期 pending 诚实丢弃），本面只读
//! 统计 + 规则文件管理，无独立 estop 闸（规则写入不触发 agent 活动）。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct EventsHandler;

/// workspace config/events.json 路径（与 gateway 泵同源推导——workspace_dir
/// 注入 AppState.workspace；None = 诚实报错）。
fn rules_path(ctx: &RequestContext) -> Result<std::path::PathBuf, String> {
    let ws = ctx
        .state
        .workspace
        .as_ref()
        .ok_or("workspace not available")?;
    Ok(std::path::Path::new(ws).join("config").join("events.json"))
}

#[async_trait::async_trait]
impl ModuleHandler for EventsHandler {
    fn module_name(&self) -> &str {
        "events"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["status", "rules.get", "rules.save"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "status" => {
                let snap = ctx
                    .state
                    .events_snapshot
                    .as_ref()
                    .ok_or("events manager not available (feature off or not wired)")?;
                let snapshot = snap.read().clone();
                Ok(Some(
                    serde_json::to_value(&snapshot).map_err(|e| e.to_string())?,
                ))
            }
            "rules.get" => {
                let path = rules_path(ctx)?;
                match nemesis_events::load_rules(&path) {
                    Ok(loaded) => {
                        let rules: Vec<&nemesis_events::EventRule> = loaded.rules.iter().collect();
                        Ok(Some(serde_json::json!({
                            "rules": rules,
                            "warnings": loaded.warnings,
                        })))
                    }
                    Err(e) => Err(format!("rules file unreadable ({}): {e}", path.display())),
                }
            }
            "rules.save" => {
                let path = rules_path(ctx)?;
                let data = data.ok_or("missing data")?;
                let rules: Vec<nemesis_events::EventRule> = serde_json::from_value(
                    data.get("rules")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| format!("rules schema invalid: {e}"))?;
                nemesis_events::save_rules(&path, &rules)
                    .map_err(|e| format!("save failed: {e}"))?;
                // 落盘后回读校验（泵会以同一入口重载）——warnings 随响应给前端
                // 展示（重复 id / 非预设 session_key 等仍会装载，只是带告警）。
                let warnings = match nemesis_events::load_rules(&path) {
                    Ok(loaded) => loaded.warnings,
                    Err(e) => vec![format!("回读校验失败（泵将保留旧规则）: {e}")],
                };
                tracing::info!(
                    "[Web/WSAPI] events rules saved: {} rule(s), {} warning(s) -> {}",
                    rules.len(),
                    warnings.len(),
                    path.display()
                );
                Ok(Some(
                    serde_json::json!({ "saved": true, "warnings": warnings }),
                ))
            }
            _ => Err(format!("unknown command: events.{}", cmd)),
        }
    }
}

#[cfg(all(test, feature = "events"))]
mod tests;
