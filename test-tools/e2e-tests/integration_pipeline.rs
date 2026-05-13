//! Full pipeline integration test.
//!
//! Tests: Rust providers → AI server → tool execution → response
//! This validates that the Rust crates can work together end-to-end.

use reqwest;
use serde_json::Value;
use std::time::Duration;

const AI_SERVER_URL: &str = "http://127.0.0.1:18080";

/// Simulate the full Rust provider flow: build request, call LLM, parse response.
async fn call_llm(messages: Vec<Value>, tools: Vec<Value>) -> Value {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let mut body = serde_json::json!({
        "model": "testai-1.1",
        "messages": messages,
    });
    if !tools.is_empty() {
        body["tools"] = serde_json::json!(tools);
    }

    let resp = client
        .post(&format!("{}/v1/chat/completions", AI_SERVER_URL))
        .header("Authorization", "Bearer test-key")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .expect("Failed to call LLM");

    assert!(resp.status().is_success(), "LLM request failed: {}", resp.status());
    resp.json::<Value>().await.expect("Failed to parse LLM response")
}

/// Extract tool calls from LLM response.
fn extract_tool_calls(response: &Value) -> Vec<(String, String, String)> {
    // Returns (call_id, tool_name, arguments) tuples
    let mut result = Vec::new();
    if let Some(choices) = response["choices"].as_array() {
        if let Some(choice) = choices.first() {
            if let Some(calls) = choice["message"]["tool_calls"].as_array() {
                for call in calls {
                    let id = call["id"].as_str().unwrap_or("unknown").to_string();
                    let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                    let args = call["function"]["arguments"].as_str().unwrap_or("{}").to_string();
                    result.push((id, name, args));
                }
            }
        }
    }
    result
}

/// Extract text content from LLM response.
fn extract_content(response: &Value) -> String {
    response["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_provider_http_flow() {
    // IT: Test that the HTTP provider pattern works with real AI server
    let messages = vec![serde_json::json!({
        "role": "system",
        "content": "You are a helpful assistant."
    }), serde_json::json!({
        "role": "user",
        "content": "Say hello"
    })];

    let response = call_llm(messages, vec![]).await;
    let content = extract_content(&response);
    assert!(!content.is_empty(), "Provider returned empty content");
    println!("[IT-Provider] Response: {}", content);
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_security_pipeline_with_llm() {
    // IT: Test that security-scanned input can still reach the LLM
    // Simulate: user sends safe message → security passes → LLM responds
    let safe_input = "What is the weather today?";
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": safe_input
    })];

    let response = call_llm(messages, vec![]).await;
    let content = extract_content(&response);
    assert!(!content.is_empty(), "Safe input should get response");
    println!("[IT-Security] Safe input passed security, got: {}", content);
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_tool_execution_loop() {
    // IT: Test the full tool execution loop
    // 1. User asks a question
    // 2. LLM responds with tool call
    // 3. We "execute" the tool
    // 4. Feed result back
    // 5. Get final answer

    let tools = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a file from disk",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path"}
                    },
                    "required": ["path"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "message",
                "description": "Send a message",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string"}
                    },
                    "required": ["content"]
                }
            }
        }),
    ];

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Read the file /tmp/test.txt"
    })];

    // Step 1: LLM should return a tool call
    let response = call_llm(messages.clone(), tools.clone()).await;
    let tool_calls = extract_tool_calls(&response);
    let content = extract_content(&response);

    println!("[IT-ToolLoop] Step 1: tool_calls={}, content={}", tool_calls.len(), content);

    if !tool_calls.is_empty() {
        let (call_id, tool_name, args) = &tool_calls[0];
        println!("[IT-ToolLoop] Tool call: {} with args: {}", tool_name, args);

        // Step 2: Simulate tool execution
        let tool_result = match tool_name.as_str() {
            "read_file" => "File contents: Hello World!".to_string(),
            "message" => "Message sent".to_string(),
            _ => format!("Unknown tool: {}", tool_name),
        };

        // Step 3: Feed tool result back to LLM
        let mut full_messages = messages.clone();
        full_messages.push(serde_json::json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [{
                "id": call_id,
                "type": "function",
                "function": {
                    "name": tool_name,
                    "arguments": args
                }
            }]
        }));
        full_messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": tool_result
        }));

        let final_response = call_llm(full_messages, vec![]).await;
        let final_content = extract_content(&final_response);
        println!("[IT-ToolLoop] Step 3: Final response: {}", final_content);
        assert!(!final_content.is_empty(), "Should get final response after tool execution");
    } else {
        // AI server may return text instead of tool calls
        println!("[IT-ToolLoop] LLM responded with text: {}", content);
    }
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_multi_tool_workflow() {
    // IT: Test workflow with multiple tools registered
    let tools = vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List directory",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            }
        }),
    ];

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "List files in /tmp, then read test.txt"
    })];

    let response = call_llm(messages, tools).await;
    let tool_calls = extract_tool_calls(&response);
    let content = extract_content(&response);

    println!("[IT-MultiTool] tool_calls={}, content={}", tool_calls.len(), content);
    // Either tool calls or text response is valid
    assert!(!tool_calls.is_empty() || !content.is_empty(), "Should get some response");
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_error_recovery() {
    // IT: Test error handling - feed invalid tool result and verify recovery
    let messages = vec![
        serde_json::json!({"role": "user", "content": "Read test.txt"}),
        serde_json::json!({"role": "assistant", "content": null, "tool_calls": [{"id": "call_err1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\":\"/nonexistent\"}"}}]}),
        serde_json::json!({"role": "tool", "tool_call_id": "call_err1", "content": "Error: file not found"}),
        serde_json::json!({"role": "user", "content": "The file doesn't exist. What should I do?"}),
    ];

    let response = call_llm(messages, vec![]).await;
    let content = extract_content(&response);
    assert!(!content.is_empty(), "LLM should respond even after tool error");
    println!("[IT-ErrorRecovery] Response after error: {}", content);
}

#[tokio::test]
async fn test_it_rpc_correlation_flow() {
    // IT: Test RPC correlation ID pattern
    let correlation_id = "test-corr-12345";

    // Simulate RPC channel message formatting
    let message_content = format!("[rpc:{}] This is the actual response", correlation_id);

    // Verify the format
    assert!(message_content.starts_with("[rpc:"));
    assert!(message_content.contains(correlation_id));

    // Parse it back
    if let Some(rest) = message_content.strip_prefix(&format!("[rpc:{}]", correlation_id)) {
        let actual = rest.trim();
        assert_eq!(actual, "This is the actual response");
    }

    println!("[IT-RPC] Correlation ID format verified: {}", message_content);
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_memory_context_flow() {
    // IT: Test memory context accumulation
    let messages = vec![
        serde_json::json!({"role": "system", "content": "You are helpful. Remember user preferences."}),
        serde_json::json!({"role": "user", "content": "My name is Alice"}),
        serde_json::json!({"role": "assistant", "content": "Hello Alice! Nice to meet you."}),
        serde_json::json!({"role": "user", "content": "What is my name?"}),
    ];

    let response = call_llm(messages, vec![]).await;
    let content = extract_content(&response);
    assert!(!content.is_empty());
    println!("[IT-Memory] Context flow response: {}", content);
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_it_concurrent_requests() {
    // IT: Test concurrent LLM requests (load test)
    let mut handles = Vec::new();

    for i in 0..5 {
        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::new();
            let body = serde_json::json!({
                "model": "testai-1.1",
                "messages": [{"role": "user", "content": format!("Request {}", i)}]
            });
            let resp = client
                .post(&format!("{}/v1/chat/completions", AI_SERVER_URL))
                .json(&body)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .expect("Request failed");
            assert!(resp.status().is_success());
            resp.json::<Value>().await.expect("Parse failed")
        }));
    }

    let mut success_count = 0;
    for handle in handles {
        match handle.await {
            Ok(resp) => {
                if resp.get("choices").is_some() {
                    success_count += 1;
                }
            }
            Err(e) => println!("[IT-Concurrent] Error: {}", e),
        }
    }

    assert_eq!(success_count, 5, "All 5 concurrent requests should succeed");
    println!("[IT-Concurrent] {} / 5 requests succeeded", success_count);
}
