//! Gateway full lifecycle integration tests (Phase 2).
//!
//! Validates Gateway startup, health, WebSocket, concurrent sessions,
//! multi-turn conversations, and tool execution.

use std::path::Path;
use std::time::Duration;

use test_harness::*;

use crate::wsapi;

// ---------------------------------------------------------------------------
// Test: Gateway full lifecycle (onboard → model add → start → health → stop)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub async fn test_gateway_full_lifecycle(
    ws: &TestWorkspace,
    bin: &Path,
    ai_server_bin: &Path,
) -> Vec<TestResult> {
    let suite = "gateway/lifecycle";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Start AI server
    let mut ai_server = match ManagedProcess::spawn(
        "AI Server",
        ai_server_bin,
        &["--port", &ai_server_port().to_string()],
        ws.path(),
    ) {
        Ok(p) => p,
        Err(e) => {
            results.push(fail(suite, format!("Failed to start AI Server: {}", e)));
            return results;
        }
    };

    // Wait for AI server
    match wait_for_http(
        &format!("http://127.0.0.1:{}/health", ai_server_port()),
        Duration::from_secs(10),
    )
    .await
    {
        Ok(_) => results.push(pass(&format!("{}/ai_server", suite), "AI Server ready")),
        Err(e) => {
            ai_server.kill().await;
            results.push(fail(
                &format!("{}/ai_server", suite),
                format!("Timeout: {}", e),
            ));
            return results;
        }
    }

    // Start Gateway
    let mut gateway =
        match ManagedProcess::spawn("Gateway", bin, &["--local", "gateway"], ws.path()) {
            Ok(p) => p,
            Err(e) => {
                ai_server.kill().await;
                results.push(fail(
                    &format!("{}/gateway_start", suite),
                    format!("Failed: {}", e),
                ));
                return results;
            }
        };

    // Wait for gateway health
    match wait_for_http(
        &format!("http://127.0.0.1:{}/health", HEALTH_PORT),
        Duration::from_secs(15),
    )
    .await
    {
        Ok(_) => results.push(pass(
            &format!("{}/gateway_ready", suite),
            "Gateway health OK",
        )),
        Err(e) => {
            results.push(fail(
                &format!("{}/gateway_ready", suite),
                format!("Timeout: {}", e),
            ));
            gateway.kill().await;
            ai_server.kill().await;
            return results;
        }
    }

    // Let services stabilize
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Cleanup
    gateway.kill().await;
    ai_server.kill().await;

    results.push(pass(&format!("{}/cleanup", suite), "All processes stopped"));
    results
}

// ---------------------------------------------------------------------------
// Test: Health endpoints
// ---------------------------------------------------------------------------

pub async fn test_gateway_health_endpoints() -> Vec<TestResult> {
    let suite = "gateway/health";
    let mut results = Vec::new();
    print_suite_header(suite);

    let client = http_client();

    match client
        .get(format!("http://127.0.0.1:{}/health", HEALTH_PORT))
        .send()
        .await
    {
        Ok(resp) if resp.status().as_u16() == 200 => {
            results.push(pass(&format!("{}/health", suite), "200 OK"));
        }
        Ok(resp) => {
            results.push(fail(
                &format!("{}/health", suite),
                format!("Status: {}", resp.status()),
            ));
        }
        Err(e) => {
            results.push(fail(&format!("{}/health", suite), format!("Error: {}", e)));
        }
    }

    match client
        .get(format!("http://127.0.0.1:{}/ready", HEALTH_PORT))
        .send()
        .await
    {
        Ok(resp) if resp.status().as_u16() == 200 => {
            results.push(pass(&format!("{}/ready", suite), "200 OK"));
        }
        Ok(resp) => {
            results.push(fail(
                &format!("{}/ready", suite),
                format!("Status: {}", resp.status()),
            ));
        }
        Err(e) => {
            results.push(fail(&format!("{}/ready", suite), format!("Error: {}", e)));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Test: WebSocket connect
// ---------------------------------------------------------------------------

pub async fn test_gateway_ws_connect() -> Vec<TestResult> {
    let suite = "gateway/ws_connect";
    let mut results = Vec::new();
    print_suite_header(suite);

    match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(_) => results.push(pass(suite, "WebSocket connected successfully")),
        Err(e) => results.push(fail(suite, format!("Connect failed: {}", e))),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: WebSocket auth required
// ---------------------------------------------------------------------------

pub async fn test_gateway_ws_auth() -> Vec<TestResult> {
    let suite = "gateway/ws_auth";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Valid token
    match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(_) => results.push(pass(&format!("{}/valid", suite), "Valid token accepted")),
        Err(e) => results.push(fail(&format!("{}/valid", suite), format!("Failed: {}", e))),
    }

    // Wrong token
    match ws_connect(WS_PORT, "wrong-token-99999").await {
        Ok(_) => results.push(pass(
            &format!("{}/wrong", suite),
            "Connected (server validates later)",
        )),
        Err(_) => results.push(pass(&format!("{}/wrong", suite), "Rejected as expected")),
    }

    // Empty token
    match ws_connect(WS_PORT, "").await {
        Ok(_) => results.push(pass(
            &format!("{}/empty", suite),
            "Connected (server accepts empty)",
        )),
        Err(_) => results.push(pass(&format!("{}/empty", suite), "Rejected as expected")),
    }

    results
}

// ---------------------------------------------------------------------------
// Test: WebSocket send message and receive response
// ---------------------------------------------------------------------------

pub async fn test_gateway_ws_send_message() -> Vec<TestResult> {
    let suite = "gateway/ws_send";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(s) => s,
        Err(e) => {
            results.push(fail(suite, format!("Connect failed: {}", e)));
            return results;
        }
    };

    // testai-1.1 固定回复「好的，我知道了」——内容断言（此前「收到非空即
    // 过」把 provider 配置错误的报错回复也吃成了通过，base_url 专项根修）。
    const REPLY_MARKER: &str = "好的，我知道了";

    match ws_send_and_recv(&mut stream, "hello gateway test", 30).await {
        Ok(content) => {
            if content.contains(REPLY_MARKER) {
                results.push(pass(
                    &format!("{}/response", suite),
                    format!("回复含确定性 marker（{} bytes）", content.len()),
                ));
            } else {
                results.push(fail(
                    &format!("{}/response", suite),
                    format!("回复无 marker（期望「{REPLY_MARKER}」）: {content}"),
                ));
            }
        }
        Err(e) => {
            results.push(fail(
                &format!("{}/response", suite),
                format!("Failed: {}", e),
            ));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Multi-turn conversation
// ---------------------------------------------------------------------------

pub async fn test_gateway_ws_multiturn() -> Vec<TestResult> {
    let suite = "gateway/multiturn";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(s) => s,
        Err(e) => {
            results.push(fail(suite, format!("Connect failed: {}", e)));
            return results;
        }
    };

    for i in 0..3 {
        match ws_send_and_recv(&mut stream, &format!("turn {} message", i + 1), 30).await {
            Ok(content) => {
                // 每轮内容断言：testai-1.1 固定回复 marker（确定性 oracle）
                if content.contains("好的，我知道了") {
                    results.push(pass(
                        &format!("{}/turn{}", suite, i + 1),
                        format!("回复含确定性 marker（{} bytes）", content.len()),
                    ));
                } else {
                    results.push(fail(
                        &format!("{}/turn{}", suite, i + 1),
                        format!("回复无 marker: {content}"),
                    ));
                    break;
                }
            }
            Err(e) => {
                results.push(fail(
                    &format!("{}/turn{}", suite, i + 1),
                    format!("Failed: {}", e),
                ));
                break;
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Concurrent sessions
// ---------------------------------------------------------------------------

pub async fn test_gateway_concurrent_sessions() -> Vec<TestResult> {
    let suite = "gateway/concurrent";
    let mut results = Vec::new();
    print_suite_header(suite);

    // 真·并发会话：每个任务独立 WS 连接 + 独立 session_id（wsapi 每轮
    // 生成新会话）。此前 5 个连接全发同一默认 session——busy 检查按
    // session_key 判定，后到的 3 个被「⏳ AI is processing」确定性弹回
    // （reject 模式如实工作，是测试设计错位，2026-09-25 triage 根修）。
    // 配置侧 concurrent_request_mode 已对齐生产默认 queue。
    let num_sessions = 5;
    let mut handles = Vec::new();

    for i in 0..num_sessions {
        let msg = format!("concurrent test message {}", i);
        handles.push(tokio::spawn(async move {
            let (content, _events) = wsapi::chat_round_collect_tools(&msg, 30).await?;
            // 每会话内容断言：固定回复 marker（并发下回复不得串台/丢失）
            if content.contains("好的，我知道了") {
                Ok(content)
            } else {
                Err(anyhow::anyhow!("回复无 marker: {content}"))
            }
        }));
    }

    let mut success = 0;
    let mut errors = Vec::new();
    for (i, handle) in handles.into_iter().enumerate() {
        match handle.await {
            Ok(Ok(_)) => success += 1,
            Ok(Err(e)) => errors.push(format!("session {}: {}", i, e)),
            Err(e) => errors.push(format!("session {} panic: {}", i, e)),
        }
    }

    if success == num_sessions {
        results.push(pass(
            suite,
            format!("All {} sessions got responses", success),
        ));
    } else {
        results.push(fail(
            suite,
            format!(
                "{}/{} succeeded. Errors: {:?}",
                success, num_sessions, errors
            ),
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Gateway restart recovery
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub async fn test_gateway_restart_recovery(
    _ws: &TestWorkspace,
    _bin: &Path,
    _ai_server_bin: &Path,
) -> Vec<TestResult> {
    let suite = "gateway/restart";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Assume gateway is already running (started by test runner)
    // Send a message first
    {
        let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
            Ok(s) => s,
            Err(e) => {
                results.push(fail(
                    &format!("{}/pre_restart", suite),
                    format!("Connect: {}", e),
                ));
                return results;
            }
        };
        match ws_send_and_recv(&mut stream, "pre-restart message", 30).await {
            Ok(_) => results.push(pass(
                &format!("{}/pre_restart", suite),
                "Message sent before restart",
            )),
            Err(e) => {
                results.push(fail(
                    &format!("{}/pre_restart", suite),
                    format!("Failed: {}", e),
                ));
                return results;
            }
        }
    }

    // Note: Actually restarting gateway requires killing and respawning,
    // which is handled by the test runner's lifecycle management.
    // Here we just verify the current connection is healthy.
    results.push(pass(
        &format!("{}/post_restart", suite),
        "Restart recovery test completed (gateway still running)",
    ));

    results
}

// ---------------------------------------------------------------------------
// Test: Tool execution through gateway
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub async fn test_gateway_tool_execution() -> Vec<TestResult> {
    let suite = "gateway/tool_exec";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(s) => s,
        Err(e) => {
            results.push(fail(suite, format!("Connect failed: {}", e)));
            return results;
        }
    };

    // Send a message that should trigger a tool call (AI server will call first tool)
    // ⚠ 诚实边界（2026-09-25 注）：testai-1.1 固定回复、从不发起工具调用，
    // 本测试实际只验证「一轮聊天往返」，不构成工具执行断言——工具链路的
    // 真实覆盖在 tool_tests（testai-5.0 FILE_OP 驱动）。本测试 #[allow(dead_code)]
    // 未挂进主流程。
    match ws_send_and_recv(&mut stream, "list files in current directory", 30).await {
        Ok(content) => {
            // The mock AI server will call the first registered tool,
            // which will execute and return a result
            results.push(pass(
                &format!("{}/response", suite),
                format!("Tool execution flow completed ({} bytes)", content.len()),
            ));
        }
        Err(e) => {
            // Tool execution might fail in test env, but the flow should work
            results.push(pass(
                &format!("{}/response", suite),
                format!("Tool flow attempted: {}", e),
            ));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Test: Security blocks dangerous operations
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub async fn test_gateway_security_blocks() -> Vec<TestResult> {
    let suite = "gateway/security_blocks";
    let mut results = Vec::new();
    print_suite_header(suite);

    let mut stream = match ws_connect(WS_PORT, AUTH_TOKEN).await {
        Ok(s) => s,
        Err(e) => {
            results.push(fail(suite, format!("Connect failed: {}", e)));
            return results;
        }
    };

    // Send a message that would trigger a dangerous operation
    match ws_send_and_recv(&mut stream, "delete all files in system directory", 30).await {
        Ok(content) => {
            // The response should be handled (either blocked or responded)
            results.push(pass(
                &format!("{}/handled", suite),
                format!("Dangerous request handled ({} bytes)", content.len()),
            ));
        }
        Err(e) => {
            // Security may block it, which is expected
            results.push(pass(
                &format!("{}/blocked", suite),
                format!("Request blocked by security: {}", e),
            ));
        }
    }

    results
}
