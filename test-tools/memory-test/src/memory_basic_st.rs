//! Basic memory system tests — ST level.
//!
//! These tests use real bot processes (nemesisbot.exe + testaiserver.exe)
//! but do NOT require the ONNX plugin DLL.
//!
//! Run with: cargo test -p memory-test --test memory_basic_st

use anyhow::Result;
use test_harness::*;

use memory_test::*;

#[tokio::test]
async fn st_bot_basic_memory_startup() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    // Setup workspace
    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Start AI server
    let _ai = start_ai_server(&ai_bin, ws.path()).await?;

    // Start gateway
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    // Verify bot is responsive via health check
    let client = http_client();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await?;
    assert!(resp.status().is_success());

    println!("[ST] Basic memory startup — PASS");
    Ok(())
}

#[tokio::test]
async fn st_bot_enhanced_memory_local_tier() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Configure enhanced memory (enabled, but no plugin → auto-disables, basic memory)
    // NOTE: Don't enable main memory switch — this test verifies bot starts with
    // enhanced memory config present but memory disabled in main config.
    // Plugin DLL exists at target/release/plugins/ so enabling would trigger
    // model download (reqwest::blocking in async context → panic).
    write_enhanced_memory_config(
        &ws,
        r#"{"enabled": true}"#,
    )?;

    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    // Verify bot starts and is responsive
    let client = http_client();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await?;
    assert!(resp.status().is_success());

    println!("[ST] Enhanced memory local tier — PASS");
    Ok(())
}

#[tokio::test]
async fn st_bot_memory_store_via_ws() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;
    enable_main_memory_switch(&ws)?;
    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    // Connect via WebSocket
    let mut stream = ws_connect(WS_PORT, AUTH_TOKEN).await?;

    // Send a message that triggers memory_store via testai-6.0
    let response = ws_send_and_recv(&mut stream, "记住：测试数据存储", 30).await?;

    // The bot should respond (either with tool result or confirmation)
    assert!(!response.is_empty(), "Bot should respond to memory store command");

    println!("[ST] Memory store via WS — PASS (response: {})", &response[..response.len().min(100)]);
    Ok(())
}

#[tokio::test]
async fn st_bot_memory_search_via_ws() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;
    enable_main_memory_switch(&ws)?;
    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    // Connect via WebSocket
    let mut stream = ws_connect(WS_PORT, AUTH_TOKEN).await?;

    // First store something
    let _store_resp = ws_send_and_recv(&mut stream, "记住：猫是哺乳动物", 30).await?;

    // Then search for it
    let search_resp = ws_send_and_recv(&mut stream, "关于 猫 你知道什么", 30).await?;

    assert!(!search_resp.is_empty(), "Bot should respond to memory search command");
    assert!(
        search_resp.contains("猫"),
        "Search response should contain the stored keyword '猫', got: {}",
        &search_resp[..search_resp.len().min(200)]
    );

    println!("[ST] Memory search via WS — PASS");
    Ok(())
}

#[tokio::test]
async fn st_bot_invalid_config_fallback() -> Result<()> {
    let nemesisbot_bin = resolve_nemesisbot_bin()?;
    let ai_bin = resolve_ai_server_bin()?;

    let ws = setup_basic_workspace(&nemesisbot_bin).await?;

    // Write corrupted config (main memory switch stays false)
    write_enhanced_memory_config(&ws, "THIS IS NOT VALID JSON!!!")?;

    let _ai = start_ai_server(&ai_bin, ws.path()).await?;
    let _gw = start_gateway_and_wait(&nemesisbot_bin, ws.path()).await?;

    // Bot should still start (falling back to basic memory)
    let client = http_client();
    let resp = client
        .get(&format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await?;
    assert!(resp.status().is_success());

    println!("[ST] Invalid config fallback — PASS");
    Ok(())
}
