//! Memory command - manage enhanced memory system.
//!
//! Subcommands: enable, disable, status.
//!
//! The main switch lives in `config.json: memory.enabled`.
//! The sub-switch lives in `config.enhanced_memory.json: enabled`.
//!
//! Plugin is always auto-detected at `{exe_dir}/plugins/` (platform-specific filename).

use std::path::Path;

use anyhow::{Result, Context, bail};
use serde_json::Value;

use crate::common;

// ---------------------------------------------------------------------------
// Clap subcommands
// ---------------------------------------------------------------------------

#[derive(clap::Subcommand)]
pub enum MemoryAction {
    /// Enable enhanced memory (requires plugin library in plugins/)
    Enable,
    /// Disable enhanced memory
    Disable,
    /// Show memory system status
    Status,
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub async fn run(action: MemoryAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);

    match action {
        MemoryAction::Enable => cmd_enable(&home).await,
        MemoryAction::Disable => cmd_disable(&home),
        MemoryAction::Status => cmd_status(&home),
    }
}

// ---------------------------------------------------------------------------
// Enable
// ---------------------------------------------------------------------------

async fn cmd_enable(home: &Path) -> Result<()> {
    let cfg_path = common::config_path(home);
    let em_cfg_path = common::enhanced_memory_config_path(home);
    let config_dir = em_cfg_path.parent().unwrap_or(home);

    // Ensure config directories exist
    if let Some(parent) = em_cfg_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating config dir {}", parent.display()))?;
    }

    // --- Step 1: Verify plugin library exists ---
    let plugin_path = detect_plugin_path().ok_or_else(|| {
        let label = nemesis_utils::plugin_library_label();
        let filename = nemesis_utils::plugin_library_filename("plugin_onnx");
        anyhow::anyhow!(
            "Plugin {} not found. Enhanced memory requires {}.\n\
             Expected location: {{exe_dir}}/plugins/{}",
            label, filename, filename
        )
    })?;
    println!("Plugin found: {}", plugin_path);

    // --- Step 2: Verify model files are installed ---
    let emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
    let (_, dim) = nemesis_memory::vector::embedding_config::resolve_model_files(&emb_cfg, config_dir)
        .map_err(|e| anyhow::anyhow!("{}. 请先运行 nemesisbot memory install 下载模型", e))?;
    println!("Model files ready (dim={})", dim);

    // --- Step 3: Write config.enhanced_memory.json with enabled=true ---
    let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
    emb_cfg.enabled = true;
    nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);
    println!("Enhanced memory config saved");

    // --- Step 4: Set config.json memory.enabled = true ---
    set_main_switch(&cfg_path, true)?;

    // --- Summary ---
    println!();
    println!("Enhanced memory ENABLED");
    println!("  Main switch:     config.json → memory.enabled = true");
    println!("  Sub-switch:      {} → enabled = true", em_cfg_path.display());
    println!("  Plugin:          {}", plugin_path);
    println!("  Dimension:       {}", dim);
    println!();
    println!("Restart the gateway to apply changes: nemesisbot gateway");

    Ok(())
}

// ---------------------------------------------------------------------------
// Disable
// ---------------------------------------------------------------------------

fn cmd_disable(home: &Path) -> Result<()> {
    let cfg_path = common::config_path(home);
    let em_cfg_path = common::enhanced_memory_config_path(home);

    if !cfg_path.exists() {
        bail!("Config file not found: {}", cfg_path.display());
    }

    set_main_switch(&cfg_path, false)?;

    // Also set sub-switch to false via unified config
    let config_dir = em_cfg_path.parent().unwrap_or(home);
    let mut emb_cfg = nemesis_memory::vector::embedding_config::load_embedding_config(config_dir);
    emb_cfg.enabled = false;
    nemesis_memory::vector::embedding_config::save_embedding_config(&emb_cfg, config_dir);

    println!("Enhanced memory DISABLED");
    println!("  config.json → memory.enabled = false");
    println!();
    println!("Restart the gateway to apply changes: nemesisbot gateway");

    Ok(())
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

fn cmd_status(home: &Path) -> Result<()> {
    let cfg_path = common::config_path(home);
    let em_cfg_path = common::enhanced_memory_config_path(home);

    // 1. Main switch
    let main_enabled = read_main_switch(&cfg_path);

    println!("Enhanced Memory Status");
    println!("─────────────────────────────────────");
    println!(
        "  Main switch:    {} ({})",
        if main_enabled { "ENABLED" } else { "DISABLED" },
        cfg_path.display()
    );

    // 2. Sub-switch
    let sub_enabled = if em_cfg_path.exists() {
        match std::fs::read_to_string(&em_cfg_path) {
            Ok(content) => match serde_json::from_str::<Value>(&content) {
                Ok(cfg) => {
                    let enabled = cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                    println!(
                        "  Sub-switch:     {} ({})",
                        if enabled { "ENABLED" } else { "DISABLED" },
                        em_cfg_path.display()
                    );
                    enabled
                }
                Err(e) => {
                    println!("  Sub-switch:     PARSE ERROR ({})", e);
                    false
                }
            },
            Err(e) => {
                println!("  Sub-switch:     READ ERROR ({})", e);
                false
            }
        }
    } else {
        println!("  Sub-switch:     NOT CONFIGURED ({})", em_cfg_path.display());
        false
    };

    // 3. Check plugin DLL
    let plugin_path = detect_plugin_path();
    match &plugin_path {
        Some(p) => println!("  Plugin DLL:     {} [OK]", p),
        None => println!("  Plugin DLL:     NOT FOUND [MISSING]"),
    }

    // 4. Check model files
    let config_dir = em_cfg_path.parent().unwrap_or(home);
    let emb_data_dir = nemesis_memory::vector::embedding_config::embedding_data_dir(config_dir);
    let has_model = emb_data_dir.exists() && has_onnx_files(&emb_data_dir);
    println!(
        "  Model files:    {} [{}]",
        emb_data_dir.display(),
        common::status_icon(has_model)
    );

    // 5. Overall status
    println!("─────────────────────────────────────");
    if main_enabled && sub_enabled && plugin_path.is_some() && has_model {
        println!("  Overall:        READY (semantic search active)");
    } else if main_enabled {
        println!("  Overall:        DEGRADED (keyword search only)");
        if !sub_enabled {
            println!("                   Reason: sub-switch disabled (previous init failure?)");
        }
        if plugin_path.is_none() {
            println!("                   Reason: plugin DLL not found");
        }
        if !has_model {
            println!("                   Reason: model files not downloaded");
        }
    } else {
        println!("  Overall:        DISABLED (memory.enabled = false in config.json)");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the `memory.enabled` field from the main config.json.
fn read_main_switch(cfg_path: &Path) -> bool {
    if !cfg_path.exists() {
        return false;
    }
    std::fs::read_to_string(cfg_path).ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|v| v.get("memory").and_then(|m| m.get("enabled")).and_then(|e| e.as_bool()))
        .unwrap_or(false)
}

/// Set the `memory.enabled` field in the main config.json.
fn set_main_switch(cfg_path: &Path, enabled: bool) -> Result<()> {
    if !cfg_path.exists() {
        bail!("Config file not found: {}", cfg_path.display());
    }

    let content = std::fs::read_to_string(cfg_path)
        .with_context(|| format!("reading {}", cfg_path.display()))?;

    let mut cfg: Value = serde_json::from_str(&content)
        .with_context(|| format!("parsing {}", cfg_path.display()))?;

    // Ensure memory object exists
    if cfg.get("memory").is_none() {
        cfg.as_object_mut()
            .map(|o| o.insert("memory".to_string(), serde_json::json!({})));
    }

    if let Some(mem) = cfg.get_mut("memory").and_then(|m| m.as_object_mut()) {
        mem.insert("enabled".to_string(), Value::Bool(enabled));
    }

    let updated = serde_json::to_string_pretty(&cfg)
        .context("serializing config")?;
    std::fs::write(cfg_path, updated)
        .with_context(|| format!("writing {}", cfg_path.display()))?;

    Ok(())
}

/// Auto-detect plugin library next to the current executable.
fn detect_plugin_path() -> Option<String> {
    nemesis_utils::find_plugin_library("plugin_onnx")
        .map(|p| p.to_string_lossy().to_string())
}

/// Check if any .onnx files exist under the given directory (recursively).
fn has_onnx_files(dir: &Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if has_onnx_files(&path) {
                    return true;
                }
            } else if path.extension().map_or(false, |e| e == "onnx") {
                return true;
            }
        }
    }
    false
}
