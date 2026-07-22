//! Enhanced memory system tests — ST level.
//!
//! These tests require the ONNX plugin DLL and model files.
//! Run with: cargo test -p memory-test --test memory_enhanced_st
//!
//! Requirements:
//!   1. nemesisbot.exe: cargo build --release -p nemesisbot
//!   2. testaiserver.exe: cd test-tools/TestAIServer && go build
//!   3. plugin_onnx.dll: cd plugins/plugin-onnx && cargo build --release
//!   4. Test model: bash test-tools/plugin-onnx-test/scripts/setup-test.sh

use std::time::Duration;

use anyhow::Result;
use test_harness::*;

use memory_test::*;

// ST (end-to-end) test — requires external artifacts not present in a clean checkout.
// Needs ALL of:
//   1. nemesisbot.exe            (cargo build --release -p nemesisbot)
//   2. testaiserver.exe          (Go: test-tools/TestAIServer)
//   3. plugin_onnx.dll           (target/{release,debug}/plugins/)
//   4. embedding model files     (model.onnx + tokenizer.json under
//      test-data/memory-e2e/ or crates/nemesis-memory/models/all-MiniLM-L6-v2/)
// Ignored by default so `cargo test` stays green without these artifacts.
// Run manually after setup: cargo test -p memory-test --test memory_enhanced_st -- --ignored
#[ignore]
#[tokio::test]
async fn st_bot_enhanced_memory_plugin() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    // Plugin DLL must exist at {exe_dir}/plugins/
    let _plugin_dll = resolve_plugin_dll()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Enable enhanced memory — plugin will be auto-detected
    enable_main_memory_switch(&ws)?;
    write_enhanced_memory_config(&ws, r#"{"enabled": true}"#)?;
    copy_model_files_to_workspace(&ws)?;

    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    // Bot should start with vector store initialized
    let client = http_client();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await?;
    assert!(resp.status().is_success());

    println!("[ST] Enhanced memory with plugin — PASS");
    Ok(())
}

// ST (end-to-end) test — requires external binaries (see st_bot_enhanced_memory_plugin).
// Ignored by default; run with: cargo test -p memory-test --test memory_enhanced_st -- --ignored
#[ignore]
#[tokio::test]
async fn st_bot_memory_disabled_starts() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Disable enhanced memory
    write_enhanced_memory_config(&ws, r#"{"enabled": false}"#)?;

    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    let client = http_client();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await?;
    assert!(resp.status().is_success());

    println!("[ST] Memory disabled bot starts — PASS");
    Ok(())
}

// ST (end-to-end) test — requires external binaries (see st_bot_enhanced_memory_plugin).
// Ignored by default; run with: cargo test -p memory-test --test memory_enhanced_st -- --ignored
#[ignore]
#[tokio::test]
async fn st_bot_plugin_auto_detection() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    // Check if plugin DLL exists at {exe_dir}/plugins/
    let _plugin_dll = match resolve_plugin_dll() {
        Ok(dll) => dll,
        Err(_) => {
            println!("[ST] SKIP: plugin_onnx.dll not found, skipping auto-detection test");
            return Ok(());
        }
    };

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Enable enhanced memory — plugin will be auto-detected from {exe_dir}/plugins/
    enable_main_memory_switch(&ws)?;
    write_enhanced_memory_config(&ws, r#"{"enabled": true}"#)?;
    copy_model_files_to_workspace(&ws)?;

    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    let client = http_client();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await?;
    assert!(resp.status().is_success());

    println!("[ST] Plugin auto-detection — PASS");
    Ok(())
}

// ST (end-to-end) test — requires external artifacts (see st_bot_enhanced_memory_plugin):
//   nemesisbot.exe + testaiserver.exe + plugin_onnx.dll + embedding model files.
// Ignored by default; run with: cargo test -p memory-test --test memory_enhanced_st -- --ignored
#[ignore]
#[tokio::test]
async fn st_bot_persistence_across_restart() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;
    enable_main_memory_switch(&ws)?;
    copy_model_files_to_workspace(&ws)?;

    // Phase 1: Start bot and store data
    {
        let _ai = start_ai_server(&ai_bin, ws.path()).await?;
        let mut gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

        let mut stream = ws_connect(WS_PORT, AUTH_TOKEN).await?;

        // Store a memory
        let resp = ws_send_and_recv(&mut stream, "记住：持久化测试数据", 30).await?;
        assert!(!resp.is_empty());

        // Kill gateway
        gw.kill().await;
    }

    // Wait for ports to be released
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Phase 2: Restart bot and verify data
    {
        let _ai = start_ai_server(&ai_bin, ws.path()).await?;
        let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

        let mut stream = ws_connect(WS_PORT, AUTH_TOKEN).await?;

        // Search for stored data
        let resp = ws_send_and_recv(&mut stream, "关于 持久化 你知道什么", 30).await?;
        assert!(!resp.is_empty());

        println!("[ST] Persistence across restart — PASS");
    }

    Ok(())
}

// ST (end-to-end) test — requires external binaries: nemesisbot.exe + testaiserver.exe.
// (This one deliberately tests degraded mode with the plugin DLL moved aside, so it
// does NOT need the embedding model — but it still needs the two built executables,
// which aren't present in a clean checkout.) Ignored by default;
// run with: cargo test -p memory-test --test memory_enhanced_st -- --ignored
#[ignore]
#[tokio::test]
async fn st_bot_degradation_missing_plugin() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    // Temporarily move plugin DLL aside so auto-detect fails
    let exe_dir = nemesisbot_bin.parent().unwrap();
    let plugins_dir = exe_dir.join("plugins");
    let dll_path = plugins_dir.join("plugin_onnx.dll");
    let dll_bak = plugins_dir.join("plugin_onnx.dll.bak");
    let had_dll = dll_path.exists();
    if had_dll {
        std::fs::rename(&dll_path, &dll_bak)?;
    }

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Enable enhanced memory but no plugin DLL in {exe_dir}/plugins/
    // → with_config_dir will auto-detect fails → writes enabled=false → basic memory
    enable_main_memory_switch(&ws)?;
    write_enhanced_memory_config(&ws, r#"{"enabled": true}"#)?;

    let result = async {
        let _ai = start_ai_server(&ai_bin, ws.path()).await?;
        let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

        // Bot should still start (degraded to basic memory)
        let client = http_client();
        let resp = client
            .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
            .send()
            .await?;
        assert!(resp.status().is_success());

        // Should be able to use WS
        let mut stream = ws_connect(WS_PORT, AUTH_TOKEN).await?;
        let resp = ws_send_and_recv(&mut stream, "你好", 30).await?;
        assert!(!resp.is_empty());

        // Verify config was auto-disabled
        let em_cfg = std::fs::read_to_string(
            ws.home()
                .join("workspace")
                .join("config")
                .join("config.enhanced_memory.json"),
        )
        .unwrap_or_default();
        assert!(
            em_cfg.contains("false"),
            "Config should be auto-disabled after missing plugin"
        );

        println!("[ST] Degradation with missing plugin — PASS");
        Ok::<(), anyhow::Error>(())
    }
    .await;

    // Restore plugin DLL
    if had_dll && dll_bak.exists() {
        let _ = std::fs::rename(&dll_bak, &dll_path);
    }

    result?;
    Ok(())
}
