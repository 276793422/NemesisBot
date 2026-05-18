//! Memory system test helpers.
//!
//! Shared utilities for ST (system tests) that drive real bot processes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use test_harness::*;

/// Resolve the plugin DLL path (relative to project root).
pub fn resolve_plugin_dll() -> Result<PathBuf> {
    let root = resolve_project_root()?;
    let dll = root.join("target/release/plugins/plugin_onnx.dll");
    if dll.exists() {
        return Ok(dll);
    }
    let dll = root.join("target/debug/plugins/plugin_onnx.dll");
    if dll.exists() {
        return Ok(dll);
    }
    bail!("plugin_onnx.dll not found in target/release/plugins or target/debug/plugins")
}

/// Resolve model directory containing model.onnx + tokenizer.json.
/// Searches: test-data/memory-e2e/, crates/nemesis-memory/models/all-MiniLM-L6-v2/
pub fn resolve_model_dir() -> Result<PathBuf> {
    let root = resolve_project_root()?;
    // Preferred: test-data/memory-e2e/ (has model.onnx at root level)
    let alt = root.join("test-data").join("memory-e2e");
    if alt.join("model.onnx").exists() && alt.join("tokenizer.json").exists() {
        return Ok(alt);
    }
    // Also check: crates/nemesis-memory/models/all-MiniLM-L6-v2/
    let model_dir = root.join("crates").join("nemesis-memory").join("models").join("all-MiniLM-L6-v2");
    if model_dir.join("model.onnx").exists() && model_dir.join("tokenizer.json").exists() {
        return Ok(model_dir);
    }
    bail!("model.onnx + tokenizer.json not found in test-data/memory-e2e/ or crates/nemesis-memory/models/all-MiniLM-L6-v2/")
}

/// Copy embedding model files to workspace config dir so the plugin can find them.
/// Copies: embedding.toml, model.onnx, tokenizer.json
pub fn copy_model_files_to_workspace(workspace: &TestWorkspace) -> Result<()> {
    let model_src = resolve_model_dir()?;
    let config_dir = workspace.home().join("workspace").join("config");
    std::fs::create_dir_all(&config_dir)?;

    // Copy embedding.toml from project
    let root = resolve_project_root()?;
    let emb_toml_src = root.join("crates").join("nemesis-memory").join("config").join("embedding.toml");
    if emb_toml_src.exists() {
        std::fs::copy(&emb_toml_src, config_dir.join("embedding.toml"))
            .context("copying embedding.toml")?;
    }

    // Copy model.onnx and tokenizer.json
    std::fs::copy(model_src.join("model.onnx"), config_dir.join("model.onnx"))
        .context("copying model.onnx")?;
    std::fs::copy(model_src.join("tokenizer.json"), config_dir.join("tokenizer.json"))
        .context("copying tokenizer.json")?;
    Ok(())
}

/// Write a config.enhanced_memory.json to the workspace.
pub fn write_enhanced_memory_config(workspace: &TestWorkspace, json: &str) -> Result<()> {
    let config_dir = workspace.home().join("workspace").join("config");
    std::fs::create_dir_all(&config_dir)?;
    let path = config_dir.join("config.enhanced_memory.json");
    std::fs::write(&path, json)?;
    Ok(())
}

/// Enable the main memory switch in config.json.
/// The gateway checks config.json: memory.enabled before creating MemoryManager.
pub fn enable_main_memory_switch(workspace: &TestWorkspace) -> Result<()> {
    let cfg_path = workspace.config_path();
    let content = std::fs::read_to_string(&cfg_path)
        .context("reading config.json")?;
    let mut cfg: serde_json::Value = serde_json::from_str(&content)?;
    if let Some(mem) = cfg.get_mut("memory") {
        if let Some(obj) = mem.as_object_mut() {
            obj.insert("enabled".into(), serde_json::Value::Bool(true));
        }
    }
    let json = serde_json::to_string_pretty(&cfg)?;
    std::fs::write(&cfg_path, json)?;
    Ok(())
}

/// Set up a basic workspace for memory testing:
/// 1. Create TestWorkspace
/// 2. Run `onboard default --local`
/// 3. Add test AI model
pub async fn setup_basic_workspace(nemesisbot_bin: &Path) -> Result<TestWorkspace> {
    let ws = TestWorkspace::new()?;

    // Initialize workspace
    let output = ws.run_cli(nemesisbot_bin, &["onboard", "default"]).await;
    if !output.success() {
        bail!("onboard default failed: {}", output.stderr);
    }

    // Add test model
    let output = ws
        .run_cli(
            nemesisbot_bin,
            &[
                "model",
                "add",
                "--model",
                "test/testai-6.0",
                "--base",
                "http://127.0.0.1:8080/v1",
                "--key",
                "test-key",
                "--default",
            ],
        )
        .await;
    if !output.success() {
        bail!("model add failed: {}", output.stderr);
    }

    Ok(ws)
}

/// Start the TestAIServer and return a ManagedProcess.
pub async fn start_ai_server(ai_bin: &Path, cwd: &Path) -> Result<ManagedProcess> {
    cleanup_ports(&[AI_SERVER_PORT]);
    ManagedProcess::spawn("AI Server", ai_bin, &[], cwd)
}

/// Start the Gateway and wait for it to become healthy.
pub async fn start_gateway_and_wait(
    nemesisbot_bin: &Path,
    cwd: &Path,
) -> Result<ManagedProcess> {
    cleanup_ports(&[WEB_PORT, HEALTH_PORT]);
    let gw = ManagedProcess::spawn("Gateway", nemesisbot_bin, &["gateway"], cwd)?;
    wait_for_http(
        &format!("http://127.0.0.1:{}/health", HEALTH_PORT),
        Duration::from_secs(30),
    )
    .await
    .context("Gateway health check failed")?;
    Ok(gw)
}
