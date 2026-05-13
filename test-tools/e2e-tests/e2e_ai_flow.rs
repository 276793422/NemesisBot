//! End-to-end AI flow integration test.
//!
//! Tests the complete AI pipeline: Message → Agent → LLM → Tool → Response
//! using the test AI server as the LLM backend.

use reqwest;
use serde_json::Value;
use std::time::Duration;

const AI_SERVER_URL: &str = "http://127.0.0.1:18080";

/// Helper to send a chat completion request.
async fn chat_request(messages: Vec<Value>, tools: Vec<Value>) -> Value {
    let client = reqwest::Client::new();
    let mut body = serde_json::json!({
        "model": "testai-1.1",
        "messages": messages,
    });
    if !tools.is_empty() {
        body["tools"] = serde_json::json!(tools);
    }

    let resp = client
        .post(&format!("{}/v1/chat/completions", AI_SERVER_URL))
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .expect("Failed to send request");

    resp.json::<Value>().await.expect("Failed to parse response")
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_e2e_ai_server_health() {
    let resp = reqwest::get(&format!("{}/health", AI_SERVER_URL))
        .await
        .expect("Failed to connect to AI server");
    assert!(resp.status().is_success(), "AI server health check failed");
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_e2e_list_models() {
    let resp = reqwest::get(&format!("{}/v1/models", AI_SERVER_URL))
        .await
        .expect("Failed to get models");
    let data: Value = resp.json().await.expect("Failed to parse");
    let models = data["data"].as_array().expect("No models array");
    assert!(!models.is_empty(), "No models available");
    assert_eq!(models[0]["id"].as_str().unwrap(), "testai-1.1");
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_e2e_simple_chat() {
    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Hello, who are you?"
    })];

    let resp = chat_request(messages, vec![]).await;
    assert!(resp.get("choices").is_some(), "No choices in response");

    let content = resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    assert!(!content.is_empty(), "Empty response from AI");
    println!("[E2E] Simple chat response: {}", content);
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_e2e_tool_call_flow() {
    // Simulate the full tool call flow:
    // 1. User sends message
    // 2. LLM responds with tool call
    // 3. We execute the tool
    // 4. Feed tool result back to LLM
    // 5. Get final response

    let tools = vec![serde_json::json!({
        "type": "function",
        "function": {
            "name": "echo",
            "description": "Echo back the input",
            "parameters": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to echo"}
                },
                "required": ["text"]
            }
        }
    })];

    let messages = vec![serde_json::json!({
        "role": "user",
        "content": "Please echo 'hello world'"
    })];

    // Step 1: Initial request with tools
    let resp = chat_request(messages.clone(), tools.clone()).await;
    println!("[E2E] Step 1 - LLM response: {}", serde_json::to_string_pretty(&resp).unwrap());

    // Verify we got a response
    assert!(resp.get("choices").is_some(), "No choices in step 1");
    let choice = &resp["choices"][0];

    // Check if we got a tool call or just a text response
    let tool_calls = choice["message"]["tool_calls"].as_array();
    let content = choice["message"]["content"].as_str().unwrap_or("");

    if let Some(calls) = tool_calls {
        if !calls.is_empty() {
            println!("[E2E] Got {} tool call(s)", calls.len());

            // Step 2: Simulate tool execution
            let tool_call = &calls[0];
            let call_id = tool_call["id"].as_str().unwrap_or("call_0");
            let tool_name = tool_call["function"]["name"].as_str().unwrap_or("");
            let tool_args = tool_call["function"]["arguments"].as_str().unwrap_or("{}");

            println!("[E2E] Tool call: {}({})", tool_name, tool_args);

            // Execute tool locally (echo)
            let tool_result = if tool_name == "echo" {
                let args: Value = serde_json::from_str(tool_args).unwrap_or_default();
                args["text"].as_str().unwrap_or("no text").to_string()
            } else {
                format!("Unknown tool: {}", tool_name)
            };
            println!("[E2E] Tool result: {}", tool_result);

            // Step 3: Feed tool result back
            let mut full_messages = messages.clone();
            full_messages.push(serde_json::json!({
                "role": "assistant",
                "content": null,
                "tool_calls": calls
            }));
            full_messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": tool_result
            }));

            let final_resp = chat_request(full_messages, vec![]).await;
            println!("[E2E] Step 3 - Final response: {}", serde_json::to_string_pretty(&final_resp).unwrap());
            assert!(final_resp.get("choices").is_some(), "No choices in step 3");
        }
    } else {
        println!("[E2E] LLM responded with text only: {}", content);
    }
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_e2e_multi_turn_conversation() {
    // Simulate a multi-turn conversation
    let messages = vec![
        serde_json::json!({"role": "user", "content": "Hello"}),
        serde_json::json!({"role": "assistant", "content": "Hi there!"}),
        serde_json::json!({"role": "user", "content": "What can you do?"}),
    ];

    let resp = chat_request(messages, vec![]).await;
    assert!(resp.get("choices").is_some());
    let content = resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");
    assert!(!content.is_empty());
    println!("[E2E] Multi-turn response: {}", content);
}

#[tokio::test]
#[ignore] // Requires external AI server on port 18080
async fn test_e2e_usage_tracking() {
    let messages = vec![serde_json::json!({"role": "user", "content": "test"})];
    let resp = chat_request(messages, vec![]).await;

    let usage = resp.get("usage").expect("No usage info in response");
    assert!(usage["total_tokens"].as_i64().unwrap_or(0) > 0, "Usage should have tokens");
    println!("[E2E] Usage: prompt={}, completion={}, total={}",
        usage["prompt_tokens"].as_i64().unwrap_or(0),
        usage["completion_tokens"].as_i64().unwrap_or(0),
        usage["total_tokens"].as_i64().unwrap_or(0));
}
