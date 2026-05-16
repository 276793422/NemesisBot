//! Forge command - manage self-learning module.
//!
//! Mirrors Go command/forge.go with full lifecycle:
//! Status, Enable, Disable, Reflect, List, Evaluate, Export, Learning.

use anyhow::Result;
use crate::common;

#[derive(clap::Subcommand)]
pub enum ForgeAction {
    /// Show forge status
    Status,
    /// Enable forge module
    Enable,
    /// Disable forge module
    Disable,
    /// Trigger manual reflection
    Reflect,
    /// List forge artifacts
    List {
        #[arg(long, default_value = "all")]
        r#type: String,
    },
    /// Evaluate a forge artifact
    Evaluate {
        /// Artifact ID
        id: String,
    },
    /// Export forge artifacts
    Export {
        /// Artifact ID to export (omit for all active artifacts)
        id: Option<String>,
        #[arg(long)]
        output: Option<String>,
        /// Export all artifacts (not just active)
        #[arg(long)]
        all: bool,
    },
    /// Learning management (Phase 6)
    Learning {
        #[command(subcommand)]
        action: Option<LearningAction>,
    },
}

#[derive(clap::Subcommand)]
pub enum LearningAction {
    /// Show learning status
    Status,
    /// Enable learning loop
    Enable,
    /// Disable learning loop
    Disable,
    /// Show learning history
    History {
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

// ---------------------------------------------------------------------------
// Forge config helpers
// ---------------------------------------------------------------------------

/// Default forge.json configuration.
fn default_forge_config() -> serde_json::Value {
    serde_json::json!({
        "collect_interval_sec": 300,
        "reflect_interval_sec": 3600,
        "min_experiences": 5,
        "llm_semantic_analysis": true,
        "default_artifact_status": "draft",
        "trace_collection": true,
        "learning_enabled": false,
        "learning": {
            "min_pattern_frequency": 3,
            "high_confidence_threshold": 0.8,
            "max_auto_creates": 3,
            "max_refine_rounds": 3,
            "min_outcome_samples": 5,
            "monitor_window_days": 7,
            "degrade_threshold": -0.2,
            "degrade_cooldown_days": 7,
            "llm_budget_tokens": 8000
        }
    })
}

/// Load forge config from forge.json.
fn load_forge_config(forge_dir: &std::path::Path) -> serde_json::Value {
    let config_path = forge_dir.join("forge.json");
    if config_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&config_path) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                return cfg;
            }
        }
    }
    default_forge_config()
}

/// Save forge config to forge.json.
fn save_forge_config(forge_dir: &std::path::Path, cfg: &serde_json::Value) -> Result<()> {
    let _ = std::fs::create_dir_all(forge_dir);
    let config_path = forge_dir.join("forge.json");
    std::fs::write(&config_path, serde_json::to_string_pretty(cfg).unwrap_or_default())?;
    Ok(())
}

/// Load forge registry from registry.json.
fn load_registry(forge_dir: &std::path::Path) -> Vec<serde_json::Value> {
    let registry_path = forge_dir.join("registry.json");
    if registry_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&registry_path) {
            if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&data) {
                return arr;
            }
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// Command implementations
// ---------------------------------------------------------------------------

fn cmd_status(_home: &std::path::Path, cfg_path: &std::path::Path, forge_dir: &std::path::Path) -> Result<()> {
    println!("Forge Self-Learning Module");
    println!("==========================");

    // Check if forge is enabled in main config
    let enabled = if cfg_path.exists() {
        let data = std::fs::read_to_string(cfg_path)?;
        let cfg: serde_json::Value = serde_json::from_str(&data)?;
        cfg.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(false)
    } else {
        false
    };
    println!("  Enabled: {}", enabled);

    // Show forge config details
    let forge_cfg = load_forge_config(forge_dir);
    println!("  Collection interval: {}s", forge_cfg.get("collect_interval_sec").and_then(|v| v.as_u64()).unwrap_or(300));
    println!("  Reflection interval: {}s", forge_cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()).unwrap_or(3600));
    println!("  Min experiences: {}", forge_cfg.get("min_experiences").and_then(|v| v.as_u64()).unwrap_or(5));
    println!("  LLM semantic analysis: {}", forge_cfg.get("llm_semantic_analysis").and_then(|v| v.as_bool()).unwrap_or(true));
    println!("  Default artifact status: {}", forge_cfg.get("default_artifact_status").and_then(|v| v.as_str()).unwrap_or("draft"));
    println!("  Trace collection: {}", forge_cfg.get("trace_collection").and_then(|v| v.as_bool()).unwrap_or(true));

    // Learning status
    let learning_enabled = forge_cfg.get("learning_enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    println!("  Learning enabled: {}", learning_enabled);

    // Show directory status (7 dirs)
    println!();
    println!("  Directories:");
    for d in &["experiences", "reflections", "skills", "scripts", "mcp", "traces", "learning"] {
        let path = forge_dir.join(d);
        let exists = path.exists();
        let count = if exists {
            std::fs::read_dir(&path).map(|r| r.count()).unwrap_or(0)
        } else {
            0
        };
        println!("    {}: [{}] ({})", d, common::status_icon(exists), count);
    }

    // Show prompts dir
    let prompts_dir = forge_dir.join("prompts");
    println!("    prompts: [{}]", common::status_icon(prompts_dir.exists()));

    // Show forge config file path
    let forge_config = forge_dir.join("forge.json");
    println!();
    if forge_config.exists() {
        println!("  Config: {}", forge_config.display());
    } else {
        println!("  Config: not created (using defaults)");
    }

    // Registry stats
    let registry = load_registry(forge_dir);
    println!("  Registry: {} artifact(s)", registry.len());
    if !registry.is_empty() {
        // Group by type
        let mut type_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut status_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for artifact in &registry {
            let t = artifact.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
            let s = artifact.get("status").and_then(|v| v.as_str()).unwrap_or("unknown");
            *type_counts.entry(t).or_insert(0) += 1;
            *status_counts.entry(s).or_insert(0) += 1;
        }
        print!("    Types:");
        for (t, c) in &type_counts {
            print!(" {}={}", t, c);
        }
        println!();
        print!("    Status:");
        for (s, c) in &status_counts {
            print!(" {}={}", s, c);
        }
        println!();
    }

    Ok(())
}

fn cmd_enable(cfg_path: &std::path::Path, forge_dir: &std::path::Path) -> Result<()> {
    // Enable in main config, preserving existing forge fields
    if cfg_path.exists() {
        let data = std::fs::read_to_string(cfg_path)?;
        let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
        if let Some(obj) = cfg.as_object_mut() {
            if let Some(existing) = obj.get_mut("forge") {
                // Preserve all existing fields, just set enabled = true
                if let Some(forge_obj) = existing.as_object_mut() {
                    forge_obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                }
            } else {
                obj.insert("forge".to_string(), serde_json::json!({"enabled": true}));
            }
            std::fs::write(cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
        }
    }

    // Create 7 forge directories + prompts
    for d in &["experiences", "reflections", "skills", "scripts", "mcp", "traces", "learning"] {
        let _ = std::fs::create_dir_all(forge_dir.join(d));
    }
    let _ = std::fs::create_dir_all(forge_dir.join("prompts"));

    // Create forge.json with defaults if not exists
    let forge_config = forge_dir.join("forge.json");
    if !forge_config.exists() {
        save_forge_config(forge_dir, &default_forge_config())?;
    }

    // Create empty registry.json if not exists
    let registry_path = forge_dir.join("registry.json");
    if !registry_path.exists() {
        std::fs::write(&registry_path, "[]")?;
    }

    println!("Forge module enabled.");
    println!("  Created 7 workspace directories + prompts");
    println!("  Configuration: {}", forge_config.display());
    println!("  Restart gateway to apply.");
    Ok(())
}

fn cmd_disable(cfg_path: &std::path::Path) -> Result<()> {
    if cfg_path.exists() {
        let data = std::fs::read_to_string(cfg_path)?;
        let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
        if let Some(obj) = cfg.as_object_mut() {
            // Preserve all existing forge fields, only set enabled = false
            if let Some(existing) = obj.get_mut("forge") {
                if let Some(forge_obj) = existing.as_object_mut() {
                    forge_obj.insert("enabled".to_string(), serde_json::Value::Bool(false));
                }
            } else {
                obj.insert("forge".to_string(), serde_json::json!({"enabled": false}));
            }
            std::fs::write(cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
        }
    }
    println!("Forge module disabled. Restart gateway to apply.");
    Ok(())
}

fn cmd_reflect(cfg_path: &std::path::Path, forge_dir: &std::path::Path) -> Result<()> {
    // Check if forge is enabled in main config
    if cfg_path.exists() {
        if let Ok(data) = std::fs::read_to_string(cfg_path) {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                let enabled = cfg.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(false);
                if !enabled {
                    println!("Forge module is not enabled. Run 'nemesisbot forge enable' first.");
                    return Ok(());
                }
            }
        }
    }

    println!("Triggering manual reflection...");

    // Check forge directory exists
    if !forge_dir.exists() {
        println!("Error: Forge workspace not initialized. Run 'nemesisbot forge enable' first.");
        return Ok(());
    }

    // Check if experiences exist
    let exp_dir = forge_dir.join("experiences");
    let exp_count = if exp_dir.exists() {
        std::fs::read_dir(&exp_dir).map(|r| r.count()).unwrap_or(0)
    } else {
        0
    };

    if exp_count == 0 {
        println!("  No experiences collected yet.");
        println!("  Experiences are collected during gateway operation.");
        println!("  Start the gateway and interact with the bot to generate experiences.");
        return Ok(());
    }

    println!("  Found {} experience file(s) to reflect on.", exp_count);
    println!();

    // Create a real Forge instance
    let workspace = forge_dir.parent().unwrap_or(forge_dir).to_path_buf();
    let forge_cfg = nemesis_forge::config::ForgeConfig::default();
    let mut forge = nemesis_forge::forge::Forge::new(forge_cfg, workspace);

    // Initialize a real Reflector with the reflections directory
    let reflect_dir = forge_dir.join("reflections");
    let reflector = nemesis_forge::reflector::Reflector::with_reflections_dir(reflect_dir.clone());
    forge.init_reflector(reflector);

    // Read experiences from the experience store
    let store = nemesis_forge::experience_store::ExperienceStore::from_forge_dir(forge_dir);
    let experiences = match tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(store.read_aggregated())
    }) {
        Ok(exps) => {
            // Convert AggregatedExperience to CollectedExperience for the reflector
            exps.iter().map(|ae| {
                let success = ae.success_rate >= 0.5;
                nemesis_forge::types::CollectedExperience {
                    experience: nemesis_forge::types::Experience {
                        id: ae.pattern_hash.clone(),
                        tool_name: ae.tool_name.clone(),
                        input_summary: String::new(),
                        output_summary: String::new(),
                        success,
                        duration_ms: ae.avg_duration_ms as u64,
                        timestamp: ae.last_seen.clone(),
                        session_key: String::new(),
                    },
                    dedup_hash: ae.pattern_hash.clone(),
                }
            }).collect::<Vec<_>>()
        }
        Err(_) => {
            println!("  No aggregated experiences found. Nothing to reflect on.");
            return Ok(());
        }
    };

    if experiences.is_empty() {
        println!("  No experiences loaded. Nothing to reflect on.");
        return Ok(());
    }

    println!("  Loaded {} aggregated experience(s).", experiences.len());

    // Run the real reflector (Stages 1-4: statistical + trace analysis)
    let report = forge.reflector()
        .expect("reflector initialized above")
        .reflect(&experiences, None, "today", "all");

    // Write the reflection report to disk
    let reflector_ref = forge.reflector()
        .expect("reflector initialized above");
    match reflector_ref.write_report(&report) {
        Ok(path) => {
            println!("  Reflection report saved: {}", path.display());
        }
        Err(e) => {
            println!("  Warning: Failed to write report file: {}", e);
        }
    }

    // Output real results
    println!();
    println!("  Reflection Results:");
    println!("    Date: {}", report.date);
    println!("    Period: {}", report.period);
    println!("    Total records: {}", report.stats.total_records);
    println!("    Unique patterns: {}", report.stats.unique_patterns);
    println!("    Avg success rate: {:.1}%", report.stats.avg_success_rate * 100.0);

    if !report.stats.top_patterns.is_empty() {
        println!();
        println!("    Top patterns:");
        for p in report.stats.top_patterns.iter().take(5) {
            println!("      {} - {} uses, {:.0}% success",
                p.tool_name, p.count, p.success_rate * 100.0);
        }
    }

    if !report.stats.low_success.is_empty() {
        println!();
        println!("    Low success patterns:");
        for p in &report.stats.low_success {
            println!("      {} - {:.0}% success over {} calls",
                p.tool_name, p.success_rate * 100.0, p.count);
        }
    }

    if let Some(ref trace_stats) = report.trace_stats {
        println!();
        println!("    Trace analysis:");
        println!("      Total traces: {}", trace_stats.total_traces);
        println!("      Avg duration: {}ms", trace_stats.avg_duration_ms);
        println!("      Efficiency: {:.2}", trace_stats.efficiency_score);
    }

    Ok(())
}

fn cmd_list(forge_dir: &std::path::Path, r#type: &str) -> Result<()> {
    println!("  Forge Artifacts");
    println!("  ===============");

    let registry = load_registry(forge_dir);

    if !registry.is_empty() {
        // Use registry for formatted output (matches Go behavior)
        let filtered: Vec<_> = registry.iter()
            .filter(|a| {
                if r#type == "all" { true }
                else {
                    a.get("type").and_then(|v| v.as_str()).unwrap_or("") == r#type
                }
            })
            .collect();

        if filtered.is_empty() {
            println!("  (no artifacts matching type '{}')", r#type);
        } else {
            for artifact in &filtered {
                let id = artifact.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                let t = artifact.get("type").and_then(|v| v.as_str()).unwrap_or("?");
                let name = artifact.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                let version = artifact.get("version").and_then(|v| v.as_str()).unwrap_or("-");
                let status = artifact.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                println!("  ID: {}", id);
                println!("    Type: {}", t);
                println!("    Name: {}", name);
                println!("    Version: {}", version);
                println!("    Status: {}", status);
            }
            println!();
            println!("  Total: {} artifact(s)", filtered.len());
        }
    } else {
        // Fallback: scan directories for files
        let dirs_to_check = if r#type == "all" {
            vec!["skills", "scripts", "mcp"]
        } else {
            vec![r#type]
        };
        for d in &dirs_to_check {
            let path = forge_dir.join(d);
            println!("  {}:", d);
            if path.exists() {
                let entries: Vec<_> = std::fs::read_dir(&path)?
                    .filter_map(|e| e.ok())
                    .collect();
                if entries.is_empty() {
                    println!("    (none)");
                } else {
                    for entry in entries {
                        println!("    - {}", entry.file_name().to_string_lossy());
                    }
                }
            } else {
                println!("    (directory not found)");
            }
        }
    }
    Ok(())
}

fn cmd_evaluate(forge_dir: &std::path::Path, id: &str) -> Result<()> {
    println!("Evaluating artifact: {}", id);

    let registry = load_registry(forge_dir);
    let artifact = registry.iter().find(|a| {
        a.get("id").and_then(|v| v.as_str()) == Some(id)
    });

    if let Some(artifact) = artifact {
        println!("  Name: {}", artifact.get("name").and_then(|v| v.as_str()).unwrap_or("?"));
        println!("  Type: {}", artifact.get("type").and_then(|v| v.as_str()).unwrap_or("?"));
        println!("  Version: {}", artifact.get("version").and_then(|v| v.as_str()).unwrap_or("?"));
        println!("  Status: {}", artifact.get("status").and_then(|v| v.as_str()).unwrap_or("?"));

        if let Some(score) = artifact.get("score").and_then(|v| v.as_f64()) {
            println!("  Score: {:.2}", score);
        }
        if let Some(usage) = artifact.get("usage_count").and_then(|v| v.as_u64()) {
            println!("  Usage count: {}", usage);
        }
        println!();
        println!("  Note: Full evaluation requires running gateway with LLM access.");
    } else {
        println!("  Artifact '{}' not found in registry.", id);
    }
    Ok(())
}

fn cmd_export(forge_dir: &std::path::Path, output: Option<&str>, export_all: bool, artifact_id: Option<&str>) -> Result<()> {
    if !forge_dir.exists() {
        println!("  Forge workspace not initialized. Run 'nemesisbot forge enable' first.");
        return Ok(());
    }

    let workspace = forge_dir.parent().unwrap_or(forge_dir).to_path_buf();

    // Determine target directory: workspace/forge/exports/ (matching Go)
    let target_dir = match output {
        Some(path) if std::path::Path::new(path).is_absolute() => {
            std::path::PathBuf::from(path)
        }
        _ => forge_dir.join("exports"),
    };

    // Create a registry with the forge dir's registry.json and load from disk
    let registry = std::sync::Arc::new(nemesis_forge::registry::Registry::new(
        nemesis_forge::types::RegistryConfig {
            index_path: forge_dir.join("registry.json").to_string_lossy().to_string(),
        }
    ));

    // Load registry from disk (async)
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(registry.load())
    }).map_err(|e| anyhow::anyhow!("Failed to load registry: {}", e))?;

    // Check if registry has any artifacts
    let all_artifacts = registry.list(None, None);
    if all_artifacts.is_empty() {
        println!("  No artifacts in registry. Nothing to export.");
        return Ok(());
    }

    // Create a real Exporter with the registry
    let export_config = nemesis_forge::exporter::ExportConfig::with_registry(&workspace, registry.clone());
    let exporter = nemesis_forge::exporter::Exporter::new(export_config);

    if let Some(id) = artifact_id {
        // Export a specific artifact
        println!("Exporting artifact: {}", id);

        let artifact = match registry.get(id) {
            Some(a) => a,
            None => {
                println!("  Artifact '{}' not found in registry.", id);
                return Ok(());
            }
        };

        let export_dir = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(exporter.export_artifact(&artifact, &target_dir))
        }).map_err(|e| anyhow::anyhow!("Export failed: {}", e))?;

        println!("  Artifact exported to: {}", export_dir.display());
    } else {
        // Export all (active or all) artifacts
        if export_all {
            println!("Exporting all artifacts...");
        } else {
            println!("Exporting all active artifacts...");
        }

        let count = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(exporter.export_all(&target_dir))
        }).map_err(|e| anyhow::anyhow!("Export failed: {}", e))?;

        if count == 0 {
            println!("  No artifacts to export.");
            if !export_all {
                println!("  Use --all to include non-active artifacts.");
            }
        } else {
            println!("  Exported {} artifact(s) to: {}", count, target_dir.display());
        }
    }

    Ok(())
}

fn cmd_learning_status(forge_dir: &std::path::Path) -> Result<()> {
    println!("Learning Loop Status");
    println!("====================");

    let forge_cfg = load_forge_config(forge_dir);
    let enabled = forge_cfg.get("learning_enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    println!("  Enabled: {}", enabled);

    // Show detailed learning config
    if let Some(learning) = forge_cfg.get("learning") {
        println!();
        println!("  Configuration:");
        println!("    Min Pattern Frequency: {}", learning.get("min_pattern_frequency").and_then(|v| v.as_u64()).unwrap_or(3));
        println!("    High Confidence Threshold: {}", learning.get("high_confidence_threshold").and_then(|v| v.as_f64()).unwrap_or(0.8));
        println!("    Max Auto Creates: {}", learning.get("max_auto_creates").and_then(|v| v.as_u64()).unwrap_or(3));
        println!("    Max Refine Rounds: {}", learning.get("max_refine_rounds").and_then(|v| v.as_u64()).unwrap_or(3));
        println!("    Min Outcome Samples: {}", learning.get("min_outcome_samples").and_then(|v| v.as_u64()).unwrap_or(5));
        println!("    Monitor Window (days): {}", learning.get("monitor_window_days").and_then(|v| v.as_u64()).unwrap_or(7));
        println!("    Degrade Threshold: {}", learning.get("degrade_threshold").and_then(|v| v.as_f64()).unwrap_or(-0.2));
        println!("    Degrade Cooldown (days): {}", learning.get("degrade_cooldown_days").and_then(|v| v.as_u64()).unwrap_or(7));
        println!("    LLM Budget Tokens: {}", learning.get("llm_budget_tokens").and_then(|v| v.as_u64()).unwrap_or(8000));
    }

    // Show trace collection status
    let trace_collection = forge_cfg.get("trace_collection").and_then(|v| v.as_bool()).unwrap_or(true);
    println!();
    println!("  Trace Collection: {}", trace_collection);

    Ok(())
}

fn cmd_learning_enable(forge_dir: &std::path::Path) -> Result<()> {
    println!("Enabling learning loop...");

    let mut cfg = load_forge_config(forge_dir);
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        // Auto-enable trace collection when learning is enabled
        obj.insert("trace_collection".to_string(), serde_json::Value::Bool(true));
    }

    // Ensure learning directory exists
    let _ = std::fs::create_dir_all(forge_dir.join("learning"));
    let _ = std::fs::create_dir_all(forge_dir.join("traces"));

    save_forge_config(forge_dir, &cfg)?;
    println!("Learning loop enabled.");
    println!("  Trace collection: auto-enabled");
    println!("  Learning directory: {}", forge_dir.join("learning").display());
    println!("  Restart gateway to apply.");
    Ok(())
}

fn cmd_learning_disable(forge_dir: &std::path::Path) -> Result<()> {
    println!("Disabling learning loop...");

    let mut cfg = load_forge_config(forge_dir);
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(false));
    }

    save_forge_config(forge_dir, &cfg)?;
    println!("Learning loop disabled.");
    Ok(())
}

fn cmd_learning_history(forge_dir: &std::path::Path, limit: usize) -> Result<()> {
    println!("Learning History (last {} cycles)", limit);
    println!("==================================");

    let cycle_store = forge_dir.join("learning").join("learning_cycles.jsonl");
    if cycle_store.exists() {
        if let Ok(data) = std::fs::read_to_string(&cycle_store) {
            let lines: Vec<&str> = data.lines().filter(|l| !l.trim().is_empty()).collect();
            if lines.is_empty() {
                println!("  No learning history found.");
            } else {
                for line in lines.iter().rev().take(limit) {
                    if let Ok(evt) = serde_json::from_str::<serde_json::Value>(line) {
                        let ts = evt.get("timestamp").and_then(|v| v.as_str()).unwrap_or("?");
                        let patterns = evt.get("patterns_found").and_then(|v| v.as_u64()).unwrap_or(0);
                        let actions = evt.get("actions_generated").and_then(|v| v.as_u64()).unwrap_or(0);
                        let deployed = evt.get("actions_deployed").and_then(|v| v.as_u64()).unwrap_or(0);
                        println!("  [{}] patterns={} actions={} deployed={}", ts, patterns, actions, deployed);
                    } else {
                        println!("  {}", line);
                    }
                }
                println!();
                println!("  Total cycles: {}", lines.len());
            }
        }
    } else {
        println!("  No learning history found.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub fn run(action: ForgeAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);
    let cfg_path = common::config_path(&home);
    let forge_dir = common::workspace_path(&home).join("forge");

    match action {
        ForgeAction::Status => cmd_status(&home, &cfg_path, &forge_dir)?,
        ForgeAction::Enable => cmd_enable(&cfg_path, &forge_dir)?,
        ForgeAction::Disable => cmd_disable(&cfg_path)?,
        ForgeAction::Reflect => cmd_reflect(&cfg_path, &forge_dir)?,
        ForgeAction::List { r#type } => cmd_list(&forge_dir, &r#type)?,
        ForgeAction::Evaluate { id } => cmd_evaluate(&forge_dir, &id)?,
        ForgeAction::Export { id, output, all } => {
            let effective_output = output
                .or_else(|| id.clone())
                .unwrap_or_else(|| "forge_export.json".to_string());
            cmd_export(&forge_dir, Some(&effective_output), all, id.as_deref())?
        }
        ForgeAction::Learning { action } => {
            match action {
                None => cmd_learning_status(&forge_dir)?,
                Some(LearningAction::Status) => cmd_learning_status(&forge_dir)?,
                Some(LearningAction::Enable) => cmd_learning_enable(&forge_dir)?,
                Some(LearningAction::Disable) => cmd_learning_disable(&forge_dir)?,
                Some(LearningAction::History { limit }) => cmd_learning_history(&forge_dir, limit)?,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_forge_config() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
        assert_eq!(cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()), Some(3600));
        assert_eq!(cfg.get("min_experiences").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(cfg.get("learning_enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_load_forge_config_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = load_forge_config(tmp.path());
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
    }

    #[test]
    fn test_save_and_load_forge_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        }
        save_forge_config(tmp.path(), &cfg).unwrap();
        let loaded = load_forge_config(tmp.path());
        assert_eq!(loaded.get("learning_enabled").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_load_registry_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reg = load_registry(tmp.path());
        assert!(reg.is_empty());
    }

    #[test]
    fn test_load_registry_with_data() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        std::fs::write(&registry_path, r#"[{"id":"test-1","type":"skill","name":"test","status":"draft"}]"#).unwrap();
        let reg = load_registry(tmp.path());
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].get("id").and_then(|v| v.as_str()), Some("test-1"));
    }

    // -------------------------------------------------------------------------
    // default_forge_config comprehensive tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_forge_config_collect_interval() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
    }

    #[test]
    fn test_default_forge_config_reflect_interval() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("reflect_interval_sec").and_then(|v| v.as_u64()), Some(3600));
    }

    #[test]
    fn test_default_forge_config_min_experiences() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("min_experiences").and_then(|v| v.as_u64()), Some(5));
    }

    #[test]
    fn test_default_forge_config_llm_semantic_analysis() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("llm_semantic_analysis").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_default_forge_config_default_artifact_status() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("default_artifact_status").and_then(|v| v.as_str()), Some("draft"));
    }

    #[test]
    fn test_default_forge_config_trace_collection() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("trace_collection").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_default_forge_config_learning_enabled_false() {
        let cfg = default_forge_config();
        assert_eq!(cfg.get("learning_enabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_default_forge_config_learning_subsection() {
        let cfg = default_forge_config();
        let learning = cfg.get("learning").unwrap();
        assert_eq!(learning.get("min_pattern_frequency").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(learning.get("high_confidence_threshold").and_then(|v| v.as_f64()), Some(0.8));
        assert_eq!(learning.get("max_auto_creates").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(learning.get("max_refine_rounds").and_then(|v| v.as_u64()), Some(3));
        assert_eq!(learning.get("min_outcome_samples").and_then(|v| v.as_u64()), Some(5));
        assert_eq!(learning.get("monitor_window_days").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(learning.get("degrade_threshold").and_then(|v| v.as_f64()), Some(-0.2));
        assert_eq!(learning.get("degrade_cooldown_days").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(learning.get("llm_budget_tokens").and_then(|v| v.as_u64()), Some(8000));
    }

    // -------------------------------------------------------------------------
    // load_forge_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_load_forge_config_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        // Write invalid JSON
        std::fs::write(forge_dir.join("forge.json"), "not valid json {{{").unwrap();
        let cfg = load_forge_config(&forge_dir);
        // Should fall back to defaults
        assert_eq!(cfg.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(300));
    }

    // -------------------------------------------------------------------------
    // save_forge_config edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_save_forge_config_creates_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let new_dir = tmp.path().join("new_forge_dir");
        assert!(!new_dir.exists());
        save_forge_config(&new_dir, &default_forge_config()).unwrap();
        assert!(new_dir.exists());
        assert!(new_dir.join("forge.json").exists());
    }

    #[test]
    fn test_save_forge_config_overwrites() {
        let tmp = tempfile::TempDir::new().unwrap();
        save_forge_config(tmp.path(), &default_forge_config()).unwrap();

        let mut custom = default_forge_config();
        if let Some(obj) = custom.as_object_mut() {
            obj.insert("collect_interval_sec".to_string(), serde_json::Value::Number(600.into()));
        }
        save_forge_config(tmp.path(), &custom).unwrap();

        let loaded = load_forge_config(tmp.path());
        assert_eq!(loaded.get("collect_interval_sec").and_then(|v| v.as_u64()), Some(600));
    }

    // -------------------------------------------------------------------------
    // load_registry edge cases
    // -------------------------------------------------------------------------

    #[test]
    fn test_load_registry_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        std::fs::write(&registry_path, "invalid json").unwrap();
        let reg = load_registry(tmp.path());
        assert!(reg.is_empty());
    }

    #[test]
    fn test_load_registry_empty_array() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        std::fs::write(&registry_path, "[]").unwrap();
        let reg = load_registry(tmp.path());
        assert!(reg.is_empty());
    }

    #[test]
    fn test_load_registry_multiple_artifacts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.json");
        let data = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "skill1", "status": "active", "version": "1.0"},
            {"id": "a2", "type": "script", "name": "script1", "status": "draft", "version": "0.1"},
            {"id": "a3", "type": "mcp", "name": "mcp1", "status": "active", "version": "2.0"}
        ]);
        std::fs::write(&registry_path, serde_json::to_string(&data).unwrap()).unwrap();
        let reg = load_registry(tmp.path());
        assert_eq!(reg.len(), 3);
        assert_eq!(reg[0].get("type").and_then(|v| v.as_str()), Some("skill"));
        assert_eq!(reg[1].get("type").and_then(|v| v.as_str()), Some("script"));
        assert_eq!(reg[2].get("type").and_then(|v| v.as_str()), Some("mcp"));
    }

    // -------------------------------------------------------------------------
    // cmd_status (requires config file)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_status_no_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        // Don't create config file, should report disabled
        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_status_with_forge_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_status_with_forge_disabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": false}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_list tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_list_empty_registry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        cmd_list(&forge_dir, "all").unwrap();
    }

    #[test]
    fn test_cmd_list_with_registry_artifacts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"},
            {"id": "a2", "type": "script", "name": "Test Script", "version": "0.5", "status": "draft"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_list(&forge_dir, "all").unwrap();
    }

    #[test]
    fn test_cmd_list_filter_by_type() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"},
            {"id": "a2", "type": "script", "name": "Test Script", "version": "0.5", "status": "draft"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        // Filter by type "skill" - should only show skill artifacts
        cmd_list(&forge_dir, "skill").unwrap();
    }

    #[test]
    fn test_cmd_list_filter_no_match() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        // Filter by non-existent type
        cmd_list(&forge_dir, "nonexistent").unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_evaluate tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_evaluate_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "art-1", "type": "skill", "name": "Test Skill", "version": "1.0", "status": "active", "score": 0.95, "usage_count": 42}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_evaluate(&forge_dir, "art-1").unwrap();
    }

    #[test]
    fn test_cmd_evaluate_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "art-1", "type": "skill", "name": "Test Skill"}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_evaluate(&forge_dir, "nonexistent-id").unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_learning_status tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_learning_status_defaults() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        cmd_learning_status(&forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_learning_status_custom_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        }
        save_forge_config(&forge_dir, &cfg).unwrap();
        cmd_learning_status(&forge_dir).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_enable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_enable_creates_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        cmd_enable(&cfg_path, &forge_dir).unwrap();

        // Check all 7 + prompts directories were created
        for d in &["experiences", "reflections", "skills", "scripts", "mcp", "traces", "learning"] {
            assert!(forge_dir.join(d).exists(), "Directory '{}' should exist", d);
        }
        assert!(forge_dir.join("prompts").exists());
        assert!(forge_dir.join("forge.json").exists());
        assert!(forge_dir.join("registry.json").exists());
    }

    #[test]
    fn test_cmd_enable_preserves_existing_forge_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"some_field": "preserved"}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        cmd_enable(&cfg_path, &forge_dir).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(loaded.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()), Some(true));
        assert_eq!(loaded.get("forge").and_then(|f| f.get("some_field")).and_then(|v| v.as_str()), Some("preserved"));
    }

    // -------------------------------------------------------------------------
    // cmd_disable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_disable_sets_false() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let cfg = serde_json::json!({"forge": {"enabled": true, "some_field": "kept"}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        cmd_disable(&cfg_path).unwrap();

        let loaded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(loaded.get("forge").and_then(|f| f.get("enabled")).and_then(|v| v.as_bool()), Some(false));
        assert_eq!(loaded.get("forge").and_then(|f| f.get("some_field")).and_then(|v| v.as_str()), Some("kept"));
    }

    // -------------------------------------------------------------------------
    // cmd_learning_enable / cmd_learning_disable tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_learning_enable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        save_forge_config(&forge_dir, &default_forge_config()).unwrap();

        cmd_learning_enable(&forge_dir).unwrap();

        let loaded = load_forge_config(&forge_dir);
        assert_eq!(loaded.get("learning_enabled").and_then(|v| v.as_bool()), Some(true));
        // Should also auto-enable trace collection
        assert_eq!(loaded.get("trace_collection").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_cmd_learning_disable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("learning_enabled".to_string(), serde_json::Value::Bool(true));
        }
        save_forge_config(&forge_dir, &cfg).unwrap();

        cmd_learning_disable(&forge_dir).unwrap();

        let loaded = load_forge_config(&forge_dir);
        assert_eq!(loaded.get("learning_enabled").and_then(|v| v.as_bool()), Some(false));
    }

    // -------------------------------------------------------------------------
    // cmd_learning_history tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_learning_history_no_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        cmd_learning_history(&forge_dir, 10).unwrap();
    }

    #[test]
    fn test_cmd_learning_history_with_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        let learning_dir = forge_dir.join("learning");
        std::fs::create_dir_all(&learning_dir).unwrap();
        let entries = serde_json::json!([
            {"timestamp": "2026-01-01T00:00:00Z", "patterns_found": 5, "actions_generated": 3, "actions_deployed": 2},
            {"timestamp": "2026-01-02T00:00:00Z", "patterns_found": 8, "actions_generated": 6, "actions_deployed": 4}
        ]);
        let jsonl: String = entries.as_array().unwrap().iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(learning_dir.join("learning_cycles.jsonl"), jsonl).unwrap();

        cmd_learning_history(&forge_dir, 10).unwrap();
    }

    // -------------------------------------------------------------------------
    // cmd_reflect edge cases (non-runtime parts)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_reflect_forge_not_enabled() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": false}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Should print "not enabled" and return Ok
        // This doesn't need tokio runtime since it returns early
        cmd_reflect(&cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_reflect_forge_dir_not_exists() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
        // forge_dir doesn't exist - should print error and return Ok
        cmd_reflect(&cfg_path, &forge_dir).unwrap();
    }

    // -------------------------------------------------------------------------
    // Additional forge tests for coverage
    // -------------------------------------------------------------------------

    #[test]
    fn test_cmd_enable_no_existing_config_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        // Don't create config file - cmd_enable only writes if config exists
        std::fs::create_dir_all(&home).unwrap();

        cmd_enable(&cfg_path, &forge_dir).unwrap();

        // Directories should still be created
        assert!(forge_dir.join("experiences").exists());
        assert!(forge_dir.join("forge.json").exists());
        assert!(forge_dir.join("registry.json").exists());
    }

    #[test]
    fn test_cmd_disable_no_config_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        // Don't create config file - should be a no-op
        cmd_disable(&cfg_path).unwrap();
    }

    #[test]
    fn test_cmd_status_with_forge_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Create some forge directories with content
        std::fs::create_dir_all(forge_dir.join("experiences")).unwrap();
        std::fs::write(forge_dir.join("experiences").join("exp1.json"), "{}").unwrap();
        std::fs::create_dir_all(forge_dir.join("reflections")).unwrap();

        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_status_with_registry_artifacts_and_types() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let cfg_path = home.join("config.json");
        let forge_dir = home.join("workspace").join("forge");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&forge_dir).unwrap();
        let cfg = serde_json::json!({"forge": {"enabled": true}});
        std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

        // Create registry with various types and statuses
        let registry = serde_json::json!([
            {"id": "a1", "type": "skill", "status": "active"},
            {"id": "a2", "type": "skill", "status": "draft"},
            {"id": "a3", "type": "script", "status": "active"},
            {"id": "a4", "type": "mcp", "status": "deprecated"}
        ]);
        std::fs::write(
            forge_dir.join("registry.json"),
            serde_json::to_string(&registry).unwrap()
        ).unwrap();

        cmd_status(&home, &cfg_path, &forge_dir).unwrap();
    }

    #[test]
    fn test_cmd_evaluate_with_score_and_usage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        let registry = serde_json::json!([
            {"id": "eval-1", "type": "skill", "name": "Scored Skill", "version": "2.0", "status": "active", "score": 0.85, "usage_count": 150}
        ]);
        std::fs::write(forge_dir.join("registry.json"), serde_json::to_string(&registry).unwrap()).unwrap();
        cmd_evaluate(&forge_dir, "eval-1").unwrap();
    }

    #[test]
    fn test_cmd_list_fallback_directory_scan() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        // Don't create registry.json - should fall back to directory scan
        std::fs::create_dir_all(forge_dir.join("skills")).unwrap();
        std::fs::write(forge_dir.join("skills").join("skill1.json"), "{}").unwrap();
        std::fs::create_dir_all(forge_dir.join("scripts")).unwrap();
        // scripts dir is empty

        cmd_list(&forge_dir, "all").unwrap();
    }

    #[test]
    fn test_cmd_list_fallback_specific_type() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        // No registry, scan specific type directory
        std::fs::create_dir_all(forge_dir.join("mcp")).unwrap();
        std::fs::write(forge_dir.join("mcp").join("server1.json"), "{}").unwrap();

        cmd_list(&forge_dir, "mcp").unwrap();
    }

    #[test]
    fn test_cmd_learning_history_with_limit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        let learning_dir = forge_dir.join("learning");
        std::fs::create_dir_all(&learning_dir).unwrap();

        // Create 5 entries, limit to 2
        let entries: Vec<String> = (0..5).map(|i| {
            serde_json::json!({
                "timestamp": format!("2026-01-0{}T00:00:00Z", i + 1),
                "patterns_found": i,
                "actions_generated": i * 2,
                "actions_deployed": i
            }).to_string()
        }).collect();
        std::fs::write(learning_dir.join("learning_cycles.jsonl"), entries.join("\n")).unwrap();

        cmd_learning_history(&forge_dir, 2).unwrap();
    }

    #[test]
    fn test_cmd_learning_history_invalid_jsonl_line() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        let learning_dir = forge_dir.join("learning");
        std::fs::create_dir_all(&learning_dir).unwrap();

        // Mix of valid and invalid JSON lines
        let jsonl = r#"{"timestamp":"2026-01-01","patterns_found":1,"actions_generated":1,"actions_deployed":1}
invalid json line
{"timestamp":"2026-01-02","patterns_found":2,"actions_generated":2,"actions_deployed":2}"#;
        std::fs::write(learning_dir.join("learning_cycles.jsonl"), jsonl).unwrap();

        cmd_learning_history(&forge_dir, 10).unwrap();
    }

    #[test]
    fn test_learning_enable_creates_directories() {
        let tmp = tempfile::TempDir::new().unwrap();
        let forge_dir = tmp.path().join("forge");
        std::fs::create_dir_all(&forge_dir).unwrap();
        save_forge_config(&forge_dir, &default_forge_config()).unwrap();

        cmd_learning_enable(&forge_dir).unwrap();

        assert!(forge_dir.join("learning").exists());
        assert!(forge_dir.join("traces").exists());
    }

    #[test]
    fn test_forge_config_round_trip_preserves_custom_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = default_forge_config();
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("custom_field".to_string(), serde_json::json!("custom_value"));
            obj.insert("collect_interval_sec".to_string(), serde_json::json!(600));
        }
        save_forge_config(tmp.path(), &cfg).unwrap();

        let loaded = load_forge_config(tmp.path());
        assert_eq!(loaded["custom_field"], "custom_value");
        assert_eq!(loaded["collect_interval_sec"], 600);
    }
}
