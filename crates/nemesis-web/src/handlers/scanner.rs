//! Scanner handler — engine management commands for the Scanner management page.
//!
//! Commands: status, check, enable, disable, install, update_db, test,
//! engine.update_config, add, config.get, config.save

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_config::{load_scanner_config, save_scanner_config, EngineState, ScannerFullConfig};
use nemesis_security::scanner::{
    available_engines, create_engine, ClamAVEngine, InstallableEngine,
    INSTALL_STATUS_FAILED, INSTALL_STATUS_INSTALLED, INSTALL_STATUS_PENDING,
    DB_STATUS_MISSING, DB_STATUS_READY,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const DATABASE_FILE: &str = "daily.cvd";

fn active_ops() -> &'static Arc<Mutex<HashMap<String, CancellationToken>>> {
    static INSTANCE: OnceLock<Arc<Mutex<HashMap<String, CancellationToken>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

pub struct ScannerHandler {
    _priv: (),
}

impl ScannerHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ScannerHandler {
    fn module_name(&self) -> &str {
        "scanner"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let d = data.ok_or("missing data")?;
                self.config_save(workspace, &d)
            }
            "status" => self.cmd_status(workspace),
            "check" => self.cmd_check(workspace, data),
            "enable" => {
                let d = data.ok_or("missing data")?;
                let name = get_str_field(&d, "name")?;
                self.cmd_enable(workspace, &name)
            }
            "disable" => {
                let d = data.ok_or("missing data")?;
                let name = get_str_field(&d, "name")?;
                self.cmd_disable(workspace, &name)
            }
            "install" => {
                let d = data.ok_or("missing data")?;
                self.cmd_install(workspace, &d, ctx).await
            }
            "update_db" => {
                let d = data.ok_or("missing data")?;
                self.cmd_update_db(workspace, &d, ctx).await
            }
            "test" => {
                let d = data.ok_or("missing data")?;
                self.cmd_test(workspace, &d).await
            }
            "engine.update_config" => {
                let d = data.ok_or("missing data")?;
                self.cmd_engine_update_config(workspace, &d)
            }
            "add" => {
                let d = data.ok_or("missing data")?;
                self.cmd_add(workspace, &d)
            }
            "cancel" => {
                let d = data.ok_or("missing data")?;
                self.cmd_cancel(&d).await
            }
            _ => Err(format!("unknown command: scanner.{}", cmd)),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scanner_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.scanner.json")
}

/// Parse engine config JSON into the nemesis-config ClamAVEngineConfig type.
fn parse_engine_config(raw: &serde_json::Value) -> nemesis_config::ClamAVEngineConfig {
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

fn check_executables_at_path(dir: &str) -> bool {
    let path = std::path::Path::new(dir);
    let targets = if cfg!(windows) {
        &["clamd.exe", "clamscan.exe"][..]
    } else {
        &["clamd", "clamscan"][..]
    };
    targets.iter().any(|t| path.join(t).exists())
}

fn resolve_tools_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("tools")
}

fn get_str_field(data: &serde_json::Value, field: &str) -> Result<String, String> {
    data.get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("missing field: {}", field))
}

fn get_opt_str_field(data: &serde_json::Value, field: &str) -> Option<String> {
    data.get(field).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn build_engine_response(
    name: &str,
    raw_config: &serde_json::Value,
    is_enabled: bool,
) -> serde_json::Value {
    let mut engine_json = serde_json::json!({
        "name": name,
        "enabled": is_enabled,
    });
    if let Some(obj) = raw_config.as_object() {
        let map = engine_json.as_object_mut().unwrap();
        for (k, v) in obj {
            map.insert(k.clone(), v.clone());
        }
    }
    engine_json
}

fn build_all_engines_status(workspace: &str) -> Result<Vec<serde_json::Value>, String> {
    let path = scanner_config_path(workspace);
    let cfg = load_scanner_config(&path)
        .map_err(|e| format!("failed to load scanner config: {}", e))?;

    let enabled_set: HashSet<&str> = cfg.enabled.iter().map(|s| s.as_str()).collect();

    let mut engines: Vec<serde_json::Value> = cfg
        .engines
        .iter()
        .map(|(name, config)| {
            build_engine_response(name, config, enabled_set.contains(name.as_str()))
        })
        .collect();

    engines.sort_by(|a, b| {
        let a_name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let b_name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        a_name.cmp(b_name)
    });

    Ok(engines)
}

async fn mark_op_started(name: &str) -> Option<CancellationToken> {
    let mut ops = active_ops().lock().await;
    if ops.contains_key(name) {
        return None;
    }
    let token = CancellationToken::new();
    ops.insert(name.to_string(), token.clone());
    Some(token)
}

async fn mark_op_finished(name: &str) {
    let mut ops = active_ops().lock().await;
    ops.remove(name);
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

impl ScannerHandler {
    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = scanner_config_path(workspace);
        let config = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;
        let json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let config: ScannerFullConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid scanner config: {}", e))?;
        let path = scanner_config_path(workspace);
        save_scanner_config(&path, &config)
            .map_err(|e| format!("failed to save scanner config: {}", e))?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn cmd_status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let engines = build_all_engines_status(workspace)?;
        Ok(Some(serde_json::json!({ "engines": engines })))
    }

    fn cmd_check(
        &self,
        workspace: &str,
        data: Option<serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = scanner_config_path(workspace);
        let mut cfg = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;

        let target_name = data.and_then(|d| d.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()));
        let enabled_set: HashSet<&str> = cfg.enabled.iter().map(|s| s.as_str()).collect();

        let names_to_check: Vec<String> = if let Some(ref name) = target_name {
            vec![name.clone()]
        } else {
            cfg.engines.keys().cloned().collect()
        };

        let mut results = Vec::new();
        let mut changed = false;

        for name in &names_to_check {
            let raw = match cfg.engines.get(name) {
                Some(v) => v.clone(),
                None => continue,
            };
            let engine_cfg = parse_engine_config(&raw);
            let mut state = engine_cfg.state.clone();
            let is_enabled = enabled_set.contains(name.as_str());

            if is_enabled {
                let resolved_path = engine_cfg.clamav_path.clone();

                if !resolved_path.is_empty() {
                    if check_executables_at_path(&resolved_path) {
                        state.install_status = INSTALL_STATUS_INSTALLED.to_string();
                        state.install_error = String::new();
                    } else {
                        state.install_status = INSTALL_STATUS_FAILED.to_string();
                        state.install_error = format!("executable not found at {}", resolved_path);
                    }
                } else if state.install_status.is_empty() {
                    state.install_status = INSTALL_STATUS_PENDING.to_string();
                }

                let data_dir = if !engine_cfg.data_dir.is_empty() {
                    engine_cfg.data_dir.clone()
                } else if !resolved_path.is_empty() {
                    resolved_path.clone()
                } else {
                    String::new()
                };

                if !data_dir.is_empty() {
                    let db_file = std::path::Path::new(&data_dir)
                        .join("database")
                        .join(DATABASE_FILE);
                    state.db_status = if db_file.exists() {
                        DB_STATUS_READY.to_string()
                    } else {
                        DB_STATUS_MISSING.to_string()
                    };
                }

                let old_state = parse_engine_config(&raw).state;
                if state.install_status != old_state.install_status
                    || state.db_status != old_state.db_status
                    || state.install_error != old_state.install_error
                {
                    let mut updated = raw.clone();
                    if let Some(obj) = updated.as_object_mut() {
                        if let Ok(state_val) = serde_json::to_value(&state) {
                            obj.insert("state".to_string(), state_val);
                        }
                    }
                    cfg.engines.insert(name.clone(), updated);
                    changed = true;
                }
            }

            results.push(build_engine_response(name, cfg.engines.get(name).unwrap_or(&raw), is_enabled));
        }

        if changed {
            if let Err(e) = save_scanner_config(&path, &cfg) {
                tracing::warn!("Failed to save scanner state after check: {}", e);
            }
        }

        if target_name.is_some() && results.len() == 1 {
            Ok(Some(results.into_iter().next().unwrap()))
        } else {
            Ok(Some(serde_json::json!({ "engines": results })))
        }
    }

    fn cmd_enable(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let path = scanner_config_path(workspace);
        let mut cfg = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;

        if !cfg.engines.contains_key(name) {
            return Err(format!("engine '{}' not found in configuration", name));
        }

        if !cfg.enabled.iter().any(|e| e.eq_ignore_ascii_case(name)) {
            cfg.enabled.push(name.to_string());
        }

        if let Some(raw) = cfg.engines.get(name) {
            let engine_cfg = parse_engine_config(raw);
            if engine_cfg.state.install_status.is_empty() {
                let mut updated = raw.clone();
                if let Some(obj) = updated.as_object_mut() {
                    let state = EngineState {
                        install_status: INSTALL_STATUS_PENDING.to_string(),
                        ..Default::default()
                    };
                    if let Ok(state_val) = serde_json::to_value(&state) {
                        obj.insert("state".to_string(), state_val);
                    }
                }
                cfg.engines.insert(name.to_string(), updated);
            }
        }

        save_scanner_config(&path, &cfg)
            .map_err(|e| format!("failed to save config: {}", e))?;

        let engines = build_all_engines_status(workspace)?;
        Ok(Some(serde_json::json!({ "engines": engines })))
    }

    fn cmd_disable(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let path = scanner_config_path(workspace);
        let mut cfg = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;

        cfg.enabled.retain(|e| !e.eq_ignore_ascii_case(name));

        save_scanner_config(&path, &cfg)
            .map_err(|e| format!("failed to save config: {}", e))?;

        let engines = build_all_engines_status(workspace)?;
        Ok(Some(serde_json::json!({ "engines": engines })))
    }

    async fn cmd_install(
        &self,
        workspace: &str,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str_field(data, "name")?;
        let force = data.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let url_override = get_opt_str_field(data, "url");

        let cancel_token = mark_op_started(&name).await
            .ok_or_else(|| format!("{} operation already in progress", name))?;

        let hub = ctx.state.event_hub.clone();
        let ws = workspace.to_string();
        let response_name = name.clone();

        let progress_cb = make_download_progress_cb(hub.clone(), &name);

        tokio::spawn(async move {
            let result = install_engine_inner(
                &ws, &name, force, url_override.as_deref(),
                &hub, &cancel_token, &progress_cb,
            ).await;

            match result {
                Ok(()) => {
                    hub.publish("scanner-progress", serde_json::json!({
                        "engine": name, "phase": "complete", "progress": 100,
                        "message": format!("{} installed successfully", name)
                    }));
                }
                Err(e) => {
                    if e == "download cancelled" {
                        hub.publish("scanner-progress", serde_json::json!({
                            "engine": name, "phase": "cancelled", "progress": 0,
                            "message": format!("{} installation cancelled", name)
                        }));
                    } else {
                        let path = scanner_config_path(&ws);
                        if let Ok(mut cfg) = load_scanner_config(&path) {
                            if let Some(raw) = cfg.engines.get(&name).cloned() {
                                let mut updated = raw.clone();
                                if let Some(obj) = updated.as_object_mut() {
                                    let state = EngineState {
                                        install_status: INSTALL_STATUS_FAILED.to_string(),
                                        install_error: e.clone(),
                                        last_install_attempt: chrono::Local::now().to_rfc3339(),
                                        ..parse_engine_config(&raw).state
                                    };
                                    if let Ok(state_val) = serde_json::to_value(&state) {
                                        obj.insert("state".to_string(), state_val);
                                    }
                                }
                                cfg.engines.insert(name.clone(), updated);
                                let _ = save_scanner_config(&path, &cfg);
                            }
                        }

                        hub.publish("scanner-progress", serde_json::json!({
                            "engine": name, "phase": "error", "progress": 0,
                            "message": format!("Installation failed: {}", e)
                        }));
                    }
                }
            }

            mark_op_finished(&name).await;
        });

        Ok(Some(serde_json::json!({ "started": true, "engine": response_name })))
    }

    async fn cmd_update_db(
        &self,
        workspace: &str,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str_field(data, "name")?;
        let op_key = format!("{}-update-db", name);

        let cancel_token = mark_op_started(&op_key).await
            .ok_or_else(|| format!("{} database update already in progress", name))?;

        let hub = ctx.state.event_hub.clone();
        let ws = workspace.to_string();
        let response_name = name.clone();

        tokio::spawn(async move {
            hub.publish("scanner-progress", serde_json::json!({
                "engine": name, "phase": "downloading-db", "progress": 0,
                "message": format!("Starting {} database update...", name)
            }));

            let result = update_db_inner(&ws, &name, &hub, &cancel_token).await;

            match result {
                Ok(()) => {
                    let path = scanner_config_path(&ws);
                    if let Ok(mut cfg) = load_scanner_config(&path) {
                        if let Some(raw) = cfg.engines.get(&name).cloned() {
                            let mut updated = raw.clone();
                            if let Some(obj) = updated.as_object_mut() {
                                let mut state = parse_engine_config(&raw).state;
                                state.db_status = DB_STATUS_READY.to_string();
                                state.last_db_update = chrono::Local::now().to_rfc3339();
                                if let Ok(state_val) = serde_json::to_value(&state) {
                                    obj.insert("state".to_string(), state_val);
                                }
                            }
                            cfg.engines.insert(name.clone(), updated);
                            let _ = save_scanner_config(&path, &cfg);
                        }
                    }

                    hub.publish("scanner-progress", serde_json::json!({
                        "engine": name, "phase": "complete", "progress": 100,
                        "message": format!("{} database updated", name)
                    }));
                }
                Err(e) => {
                    if e == "database update cancelled" {
                        hub.publish("scanner-progress", serde_json::json!({
                            "engine": name, "phase": "cancelled", "progress": 0,
                            "message": format!("{} database update cancelled", name)
                        }));
                    } else {
                        hub.publish("scanner-progress", serde_json::json!({
                            "engine": name, "phase": "error", "progress": 0,
                            "message": format!("Database update failed: {}", e)
                        }));
                    }
                }
            }

            mark_op_finished(&op_key).await;
        });

        Ok(Some(serde_json::json!({ "started": true, "engine": response_name })))
    }

    async fn cmd_test(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str_field(data, "name")?;
        let file_path = get_str_field(data, "path")?;

        let path = scanner_config_path(workspace);
        let cfg = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;

        let raw = cfg.engines.get(&name)
            .ok_or_else(|| format!("engine '{}' not found", name))?
            .clone();

        let engine = create_engine(&name, &raw)
            .map_err(|e| format!("failed to create engine: {}", e))?;

        let result = engine.scan_file(std::path::Path::new(&file_path)).await;

        Ok(Some(serde_json::json!({
            "path": result.path,
            "infected": result.infected,
            "virus": result.virus,
            "raw": result.raw,
            "engine": result.engine,
            "duration": result.duration,
        })))
    }

    fn cmd_engine_update_config(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str_field(data, "name")?;
        let updates = data.get("config").ok_or("missing field: config")?;

        let path = scanner_config_path(workspace);
        let mut cfg = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;

        let raw = cfg.engines.get(&name)
            .ok_or_else(|| format!("engine '{}' not found", name))?
            .clone();

        let mut engine_cfg = parse_engine_config(&raw);

        if let Some(v) = updates.get("url").and_then(|v| v.as_str()) {
            engine_cfg.url = v.to_string();
        }
        if let Some(v) = updates.get("clamav_path").and_then(|v| v.as_str()) {
            engine_cfg.clamav_path = v.to_string();
        }
        if let Some(v) = updates.get("address").and_then(|v| v.as_str()) {
            engine_cfg.address = v.to_string();
        }
        if let Some(v) = updates.get("data_dir").and_then(|v| v.as_str()) {
            engine_cfg.data_dir = v.to_string();
        }
        if let Some(v) = updates.get("scan_on_write").and_then(|v| v.as_bool()) {
            engine_cfg.scan_on_write = v;
        }
        if let Some(v) = updates.get("scan_on_download").and_then(|v| v.as_bool()) {
            engine_cfg.scan_on_download = v;
        }
        if let Some(v) = updates.get("scan_on_exec").and_then(|v| v.as_bool()) {
            engine_cfg.scan_on_exec = v;
        }
        if let Some(v) = updates.get("max_file_size").and_then(|v| v.as_i64()) {
            engine_cfg.max_file_size = v;
        }
        if let Some(v) = updates.get("update_interval").and_then(|v| v.as_str()) {
            engine_cfg.update_interval = v.to_string();
        }

        let updated = serde_json::to_value(&engine_cfg)
            .map_err(|e| format!("failed to serialize engine config: {}", e))?;
        cfg.engines.insert(name.clone(), updated);

        save_scanner_config(&path, &cfg)
            .map_err(|e| format!("failed to save config: {}", e))?;

        let engines = build_all_engines_status(workspace)?;
        Ok(Some(serde_json::json!({ "engines": engines })))
    }

    fn cmd_add(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str_field(data, "name")?;

        let valid = available_engines();
        if !valid.contains(&name.as_str()) {
            return Err(format!("unknown engine: {}. Available: {:?}", name, valid));
        }

        let path = scanner_config_path(workspace);
        let mut cfg = load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;

        let mut engine_cfg = nemesis_config::ClamAVEngineConfig::default();
        engine_cfg.address = "127.0.0.1:3310".to_string();
        engine_cfg.state = EngineState {
            install_status: INSTALL_STATUS_PENDING.to_string(),
            ..Default::default()
        };

        if let Some(v) = get_opt_str_field(data, "url") {
            engine_cfg.url = v;
        }
        if let Some(v) = get_opt_str_field(data, "address") {
            engine_cfg.address = v;
        }

        let engine_json = serde_json::to_value(&engine_cfg)
            .map_err(|e| format!("failed to serialize engine config: {}", e))?;

        cfg.engines.insert(name.clone(), engine_json);
        save_scanner_config(&path, &cfg)
            .map_err(|e| format!("failed to save config: {}", e))?;

        let engines = build_all_engines_status(workspace)?;
        Ok(Some(serde_json::json!({ "engines": engines })))
    }

    async fn cmd_cancel(
        &self,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = get_str_field(data, "name")?;
        let ops = active_ops().lock().await;

        // Try exact match first, then prefix match for update-db ops
        let key = if ops.contains_key(&name) {
            name.clone()
        } else {
            let update_key = format!("{}-update-db", name);
            if ops.contains_key(&update_key) {
                update_key
            } else {
                return Err(format!("no active operation for {}", name));
            }
        };

        if let Some(token) = ops.get(&key) {
            token.cancel();
            Ok(Some(serde_json::json!({ "cancelled": true, "engine": name })))
        } else {
            Err(format!("no active operation for {}", name))
        }
    }
}

// ---------------------------------------------------------------------------
// Background operation helpers
// ---------------------------------------------------------------------------

/// Create a progress callback that publishes SSE events via the EventHub.
fn make_download_progress_cb(
    hub: std::sync::Arc<crate::events::EventHub>,
    name: &str,
) -> Arc<dyn Fn(u64, u64) + Send + Sync> {
    let hub = hub.clone();
    let engine_name = name.to_string();
    Arc::new(move |written: u64, total: u64| {
        let (pct, msg) = if total > 0 {
            let p = (written as f64 / total as f64 * 100.0).min(100.0) as u32;
            (p, format!("下载中 {}% ({}/{})", p, format_bytes(written), format_bytes(total)))
        } else {
            (0, format!("下载中 {} bytes", format_bytes(written)))
        };
        hub.publish("scanner-progress", serde_json::json!({
            "engine": engine_name, "phase": "downloading", "progress": pct, "message": msg
        }));
    })
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Inner install logic — runs in a spawned task.
async fn install_engine_inner(
    workspace: &str,
    name: &str,
    force: bool,
    url_override: Option<&str>,
    hub: &crate::events::EventHub,
    cancel_token: &CancellationToken,
    on_progress: &Arc<dyn Fn(u64, u64) + Send + Sync>,
) -> Result<(), String> {
    let path = scanner_config_path(workspace);
    let cfg = load_scanner_config(&path)
        .map_err(|e| format!("load config: {}", e))?;

    let raw = cfg.engines.get(name)
        .ok_or_else(|| format!("engine '{}' not found", name))?
        .clone();

    let engine_cfg = parse_engine_config(&raw);

    if engine_cfg.state.install_status == INSTALL_STATUS_INSTALLED && !force {
        return Err(format!("{} already installed. Use force=true to reinstall.", name));
    }

    let install_dir = if !engine_cfg.clamav_path.is_empty() {
        PathBuf::from(&engine_cfg.clamav_path)
    } else {
        resolve_tools_dir(workspace)
    };
    std::fs::create_dir_all(&install_dir)
        .map_err(|e| format!("create install dir: {}", e))?;

    // Convert nemesis-config type to nemesis-security type
    let security_config = nemesis_security::scanner::ClamAVEngineConfig {
        url: url_override.map(|s| s.to_string()).unwrap_or(engine_cfg.url),
        clamav_path: engine_cfg.clamav_path.clone(),
        address: engine_cfg.address.clone(),
        scan_on_write: engine_cfg.scan_on_write,
        scan_on_download: engine_cfg.scan_on_download,
        scan_on_exec: engine_cfg.scan_on_exec,
        scan_extensions: engine_cfg.scan_extensions.clone(),
        skip_extensions: engine_cfg.skip_extensions.clone(),
        max_file_size: engine_cfg.max_file_size,
        update_interval: engine_cfg.update_interval.clone(),
        data_dir: engine_cfg.data_dir.clone(),
        state: Default::default(),
    };

    if security_config.url.is_empty() {
        return Err("no download URL configured".to_string());
    }

    let engine = ClamAVEngine::new(security_config);

    hub.publish("scanner-progress", serde_json::json!({
        "engine": name, "phase": "downloading", "progress": 10,
        "message": format!("Downloading {}...", name)
    }));

    let dir_str = install_dir.to_string_lossy().to_string();
    engine.download(&dir_str, cancel_token.clone(), Some(on_progress.clone())).await?;

    hub.publish("scanner-progress", serde_json::json!({
        "engine": name, "phase": "extracting", "progress": 50,
        "message": format!("Extracting {}...", name)
    }));

    hub.publish("scanner-progress", serde_json::json!({
        "engine": name, "phase": "configuring", "progress": 70,
        "message": format!("Configuring {}...", name)
    }));

    let clamav_path = engine.get_clamav_path();
    if !clamav_path.is_empty() {
        let db_dir = std::path::Path::new(&clamav_path).join("database");
        let _ = std::fs::create_dir_all(&db_dir);

        let freshclam_conf = std::path::Path::new(&clamav_path).join("freshclam.conf");
        let _ = nemesis_security::clamav::config::generate_freshclam_config(
            &db_dir.to_string_lossy(),
            &freshclam_conf.to_string_lossy(),
        );

        let daemon_config = nemesis_security::clamav::config::DaemonConfig {
            clamav_path: clamav_path.clone(),
            config_file: std::path::Path::new(&clamav_path).join("clamd.conf").to_string_lossy().to_string(),
            database_dir: db_dir.to_string_lossy().to_string(),
            listen_addr: "127.0.0.1:3310".to_string(),
            temp_dir: String::new(),
            startup_timeout_secs: 120,
        };
        let _ = nemesis_security::clamav::config::generate_clamd_config(&daemon_config);
    }

    hub.publish("scanner-progress", serde_json::json!({
        "engine": name, "phase": "downloading-db", "progress": 80,
        "message": format!("Downloading {} virus database...", name)
    }));

    if !clamav_path.is_empty() {
        let updater_config = nemesis_security::clamav::updater::UpdaterConfig {
            clamav_path: clamav_path.clone(),
            database_dir: std::path::Path::new(&clamav_path).join("database").to_string_lossy().to_string(),
            config_file: std::path::Path::new(&clamav_path).join("freshclam.conf").to_string_lossy().to_string(),
            update_interval: std::time::Duration::from_secs(24 * 3600),
            mirror_urls: vec![],
        };
        let updater = nemesis_security::clamav::updater::Updater::new(updater_config);
        if let Err(e) = updater.update(cancel_token.clone(), None).await {
            tracing::warn!("Database download failed (non-fatal): {}", e);
        }
    }

    let mut cfg2 = load_scanner_config(&path)
        .map_err(|e| format!("reload config: {}", e))?;
    if let Some(old_raw) = cfg2.engines.get(name).cloned() {
        let mut updated = old_raw.clone();
        if let Some(obj) = updated.as_object_mut() {
            let state = EngineState {
                install_status: INSTALL_STATUS_INSTALLED.to_string(),
                install_error: String::new(),
                last_install_attempt: chrono::Local::now().to_rfc3339(),
                db_status: DB_STATUS_READY.to_string(),
                last_db_update: chrono::Local::now().to_rfc3339(),
            };
            if let Ok(state_val) = serde_json::to_value(&state) {
                obj.insert("state".to_string(), state_val);
            }
            obj.insert("clamav_path".to_string(), serde_json::json!(clamav_path));
        }
        cfg2.engines.insert(name.to_string(), updated);
        save_scanner_config(&path, &cfg2)
            .map_err(|e| format!("save config: {}", e))?;
    }

    Ok(())
}

async fn update_db_inner(
    workspace: &str,
    name: &str,
    hub: &crate::events::EventHub,
    cancel_token: &CancellationToken,
) -> Result<(), String> {
    let path = scanner_config_path(workspace);
    let cfg = load_scanner_config(&path)
        .map_err(|e| format!("load config: {}", e))?;

    let raw = cfg.engines.get(name)
        .ok_or_else(|| format!("engine '{}' not found", name))?
        .clone();

    let engine_cfg = parse_engine_config(&raw);
    let clamav_path = engine_cfg.clamav_path;

    if clamav_path.is_empty() {
        return Err("engine not installed (no clamav_path)".to_string());
    }

    let db_dir = if !engine_cfg.data_dir.is_empty() {
        engine_cfg.data_dir.clone()
    } else {
        std::path::Path::new(&clamav_path).join("database").to_string_lossy().to_string()
    };

    let config_file = std::path::Path::new(&clamav_path).join("freshclam.conf")
        .to_string_lossy().to_string();

    hub.publish("scanner-progress", serde_json::json!({
        "engine": name, "phase": "downloading-db", "progress": 50,
        "message": format!("Running freshclam for {}...", name)
    }));

    let updater_config = nemesis_security::clamav::updater::UpdaterConfig {
        clamav_path: clamav_path.clone(),
        database_dir: db_dir,
        config_file,
        update_interval: std::time::Duration::from_secs(24 * 3600),
        mirror_urls: vec![],
    };
    let updater = nemesis_security::clamav::updater::Updater::new(updater_config);
    updater.update(cancel_token.clone(), None).await?;

    Ok(())
}
