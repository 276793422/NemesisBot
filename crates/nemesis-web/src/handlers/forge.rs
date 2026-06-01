//! Forge handler — full self-learning dashboard API.
//!
//! Commands: status, stats, config.save, reflect,
//!           experiences.stats, reflections.list, reflections.latest,
//!           cycles.list, registry.list, registry.update

use crate::handlers::{list_workspace_dir, require_home, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ForgeHandler {
    _priv: (),
}

impl ForgeHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ForgeHandler {
    fn module_name(&self) -> &str {
        "forge"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let home = require_home(ctx)?;
        let workspace = require_workspace(ctx)?;
        match cmd {
            "status" => self.status(home, workspace, ctx),
            "stats" => self.stats(home, workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(home, &data, ctx)
            }
            "reflect" => self.reflect(workspace),

            // Experiences
            "experiences.stats" => self.experiences_stats(workspace),

            // Reflections
            "reflections.list" => self.reflections_list(workspace),
            "reflections.latest" => self.reflections_latest(workspace),

            // Learning cycles
            "cycles.list" => self.cycles_list(workspace),

            // Registry (artifacts)
            "registry.list" => self.registry_list(workspace),
            "registry.update" => {
                let data = data.ok_or("missing data")?;
                self.registry_update(workspace, &data)
            }

            // Learning
            "learning.toggle" => {
                let data = data.ok_or("missing data")?;
                self.learning_toggle(home, workspace, &data, ctx)
            }

            // Artifacts (legacy, kept for compat)
            "artifacts" => self.artifacts(workspace),

            _ => Err(format!("unknown command: forge.{}", cmd)),
        }
    }
}

fn config_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("config.json")
}

fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    nemesis_config::load_config(&config_path(home)).map_err(|e| format!("failed to load config: {}", e))
}

fn save_config_to_disk(home: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    nemesis_config::save_config(&config_path(home), config).map_err(|e| format!("failed to save config: {}", e))
}

fn forge_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("forge")
}

// ---------------------------------------------------------------------------
// Core commands
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn status(&self, home: &str, workspace: &str, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        let enabled = config.forge.as_ref().map(|f| f.enabled).unwrap_or(false);

        // Check actual runtime state from Forge instance.
        let is_running = ctx.state.forge.as_ref().map(|f| f.is_running()).unwrap_or(false);
        let started_at = ctx.state.forge.as_ref().and_then(|f| f.started_at());

        // Load forge config for intervals
        let forge_config_path = PathBuf::from(workspace)
            .join("config")
            .join("config.forge.json");
        let forge_config = if forge_config_path.exists() {
            nemesis_forge::config::load_forge_config(&forge_config_path)
        } else {
            nemesis_forge::config::ForgeConfig::default()
        };

        let fd = forge_dir(workspace);
        let forge_dir_exists = fd.exists();

        // Count experiences
        let exp_file = fd.join("experiences").join("experiences.jsonl");
        let experience_count = count_jsonl_lines(&exp_file);

        // Count reflection reports
        let reflections_dir = fd.join("reflections");
        let reflection_count = count_files_with_ext(&reflections_dir, "md");

        // Count registry artifacts
        let registry_file = fd.join("registry.json");
        let artifact_count = if registry_file.exists() {
            read_registry_artifacts(&registry_file).len()
        } else {
            0
        };

        // Count learning cycles
        let learning_dir = fd.join("learning");
        let cycle_count = count_jsonl_in_subdirs(&learning_dir);

        Ok(Some(serde_json::json!({
            "enabled": enabled,
            "running": is_running,
            "started_at": started_at,
            "reflection_interval_secs": forge_config.reflection.interval_secs,
            "cleanup_interval_secs": forge_config.storage.cleanup_interval_secs,
            "learning_enabled": ctx.state.forge.as_ref().map(|f| f.is_learning_enabled()).unwrap_or(false),
            "forge_dir_exists": forge_dir_exists,
            "experience_count": experience_count,
            "reflection_count": reflection_count,
            "artifact_count": artifact_count,
            "cycle_count": cycle_count,
        })))
    }

    fn stats(&self, home: &str, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        let forge_enabled = config.forge.as_ref().map(|f| f.enabled).unwrap_or(false);

        // Load full forge config from config dir if available
        let forge_config_path = PathBuf::from(workspace)
            .join("config")
            .join("config.forge.json");
        let forge_config = if forge_config_path.exists() {
            nemesis_forge::config::load_forge_config(&forge_config_path)
        } else {
            nemesis_forge::config::ForgeConfig::default()
        };

        let fd = forge_dir(workspace);

        // Experience stats
        let exp_file = fd.join("experiences").join("experiences.jsonl");
        let experience_count = count_jsonl_lines(&exp_file);
        let experience_stats = compute_experience_stats(&exp_file);

        // Reflection stats
        let reflections_dir = fd.join("reflections");
        let reflection_count = count_files_with_ext(&reflections_dir, "md");
        let latest_report = find_latest_file(&reflections_dir, "md");

        // Registry stats
        let registry_file = fd.join("registry.json");
        let artifacts = read_registry_artifacts(&registry_file);
        let active_count = artifacts.iter().filter(|a| {
            a.get("status").and_then(|s| s.as_str()) == Some("Active")
        }).count();
        let observing_count = artifacts.iter().filter(|a| {
            a.get("status").and_then(|s| s.as_str()) == Some("Observing")
        }).count();

        // Learning cycle stats
        let learning_dir = fd.join("learning");
        let cycles = read_learning_cycles(&learning_dir);
        let last_cycle = cycles.last().cloned();

        Ok(Some(serde_json::json!({
            "enabled": forge_enabled,
            "config": {
                "learning_enabled": forge_config.learning.enabled,
                "reflection_interval_secs": forge_config.reflection.interval_secs,
                "cleanup_interval_secs": forge_config.storage.cleanup_interval_secs,
                "collection_flush_secs": forge_config.collection.flush_interval_secs,
                "max_experience_age_days": forge_config.storage.max_experience_age_days,
                "min_pattern_frequency": forge_config.learning.min_pattern_frequency,
                "max_auto_creates": forge_config.learning.max_auto_creates,
            },
            "experiences": {
                "total": experience_count,
                "success": experience_stats.success_count,
                "failure": experience_stats.failure_count,
                "avg_duration_ms": experience_stats.avg_duration_ms,
                "tools": experience_stats.tool_counts,
            },
            "reflections": {
                "total": reflection_count,
                "latest": latest_report,
            },
            "artifacts": {
                "total": artifacts.len(),
                "active": active_count,
                "observing": observing_count,
            },
            "cycles": {
                "total": cycles.len(),
                "last": last_cycle,
            },
        })))
    }
}

// ---------------------------------------------------------------------------
// Experiences
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn experiences_stats(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let fd = forge_dir(workspace);
        let exp_file = fd.join("experiences").join("experiences.jsonl");

        if !exp_file.exists() {
            return Ok(Some(serde_json::json!({
                "total": 0,
                "success": 0,
                "failure": 0,
                "avg_duration_ms": 0.0,
                "tools": {},
                "recent": [],
            })));
        }

        let stats = compute_experience_stats(&exp_file);
        let recent = read_recent_experiences(&exp_file, 50);

        Ok(Some(serde_json::json!({
            "total": stats.total_count,
            "success": stats.success_count,
            "failure": stats.failure_count,
            "avg_duration_ms": stats.avg_duration_ms,
            "tools": stats.tool_counts,
            "recent": recent,
        })))
    }
}

// ---------------------------------------------------------------------------
// Reflections
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn reflections_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let reflections_dir = forge_dir(workspace).join("reflections");
        if !reflections_dir.exists() {
            return Ok(Some(serde_json::json!({ "reports": [] })));
        }

        let mut reports = Vec::new();
        let entries = std::fs::read_dir(&reflections_dir)
            .map_err(|e| format!("failed to read reflections dir: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                let modified = path.metadata().ok().and_then(|m| m.modified().ok()).map(|t| {
                    let dt: chrono::DateTime<chrono::Utc> = t.into();
                    dt.to_rfc3339()
                }).unwrap_or_default();

                // Extract date from filename: reflection_YYYY-MM-DD_HHMMSS.md
                let date = name.get(11..21).unwrap_or("").to_string();

                reports.push(serde_json::json!({
                    "name": name,
                    "date": date,
                    "size": size,
                    "modified": modified,
                }));
            }
        }

        // Sort by modified date descending
        reports.sort_by(|a, b| {
            b.get("modified").and_then(|v| v.as_str()).unwrap_or("")
                .cmp(a.get("modified").and_then(|v| v.as_str()).unwrap_or(""))
        });

        Ok(Some(serde_json::json!({ "reports": reports })))
    }

    fn reflections_latest(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let reflections_dir = forge_dir(workspace).join("reflections");
        if !reflections_dir.exists() {
            return Ok(Some(serde_json::json!({ "found": false, "content": "" })));
        }

        match find_latest_file_path(&reflections_dir, "md") {
            Some(path) => {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| format!("failed to read report: {}", e))?;
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                Ok(Some(serde_json::json!({
                    "found": true,
                    "name": name,
                    "content": content,
                })))
            }
            None => Ok(Some(serde_json::json!({ "found": false, "content": "" }))),
        }
    }
}

// ---------------------------------------------------------------------------
// Learning Cycles
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn cycles_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let learning_dir = forge_dir(workspace).join("learning");
        let cycles = read_learning_cycles(&learning_dir);

        let cycle_jsons: Vec<serde_json::Value> = cycles.into_iter().rev().take(100).collect();

        Ok(Some(serde_json::json!({ "cycles": cycle_jsons })))
    }
}

// ---------------------------------------------------------------------------
// Registry (Artifacts)
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn registry_list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let registry_file = forge_dir(workspace).join("registry.json");
        let artifacts = read_registry_artifacts(&registry_file);

        // Also scan forge/skills/ directory for skill files
        let skills_dir = forge_dir(workspace).join("skills");
        let mut skill_files = Vec::new();
        if skills_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let skill_md = path.join("SKILL.md");
                        let has_skill = skill_md.exists();
                        skill_files.push(serde_json::json!({
                            "name": name,
                            "has_skill_md": has_skill,
                            "type": "directory",
                        }));
                    }
                }
            }
        }

        Ok(Some(serde_json::json!({
            "artifacts": artifacts,
            "skill_directories": skill_files,
        })))
    }

    fn registry_update(&self, workspace: &str, data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let id = data.get("id").and_then(|v| v.as_str()).ok_or("missing 'id' field")?;
        let status = data.get("status").and_then(|v| v.as_str()).ok_or("missing 'status' field")?;

        let registry_file = forge_dir(workspace).join("registry.json");
        if !registry_file.exists() {
            return Err("registry not found".to_string());
        }

        // Read, update, write
        let content = std::fs::read_to_string(&registry_file)
            .map_err(|e| format!("failed to read registry: {}", e))?;
        let mut artifacts: Vec<serde_json::Value> = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse registry: {}", e))?;

        let mut found = false;
        for artifact in &mut artifacts {
            if artifact.get("id").and_then(|v| v.as_str()) == Some(id) {
                if let Some(obj) = artifact.as_object_mut() {
                    obj.insert("status".to_string(), serde_json::Value::String(status.to_string()));
                    obj.insert("updated_at".to_string(), serde_json::Value::String(chrono::Utc::now().to_rfc3339()));
                }
                found = true;
                break;
            }
        }

        if !found {
            return Err(format!("artifact '{}' not found", id));
        }

        let updated = serde_json::to_string_pretty(&artifacts)
            .map_err(|e| format!("failed to serialize registry: {}", e))?;
        std::fs::write(&registry_file, updated)
            .map_err(|e| format!("failed to write registry: {}", e))?;

        Ok(Some(serde_json::json!({ "updated": true, "id": id, "status": status })))
    }
}

// ---------------------------------------------------------------------------
// Legacy commands
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn artifacts(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let fd = forge_dir(workspace);
        if !fd.exists() {
            return Ok(Some(serde_json::json!({ "artifacts": [] })));
        }

        let mut artifacts = Vec::new();
        let entries = list_workspace_dir(workspace, "forge")?;
        for name in entries {
            let entry_path = fd.join(&name);
            if entry_path.is_dir() {
                artifacts.push(serde_json::json!({
                    "name": name,
                    "type": "directory",
                }));
            } else {
                let size = entry_path.metadata().map(|m| m.len()).unwrap_or(0);
                artifacts.push(serde_json::json!({
                    "name": name,
                    "type": "file",
                    "size": size,
                }));
            }
        }

        Ok(Some(serde_json::json!({ "artifacts": artifacts })))
    }

    fn reflect(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let fd = forge_dir(workspace);
        let exp_file = fd.join("experiences").join("experiences.jsonl");

        if !exp_file.exists() {
            return Ok(Some(serde_json::json!({
                "triggered": false,
                "message": "没有经验数据可供反思，请先使用 Bot 一段时间积累经验",
            })));
        }

        // Read experiences
        let content = std::fs::read_to_string(&exp_file)
            .map_err(|e| format!("failed to read experiences: {}", e))?;
        let experiences: Vec<nemesis_forge::types::CollectedExperience> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        if experiences.is_empty() {
            return Ok(Some(serde_json::json!({
                "triggered": false,
                "message": "经验数据为空，无法执行反思",
            })));
        }

        // Create reflector and run analysis
        let reflections_dir = fd.join("reflections");
        let reflector = nemesis_forge::reflector::Reflector::with_reflections_dir(reflections_dir);

        let report = reflector.reflect(&experiences, None, "today", "all");

        // Write report to disk
        match reflector.write_report(&report) {
            Ok(path) => {
                tracing::info!(path = %path.display(), "[Forge] Manual reflection report written");
                Ok(Some(serde_json::json!({
                    "triggered": true,
                    "message": format!("反思完成，发现 {} 条洞察，{} 条建议",
                        report.stats.total_records,
                        report.stats.top_patterns.len() + report.stats.low_success.len()),
                    "insights_count": report.stats.top_patterns.len() + report.stats.low_success.len(),
                    "total_records": report.stats.total_records,
                    "unique_patterns": report.stats.unique_patterns,
                    "avg_success_rate": report.stats.avg_success_rate,
                })))
            }
            Err(e) => {
                // Still return results even if file write fails
                Ok(Some(serde_json::json!({
                    "triggered": true,
                    "message": format!("反思分析完成（报告写入失败: {}）", e),
                    "insights_count": report.stats.top_patterns.len() + report.stats.low_success.len(),
                    "total_records": report.stats.total_records,
                })))
            }
        }
    }

    fn config_save(
        &self,
        home: &str,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or("missing or invalid 'enabled' field")?;

        let mut config = load_config(home)?;
        let forge = config.forge.get_or_insert_with(Default::default);
        let was_enabled = forge.enabled;
        forge.enabled = enabled;
        save_config_to_disk(home, &mut config)?;

        // Sync enabled to config.forge.json as well.
        let workspace = require_workspace(ctx)?;
        let forge_config_path = PathBuf::from(workspace)
            .join("config")
            .join("config.forge.json");
        if let Some(parent) = forge_config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if forge_config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&forge_config_path) {
                if let Ok(mut fc) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = fc.as_object_mut() {
                        obj.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                        if let Ok(updated) = serde_json::to_string_pretty(&fc) {
                            let _ = std::fs::write(&forge_config_path, updated);
                        }
                    }
                }
            }
        } else {
            // Auto-create from defaults with the current enabled value.
            let mut default_config = nemesis_forge::config::ForgeConfig::default();
            default_config.enabled = enabled;
            if let Ok(json) = serde_json::to_string_pretty(&default_config) {
                let _ = std::fs::write(&forge_config_path, json);
            }
        }

        // Runtime start/stop: toggle Forge background tasks without restart.
        if enabled && !was_enabled {
            if let Some(ref forge) = ctx.state.forge {
                if !forge.is_running() {
                    // start() requires Arc<Self>, so clone the Arc.
                    let forge_arc = forge.clone();
                    // Spawn start in background — it's async and takes Arc<Self>.
                    tokio::spawn(async move {
                        forge_arc.start().await;
                    });
                    tracing::info!("[Forge] Runtime start triggered via dashboard");
                }
            }
        } else if !enabled && was_enabled {
            if let Some(ref forge) = ctx.state.forge {
                if forge.is_running() {
                    // stop() is async — spawn it.
                    let forge_arc = forge.clone();
                    tokio::spawn(async move {
                        forge_arc.stop().await;
                    });
                    tracing::info!("[Forge] Runtime stop triggered via dashboard");
                }
            }
        }

        Ok(Some(serde_json::json!({ "saved": true, "enabled": enabled })))
    }

    fn learning_toggle(
        &self,
        _home: &str,
        workspace: &str,
        data: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or("missing 'enabled' field")?;

        // Ensure config.forge.json exists — auto-create from defaults if missing.
        let forge_config_path = PathBuf::from(workspace)
            .join("config")
            .join("config.forge.json");
        if !forge_config_path.exists() {
            if let Some(parent) = forge_config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let default_config = nemesis_forge::config::ForgeConfig::default();
            let json = serde_json::to_string_pretty(&default_config)
                .map_err(|e| format!("failed to serialize default forge config: {}", e))?;
            std::fs::write(&forge_config_path, json)
                .map_err(|e| format!("failed to write config.forge.json: {}", e))?;
        }

        // Update the file on disk.
        let content = std::fs::read_to_string(&forge_config_path)
            .map_err(|e| format!("failed to read config.forge.json: {}", e))?;
        let mut forge_config: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse config.forge.json: {}", e))?;
        if let Some(obj) = forge_config.as_object_mut() {
            if let Some(learning) = obj.get_mut("learning") {
                if let Some(learning_obj) = learning.as_object_mut() {
                    learning_obj.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                }
            } else {
                obj.insert("learning".to_string(), serde_json::json!({ "enabled": enabled }));
            }
        }
        let updated = serde_json::to_string_pretty(&forge_config)
            .map_err(|e| format!("failed to serialize forge config: {}", e))?;
        std::fs::write(&forge_config_path, updated)
            .map_err(|e| format!("failed to write config.forge.json: {}", e))?;

        // Update runtime flag on Forge instance.
        if let Some(ref forge) = ctx.state.forge {
            forge.set_learning_enabled(enabled);
        }

        tracing::info!(enabled, "[Forge] Learning toggle via dashboard");

        Ok(Some(serde_json::json!({ "saved": true, "learning_enabled": enabled })))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct ExperienceStatsResult {
    total_count: usize,
    success_count: usize,
    failure_count: usize,
    avg_duration_ms: f64,
    tool_counts: serde_json::Value,
}

fn compute_experience_stats(path: &PathBuf) -> ExperienceStatsResult {
    if !path.exists() {
        return ExperienceStatsResult {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: serde_json::json!({}),
        };
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return ExperienceStatsResult {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: serde_json::json!({}),
        },
    };

    let mut total = 0usize;
    let mut success = 0usize;
    let mut total_duration = 0u64;
    let mut tool_map: std::collections::HashMap<String, (usize, usize, u64)> = std::collections::HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Ok(exp) = serde_json::from_str::<serde_json::Value>(line) {
            // Navigate into experience.experience structure
            let inner = exp.get("experience").unwrap_or(&exp);
            let tool_name = inner.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let is_success = inner.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
            let duration = inner.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);

            total += 1;
            if is_success { success += 1; }
            total_duration += duration;

            let entry = tool_map.entry(tool_name).or_insert((0, 0, 0));
            entry.0 += 1;
            if is_success { entry.1 += 1; }
            entry.2 += duration;
        }
    }

    let failure = total - success;
    let avg_duration = if total > 0 { total_duration as f64 / total as f64 } else { 0.0 };

    let tool_counts: serde_json::Map<String, serde_json::Value> = tool_map.into_iter().map(|(name, (count, succ, dur))| {
        (name, serde_json::json!({
            "count": count,
            "success": succ,
            "failure": count - succ,
            "success_rate": if count > 0 { succ as f64 / count as f64 } else { 0.0 },
            "avg_duration_ms": if count > 0 { dur as f64 / count as f64 } else { 0.0 },
        }))
    }).collect();

    ExperienceStatsResult {
        total_count: total,
        success_count: success,
        failure_count: failure,
        avg_duration_ms: avg_duration,
        tool_counts: serde_json::Value::Object(tool_counts),
    }
}

fn read_recent_experiences(path: &PathBuf, limit: usize) -> Vec<serde_json::Value> {
    if !path.exists() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = if lines.len() > limit { lines.len() - limit } else { 0 };

    lines[start..].iter().filter_map(|l| {
        let parsed: serde_json::Value = serde_json::from_str(l).ok()?;
        let inner = parsed.get("experience").unwrap_or(&parsed);
        Some(serde_json::json!({
            "tool_name": inner.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "success": inner.get("success").and_then(|v| v.as_bool()).unwrap_or(false),
            "duration_ms": inner.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
            "input_summary": inner.get("input_summary").and_then(|v| v.as_str()).unwrap_or(""),
            "output_summary": inner.get("output_summary").and_then(|v| v.as_str()).unwrap_or("").chars().take(100).collect::<String>(),
            "timestamp": inner.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
        }))
    }).collect()
}

fn count_jsonl_lines(path: &PathBuf) -> usize {
    if !path.exists() {
        return 0;
    }
    std::fs::read_to_string(path)
        .map(|c| c.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

fn count_files_with_ext(dir: &PathBuf, ext: &str) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().map(|e| e == ext).unwrap_or(false))
                .count()
        })
        .unwrap_or(0)
}

fn count_jsonl_in_subdirs(dir: &PathBuf) -> usize {
    if !dir.exists() {
        return 0;
    }
    let mut count = 0;
    if let Ok(month_entries) = std::fs::read_dir(dir) {
        for month_entry in month_entries.flatten() {
            if !month_entry.path().is_dir() { continue; }
            if let Ok(file_entries) = std::fs::read_dir(month_entry.path()) {
                for file_entry in file_entries.flatten() {
                    let name = file_entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".jsonl") {
                        let content = std::fs::read_to_string(file_entry.path()).unwrap_or_default();
                        count += content.lines().filter(|l| !l.trim().is_empty()).count();
                    }
                }
            }
        }
    }
    count
}

fn find_latest_file(dir: &PathBuf, ext: &str) -> Option<serde_json::Value> {
    let path = find_latest_file_path(dir, ext)?;
    let name = path.file_name()?.to_string_lossy().to_string();
    let modified = path.metadata().ok()?.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = modified.into();
    Some(serde_json::json!({
        "name": name,
        "modified": dt.to_rfc3339(),
    }))
}

fn find_latest_file_path(dir: &PathBuf, ext: &str) -> Option<PathBuf> {
    if !dir.exists() {
        return None;
    }
    let mut latest: Option<(PathBuf, std::time::SystemTime)> = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() { continue; }
            if path.extension().map(|e| e == ext).unwrap_or(false) {
                if let Ok(meta) = path.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if latest.as_ref().map_or(true, |(_, t)| modified > *t) {
                            latest = Some((path, modified));
                        }
                    }
                }
            }
        }
    }
    latest.map(|(p, _)| p)
}

fn read_registry_artifacts(path: &PathBuf) -> Vec<serde_json::Value> {
    if !path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&content).unwrap_or_default()
}

fn read_learning_cycles(dir: &PathBuf) -> Vec<serde_json::Value> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut results = Vec::new();
    if let Ok(month_entries) = std::fs::read_dir(dir) {
        for month_entry in month_entries.flatten() {
            if !month_entry.path().is_dir() { continue; }
            if let Ok(file_entries) = std::fs::read_dir(month_entry.path()) {
                for file_entry in file_entries.flatten() {
                    let name = file_entry.file_name().to_string_lossy().to_string();
                    if !name.ends_with(".jsonl") { continue; }
                    let content = match std::fs::read_to_string(file_entry.path()) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    for line in content.lines() {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                            results.push(v);
                        }
                    }
                }
            }
        }
    }
    results
}
