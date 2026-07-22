//! Tasks handler — boot/heartbeat file ops + cron scheduling via the LIVE
//! `CronService` (not raw file I/O).
//!
//! Cron commands (`cron.list/add/update/delete/toggle/run/preview`) call the
//! runtime `CronService` shared through `AppState.cron` (set by the gateway).
//! This keeps the on-disk store (`workspace/cron/jobs.json`) as the single
//! source of truth in `CronStoreData` format and makes add/edit/toggle/run
//! take effect immediately.

use crate::handlers::{
    get_opt_str, get_str, read_workspace_file, require_workspace, write_workspace_file,
};
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_cron::{CronJob, CronJobPatch, CronSchedule, CronService};
use std::sync::Arc;

pub struct TasksHandler;

#[async_trait::async_trait]
impl ModuleHandler for TasksHandler {
    fn module_name(&self) -> &str {
        "tasks"
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
        let prompt = get_opt_str(data, "prompt").unwrap_or_default();
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let schedule = cron_expr_to_schedule(&cron);
        let job = svc.lock().unwrap().add_job_ext(
            &name,
            schedule,
            &prompt,
            true,
            channel.as_deref(),
            to.as_deref(),
            session_key.as_deref(),
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
        let patch = CronJobPatch {
            name: data.get("name").and_then(|v| v.as_str()).map(String::from),
            schedule: data
                .get("cron")
                .and_then(|v| v.as_str())
                .map(|c| cron_expr_to_schedule(c)),
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
            enabled: data.get("enabled").and_then(|v| v.as_bool()),
        };
        // Validate cron expr if a new one is provided.
        if let Some(ref sched) = patch.schedule {
            if let Some(ref e) = sched.expr {
                CronService::validate_schedule(e)?;
            }
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
        let removed = svc.lock().unwrap().remove_job(id);
        if !removed {
            return Err(format!("cron job '{}' not found", id));
        }
        Ok(Some(serde_json::json!({ "deleted": true, "id": id })))
    }

    fn cron_toggle(
        &self,
        svc: &Arc<std::sync::Mutex<CronService>>,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
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
