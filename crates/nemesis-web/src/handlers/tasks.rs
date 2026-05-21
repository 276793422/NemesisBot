//! Tasks handler — boot.get/boot.save/heartbeat.get/heartbeat.save/cron.list/cron.add/cron.update/cron.delete.

use crate::handlers::{read_workspace_file, require_workspace, write_workspace_file};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

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
        let workspace = require_workspace(ctx)?;
        match cmd {
            "boot.get" => self.get_file(workspace, "BOOT.md"),
            "boot.save" => {
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.save_file(workspace, "BOOT.md", &content)
            }
            "heartbeat.get" => self.get_file(workspace, "HEARTBEAT.md"),
            "heartbeat.save" => {
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.save_file(workspace, "HEARTBEAT.md", &content)
            }
            "cron.list" => self.cron_list(workspace),
            "cron.add" => {
                let data = data.ok_or("missing data")?;
                self.cron_add(workspace, &data)
            }
            "cron.update" => {
                let data = data.ok_or("missing data")?;
                self.cron_update(workspace, &data)
            }
            "cron.delete" => {
                let data = data.ok_or("missing data")?;
                let id = crate::handlers::get_str(&data, "id")?;
                self.cron_delete(workspace, &id)
            }
            _ => Err(format!("unknown command: tasks.{}", cmd)),
        }
    }
}

fn cron_jobs_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("cron/jobs.json")
}

fn load_cron_jobs(path: &std::path::Path) -> Result<Vec<serde_json::Value>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("failed to read jobs: {}", e))?;
    serde_json::from_str(&content).map_err(|e| format!("invalid jobs.json: {}", e))
}

fn save_cron_jobs(path: &std::path::Path, jobs: &[serde_json::Value]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("failed to create cron dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(jobs).map_err(|e| format!("failed to serialize jobs: {}", e))?;
    std::fs::write(path, json).map_err(|e| format!("failed to write jobs: {}", e))?;
    Ok(())
}

impl TasksHandler {
    fn get_file(&self, workspace: &str, filename: &str) -> Result<Option<serde_json::Value>, String> {
        let content = read_workspace_file(workspace, filename)?;
        Ok(Some(serde_json::json!({ "filename": filename, "content": content })))
    }

    fn save_file(
        &self,
        workspace: &str,
        filename: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        write_workspace_file(workspace, filename, content)?;
        Ok(Some(serde_json::json!({ "saved": true, "filename": filename })))
    }

    fn cron_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = cron_jobs_path(workspace);
        let jobs = load_cron_jobs(&path)?;
        Ok(Some(serde_json::json!({ "jobs": jobs, "total": jobs.len() })))
    }

    fn cron_add(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        let cron = crate::handlers::get_str(data, "cron")?;
        let channel = crate::handlers::get_opt_str(data, "channel");
        let prompt = crate::handlers::get_opt_str(data, "prompt").unwrap_or_default();
        let enabled = data.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);

        let id = format!("cron_{}", uuid::Uuid::new_v4().to_string().replace('-', "")[..8].to_string());

        let job = serde_json::json!({
            "id": id,
            "name": name,
            "cron": cron,
            "channel": channel,
            "prompt": prompt,
            "enabled": enabled,
            "created_at": chrono::Utc::now().to_rfc3339(),
        });

        let path = cron_jobs_path(workspace);
        let mut jobs = load_cron_jobs(&path)?;
        jobs.push(job.clone());
        save_cron_jobs(&path, &jobs)?;
        Ok(Some(serde_json::json!({ "added": true, "job": job })))
    }

    fn cron_update(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let id = crate::handlers::get_str(data, "id")?;
        let path = cron_jobs_path(workspace);
        let mut jobs = load_cron_jobs(&path)?;

        let job = jobs
            .iter_mut()
            .find(|j| j.get("id").and_then(|v| v.as_str()) == Some(&id))
            .ok_or_else(|| format!("cron job '{}' not found", id))?;

        // Update provided fields
        if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
            job["name"] = serde_json::Value::String(name.to_string());
        }
        if let Some(cron) = data.get("cron").and_then(|v| v.as_str()) {
            job["cron"] = serde_json::Value::String(cron.to_string());
        }
        if let Some(channel) = data.get("channel") {
            job["channel"] = channel.clone();
        }
        if let Some(prompt) = data.get("prompt").and_then(|v| v.as_str()) {
            job["prompt"] = serde_json::Value::String(prompt.to_string());
        }
        if let Some(enabled) = data.get("enabled").and_then(|v| v.as_bool()) {
            job["enabled"] = serde_json::Value::Bool(enabled);
        }
        job["updated_at"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());

        save_cron_jobs(&path, &jobs)?;
        Ok(Some(serde_json::json!({ "updated": true, "id": id })))
    }

    fn cron_delete(
        &self,
        workspace: &str,
        id: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = cron_jobs_path(workspace);
        let mut jobs = load_cron_jobs(&path)?;
        let before = jobs.len();
        jobs.retain(|j| j.get("id").and_then(|v| v.as_str()) != Some(id));
        if jobs.len() == before {
            return Err(format!("cron job '{}' not found", id));
        }
        save_cron_jobs(&path, &jobs)?;
        Ok(Some(serde_json::json!({ "deleted": true, "id": id })))
    }
}
