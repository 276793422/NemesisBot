//! Sandbox handler — Sandboxie install / status / start commands for the
//! Sandbox management page.
//!
//! Commands: `overview` (P5 platform-adaptive summary), `status`, `check`,
//! `pending`, `commit`, `delete`, `install_7z`, `install_sandboxie`, `start`,
//! `stop`, `open_box`, `open_explorer`, `set_network`, `set_config` (P5
//! field-wise executor switches).
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
    // resolve_home() joins `.nemesisbot` onto the env var, so pass the PARENT:
    // `home` here is already `<...>/.nemesisbot` — passing it directly made the
    // child resolve `<...>/.nemesisbot/.nemesisbot` (double-join bug; P3-2
    // catalog_update hit the same trap).
    let env_home = home.parent().unwrap_or(home);
    let output = tokio::time::timeout(
        Duration::from_secs(300),
        tokio::process::Command::new(&exe)
            .arg("sandbox")
            .arg(cmd)
            .env("NEMESISBOT_HOME", env_home)
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

/// Apply a mutation to the `executor` config section, preserving sibling fields.
/// Gateway mode goes through the process-wide ConfigStore (in-memory + persist);
/// CLI/no-store mode falls back to a direct read-merge-write of config.json. Both
/// paths merge field-by-field, so editing one switch (e.g. allow_network) never
/// clobbers the others (e.g. enabled/sandbox) — this is the fix for the earlier
/// overwrite bug where `start`/`stop` reset allow_network to false.
fn update_executor<F: FnOnce(&mut nemesis_config::ExecutorSeparationConfig)>(
    home: &std::path::Path,
    f: F,
) -> Result<(), String> {
    if let Some(store) = nemesis_config::global() {
        return store
            .update(|c| {
                let e = c.executor.get_or_insert(Default::default());
                f(e);
            })
            .map_err(|e| format!("update executor config: {e}"));
    }
    // CLI / no-store fallback: read-merge-write config.json field by field.
    let config_path = home.join("config.json");
    let raw =
        std::fs::read_to_string(&config_path).map_err(|e| format!("read config.json: {e}"))?;
    let mut val: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse config.json: {e}"))?;
    // Read the existing executor section FIRST (immutable borrow), before the
    // mutable object borrow below — borrowing `val` both ways at once won't compile.
    let existing = val
        .get("executor")
        .and_then(|v| {
            serde_json::from_value::<nemesis_config::ExecutorSeparationConfig>(v.clone()).ok()
        });
    let obj = val
        .as_object_mut()
        .ok_or_else(|| "config.json is not a JSON object".to_string())?;
    // Start from the existing executor section (if any) so unrelated fields survive.
    let mut e = existing.unwrap_or_default();
    f(&mut e);
    obj.insert(
        "executor".into(),
        serde_json::to_value(&e).map_err(|e| format!("serialize executor: {e}"))?,
    );
    let out =
        serde_json::to_string_pretty(&val).map_err(|e| format!("serialize config.json: {e}"))?;
    std::fs::write(&config_path, out).map_err(|e| format!("write config.json: {e}"))?;
    Ok(())
}

/// Read the current `executor.allow_network` (the box-network switch) for UI
/// display. ConfigStore in gateway mode, direct disk read otherwise. Defaults to
/// false (network blocked) when unset.
fn current_allow_network(home: &std::path::Path) -> bool {
    current_executor(home).allow_network
}

/// Read the whole `executor` section (all four switches: enabled / sandbox /
/// allow_network / strict) — single source for `overview` display and
/// `set_config` echo. Same dual-path resolution as [`current_allow_network`];
/// defaults mirror `ExecutorSeparationConfig::default()` (all false).
fn current_executor(home: &std::path::Path) -> nemesis_config::ExecutorSeparationConfig {
    if let Some(store) = nemesis_config::global() {
        let handle = store.handle();
        let c = handle.read();
        return c.executor.clone().unwrap_or_default();
    }
    let Ok(raw) = std::fs::read_to_string(home.join("config.json")) else {
        return Default::default();
    };
    serde_json::from_str::<serde_json::Value>(&raw)
        .ok()
        .and_then(|v| v.get("executor").cloned())
        .and_then(|e| serde_json::from_value::<nemesis_config::ExecutorSeparationConfig>(e).ok())
        .unwrap_or_default()
}

fn set_executor_config(home: &std::path::Path, enabled: bool, sandbox: bool) -> Result<(), String> {
    update_executor(home, |e| {
        e.enabled = enabled;
        e.sandbox = sandbox;
    })
}

/// Parse the shared `{ all?: bool, files?: string[] }` selection arg used by
/// `commit` and `delete`. `files` entries are real-path substrings matched
/// case-insensitively by [`select_box_files`].
fn parse_selection(d: &serde_json::Value) -> (bool, Vec<String>) {
    let all = d.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    let files: Vec<String> = d
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    (all, files)
}

/// Enumerate the box and keep either ALL pending files (`all=true`) or those
/// whose real path contains one of `files` (case-insensitive substring). The
/// real paths come straight from [`enumerate_box`], so callers can only target
/// files that actually exist in the box — no arbitrary path injection.
fn select_box_files(
    paths: &nemesis_sandbox::SandboxPaths,
    all: bool,
    files: &[String],
) -> Result<Vec<nemesis_sandbox::pending::PendingFile>, String> {
    let up = user_profile();
    let pending = nemesis_sandbox::pending::enumerate_box(&paths.box_root, &up)
        .map_err(|e| format!("enumerate box: {e}"))?;
    if all {
        return Ok(pending);
    }
    let needles: Vec<String> = files.iter().map(|s| s.to_lowercase()).collect();
    let selected = pending
        .into_iter()
        .filter(|p| {
            let rp = p.real_path.to_string_lossy().to_lowercase();
            needles.iter().any(|n| rp.contains(n))
        })
        .collect();
    Ok(selected)
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
                let ready = matches!(sbiesvc, ServiceState::Running) && start_exe_present;
                Ok(Some(serde_json::json!({
                    "sbiesvc": format!("{:?}", sbiesvc),
                    "sbiedrv": format!("{:?}", sbiedrv),
                    "start_exe_present": start_exe_present,
                    "ready": ready,
                    "allow_network": current_allow_network(&home),
                    "box_root": paths.box_root.to_string_lossy(),
                })))
            }
            // P5-1/P5-2 平台自适应总览：一次调用回答「我在什么平台 / 四个
            // executor 开关现在是什么（live）/ 后端探测看到什么 / 沙盒执行现在
            // 是否真的会被兑现」。Windows → Sandboxie 探测（kind=sandboxie）；
            // 其他平台 → 用户态后端逐个探测（kind=userland，含每个后端的
            // 可用性 + 缺口/原因）。`ready` 与 exec_world 装配的就绪语义一致
            // （Windows = Start.exe + SbieSvc Running + engine_owned；非
            // Windows = detect_backend() 选中了后端）。
            "overview" => {
                let executor = current_executor(&home);
                let platform = if cfg!(target_os = "windows") {
                    "windows"
                } else if cfg!(target_os = "linux") {
                    "linux"
                } else if cfg!(target_os = "macos") {
                    "macos"
                } else {
                    "other"
                };
                let (backend_probe, ready) = if cfg!(target_os = "windows") {
                    let start_exe_present = paths.start_exe().exists();
                    let sbiesvc_running =
                        matches!(nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE), ServiceState::Running);
                    let engine_owned = nemesis_sandbox::status::engine_owned(&paths);
                    (
                        serde_json::json!({
                            "kind": "sandboxie",
                            "start_exe_present": start_exe_present,
                            "sbiesvc_running": sbiesvc_running,
                            "engine_owned": engine_owned,
                        }),
                        start_exe_present && sbiesvc_running && engine_owned,
                    )
                } else {
                    let backends: Vec<_> = nemesis_sandbox::backend::probe_userland_backends()
                        .into_iter()
                        .map(|p| {
                            let (availability, detail) = match p.availability {
                                nemesis_sandbox::backend::Availability::Full => ("full", vec![]),
                                nemesis_sandbox::backend::Availability::Partial(gaps) => {
                                    ("partial", gaps)
                                }
                                nemesis_sandbox::backend::Availability::Unavailable(reason) => {
                                    ("unavailable", vec![reason])
                                }
                            };
                            serde_json::json!({
                                "name": p.name,
                                "form": format!("{:?}", p.form),
                                "availability": availability,
                                "detail": detail,
                            })
                        })
                        .collect();
                    let selected =
                        nemesis_sandbox::backend::detect_backend().map(|b| b.name().to_string());
                    (
                        serde_json::json!({
                            "kind": "userland",
                            "backends": backends,
                            "selected": selected,
                        }),
                        selected.is_some(),
                    )
                };
                Ok(Some(serde_json::json!({
                    "platform": platform,
                    "executor": {
                        "enabled": executor.enabled,
                        "sandbox": executor.sandbox,
                        "allow_network": executor.allow_network,
                        "strict": executor.strict,
                    },
                    "backend_probe": backend_probe,
                    "ready": ready,
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
                    "allow_network": current_allow_network(&home),
                })))
            }
            "pending" => {
                let up = user_profile();
                let all = nemesis_sandbox::pending::enumerate_box(&paths.box_root, &up)
                    .map_err(|e| format!("enumerate box: {e}"))?;
                let files: Vec<_> = all
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
                // Sync selected (or all) box files → real disk.
                let (all, files) = parse_selection(&data.unwrap_or_default());
                let to_commit = select_box_files(&paths, all, &files)?;
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
            "delete" => {
                // Delete selected (or all) box files — removes the in-box virtual
                // file only; the real disk path is never touched. Same selection
                // logic as "commit".
                let (all, files) = parse_selection(&data.unwrap_or_default());
                let to_delete = select_box_files(&paths, all, &files)?;
                let mut deleted = 0usize;
                let mut errors: Vec<String> = Vec::new();
                for p in &to_delete {
                    match nemesis_sandbox::pending::delete_file(p) {
                        Ok(true) => deleted += 1,
                        Ok(false) => {} // already gone — not an error
                        Err(e) => errors.push(format!("{}: {e}", p.real_path.display())),
                    }
                }
                Ok(Some(serde_json::json!({
                    "deleted": deleted,
                    "total": to_delete.len(),
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
                Ok(Some(
                    serde_json::json!({ "ok": true, "restart_required": true }),
                ))
            }
            "stop" => {
                run_cli_subcmd(&home, "stop").await?;
                set_executor_config(&home, false, false)?;
                Ok(Some(
                    serde_json::json!({ "ok": true, "restart_required": true }),
                ))
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
            // Open an explorer window INSIDE the box: Start.exe /box:NemesisBox explorer.exe.
            // Unlike open_box (host explorer viewing the box folder), this runs explorer as
            // a boxed process — anything the user launches from it (cmd, browser, installer)
            // inherits the box via Sandboxie process-tree propagation. cwd = %USERPROFILE%.
            "open_explorer" => {
                let start_exe = paths.start_exe();
                let ready = matches!(
                    nemesis_sandbox::status::service_state(nemesis_sandbox::USERMODE_SERVICE),
                    ServiceState::Running
                ) && start_exe.exists();
                if !ready {
                    return Err("sandbox not ready — start the engine first".into());
                }
                #[cfg(target_os = "windows")]
                {
                    std::process::Command::new(start_exe)
                        .arg(format!("/box:{}", nemesis_sandbox::DEFAULT_BOX_NAME))
                        .arg("explorer.exe")
                        .arg(user_profile())
                        .spawn()
                        .map_err(|e| format!("spawn in-box explorer: {e}"))?;
                }
                Ok(Some(serde_json::json!({ "ok": true })))
            }
            // Toggle the box-level network switch (AllowNetworkAccess). Persists to config,
            // rewrites Sandboxie.ini, and reloads Sandboxie so NEW box processes pick up the
            // new rule. Already-running box processes keep their old network state until
            // restarted (Sandboxie WFP caches BlockInternet per-process; reload doesn't
            // refresh it). reload is fire-and-forget — Start.exe exits right after re-reading.
            "set_network" => {
                let enabled = data
                    .as_ref()
                    .and_then(|d| d.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .ok_or("set_network requires { enabled: bool }")?;
                update_executor(&home, |e| {
                    e.allow_network = enabled;
                })?;
                nemesis_sandbox::ini::write_sandboxie_ini(
                    &paths.ini_path,
                    nemesis_sandbox::DEFAULT_BOX_NAME,
                    &paths.box_root,
                    enabled,
                )
                .map_err(|e| format!("rewrite Sandboxie.ini: {e}"))?;
                #[cfg(target_os = "windows")]
                {
                    std::process::Command::new(paths.start_exe())
                        .arg("/reload")
                        .spawn()
                        .map_err(|e| format!("spawn Start.exe /reload: {e}"))?;
                }
                Ok(Some(serde_json::json!({
                    "ok": true,
                    "allow_network": enabled,
                    "restart_hint": "newly started box processes pick this up immediately; already-open ones need reopening",
                })))
            }
            // P5-1/P5-2 executor 开关的逐字段变更（Linux 联动开关 + 全平台
            // strict 开关走这里）。每个 bool 只在**显式出现**时应用，其余兄弟
            // 字段原样保留（update_executor 逐字段合并）；出现但非 bool → 报
            // 错（不静默忽略）。注意：Windows 的盒级联网开关仍走 `set_network`
            // （那里还有 Sandboxie.ini 重写 + /reload 副作用）；Linux/macOS 的
            // allow_network 直接经此处生效（无 Sandboxie 副作用）。
            "set_config" => {
                let d = data.clone().unwrap_or_default();
                let get_bool = |key: &str| -> Result<Option<bool>, String> {
                    match d.get(key) {
                        None => Ok(None),
                        Some(v) => v.as_bool().map(Some).ok_or_else(|| {
                            format!("set_config field '{key}' must be a bool, got: {v}")
                        }),
                    }
                };
                let enabled = get_bool("enabled")?;
                let sandbox = get_bool("sandbox")?;
                let allow_network = get_bool("allow_network")?;
                let strict = get_bool("strict")?;
                if enabled.is_none()
                    && sandbox.is_none()
                    && allow_network.is_none()
                    && strict.is_none()
                {
                    return Err(
                        "set_config requires at least one of { enabled, sandbox, allow_network, strict: bool }"
                            .into(),
                    );
                }
                update_executor(&home, |e| {
                    if let Some(v) = enabled {
                        e.enabled = v;
                    }
                    if let Some(v) = sandbox {
                        e.sandbox = v;
                    }
                    if let Some(v) = allow_network {
                        e.allow_network = v;
                    }
                    if let Some(v) = strict {
                        e.strict = v;
                    }
                })?;
                let now = current_executor(&home);
                Ok(Some(serde_json::json!({
                    "ok": true,
                    "executor": {
                        "enabled": now.enabled,
                        "sandbox": now.sandbox,
                        "allow_network": now.allow_network,
                        "strict": now.strict,
                    },
                    // enabled 决定装配期是否建通道（agent 重启才生效）；sandbox
                    // /strict 是 live probe，下一次工具调用即生效。
                    "restart_hint": "executor.enabled 变更需重启 Agent；sandbox/strict 对后续工具调用实时生效",
                })))
            }
            other => Err(format!("unknown sandbox command: {other}")),
        }
    }
}

#[cfg(test)]
mod tests;

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): fake-box-tree
// selection/commit/delete paths + set_executor_config + set_config
// allow_network arm. All offline — no Sandboxie/UAC/downloads.
#[cfg(test)]
mod s10b_tests;
