//! Tasks handler — boot/heartbeat file ops + cron scheduling via the LIVE
//! `CronService` (not raw file I/O).
//!
//! Cron commands (`cron.list/add/update/delete/toggle/run/preview`) call the
//! runtime `CronService` shared through `AppState.cron` (set by the gateway).
//! This keeps the on-disk store (`workspace/cron/jobs.json`) as the single
//! source of truth in `CronStoreData` format and makes add/edit/toggle/run
//! take effect immediately.

use crate::handlers::{
    get_opt_bool_loud, get_opt_str, get_str, read_workspace_file, require_workspace,
    write_workspace_file,
};
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_cron::{CronJob, CronJobPatch, CronSchedule, CronService};
use std::sync::Arc;

pub struct TasksHandler;

/// P1-2 (2026-08-24 UI entry gap): three-state `max_rounds` parsing shared by
/// cron.add / cron.update —
/// * key absent → `Ok(None)` = leave unchanged (update) / global default (add)
/// * `null`     → `Ok(Some(None))` = explicitly clear to the global default
/// * integer    → `Ok(Some(Some(n)))` = set
///
/// Present-but-invalid values (wrong type, negative, fractional, 0,
/// > u32::MAX) error LOUDLY. Degrading them to "absent" would silently wipe a
/// > job's budget on a typo'd payload, and 0 specifically is filtered to
/// > "no budget" downstream (`loop.rs` cron_max_rounds `.filter(|v| *v > 0)`),
/// > so accepting it would mean "unlimited" while looking like "zero" — both
/// > are rejected instead.
fn parse_max_rounds_patch(data: &serde_json::Value) -> Result<Option<Option<u32>>, String> {
    match data.get("max_rounds") {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(v) => match v.as_u64() {
            Some(n) if (1..=u32::MAX as u64).contains(&n) => Ok(Some(Some(n as u32))),
            _ => Err(format!(
                "max_rounds 必须是 1-{} 的整数或 null（收到 {v}）",
                u32::MAX
            )),
        },
    }
}

#[async_trait::async_trait]
impl ModuleHandler for TasksHandler {
    fn module_name(&self) -> &str {
        "tasks"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "boot.get",
            "boot.save",
            "heartbeat.get",
            "heartbeat.save",
            "cron.list",
            "cron.add",
            "cron.update",
            "cron.delete",
            "cron.toggle",
            "cron.run",
            "cron.preview",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            // --- boot / heartbeat (unchanged: workspace file ops) ---
            "boot.get" => {
                let workspace = require_workspace(ctx)?;
                self.get_file(workspace, "BOOT.md")
            }
            "boot.save" => {
                let workspace = require_workspace(ctx)?;
                let data = data.ok_or("missing data")?;
                let content = get_str(&data, "content")?;
                self.save_file(workspace, "BOOT.md", &content)
            }
            "heartbeat.get" => {
                let workspace = require_workspace(ctx)?;
                self.get_file(workspace, "HEARTBEAT.md")
            }
            "heartbeat.save" => {
                let workspace = require_workspace(ctx)?;
                let data = data.ok_or("missing data")?;
                let content = get_str(&data, "content")?;
                self.save_file(workspace, "HEARTBEAT.md", &content)
            }
            // --- cron: all go through the live CronService ---
            "cron.list" => {
                let svc = require_cron(ctx)?;
                self.cron_list(&svc)
            }
            "cron.add" => {
                let svc = require_cron(ctx)?;
                let data = data.ok_or("missing data")?;
                self.cron_add(&svc, &data)
            }
            "cron.update" => {
                let svc = require_cron(ctx)?;
                let data = data.ok_or("missing data")?;
                self.cron_update(&svc, &data)
            }
            "cron.delete" => {
                let svc = require_cron(ctx)?;
                let data = data.ok_or("missing data")?;
                let id = get_str(&data, "id")?;
                self.cron_delete(&svc, &id)
            }
            "cron.toggle" => {
                let svc = require_cron(ctx)?;
                let data = data.ok_or("missing data")?;
                let id = get_str(&data, "id")?;
                self.cron_toggle(&svc, &id)
            }
            "cron.run" => {
                let svc = require_cron(ctx)?;
                let data = data.ok_or("missing data")?;
                let id = get_str(&data, "id")?;
                self.cron_run(&svc, &id)
            }
            "cron.preview" => {
                let data = data.ok_or("missing data")?;
                let expr = get_str(&data, "cron")?;
                Ok(Some(cron_preview(&expr)))
            }
            _ => Err(format!("unknown command: tasks.{}", cmd)),
        }
    }
}

/// 系统内部作业的用户面变更闸（`board-ap:` / `memory-dreaming:` 保留命名
/// 空间，单一真相源 [`nemesis_cron::reserved_prefix_hit`]）：这些作业由
/// 看板 autopilot 规则与 `memory.dreaming` 配置各自拥有真相源，gateway
/// 运行中经本页删除/停用/改排程会静默破坏挂载——启动同步（仅 gateway
/// 启动时跑一次）之前规则/排程一直停摆，且规则侧「已挂载」状态按
/// `cron_job_id` 判定会继续说谎。管理走专属面：看板 Autopilot 面板 /
/// Memory 设置页（内部装配路径 arm/sync/disarm 直走 service 层，不经本闸）。
/// 任务不存在时放行（由后续 remove/toggle/patch 给出既有的 not-found 语义）。
fn ensure_user_job(svc: &Arc<std::sync::Mutex<CronService>>, id: &str) -> Result<(), String> {
    let reserved = svc
        .lock()
        .unwrap()
        .get_job(id)
        .map(|j| nemesis_cron::reserved_prefix_hit(&j.name).is_some())
        .unwrap_or(false);
    if reserved {
        return Err(format!(
            "任务 {id} 是系统内部作业（保留命名空间），不能从定时任务页删除/停用/修改\
             ——看板定时规则在 Autopilot 面板管理，记忆 dreaming 在 Memory 设置页管理"
        ));
    }
    Ok(())
}

impl TasksHandler {
    fn get_file(
        &self,
        workspace: &str,
        filename: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let content = read_workspace_file(workspace, filename)?;
        Ok(Some(
            serde_json::json!({ "filename": filename, "content": content }),
        ))
    }

    fn save_file(
        &self,
        workspace: &str,
        filename: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        write_workspace_file(workspace, filename, content)?;
        Ok(Some(
            serde_json::json!({ "saved": true, "filename": filename }),
        ))
    }

    fn cron_list(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
    ) -> Result<Option<serde_json::Value>, String> {
        let jobs = svc.lock().unwrap().list_jobs(true);
        let views: Vec<_> = jobs.iter().map(job_to_view).collect();
        let total = views.len();
        Ok(Some(serde_json::json!({ "jobs": views, "total": total })))
    }

    fn cron_add(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str(data, "name")?;
        let cron = get_str(data, "cron")?;
        // Validate the cron expression before creating the job.
        CronService::validate_schedule(&cron)?;
        let channel = get_opt_str(data, "channel");
        let to = get_opt_str(data, "to");
        let session_key = get_opt_str(data, "session_key");
        // T3 (U12): optional per-fire tool-round budget (continuation turns).
        // Absent/null → global default; invalid present values rejected.
        let max_rounds = parse_max_rounds_patch(data)?.flatten();
        let prompt = get_opt_str(data, "prompt").unwrap_or_default();
        let enabled = get_opt_bool_loud(data, "enabled")?.unwrap_or(true);
        let schedule = cron_expr_to_schedule(&cron);
        let job = svc.lock().unwrap().add_job_ext(
            &name,
            schedule,
            &prompt,
            true,
            channel.as_deref(),
            to.as_deref(),
            session_key.as_deref(),
            max_rounds,
            enabled,
        )?;
        Ok(Some(
            serde_json::json!({ "added": true, "job": job_to_view(&job) }),
        ))
    }

    fn cron_update(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let id = get_str(data, "id")?;
        ensure_user_job(svc, &id)?;
        let patch = CronJobPatch {
            name: data.get("name").and_then(|v| v.as_str()).map(String::from),
            schedule: data
                .get("cron")
                .and_then(|v| v.as_str())
                .map(cron_expr_to_schedule),
            message: data
                .get("prompt")
                .and_then(|v| v.as_str())
                .map(String::from),
            channel: data
                .get("channel")
                .and_then(|v| v.as_str())
                .map(String::from),
            to: data.get("to").and_then(|v| v.as_str()).map(String::from),
            session_key: data
                .get("session_key")
                .and_then(|v| v.as_str())
                .map(String::from),
            enabled: get_opt_bool_loud(data, "enabled")?,
            // P1-2: three states via parse_max_rounds_patch — key absent =
            // leave unchanged, null = clear (global default), 1..=u32::MAX =
            // set; present-but-invalid is rejected loudly (would otherwise
            // degrade to "clear" and silently wipe the job's budget).
            max_rounds: parse_max_rounds_patch(data)?,
        };
        // Validate cron expr if a new one is provided.
        if let Some(ref sched) = patch.schedule
            && let Some(ref e) = sched.expr
        {
            CronService::validate_schedule(e)?;
        }
        let job = svc.lock().unwrap().patch_job(&id, &patch)?;
        Ok(Some(
            serde_json::json!({ "updated": true, "job": job_to_view(&job) }),
        ))
    }

    fn cron_delete(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        ensure_user_job(svc, id)?;
        match svc.lock().unwrap().remove_job(id) {
            Ok(true) => Ok(Some(serde_json::json!({ "deleted": true, "id": id }))),
            Ok(false) => Err(format!("cron job '{}' not found", id)),
            Err(e) => Err(format!("cron job '{}' delete failed: {}", id, e)),
        }
    }

    fn cron_toggle(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        ensure_user_job(svc, id)?;
        let enabled = svc.lock().unwrap().toggle_job(id)?;
        Ok(Some(serde_json::json!({ "id": id, "enabled": enabled })))
    }

    /// "Run now": fires the job's on_job handler immediately (without advancing
    /// the schedule). With execute_job now actually invoking on_job, this
    /// triggers the agent for real.
    fn cron_run(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        svc.lock().unwrap().execute_job(id)?;
        Ok(Some(serde_json::json!({ "ran": true, "id": id })))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Get the live CronService handle from AppState, or error if not injected.
fn require_cron(ctx: &RequestContext) -> Result<Arc<std::sync::Mutex<CronService>>, String> {
    ctx.state
        .cron
        .clone()
        .ok_or_else(|| "cron service not available".to_string())
}

/// Map a cron expression string into a `kind="cron"` schedule.
fn cron_expr_to_schedule(expr: &str) -> CronSchedule {
    CronSchedule {
        kind: "cron".to_string(),
        at_ms: None,
        every_ms: None,
        expr: Some(expr.to_string()),
        tz: None,
    }
}

/// Project a stored `CronJob` into the frontend-facing view, adding a
/// human-readable `description` (from `describe_schedule`).
fn job_to_view(job: &CronJob) -> serde_json::Value {
    let expr = job.schedule.expr.clone().unwrap_or_default();
    let description = if expr.is_empty() {
        String::new()
    } else {
        CronService::describe_schedule(&expr)
    };
    serde_json::json!({
        "id": job.id,
        "name": job.name,
        "cron": expr,
        "channel": job.payload.channel,
        "to": job.payload.to,
        "session_key": job.payload.session_key,
        "prompt": job.payload.message,
        // P1-2: current per-job budget (null = global default applies).
        "max_rounds": job.payload.max_rounds,
        "enabled": job.enabled,
        "description": description,
        "next_run_at_ms": job.state.next_run_at_ms,
        "last_run_at_ms": job.state.last_run_at_ms,
        "last_status": job.state.last_status,
        "last_error": job.state.last_error,
        "history": job.state.history,
        "created_at_ms": job.created_at_ms,
        "updated_at_ms": job.updated_at_ms,
    })
}

/// Live preview of a cron expression: validity + human-readable description +
/// next-run timestamp (ms). Powers the UI's "next run" hint as the user types.
fn cron_preview(expr: &str) -> serde_json::Value {
    match CronService::validate_schedule(expr) {
        Err(e) => serde_json::json!({
            "valid": false,
            "description": e,
            "next_run_at_ms": serde_json::Value::Null,
        }),
        Ok(()) => {
            let desc = CronService::describe_schedule(expr);
            let now_ms = chrono::Local::now().timestamp_millis();
            let sched = cron_expr_to_schedule(expr);
            let next = nemesis_cron::service::compute_next_run(&sched, now_ms);
            serde_json::json!({
                "valid": true,
                "description": desc,
                "next_run_at_ms": next,
            })
        }
    }
}

#[cfg(test)]
mod tests;
