//! 会话资源盘点 provider——cron（资源看板类目 #4「定时器」）。
//!
//! 设计真相源：`docs/PLAN/2026-11-17_会话资源盘点看板计划.md` §3.4。
//! 构造收 `Option<Arc<..>>`（与 AppState.cron 持有形态逐字节一致），
//! 装配点无条件注册；运行态缺席在 collect 内判，三态语义闭环在 provider
//! 内部（§3.1）。

use crate::service::CronService;
use nemesis_types::resources::{CategoryView, GlobalResourceProvider, ProviderMeta, ResourceCtx};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// 定时器 provider——遍历 cron store 全量 job，按 `payload.session_key`
/// 过滤归属本会话的条目（job 的 session_key=None = 不绑会话，不进本会话
/// 盘点）。
pub struct CronResourceProvider {
    /// 与 AppState.cron 同型。None = 服务未装配 → Unavailable。
    service: Option<Arc<Mutex<CronService>>>,
}

impl CronResourceProvider {
    pub fn new(service: Option<Arc<Mutex<CronService>>>) -> Self {
        Self { service }
    }
}

/// schedule 的人读形态（与 tasks.rs `job_to_view` 同款规则，勿自造口径）。
fn schedule_summary(schedule: &crate::service::CronSchedule) -> serde_json::Value {
    use crate::service::CronService;
    let expr = match (schedule.kind.as_str(), schedule.expr.as_deref()) {
        ("cron", Some(e)) => Some(e.to_string()),
        _ => None,
    };
    let desc = expr.as_deref().map(CronService::describe_schedule);
    json!({
        "kind": schedule.kind,
        "expr": schedule.expr,
        "every_ms": schedule.every_ms,
        "at_ms": schedule.at_ms,
        "description": desc,
    })
}

impl GlobalResourceProvider for CronResourceProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("cron", "定时器", 50, false)
    }

    fn collect(&self, ctx: &ResourceCtx) -> CategoryView {
        let Some(service) = &self.service else {
            return CategoryView::unavailable("cron 服务未装配");
        };
        let Ok(store) = service.lock() else {
            return CategoryView::unavailable("cron 服务锁被占用（poisoned）");
        };
        // include_disabled=true：禁用的定时器仍是该会话拥有的资源，照常盘点
        // （enabled 位进 summary 由前端呈现）。
        let jobs = store.list_jobs(true);
        let mut rows: Vec<serde_json::Value> = Vec::new();
        let mut total = 0usize;
        for job in jobs {
            if job.payload.session_key.as_deref() != Some(ctx.session_key) {
                continue;
            }
            total += 1;
            // 摘要带前几条（条数上限与其它 provider 一致取 5），重明细走
            // tasks.cron.list。
            if rows.len() < 5 {
                rows.push(json!({
                    "id": job.id,
                    "name": job.name,
                    "enabled": job.enabled,
                    "schedule": schedule_summary(&job.schedule),
                    "next_run_at_ms": job.state.next_run_at_ms,
                    "last_status": job.state.last_status,
                }));
            }
        }
        if total == 0 {
            return CategoryView::empty();
        }
        CategoryView::ok(
            total,
            json!({ "jobs": rows, "omitted": total.saturating_sub(rows.len()) }),
        )
    }
}

#[cfg(test)]
mod tests;
