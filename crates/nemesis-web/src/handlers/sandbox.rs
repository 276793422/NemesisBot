//! Sandbox handler — Sandboxie install / status / start commands for the
//! Sandbox management page.
//!
//! Commands: `status`, `check`, `pending`, `install_7z`, `install_sandboxie`,
//! `start`.
//!
//! `install_sandboxie` / `start` need admin (driver + service ops) → they spawn
//! the `nemesisbot sandbox <install|start>` CLI subprocess, which self-elevates
//! via UAC (re-uses the elevation path from the CLI). The gateway process itself
//! stays non-elevated. `status` / `check` / `pending` / `install_7z` are direct
//! (no elevation).

#![cfg(feature = "sandbox")]

use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;
use std::time::Duration;

pub struct SandboxHandler;

impl SandboxHandler {
    pub fn new() -> Self {
        Self
    }
}

fn home_of(ctx: &RequestContext) -> Result<PathBuf, String> {
    ctx.home
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| "home not configured".to_string())
}

fn user_profile() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Spawn `nemesisbot sandbox <cmd>` (self-elevating) with NEMESISBOT_HOME set,
/// await it. Generous timeout for UAC + download + KmdUtil install.
async fn run_cli_subcmd(home: &std::path::Path, cmd: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let output = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::process::Command::new(&exe)
            .arg("sandbox")
            .arg(cmd)
            .env("NEMESISBOT_HOME", home)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| format!("nemesisbot sandbox {cmd} timed out (5min)"))?
    .map_err(|e| format!("spawn nemesisbot sandbox {cmd}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "nemesisbot sandbox {cmd} failed (status {}): {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(())
}

/// Write `executor.enabled` + `executor.sandbox` into `<home>/config.json` so the
/// gateway picks up the sandbox on next start. Called by `start` (true,true) and
/// `stop` (false,false) so the UI toggle fully reflects in config.
fn set_executor_config(home: &std::path::Path, enabled: bool, sandbox: bool) -> Result<(), String> {
    let config_path = home.join("config.json");
    let raw = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("read config.json: {e}"))?;
    let mut val: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse config.json: {e}"))?;
    let entry = serde_json::json!({ "enabled": enabled, "sandbox": sandbox });
    if let Some(obj) = val.as_object_mut() {
        obj.insert("executor".into(), entry);
    } else {
        return Err("config.json is not a JSON object".into());
    }
    let out = serde_json::to_string_pretty(&val).map_err(|e| format!("serialize config.json: {e}"))?;
    std::fs::write(&config_path, out).map_err(|e| format!("write config.json: {e}"))?;
    Ok(())
}

#[async_trait::async_trait]
impl ModuleHandler for SandboxHandler {
    fn module_name(&self) -> &str {
        "sandbox"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        use nemesis_sandbox::status::ServiceState;
        let home = home_of(ctx)?;
        let paths = nemesis_sandbox::SandboxPaths::new(&home);
        match cmd {
            "status" => {
                let sbiesvc =
                    nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE);
                let sbiedrv =
                    nemesis_sandbox::status::service_state(nemesis_sandbox::DRIVER_SERVICE);
                let start_exe_present = paths.start_exe().exists();
                let ready =
                    matches!(sbiesvc, ServiceState::Running) && start_exe_present;
                Ok(Some(serde_json::json!({
                    "sbiesvc": format!("{:?}", sbiesvc),
                    "sbiedrv": format!("{:?}", sbiedrv),
                    "start_exe_present": start_exe_present,
                    "ready": ready,
                    "box_root": paths.box_root.to_string_lossy(),
                })))
            }
            "check" => {
                let (sz_available, sz_source) =
                    nemesis_sandbox::extract::seven_zip_status(&paths.runtime_dir);
                let sbiesvc =
                    nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE);
                let sbiedrv =
                    nemesis_sandbox::status::service_state(nemesis_sandbox::DRIVER_SERVICE);
                let start_exe_present = paths.start_exe().exists();
                let driver_installed = !matches!(sbiedrv, ServiceState::NotFound);
                let sbiesvc_running = matches!(sbiesvc, ServiceState::Running);
                Ok(Some(serde_json::json!({
                    "seven_zip": { "available": sz_available, "source": sz_source },
                    "sandboxie": {
                        "files_acquired": start_exe_present,
                        "driver_installed": driver_installed,
                        "sbiesvc_running": sbiesvc_running,
                    },
                })))
            }
            "pending" => {
                let workspace = ctx.workspace.as_deref().unwrap_or("");
                let ws = PathBuf::from(workspace);
                let up = user_profile();
                let pending = nemesis_sandbox::pending::pending_workspace(
                    &paths.box_root,
                    &ws,
                    &up,
                )
                .map_err(|e| format!("enumerate pending: {e}"))?;
                let files: Vec<_> = pending
                    .into_iter()
                    .map(|p| {
                        serde_json::json!({
                            "real_path": p.real_path.to_string_lossy(),
                            "size": p.size,
                        })
                    })
                    .collect();
                Ok(Some(serde_json::json!({ "files": files })))
            }
            "commit" => {
                // Sync selected (or all) pending workspace files box → real disk.
                let workspace = ctx.workspace.as_deref().unwrap_or("");
                let ws = PathBuf::from(workspace);
                let up = user_profile();
                let pending = nemesis_sandbox::pending::pending_workspace(
                    &paths.box_root,
                    &ws,
                    &up,
                )
                .map_err(|e| format!("enumerate pending: {e}"))?;
                let d = data.unwrap_or_default();
                let all = d.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
                let files: Vec<String> = d
                    .get("files")
                    .and_then(|v| serde_json::from_value(v.clone()).unwrap_or(None))
                    .unwrap_or_default();
                let needles: Vec<String> = files.iter().map(|s| s.to_lowercase()).collect();
                let to_commit: Vec<&nemesis_sandbox::pending::PendingFile> = if all {
                    pending.iter().collect()
                } else {
                    pending
                        .iter()
                        .filter(|p| {
                            let rp = p.real_path.to_string_lossy().to_lowercase();
                            needles.iter().any(|n| rp.contains(n))
                        })
                        .collect()
                };
                let mut committed = 0usize;
                let mut errors: Vec<String> = Vec::new();
                for p in &to_commit {
                    match nemesis_sandbox::pending::commit_file(p) {
                        Ok(_) => committed += 1,
                        Err(e) => errors.push(format!("{}: {e}", p.real_path.display())),
                    }
                }
                Ok(Some(serde_json::json!({
                    "committed": committed,
                    "total": to_commit.len(),
                    "errors": errors,
                })))
            }
            "install_7z" => {
                nemesis_sandbox::extract::resolve_seven_zip(&paths.runtime_dir)
                    .await
                    .map_err(|e| format!("7z install: {e}"))?;
                Ok(Some(serde_json::json!({ "ok": true })))
            }
            "install_sandboxie" => {
                // Acquire files only (download + extract) — no driver, no UAC.
                nemesis_sandbox::install::install(&paths)
                    .await
                    .map_err(|e| format!("acquire files: {e}"))?;
                Ok(Some(serde_json::json!({ "ok": true })))
            }
            "start" => {
                run_cli_subcmd(&home, "start").await?;
                set_executor_config(&home, true, true)?;
                Ok(Some(serde_json::json!({ "ok": true, "restart_required": true })))
            }
            "stop" => {
                run_cli_subcmd(&home, "stop").await?;
                set_executor_config(&home, false, false)?;
                Ok(Some(serde_json::json!({ "ok": true, "restart_required": true })))
            }
            "open_box" => {
                #[cfg(target_os = "windows")]
                {
                    std::process::Command::new("explorer")
                        .arg(&paths.box_root)
                        .spawn()
                        .map_err(|e| format!("open explorer: {e}"))?;
                }
                Ok(Some(serde_json::json!({ "ok": true })))
            }
            other => Err(format!("unknown sandbox command: {other}")),
        }
    }
}
