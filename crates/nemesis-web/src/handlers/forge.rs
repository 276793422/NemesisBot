//! Forge handler — full self-learning dashboard API.
//!
//! Commands: status, stats, config.save, reflect,
//!           experiences.stats, reflections.list, reflections.latest,
//!           cycles.list, registry.list, registry.update

use crate::handlers::{list_workspace_dir, require_home, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};

/// A 线行为回归执行器槽（bot 注入；PROJECTS_BRIDGE 同款静态槽模式）。
/// 未注入 = `forge.test` 诚实报「未装配」。
static FORGE_TEST_RUNNER: parking_lot::RwLock<
    Option<std::sync::Arc<dyn nemesis_forge::skill_tests::SkillTestRunner>>,
> = parking_lot::RwLock::new(None);

/// gateway 装配点注入执行器。
pub fn set_forge_test_runner(
    runner: std::sync::Arc<dyn nemesis_forge::skill_tests::SkillTestRunner>,
) {
    *FORGE_TEST_RUNNER.write() = Some(runner);
}

pub struct ForgeHandler {
    _priv: (),
}

impl Default for ForgeHandler {
    fn default() -> Self {
        Self::new()
    }
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

    fn commands(&self) -> &'static [&'static str] {
        &[
            "status",
            "stats",
            "config.save",
            "reflect",
            "experiences.stats",
            "reflections.list",
            "reflections.latest",
            "cycles.list",
            "registry.list",
            "registry.update",
            "learning.toggle",
            "artifacts",
            "test",
            "replay_list",
            "replay_confirm",
            "replay_extract",
        ]
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
            "reflect" => self.reflect(home, workspace).await,
            "test" => {
                let data = data.ok_or("missing data")?;
                forge_test_cmd(home, workspace, &data)
            }

            // 失败重放（C 线）：提取/待审列表/确认入库（执行面归 CLI）。
            "replay_list" => forge_replay_list(workspace),
            "replay_confirm" => {
                let name = data
                    .as_ref()
                    .and_then(|d| d.get("name"))
                    .and_then(|v| v.as_str());
                let n = nemesis_forge::replay::confirm(Path::new(workspace), name)?;
                Ok(Some(serde_json::json!({ "confirmed": n })))
            }
            "replay_extract" => {
                let recs = nemesis_forge::replay::extract_failures(Path::new(workspace))?;
                let cases: Vec<_> = recs
                    .iter()
                    .map(nemesis_forge::replay::to_test_case)
                    .collect();
                let auto = load_forge_config(workspace).replay_auto_ingest;
                let (dest, total) =
                    nemesis_forge::replay::ingest(Path::new(workspace), &cases, auto)?;
                Ok(Some(serde_json::json!({
                    "extracted": cases.len(),
                    "dest": dest,
                    "total": total,
                })))
            }

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

fn forge_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("forge")
}

// ---------------------------------------------------------------------------
// Core commands
// ---------------------------------------------------------------------------

impl ForgeHandler {
    fn status(
        &self,
        home: &str,
        workspace: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let config = super::config::load_config(home)?;
        let enabled = config.forge.as_ref().map(|f| f.enabled).unwrap_or(false);

        // Check actual runtime state from Forge instance.
        let is_running = ctx
            .state
            .forge
            .as_ref()
            .map(|f| f.is_running())
            .unwrap_or(false);
        let started_at = ctx.state.forge.as_ref().and_then(|f| f.started_at());

        // Load forge config for intervals
        let forge_config_path =
            nemesis_path::resolve_forge_config_path_in_workspace(Path::new(workspace));
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
        let config = super::config::load_config(home)?;
        let forge_enabled = config.forge.as_ref().map(|f| f.enabled).unwrap_or(false);

        // Load full forge config from config dir if available
        let forge_config_path =
            nemesis_path::resolve_forge_config_path_in_workspace(Path::new(workspace));
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
        let active_count = artifacts
            .iter()
            .filter(|a| a.get("status").and_then(|s| s.as_str()) == Some("Active"))
            .count();
        let observing_count = artifacts
            .iter()
            .filter(|a| a.get("status").and_then(|s| s.as_str()) == Some("Observing"))
            .count();

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
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let size = path.metadata().map(|m| m.len()).unwrap_or(0);
                let modified = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        dt.to_rfc3339()
                    })
                    .unwrap_or_default();

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
            b.get("modified")
                .and_then(|v| v.as_str())
                .unwrap_or("")
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
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
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
        if skills_dir.exists()
            && let Ok(entries) = std::fs::read_dir(&skills_dir)
        {
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

        Ok(Some(serde_json::json!({
            "artifacts": artifacts,
            "skill_directories": skill_files,
        })))
    }

    fn registry_update(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let id = data
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("missing 'id' field")?;
        let status = data
            .get("status")
            .and_then(|v| v.as_str())
            .ok_or("missing 'status' field")?;
        // 状态值必须能反序列化为 ArtifactStatus（R17，单一真相源校验）：
        // 此前任意字符串直写 registry.json，而 Registry::load 用
        // Vec<Artifact> 严格反序列化——一个毒值让整个注册表加载必炸
        // 且无自愈（web 统计静默归零、CLI 反思去重失效会追加重复条目）。
        if serde_json::from_value::<nemesis_types::forge::ArtifactStatus>(
            serde_json::Value::String(status.to_string()),
        )
        .is_err()
        {
            return Err(format!(
                "invalid status '{}': allowed values are Draft/Active/Observing/Degraded/Negative/Archived",
                status
            ));
        }

        // registry.json 是 load→改→写 RMW（并发 registry_update 后写覆盖先写
        // 丢状态翻转），与 config 同族文件共用写串行锁（2026-10-13）。
        // 锁在 update_json_file 内；顶层是数组，Value 事务天然兼容。
        let registry_file = forge_dir(workspace).join("registry.json");
        super::config::update_json_file(&registry_file, |v| {
            if !registry_file.exists() {
                return Err("registry not found".to_string());
            }
            let artifacts: Vec<serde_json::Value> = serde_json::from_value(std::mem::take(v))
                .map_err(|e| format!("failed to parse registry: {}", e))?;
            let mut artifacts = artifacts;

            let mut found = false;
            for artifact in &mut artifacts {
                if artifact.get("id").and_then(|v| v.as_str()) == Some(id) {
                    if let Some(obj) = artifact.as_object_mut() {
                        obj.insert(
                            "status".to_string(),
                            serde_json::Value::String(status.to_string()),
                        );
                        obj.insert(
                            "updated_at".to_string(),
                            serde_json::Value::String(chrono::Local::now().to_rfc3339()),
                        );
                    }
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(format!("artifact '{}' not found", id));
            }

            *v = serde_json::Value::Array(artifacts);
            Ok(())
        })?;

        Ok(Some(
            serde_json::json!({ "updated": true, "id": id, "status": status }),
        ))
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

    async fn reflect(
        &self,
        home: &str,
        workspace: &str,
    ) -> Result<Option<serde_json::Value>, String> {
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

        // use_llm 消费（2026-10-16 R16 缺陷 21）：R11 契约「use_llm = 是否用
        // LLM 做语义分析，gateway 与 CLI 同语义」的第三入口——此前本命令
        // 无条件纯统计反思，Dashboard 手动反思在默认配置（use_llm=true）下
        // 永远拿不到 LLM 洞察，静默缺失。接线与 CLI `forge reflect` 同源：
        // 杂务链（resolve_judge_caller 单一真相源：judge_model → small_model
        // → 主模型）挂 reflector，带全量 artifacts（LLM 看到已部署技能不重复
        // 提名，注册表先从盘回载）+ 统一杂务预算；use_llm=false 或无可用模型
        // = 统计反思，响应 `llm_insights` 字段诚实标注（LLM 调用失败也落
        // false，不假报）。
        let forge_cfg = load_forge_config(workspace);
        let caller = if forge_cfg.reflection.use_llm {
            resolve_judge_caller(home, workspace)?
        } else {
            None
        };
        if let Some(caller) = caller {
            reflector.set_provider(caller);
        }
        let llm_requested = forge_cfg.reflection.use_llm;

        let registry =
            nemesis_forge::registry::Registry::new(nemesis_forge::types::RegistryConfig {
                index_path: fd.join("registry.json").to_string_lossy().to_string(),
            });
        let _ = registry.load().await;
        let artifacts = registry.list(None, None);

        let report = if llm_requested {
            reflector
                .reflect_with_llm(
                    &experiences,
                    &artifacts,
                    None,
                    "today",
                    "all",
                    Some(nemesis_forge::reflector_llm::AUX_FORGE_MAX_TOKENS),
                )
                .await
        } else {
            reflector.reflect(&experiences, None, "today", "all")
        };
        let llm_insights = report.llm_insights.is_some();

        // Write report to disk
        match reflector.write_report(&report) {
            Ok(path) => {
                tracing::info!(path = %path.display(), "[Forge] Manual reflection report written");
                let mut message = format!(
                    "反思完成，发现 {} 条洞察，{} 条建议",
                    report.stats.total_records,
                    report.stats.top_patterns.len() + report.stats.low_success.len(),
                );
                if llm_requested && !llm_insights {
                    message.push_str("（LLM 语义分析未产出：无可用模型或调用失败）");
                }
                Ok(Some(serde_json::json!({
                    "triggered": true,
                    "message": message,
                    "llm_insights": llm_insights,
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
                    "llm_insights": llm_insights,
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

        // config.json 锁在 update_config 内；闭包返回 was_enabled 供运行时启停用。
        let was_enabled = super::config::update_config(home, |config| {
            let forge = config.forge.get_or_insert_with(Default::default);
            let was = forge.enabled;
            forge.enabled = enabled;
            Ok(was)
        })?;

        // Sync enabled to config.forge.json as well.
        // 顺序第二个事务（非嵌套——update_* 持同一把非重入锁，嵌套即死锁）：
        // config.json 事务已落盘释放，这里再开 config.forge.json 事务。
        let workspace = require_workspace(ctx)?;
        let forge_config_path =
            nemesis_path::resolve_forge_config_path_in_workspace(Path::new(workspace));
        if forge_config_path.exists() {
            // Best-effort 同步（存量语义：读/解析失败跳过，不阻断主流程）。
            let _ = super::config::update_json_file(&forge_config_path, |fc| {
                if let Some(obj) = fc.as_object_mut() {
                    obj.insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                }
                Ok(())
            });
        } else {
            // Auto-create from defaults with the current enabled value.
            if let Some(parent) = forge_config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let mut default_config = nemesis_forge::config::ForgeConfig::default();
            default_config.enabled = enabled;
            if let Ok(json) = serde_json::to_string_pretty(&default_config) {
                // REL-002：原子写入。
                let _ = nemesis_utils::write_file_atomic(
                    &forge_config_path.to_string_lossy(),
                    json.as_bytes(),
                    0o600,
                );
            }
        }

        // Runtime start/stop: toggle Forge background tasks without restart.
        if enabled && !was_enabled {
            if let Some(ref forge) = ctx.state.forge
                && !forge.is_running()
            {
                // start() requires Arc<Self>, so clone the Arc.
                let forge_arc = forge.clone();
                // Spawn start in background — it's async and takes Arc<Self>.
                tokio::spawn(async move {
                    forge_arc.start().await;
                });
                tracing::info!("[Forge] Runtime start triggered via dashboard");
            }
        } else if !enabled
            && was_enabled
            && let Some(ref forge) = ctx.state.forge
            && forge.is_running()
        {
            // stop() is async — spawn it.
            let forge_arc = forge.clone();
            tokio::spawn(async move {
                forge_arc.stop().await;
            });
            tracing::info!("[Forge] Runtime stop triggered via dashboard");
        }

        Ok(Some(
            serde_json::json!({ "saved": true, "enabled": enabled }),
        ))
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
        // 纯创建（非 RMW），锁外执行；随后的 update_json_file 事务罩真正的
        // 读→改→写（与 config_save 等写臂互斥，同族配置写串行锁，2026-10-13）。
        let forge_config_path =
            nemesis_path::resolve_forge_config_path_in_workspace(Path::new(workspace));
        if !forge_config_path.exists() {
            if let Some(parent) = forge_config_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let default_config = nemesis_forge::config::ForgeConfig::default();
            let json = serde_json::to_string_pretty(&default_config)
                .map_err(|e| format!("failed to serialize default forge config: {}", e))?;
            // REL-002：原子写入。
            nemesis_utils::write_file_atomic(
                &forge_config_path.to_string_lossy(),
                json.as_bytes(),
                0o600,
            )
            .map_err(|e| format!("failed to write config.forge.json: {}", e))?;
        }

        // Update the file on disk（锁在 update_json_file 内，原子落盘）。
        super::config::update_json_file(&forge_config_path, |forge_config| {
            if let Some(obj) = forge_config.as_object_mut() {
                if let Some(learning) = obj.get_mut("learning") {
                    if let Some(learning_obj) = learning.as_object_mut() {
                        learning_obj
                            .insert("enabled".to_string(), serde_json::Value::Bool(enabled));
                    }
                } else {
                    obj.insert(
                        "learning".to_string(),
                        serde_json::json!({ "enabled": enabled }),
                    );
                }
            }
            Ok(())
        })?;

        // Update runtime flag on Forge instance.
        if let Some(ref forge) = ctx.state.forge {
            forge.set_learning_enabled(enabled);
        }

        tracing::info!(enabled, "[Forge] Learning toggle via dashboard");

        Ok(Some(
            serde_json::json!({ "saved": true, "learning_enabled": enabled }),
        ))
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
        Err(_) => {
            return ExperienceStatsResult {
                total_count: 0,
                success_count: 0,
                failure_count: 0,
                avg_duration_ms: 0.0,
                tool_counts: serde_json::json!({}),
            };
        }
    };

    let mut total = 0usize;
    let mut success = 0usize;
    let mut total_duration = 0u64;
    let mut tool_map: std::collections::HashMap<String, (usize, usize, u64)> =
        std::collections::HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(exp) = serde_json::from_str::<serde_json::Value>(line) {
            // Navigate into experience.experience structure
            let inner = exp.get("experience").unwrap_or(&exp);
            let tool_name = inner
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let is_success = inner
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let duration = inner
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            total += 1;
            if is_success {
                success += 1;
            }
            total_duration += duration;

            let entry = tool_map.entry(tool_name).or_insert((0, 0, 0));
            entry.0 += 1;
            if is_success {
                entry.1 += 1;
            }
            entry.2 += duration;
        }
    }

    let failure = total - success;
    let avg_duration = if total > 0 {
        total_duration as f64 / total as f64
    } else {
        0.0
    };

    let tool_counts: serde_json::Map<String, serde_json::Value> = tool_map
        .into_iter()
        .map(|(name, (count, succ, dur))| {
            (
                name,
                serde_json::json!({
                    "count": count,
                    "success": succ,
                    "failure": count - succ,
                    "success_rate": if count > 0 { succ as f64 / count as f64 } else { 0.0 },
                    "avg_duration_ms": if count > 0 { dur as f64 / count as f64 } else { 0.0 },
                }),
            )
        })
        .collect();

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
    let start = if lines.len() > limit {
        lines.len() - limit
    } else {
        0
    };

    lines[start..].iter().filter_map(|l| {
        let parsed: serde_json::Value = serde_json::from_str(l).ok()?;
        let inner = parsed.get("experience").unwrap_or(&parsed);
        Some(serde_json::json!({
            "id": inner.get("id").and_then(|v| v.as_str()).unwrap_or(""),
            "tool_name": inner.get("tool_name").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "success": inner.get("success").and_then(|v| v.as_bool()).unwrap_or(false),
            "duration_ms": inner.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
            "input_summary": inner.get("input_summary").and_then(|v| v.as_str()).unwrap_or(""),
            "output_summary": inner.get("output_summary").and_then(|v| v.as_str()).unwrap_or(""),
            "timestamp": inner.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
            "session_key": inner.get("session_key").and_then(|v| v.as_str()).unwrap_or(""),
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
            if !month_entry.path().is_dir() {
                continue;
            }
            if let Ok(file_entries) = std::fs::read_dir(month_entry.path()) {
                for file_entry in file_entries.flatten() {
                    let name = file_entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".jsonl") {
                        let content =
                            std::fs::read_to_string(file_entry.path()).unwrap_or_default();
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
    let dt: chrono::DateTime<chrono::Local> = modified.into();
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
            if !path.is_file() {
                continue;
            }
            if path.extension().map(|e| e == ext).unwrap_or(false)
                && let Ok(meta) = path.metadata()
                && let Ok(modified) = meta.modified()
                && latest.as_ref().is_none_or(|(_, t)| modified > *t)
            {
                latest = Some((path, modified));
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
        // 月目录按名排序（R18）：read_dir 顺序是平台实现细节（NTFS 字典序
        // vs ext4 哈希序），无序基上「取尾/倒序截尾」=任意子集。升序迭代
        // 保持本函数「旧→新」返回契约（消费方 `cycles.last()` 取最新、
        // `.rev().take(100)` 取最新 100）。
        let mut months: Vec<_> = month_entries.flatten().collect();
        months.sort_by_key(|e| e.file_name());
        for month_entry in months {
            if !month_entry.path().is_dir() {
                continue;
            }
            if let Ok(file_entries) = std::fs::read_dir(month_entry.path()) {
                // 同批日文件同样升序（老日→新日）。
                let mut files: Vec<_> = file_entries.flatten().collect();
                files.sort_by_key(|e| e.file_name());
                for file_entry in files {
                    let name = file_entry.file_name().to_string_lossy().to_string();
                    if !name.ends_with(".jsonl") {
                        continue;
                    }
                    let content = match std::fs::read_to_string(file_entry.path()) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    for line in content.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
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

// S10b (2026-08-26, quality-hardening goal 冲刺 web 批次 2): narrow arms not
// reached by forge_extra_tests.rs — offline only.
#[cfg(test)]
mod s10b_tests;

// AGT 覆盖率批次（2026-09-25）：config.save 运行时 start/stop 联动（真
// Forge + is_running 翻转）、learning.toggle 运行时旗标 + 非 object 配置
// 跳过臂、experiences 统计的空行/非法行/全非法三态、深目录读取器的目录型
// 文件兜底与 learning/reflections 深路径。豁免（live 全局 store 命中臂）
// 见 agt_tests 文件头注。
#[cfg(test)]
mod agt_tests;

/// `forge.test`：对 workspace/skills/<name> 执行 frontmatter tests 段。
/// judge 层走杂务模型回落链（resolve_judge_caller）。
fn forge_test_cmd(
    home: &str,
    workspace: &str,
    data: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    use nemesis_forge::skill_tests as st;

    let name = data
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("missing 'name'")?;
    // name 围栏（R17 引入，R18 起走 forge crate 单一真相源）：单段目录名，
    // 拒 `../x` 穿越读面与 `C:evil` 盘符相对形态（is_absolute()==false 但
    // join 整段换基座）。合法技能目录名是 kebab/单段（slugify 产物）。
    nemesis_forge::forge::valid_skill_dir_name(name)?;
    let filter = data.get("test").and_then(|v| v.as_str());

    let runner = FORGE_TEST_RUNNER.read().clone();
    let Some(runner) = runner else {
        return Err("forge test runner not wired (gateway 未注入执行器)".into());
    };

    let skill_md = Path::new(workspace)
        .join("skills")
        .join(name)
        .join("SKILL.md");
    if !skill_md.exists() {
        return Err(format!("skill not found: {}", skill_md.display()));
    }
    let content = std::fs::read_to_string(&skill_md).map_err(|e| e.to_string())?;

    let Some(spec) = st::parse_skill_tests(&content)? else {
        return Ok(Some(serde_json::json!({
            "skipped": true,
            "reason": "no behavior tests declared",
        })));
    };
    let cases: Vec<_> = spec
        .tests
        .iter()
        .filter(|c| filter.is_none() || Some(c.name.as_str()) == filter)
        .collect();
    if cases.is_empty() {
        return Err(format!("no test matching filter '{filter:?}'"));
    }

    // judge caller：回落链解析（judge_model → small_model → 主模型）。
    let caller = resolve_judge_caller(home, workspace)?;
    // 执行层缺省阈值（forge.test.judge_threshold，用例未显式声明时生效）。
    let forge_cfg = nemesis_forge::config::load_forge_config(
        &Path::new(workspace)
            .join("config")
            .join("config.forge.json"),
    );
    let default_threshold = forge_cfg.test.judge_threshold;

    // 同步包装（handler 是 async 上下文——block_in_place 桥接）。
    let fut = async {
        let mut results = Vec::new();
        let mut passed = 0u32;
        let mut failed = 0u32;
        for case in &cases {
            let transcript = runner.run_case(&content, case).await?;
            let r =
                st::evaluate_case_full(case, &transcript, caller.clone(), default_threshold).await;
            if r.passed {
                passed += 1;
            } else {
                failed += 1;
            }
            results.push(serde_json::to_value(&r).unwrap_or_default());
        }
        Ok::<_, String>((results, passed, failed))
    };
    let (results, passed, failed) = match tokio::runtime::Handle::try_current() {
        Ok(h) => tokio::task::block_in_place(|| h.block_on(fut))?,
        Err(_) => {
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            rt.block_on(fut)?
        }
    };

    Ok(Some(serde_json::json!({
        "skill": name,
        "passed": passed,
        "failed": failed,
        "gate": failed == 0,
        "results": results,
    })))
}

/// forge 配置加载（workspace/config/config.forge.json；缺失 = 全缺省）。
pub fn load_forge_config(workspace: &str) -> nemesis_forge::config::ForgeConfig {
    nemesis_forge::config::load_forge_config(
        &Path::new(workspace)
            .join("config")
            .join("config.forge.json"),
    )
}

/// `forge.replay_list`：失败重放待审区 + 已入库回归集全量。
fn forge_replay_list(workspace: &str) -> Result<Option<serde_json::Value>, String> {
    let (pending, confirmed) = nemesis_forge::replay::list_full(Path::new(workspace))?;
    let to_json = |cases: Vec<nemesis_forge::skill_tests::TestCase>| {
        cases
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "turns": c.turns.iter().map(|t| t.user.clone()).collect::<Vec<_>>(),
                    "expect": serde_json::to_value(&c.expect).unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>()
    };
    Ok(Some(serde_json::json!({
        "pending": to_json(pending),
        "confirmed": to_json(confirmed),
    })))
}

/// judge / B 线反思共用的杂务模型回落链（v3.1 计划 §四/§五：small_model →
/// 主模型）。臂序：
///
/// 1. `forge.test.judge_model` 显式别名（非 "small_model"）= 钉死该模型；
/// 2. `agents.small_model` 通道；
/// 3. 当前默认模型。
///
/// 任一臂解析/构造失败 = warn 留痕自动降下一臂，绝不因缺配置失败；
/// 全部失败 = `Ok(None)`（调用方诚实跳过 / fail-closed）。
pub fn resolve_judge_caller(
    home: &str,
    workspace: &str,
) -> Result<Option<std::sync::Arc<dyn nemesis_forge::reflector_llm::LLMCaller>>, String> {
    let cfg = crate::handlers::config::load_config(home)?;
    let forge_cfg = load_forge_config(workspace);

    let mut arms: Vec<String> = Vec::new();
    if let Some(a) = forge_cfg
        .test
        .judge_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "small_model")
    {
        arms.push(a.to_string());
    }
    if let Some(s) = cfg
        .agents
        .small_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        arms.push(s.to_string());
    }
    arms.push(String::new()); // 臂 3：默认模型（resolve 语义见下）

    for arm in &arms {
        match build_caller_for_ref(&cfg, arm, workspace) {
            Ok(Some(caller)) => return Ok(Some(caller)),
            Ok(None) => {
                // 解析失败降下一臂——warn 留痕（臂名空串 = 默认模型）。
                let shown = if arm.is_empty() { "default" } else { arm };
                tracing::warn!("[Forge] judge caller arm '{shown}' unresolved; falling back");
            }
            Err(e) => {
                tracing::warn!("[Forge] judge caller arm '{arm}' failed: {e}; falling back");
            }
        }
    }
    Ok(None)
}

/// 按模型引用构造 LLMCaller（空串 = 当前默认模型）。解析/构造失败 =
/// `Ok(None)`（回落链消费）。
fn build_caller_for_ref(
    cfg: &nemesis_config::Config,
    model_ref: &str,
    workspace: &str,
) -> Result<Option<std::sync::Arc<dyn nemesis_forge::reflector_llm::LLMCaller>>, String> {
    let llm_ref = if model_ref.is_empty() {
        nemesis_config::get_effective_llm(Some(cfg))
    } else {
        model_ref.to_string()
    };
    let Ok(resolution) = nemesis_config::resolve_model_config(cfg, &llm_ref) else {
        return Ok(None);
    };
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        proxy: resolution.proxy.clone(),
        stream_mode: resolution.stream_mode.clone(),
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: workspace.to_string(),
        connect_mode: resolution.connect_mode,
        protocol: resolution.protocol.clone(),
        timeout_secs: resolution.timeout_secs,
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    match nemesis_providers::factory::create_provider(&factory_cfg) {
        Ok(provider) => Ok(Some(std::sync::Arc::new(crate::ForgeProviderBridge::new(
            provider,
            resolution.model_name.clone(),
        )))),
        Err(_) => Ok(None),
    }
}
