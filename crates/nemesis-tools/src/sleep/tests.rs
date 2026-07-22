use super::*;

#[tokio::test]
async fn test_sleep_success() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({"duration": 1})).await;
    assert!(!result.is_error);
    assert!(result.silent);
    assert!(result.for_llm.contains("Slept for 1 seconds"));
}

#[tokio::test]
async fn test_sleep_zero_rejected() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({"duration": 0})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("at least 1"));
}

#[tokio::test]
async fn test_sleep_too_large() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({"duration": 5000})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("3600"));
}

#[tokio::test]
async fn test_sleep_missing_duration() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("must be an integer"));
}

#[tokio::test]
async fn test_sleep_non_integer() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({"duration": "abc"})).await;
    assert!(result.is_error);
}

#[test]
fn test_sleep_tool_metadata() {
    let tool = SleepTool::new();
    assert_eq!(tool.name(), "sleep");
    assert!(!tool.description().is_empty());
}

// ============================================================
// Additional sleep tool tests
// ============================================================

#[tokio::test]
async fn test_sleep_exactly_one_second() {
    let tool = SleepTool::new();
    let start = std::time::Instant::now();
    let result = tool.execute(&serde_json::json!({"duration": 1})).await;
    let elapsed = start.elapsed();
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
    assert!(result.silent, "Sleep result should be silent");
    assert!(elapsed >= std::time::Duration::from_millis(900));
}

#[tokio::test]
async fn test_sleep_tool_parameters() {
    let tool = SleepTool::new();
    let params = tool.parameters();
    assert_eq!(params["type"], "object");
    assert!(params["properties"]["duration"].is_object());

    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|r| r.as_str() == Some("duration")));
}

#[tokio::test]
async fn test_sleep_boundary_values() {
    let tool = SleepTool::new();
    // 3601 should fail (exceeds max)
    let result = tool.execute(&serde_json::json!({"duration": 3601})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("3600"));
}

#[tokio::test]
async fn test_sleep_float_duration() {
    let tool = SleepTool::new();
    let _result = tool.execute(&serde_json::json!({"duration": 1.5})).await;
    // Floats may or may not be accepted depending on implementation
    // Just verify it doesn't panic
}

#[tokio::test]
async fn test_sleep_negative_duration() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({"duration": -10})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_sleep_cancellation() {
    let tool = SleepTool::new();
    // Use tokio::time::timeout to simulate cancellation
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        tool.execute(&serde_json::json!({"duration": 30})),
    )
    .await;

    // Should timeout (sleep 30s is way longer than 50ms)
    assert!(result.is_err(), "Expected timeout");
}

#[tokio::test]
async fn test_sleep_success_result_content() {
    let tool = SleepTool::new();
    let result = tool.execute(&serde_json::json!({"duration": 1})).await;
    assert!(!result.is_error);
    assert!(result.silent);
    assert!(
        result.for_llm.contains("Slept")
            || result.for_llm.contains("slept")
            || !result.for_llm.is_empty()
    );
}

#[test]
fn test_sleep_tool_new() {
    let tool = SleepTool::new();
    assert_eq!(tool.name(), "sleep");
}
