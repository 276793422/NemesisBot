//! Comprehensive tests for cluster, error, forge, provider, security, tools, traits, and workflow modules

use crate::cluster::{NodeInfo, NodeRole, RpcMessage, Task, TaskStatus};
use crate::error::{NemesisError, Result};
use crate::forge::{
    Artifact, ArtifactKind, ArtifactStatus, CycleStatus, Experience, LearningCycle, Reflection,
};
use crate::provider::{
    FunctionCall, LlmChoice, LlmMessage, LlmRequest, LlmResponse, LlmToolCall, LlmUsage,
    ProviderConfig, StreamChoice, StreamChunk, StreamDelta, ToolDef,
};
use crate::security::{AuditEvent, Operation, RiskLevel, SecurityVerdict};
use crate::tools::{ToolContext, ToolDefinition};
use crate::workflow::{Condition, ConditionOperator, NodeType, WorkflowNode, WorkflowTrigger};
use serde_json::{from_value, to_value};

// ============================================================================
// Cluster Tests
// ============================================================================

#[test]
fn test_task_status_serialization() {
    let status = TaskStatus::Running;
    let json = to_value(status).unwrap();
    assert_eq!(json, "Running");

    let deserialized: TaskStatus = from_value(json).unwrap();
    assert_eq!(deserialized, TaskStatus::Running);
}

#[test]
fn test_task_status_equality() {
    assert_eq!(TaskStatus::Pending, TaskStatus::Pending);
    assert_ne!(TaskStatus::Running, TaskStatus::Completed);
}

#[test]
fn test_task_basic() {
    let task = Task {
        id: "task-1".to_string(),
        status: TaskStatus::Pending,
        action: "test-action".to_string(),
        peer_id: "peer-1".to_string(),
        payload: serde_json::json!({"key": "value"}),
        result: None,
        original_channel: "web".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: None,
    };

    assert_eq!(task.id, "task-1");
    assert_eq!(task.status, TaskStatus::Pending);
    assert!(task.result.is_none());
}

#[test]
fn test_task_with_result() {
    let task = Task {
        id: "task-2".to_string(),
        status: TaskStatus::Completed,
        action: "completed-action".to_string(),
        peer_id: "peer-2".to_string(),
        payload: serde_json::json!({}),
        result: Some(serde_json::json!({"result": "success"})),
        original_channel: "rpc".to_string(),
        original_chat_id: "chat-2".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: Some("2024-01-01T00:05:00Z".to_string()),
    };

    assert_eq!(task.status, TaskStatus::Completed);
    assert!(task.result.is_some());
    assert!(task.completed_at.is_some());
}

#[test]
fn test_task_serialization() {
    let task = Task {
        id: "task-1".to_string(),
        status: TaskStatus::Running,
        action: "action".to_string(),
        peer_id: "peer-1".to_string(),
        payload: serde_json::json!({"test": "data"}),
        result: None,
        original_channel: "web".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: None,
    };

    let json = to_value(&task).unwrap();
    let deserialized: Task = from_value(json).unwrap();
    assert_eq!(deserialized.id, task.id);
    assert_eq!(deserialized.status, task.status);
}

#[test]
fn test_node_role_serialization() {
    let role = NodeRole::Master;
    let json = to_value(role).unwrap();
    assert_eq!(json, "Master");

    let deserialized: NodeRole = from_value(json).unwrap();
    assert_eq!(deserialized, NodeRole::Master);
}

#[test]
fn test_node_info_basic() {
    let node = NodeInfo {
        id: "node-1".to_string(),
        name: "test-node".to_string(),
        role: NodeRole::Worker,
        address: "127.0.0.1:8080".to_string(),
        category: "development".to_string(),
        last_seen: "2024-01-01T00:00:00Z".to_string(),
    };

    assert_eq!(node.role, NodeRole::Worker);
    assert_eq!(node.address, "127.0.0.1:8080");
}

#[test]
fn test_node_info_clone() {
    let node = NodeInfo {
        id: "node-1".to_string(),
        name: "test-node".to_string(),
        role: NodeRole::Master,
        address: "127.0.0.1:8080".to_string(),
        category: "production".to_string(),
        last_seen: "2024-01-01T00:00:00Z".to_string(),
    };

    let cloned = node.clone();
    assert_eq!(node.id, cloned.id);
    assert_eq!(node.role, cloned.role);
}

#[test]
fn test_node_info_serialization() {
    let node = NodeInfo {
        id: "node-1".to_string(),
        name: "test-node".to_string(),
        role: NodeRole::Worker,
        address: "127.0.0.1:8080".to_string(),
        category: "development".to_string(),
        last_seen: "2024-01-01T00:00:00Z".to_string(),
    };

    let json = to_value(&node).unwrap();
    let deserialized: NodeInfo = from_value(json).unwrap();
    assert_eq!(deserialized.id, node.id);
    assert_eq!(deserialized.role, node.role);
}

#[test]
fn test_rpc_message_basic() {
    let msg = RpcMessage {
        id: "msg-1".to_string(),
        action: "test-action".to_string(),
        payload: serde_json::json!({"key": "value"}),
        source: "node-1".to_string(),
        target: Some("node-2".to_string()),
        timestamp: "2024-01-01T00:00:00Z".to_string(),
    };

    assert_eq!(msg.action, "test-action");
    assert!(msg.target.is_some());
}

#[test]
fn test_rpc_message_no_target() {
    let msg = RpcMessage {
        id: "msg-2".to_string(),
        action: "broadcast".to_string(),
        payload: serde_json::json!({}),
        source: "node-1".to_string(),
        target: None,
        timestamp: "2024-01-01T00:00:00Z".to_string(),
    };

    assert!(msg.target.is_none());
}

#[test]
fn test_rpc_message_serialization() {
    let msg = RpcMessage {
        id: "msg-1".to_string(),
        action: "action".to_string(),
        payload: serde_json::json!({"test": "data"}),
        source: "source-node".to_string(),
        target: Some("target-node".to_string()),
        timestamp: "2024-01-01T00:00:00Z".to_string(),
    };

    let json = to_value(&msg).unwrap();
    let deserialized: RpcMessage = from_value(json).unwrap();
    assert_eq!(deserialized.id, msg.id);
    assert_eq!(deserialized.action, msg.action);
}

// ============================================================================
// Error Tests
// ============================================================================

#[test]
fn test_nemesis_error_config() {
    let err = NemesisError::Config("config error".to_string());
    assert_eq!(err.to_string(), "Configuration error: config error");
}

#[test]
fn test_nemesis_error_security() {
    let err = NemesisError::Security("security violation".to_string());
    assert_eq!(err.to_string(), "Security violation: security violation");
}

#[test]
fn test_nemesis_error_provider() {
    let err = NemesisError::Provider("provider failed".to_string());
    assert_eq!(err.to_string(), "Provider error: provider failed");
}

#[test]
fn test_nemesis_error_channel() {
    let err = NemesisError::Channel("channel error".to_string());
    assert_eq!(err.to_string(), "Channel error: channel error");
}

#[test]
fn test_nemesis_error_agent() {
    let err = NemesisError::Agent("agent error".to_string());
    assert_eq!(err.to_string(), "Agent error: agent error");
}

#[test]
fn test_nemesis_error_cluster() {
    let err = NemesisError::Cluster("cluster error".to_string());
    assert_eq!(err.to_string(), "Cluster error: cluster error");
}

#[test]
fn test_nemesis_error_memory() {
    let err = NemesisError::Memory("memory error".to_string());
    assert_eq!(err.to_string(), "Memory error: memory error");
}

#[test]
fn test_nemesis_error_tool() {
    let err = NemesisError::Tool("tool error".to_string());
    assert_eq!(err.to_string(), "Tool error: tool error");
}

#[test]
fn test_nemesis_error_workflow() {
    let err = NemesisError::Workflow("workflow error".to_string());
    assert_eq!(err.to_string(), "Workflow error: workflow error");
}

#[test]
fn test_nemesis_error_forge() {
    let err = NemesisError::Forge("forge error".to_string());
    assert_eq!(err.to_string(), "Forge error: forge error");
}

#[test]
fn test_nemesis_error_not_found() {
    let err = NemesisError::NotFound("resource not found".to_string());
    assert_eq!(err.to_string(), "Not found: resource not found");
}

#[test]
fn test_nemesis_error_timeout() {
    let err = NemesisError::Timeout("operation timed out".to_string());
    assert_eq!(err.to_string(), "Timeout: operation timed out");
}

#[test]
fn test_nemesis_error_unauthorized() {
    let err = NemesisError::Unauthorized("unauthorized access".to_string());
    assert_eq!(err.to_string(), "Unauthorized: unauthorized access");
}

#[test]
fn test_nemesis_error_validation() {
    let err = NemesisError::Validation("validation failed".to_string());
    assert_eq!(err.to_string(), "Validation error: validation failed");
}

#[test]
fn test_nemesis_error_other() {
    let err = NemesisError::Other("other error".to_string());
    assert_eq!(err.to_string(), "other error");
}

#[test]
fn test_result_type() {
    let ok_result: Result<String> = Ok("success".to_string());
    assert!(ok_result.is_ok());

    let err_result: Result<String> = Err(NemesisError::Config("error".to_string()));
    assert!(err_result.is_err());
}

// ============================================================================
// Forge Tests
// ============================================================================

#[test]
fn test_artifact_kind_serialization() {
    let kind = ArtifactKind::Skill;
    let json = to_value(kind).unwrap();
    assert_eq!(json, "Skill");

    let deserialized: ArtifactKind = from_value(json).unwrap();
    assert_eq!(deserialized, ArtifactKind::Skill);
}

#[test]
fn test_artifact_status_serialization() {
    let status = ArtifactStatus::Active;
    let json = to_value(status).unwrap();
    assert_eq!(json, "Active");

    let deserialized: ArtifactStatus = from_value(json).unwrap();
    assert_eq!(deserialized, ArtifactStatus::Active);
}

#[test]
fn test_artifact_basic() {
    let artifact = Artifact {
        id: "artifact-1".to_string(),
        name: "test-artifact".to_string(),
        kind: ArtifactKind::Skill,
        version: "1.0.0".to_string(),
        status: ArtifactStatus::Active,
        content: "artifact content".to_string(),
        tool_signature: vec!["tool1".to_string(), "tool2".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        usage_count: 100,
        success_rate: 0.95,
        last_degraded_at: None,
        consecutive_observing_rounds: 0,
    };

    assert_eq!(artifact.kind, ArtifactKind::Skill);
    assert_eq!(artifact.usage_count, 100);
    assert_eq!(artifact.success_rate, 0.95);
}

#[test]
fn test_artifact_with_degradation() {
    let artifact = Artifact {
        id: "artifact-2".to_string(),
        name: "degraded-artifact".to_string(),
        kind: ArtifactKind::Mcp,
        version: "1.0.0".to_string(),
        status: ArtifactStatus::Degraded,
        content: "content".to_string(),
        tool_signature: vec![],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        usage_count: 50,
        success_rate: 0.7,
        last_degraded_at: Some("2024-01-01T00:00:00Z".to_string()),
        consecutive_observing_rounds: 3,
    };

    assert_eq!(artifact.status, ArtifactStatus::Degraded);
    assert!(artifact.last_degraded_at.is_some());
    assert_eq!(artifact.consecutive_observing_rounds, 3);
}

#[test]
fn test_artifact_serialization() {
    let artifact = Artifact {
        id: "artifact-1".to_string(),
        name: "test-artifact".to_string(),
        kind: ArtifactKind::Script,
        version: "1.0.0".to_string(),
        status: ArtifactStatus::Active,
        content: "content".to_string(),
        tool_signature: vec!["tool1".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        usage_count: 10,
        success_rate: 0.8,
        last_degraded_at: None,
        consecutive_observing_rounds: 0,
    };

    let json = to_value(&artifact).unwrap();
    let deserialized: Artifact = from_value(json).unwrap();
    assert_eq!(deserialized.id, artifact.id);
    assert_eq!(deserialized.kind, artifact.kind);
    assert_eq!(deserialized.success_rate, artifact.success_rate);
}

#[test]
fn test_experience_default() {
    let exp = Experience::default();
    assert_eq!(exp.id, "");
    assert_eq!(exp.success, false);
    assert_eq!(exp.duration_ms, 0);
}

#[test]
fn test_experience_basic() {
    let exp = Experience {
        id: "exp-1".to_string(),
        tool_name: "test-tool".to_string(),
        input_summary: "input".to_string(),
        output_summary: "output".to_string(),
        success: true,
        duration_ms: 1000,
        timestamp: "2024-01-01T00:00:00Z".to_string(),
        session_key: "session-key".to_string(),
    };

    assert_eq!(exp.tool_name, "test-tool");
    assert_eq!(exp.success, true);
    assert_eq!(exp.duration_ms, 1000);
}

#[test]
fn test_experience_serialization() {
    let exp = Experience {
        id: "exp-1".to_string(),
        tool_name: "tool".to_string(),
        input_summary: "input".to_string(),
        output_summary: "output".to_string(),
        success: true,
        duration_ms: 500,
        timestamp: "2024-01-01T00:00:00Z".to_string(),
        session_key: "session".to_string(),
    };

    let json = to_value(&exp).unwrap();
    let deserialized: Experience = from_value(json).unwrap();
    assert_eq!(deserialized.id, exp.id);
    assert_eq!(deserialized.success, exp.success);
}

#[test]
fn test_reflection_basic() {
    let reflection = Reflection {
        id: "ref-1".to_string(),
        period_start: "2024-01-01T00:00:00Z".to_string(),
        period_end: "2024-01-02T00:00:00Z".to_string(),
        insights: vec!["insight1".to_string(), "insight2".to_string()],
        recommendations: vec!["recommendation1".to_string()],
        statistics: serde_json::json!({"total": 100}),
        is_remote: false,
    };

    assert_eq!(reflection.insights.len(), 2);
    assert_eq!(reflection.is_remote, false);
}

#[test]
fn test_reflection_serialization() {
    let reflection = Reflection {
        id: "ref-1".to_string(),
        period_start: "2024-01-01T00:00:00Z".to_string(),
        period_end: "2024-01-02T00:00:00Z".to_string(),
        insights: vec!["insight".to_string()],
        recommendations: vec![],
        statistics: serde_json::json!({}),
        is_remote: true,
    };

    let json = to_value(&reflection).unwrap();
    let deserialized: Reflection = from_value(json).unwrap();
    assert_eq!(deserialized.id, reflection.id);
    assert_eq!(deserialized.is_remote, reflection.is_remote);
}

#[test]
fn test_cycle_status_serialization() {
    let status = CycleStatus::Running;
    let json = to_value(status).unwrap();
    assert_eq!(json, "Running");

    let deserialized: CycleStatus = from_value(json).unwrap();
    assert_eq!(deserialized, CycleStatus::Running);
}

#[test]
fn test_learning_cycle_basic() {
    let cycle = LearningCycle {
        id: "cycle-1".to_string(),
        started_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: Some("2024-01-01T01:00:00Z".to_string()),
        patterns_found: 5,
        actions_taken: 3,
        status: CycleStatus::Completed,
    };

    assert_eq!(cycle.patterns_found, 5);
    assert_eq!(cycle.status, CycleStatus::Completed);
}

#[test]
fn test_learning_cycle_serialization() {
    let cycle = LearningCycle {
        id: "cycle-1".to_string(),
        started_at: "2024-01-01T00:00:00Z".to_string(),
        completed_at: None,
        patterns_found: 10,
        actions_taken: 5,
        status: CycleStatus::Running,
    };

    let json = to_value(&cycle).unwrap();
    let deserialized: LearningCycle = from_value(json).unwrap();
    assert_eq!(deserialized.id, cycle.id);
    assert_eq!(deserialized.status, cycle.status);
}

// ============================================================================
// Provider Tests
// ============================================================================

#[test]
fn test_llm_request_basic() {
    let request = LlmRequest {
        model: "gpt-4".to_string(),
        messages: vec![],
        tools: None,
        temperature: Some(0.7),
        max_tokens: Some(1000),
        stream: false,
    };

    assert_eq!(request.model, "gpt-4");
    assert_eq!(request.stream, false);
}

#[test]
fn test_llm_request_serialization() {
    let request = LlmRequest {
        model: "gpt-3.5-turbo".to_string(),
        messages: vec![LlmMessage {
            role: "user".to_string(),
            content: Some("hello".to_string()),
            tool_calls: None,
            tool_call_id: None,
        }],
        tools: Some(vec![]),
        temperature: Some(0.5),
        max_tokens: None,
        stream: true,
    };

    let json = to_value(&request).unwrap();
    let deserialized: LlmRequest = from_value(json).unwrap();
    assert_eq!(deserialized.model, request.model);
    assert_eq!(deserialized.stream, request.stream);
}

#[test]
fn test_llm_message_basic() {
    let msg = LlmMessage {
        role: "user".to_string(),
        content: Some("hello".to_string()),
        tool_calls: None,
        tool_call_id: None,
    };

    assert_eq!(msg.role, "user");
    assert!(msg.content.is_some());
}

#[test]
fn test_llm_message_with_tool_calls() {
    let msg = LlmMessage {
        role: "assistant".to_string(),
        content: None,
        tool_calls: Some(vec![LlmToolCall {
            id: "call-1".to_string(),
            r#type: "function".to_string(),
            function: FunctionCall {
                name: "test_tool".to_string(),
                arguments: "{}".to_string(),
            },
        }]),
        tool_call_id: None,
    };

    assert!(msg.tool_calls.is_some());
    assert_eq!(msg.tool_calls.unwrap().len(), 1);
}

#[test]
fn test_llm_message_serialization() {
    let msg = LlmMessage {
        role: "system".to_string(),
        content: Some("system message".to_string()),
        tool_calls: None,
        tool_call_id: None,
    };

    let json = to_value(&msg).unwrap();
    let deserialized: LlmMessage = from_value(json).unwrap();
    assert_eq!(deserialized.role, msg.role);
    assert_eq!(deserialized.content, msg.content);
}

#[test]
fn test_function_call_basic() {
    let func = FunctionCall {
        name: "test_function".to_string(),
        arguments: "{\"arg1\": \"value1\"}".to_string(),
    };

    assert_eq!(func.name, "test_function");
}

#[test]
fn test_function_call_serialization() {
    let func = FunctionCall {
        name: "function".to_string(),
        arguments: "{\"key\":\"value\"}".to_string(),
    };

    let json = to_value(&func).unwrap();
    let deserialized: FunctionCall = from_value(json).unwrap();
    assert_eq!(deserialized.name, func.name);
    assert_eq!(deserialized.arguments, func.arguments);
}

#[test]
fn test_llm_tool_call_basic() {
    let call = LlmToolCall {
        id: "call-1".to_string(),
        r#type: "function".to_string(),
        function: FunctionCall {
            name: "tool".to_string(),
            arguments: "{}".to_string(),
        },
    };

    assert_eq!(call.id, "call-1");
    assert_eq!(call.r#type, "function");
}

#[test]
fn test_llm_tool_call_serialization() {
    let call = LlmToolCall {
        id: "call-1".to_string(),
        r#type: "function".to_string(),
        function: FunctionCall {
            name: "tool".to_string(),
            arguments: "{}".to_string(),
        },
    };

    let json = to_value(&call).unwrap();
    let deserialized: LlmToolCall = from_value(json).unwrap();
    assert_eq!(deserialized.id, call.id);
}

#[test]
fn test_tool_def_basic() {
    let tool_def = ToolDef {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: serde_json::json!({"type": "object"}),
    };

    assert_eq!(tool_def.name, "test_tool");
}

#[test]
fn test_tool_def_serialization() {
    let tool_def = ToolDef {
        name: "tool".to_string(),
        description: "description".to_string(),
        parameters: serde_json::json!({"properties": {}}),
    };

    let json = to_value(&tool_def).unwrap();
    let deserialized: ToolDef = from_value(json).unwrap();
    assert_eq!(deserialized.name, tool_def.name);
}

#[test]
fn test_llm_response_basic() {
    let response = LlmResponse {
        id: "resp-1".to_string(),
        model: "gpt-4".to_string(),
        choices: vec![],
        usage: None,
    };

    assert_eq!(response.id, "resp-1");
    assert!(response.choices.is_empty());
}

#[test]
fn test_llm_response_with_usage() {
    let response = LlmResponse {
        id: "resp-1".to_string(),
        model: "gpt-4".to_string(),
        choices: vec![LlmChoice {
            index: 0,
            message: LlmMessage {
                role: "assistant".to_string(),
                content: Some("response".to_string()),
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: Some(LlmUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        }),
    };

    assert_eq!(response.choices.len(), 1);
    assert!(response.usage.is_some());
}

#[test]
fn test_llm_response_serialization() {
    let response = LlmResponse {
        id: "resp-1".to_string(),
        model: "gpt-4".to_string(),
        choices: vec![],
        usage: Some(LlmUsage {
            prompt_tokens: 10,
            completion_tokens: 20,
            total_tokens: 30,
        }),
    };

    let json = to_value(&response).unwrap();
    let deserialized: LlmResponse = from_value(json).unwrap();
    assert_eq!(deserialized.id, response.id);
}

#[test]
fn test_llm_choice_basic() {
    let choice = LlmChoice {
        index: 0,
        message: LlmMessage {
            role: "assistant".to_string(),
            content: Some("content".to_string()),
            tool_calls: None,
            tool_call_id: None,
        },
        finish_reason: Some("stop".to_string()),
    };

    assert_eq!(choice.index, 0);
}

#[test]
fn test_llm_choice_serialization() {
    let choice = LlmChoice {
        index: 1,
        message: LlmMessage {
            role: "assistant".to_string(),
            content: None,
            tool_calls: None,
            tool_call_id: None,
        },
        finish_reason: None,
    };

    let json = to_value(&choice).unwrap();
    let deserialized: LlmChoice = from_value(json).unwrap();
    assert_eq!(deserialized.index, choice.index);
}

#[test]
fn test_llm_usage_basic() {
    let usage = LlmUsage {
        prompt_tokens: 10,
        completion_tokens: 20,
        total_tokens: 30,
    };

    assert_eq!(usage.total_tokens, 30);
}

#[test]
fn test_llm_usage_serialization() {
    let usage = LlmUsage {
        prompt_tokens: 100,
        completion_tokens: 200,
        total_tokens: 300,
    };

    let json = to_value(&usage).unwrap();
    let deserialized: LlmUsage = from_value(json).unwrap();
    assert_eq!(deserialized.total_tokens, usage.total_tokens);
}

#[test]
fn test_stream_chunk_basic() {
    let chunk = StreamChunk {
        id: "chunk-1".to_string(),
        choices: vec![],
    };

    assert_eq!(chunk.id, "chunk-1");
}

#[test]
fn test_stream_chunk_serialization() {
    let chunk = StreamChunk {
        id: "chunk-1".to_string(),
        choices: vec![StreamChoice {
            index: 0,
            delta: StreamDelta {
                content: Some("content".to_string()),
                tool_calls: None,
            },
            finish_reason: None,
        }],
    };

    let json = to_value(&chunk).unwrap();
    let deserialized: StreamChunk = from_value(json).unwrap();
    assert_eq!(deserialized.id, chunk.id);
}

#[test]
fn test_stream_choice_basic() {
    let choice = StreamChoice {
        index: 0,
        delta: StreamDelta {
            content: Some("test".to_string()),
            tool_calls: None,
        },
        finish_reason: None,
    };

    assert_eq!(choice.index, 0);
}

#[test]
fn test_stream_choice_serialization() {
    let choice = StreamChoice {
        index: 1,
        delta: StreamDelta {
            content: None,
            tool_calls: None,
        },
        finish_reason: Some("stop".to_string()),
    };

    let json = to_value(&choice).unwrap();
    let deserialized: StreamChoice = from_value(json).unwrap();
    assert_eq!(deserialized.index, choice.index);
}

#[test]
fn test_stream_delta_basic() {
    let delta = StreamDelta {
        content: Some("content".to_string()),
        tool_calls: None,
    };

    assert!(delta.content.is_some());
}

#[test]
fn test_stream_delta_serialization() {
    let delta = StreamDelta {
        content: Some("content".to_string()),
        tool_calls: None,
    };

    let json = to_value(&delta).unwrap();
    let deserialized: StreamDelta = from_value(json).unwrap();
    assert_eq!(deserialized.content, delta.content);
}

#[test]
fn test_provider_config_basic() {
    let config = ProviderConfig {
        provider: "openai".to_string(),
        model: "gpt-4".to_string(),
        api_key: Some("key".to_string()),
        base_url: Some("https://api.openai.com".to_string()),
        is_default: true,
    };

    assert_eq!(config.provider, "openai");
    assert!(config.is_default);
}

#[test]
fn test_provider_config_serialization() {
    let config = ProviderConfig {
        provider: "anthropic".to_string(),
        model: "claude-3".to_string(),
        api_key: Some("key".to_string()),
        base_url: None,
        is_default: false,
    };

    let json = to_value(&config).unwrap();
    let deserialized: ProviderConfig = from_value(json).unwrap();
    assert_eq!(deserialized.provider, config.provider);
    assert_eq!(deserialized.is_default, config.is_default);
}

// ============================================================================
// Security Tests
// ============================================================================

#[test]
fn test_risk_level_ordering() {
    assert!(RiskLevel::Low < RiskLevel::Medium);
    assert!(RiskLevel::Medium < RiskLevel::High);
    assert!(RiskLevel::High < RiskLevel::Critical);
    assert!(RiskLevel::Low <= RiskLevel::Low);
}

#[test]
fn test_risk_level_equality() {
    assert_eq!(RiskLevel::High, RiskLevel::High);
    assert_ne!(RiskLevel::Low, RiskLevel::Critical);
}

#[test]
fn test_risk_level_serialization() {
    let level = RiskLevel::High;
    let json = to_value(level).unwrap();
    assert_eq!(json, "High");

    let deserialized: RiskLevel = from_value(json).unwrap();
    assert_eq!(deserialized, RiskLevel::High);
}

#[test]
fn test_security_verdict_allowed() {
    let verdict = SecurityVerdict {
        allowed: true,
        risk_level: RiskLevel::Low,
        reason: Some("safe operation".to_string()),
        blocked_by: None,
    };

    assert!(verdict.allowed);
    assert_eq!(verdict.risk_level, RiskLevel::Low);
}

#[test]
fn test_security_verdict_blocked() {
    let verdict = SecurityVerdict {
        allowed: false,
        risk_level: RiskLevel::Critical,
        reason: Some("dangerous operation".to_string()),
        blocked_by: Some("security-middleware".to_string()),
    };

    assert!(!verdict.allowed);
    assert!(verdict.blocked_by.is_some());
}

#[test]
fn test_security_verdict_serialization() {
    let verdict = SecurityVerdict {
        allowed: true,
        risk_level: RiskLevel::Medium,
        reason: None,
        blocked_by: None,
    };

    let json = to_value(&verdict).unwrap();
    let deserialized: SecurityVerdict = from_value(json).unwrap();
    assert_eq!(deserialized.allowed, verdict.allowed);
    assert_eq!(deserialized.risk_level, verdict.risk_level);
}

#[test]
fn test_operation_basic() {
    let operation = Operation {
        action: "file_read".to_string(),
        target: Some("/path/to/file".to_string()),
        parameters: serde_json::json!({}),
        channel: "web".to_string(),
        sender_id: "user-1".to_string(),
    };

    assert_eq!(operation.action, "file_read");
    assert!(operation.target.is_some());
}

#[test]
fn test_operation_no_target() {
    let operation = Operation {
        action: "system_info".to_string(),
        target: None,
        parameters: serde_json::json!({}),
        channel: "cli".to_string(),
        sender_id: "user-1".to_string(),
    };

    assert!(operation.target.is_none());
}

#[test]
fn test_operation_serialization() {
    let operation = Operation {
        action: "file_write".to_string(),
        target: Some("/path".to_string()),
        parameters: serde_json::json!({"content": "data"}),
        channel: "web".to_string(),
        sender_id: "user-1".to_string(),
    };

    let json = to_value(&operation).unwrap();
    let deserialized: Operation = from_value(json).unwrap();
    assert_eq!(deserialized.action, operation.action);
    assert_eq!(deserialized.channel, operation.channel);
}

#[test]
fn test_audit_event_basic() {
    let operation = Operation {
        action: "test".to_string(),
        target: None,
        parameters: serde_json::json!({}),
        channel: "web".to_string(),
        sender_id: "user-1".to_string(),
    };

    let verdict = SecurityVerdict {
        allowed: true,
        risk_level: RiskLevel::Low,
        reason: None,
        blocked_by: None,
    };

    let audit = AuditEvent {
        id: "audit-1".to_string(),
        timestamp: "2024-01-01T00:00:00Z".to_string(),
        operation,
        verdict,
        hash: "hash-1".to_string(),
        prev_hash: "prev-hash".to_string(),
    };

    assert_eq!(audit.id, "audit-1");
    assert_eq!(audit.hash, "hash-1");
}

#[test]
fn test_audit_event_serialization() {
    let operation = Operation {
        action: "action".to_string(),
        target: None,
        parameters: serde_json::json!({}),
        channel: "web".to_string(),
        sender_id: "user-1".to_string(),
    };

    let verdict = SecurityVerdict {
        allowed: false,
        risk_level: RiskLevel::High,
        reason: Some("blocked".to_string()),
        blocked_by: Some("middleware".to_string()),
    };

    let audit = AuditEvent {
        id: "audit-1".to_string(),
        timestamp: "2024-01-01T00:00:00Z".to_string(),
        operation,
        verdict,
        hash: "hash".to_string(),
        prev_hash: "prev".to_string(),
    };

    let json = to_value(&audit).unwrap();
    let deserialized: AuditEvent = from_value(json).unwrap();
    assert_eq!(deserialized.id, audit.id);
    assert_eq!(deserialized.hash, audit.hash);
}

// ============================================================================
// Tools Tests
// ============================================================================

#[test]
fn test_tool_definition_basic() {
    let tool_def = ToolDefinition {
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        parameters: serde_json::json!({"type": "object"}),
        required: vec!["param1".to_string()],
    };

    assert_eq!(tool_def.name, "test_tool");
    assert_eq!(tool_def.required.len(), 1);
}

#[test]
fn test_tool_definition_serialization() {
    let tool_def = ToolDefinition {
        name: "tool".to_string(),
        description: "description".to_string(),
        parameters: serde_json::json!({"properties": {}}),
        required: vec![],
    };

    let json = to_value(&tool_def).unwrap();
    let deserialized: ToolDefinition = from_value(json).unwrap();
    assert_eq!(deserialized.name, tool_def.name);
}

#[test]
fn test_tool_context_basic() {
    let context = ToolContext {
        channel: "web".to_string(),
        chat_id: "chat-1".to_string(),
        sender_id: "user-1".to_string(),
        session_key: "session-1".to_string(),
        correlation_id: Some("corr-1".to_string()),
    };

    assert_eq!(context.channel, "web");
    assert!(context.correlation_id.is_some());
}

#[test]
fn test_tool_context_no_correlation() {
    let context = ToolContext {
        channel: "cli".to_string(),
        chat_id: "chat-1".to_string(),
        sender_id: "user-1".to_string(),
        session_key: "session-1".to_string(),
        correlation_id: None,
    };

    assert!(context.correlation_id.is_none());
}

#[test]
fn test_tool_context_serialization() {
    let context = ToolContext {
        channel: "web".to_string(),
        chat_id: "chat-1".to_string(),
        sender_id: "user-1".to_string(),
        session_key: "session-1".to_string(),
        correlation_id: Some("corr-1".to_string()),
    };

    let json = to_value(&context).unwrap();
    let deserialized: ToolContext = from_value(json).unwrap();
    assert_eq!(deserialized.channel, context.channel);
    assert_eq!(deserialized.chat_id, context.chat_id);
}

// ============================================================================
// Workflow Tests
// ============================================================================

#[test]
fn test_node_type_serialization() {
    let node_type = NodeType::Llm;
    let json = to_value(node_type).unwrap();
    assert_eq!(json["type"], "Llm");

    let deserialized: NodeType = from_value(json).unwrap();
    assert_eq!(deserialized, NodeType::Llm);
}

#[test]
fn test_all_node_types() {
    let types = vec![
        NodeType::Llm,
        NodeType::Tool,
        NodeType::Condition,
        NodeType::Parallel,
        NodeType::Loop,
        NodeType::SubWorkflow,
        NodeType::Transform,
        NodeType::Http,
        NodeType::Script,
        NodeType::Delay,
        NodeType::HumanReview,
    ];

    for node_type in &types {
        let json = to_value(node_type).unwrap();
        let deserialized: NodeType = from_value(json).unwrap();
        assert_eq!(node_type, &deserialized);
    }
}

#[test]
fn test_workflow_node_basic() {
    let node = WorkflowNode {
        id: "node-1".to_string(),
        name: "test-node".to_string(),
        node_type: NodeType::Llm,
        config: serde_json::json!({"model": "gpt-4"}),
        next: vec!["node-2".to_string()],
        error_handler: Some("error-node".to_string()),
    };

    assert_eq!(node.node_type, NodeType::Llm);
    assert_eq!(node.next.len(), 1);
    assert!(node.error_handler.is_some());
}

#[test]
fn test_workflow_node_no_error_handler() {
    let node = WorkflowNode {
        id: "node-1".to_string(),
        name: "test-node".to_string(),
        node_type: NodeType::Tool,
        config: serde_json::json!({}),
        next: vec![],
        error_handler: None,
    };

    assert!(node.next.is_empty());
    assert!(node.error_handler.is_none());
}

#[test]
fn test_workflow_node_serialization() {
    let node = WorkflowNode {
        id: "node-1".to_string(),
        name: "node".to_string(),
        node_type: NodeType::Condition,
        config: serde_json::json!({"condition": "x > 0"}),
        next: vec!["next-1".to_string()],
        error_handler: None,
    };

    let json = to_value(&node).unwrap();
    let deserialized: WorkflowNode = from_value(json).unwrap();
    assert_eq!(deserialized.id, node.id);
    assert_eq!(deserialized.node_type, node.node_type);
}

#[test]
fn test_workflow_trigger_cron() {
    let trigger = WorkflowTrigger::Cron {
        expression: "0 0 * * *".to_string(),
    };

    if let WorkflowTrigger::Cron { expression } = trigger {
        assert_eq!(expression, "0 0 * * *");
    } else {
        panic!("Expected Cron trigger");
    }
}

#[test]
fn test_workflow_trigger_event() {
    let trigger = WorkflowTrigger::Event {
        event_type: "user.message".to_string(),
    };

    if let WorkflowTrigger::Event { event_type } = trigger {
        assert_eq!(event_type, "user.message");
    } else {
        panic!("Expected Event trigger");
    }
}

#[test]
fn test_workflow_trigger_webhook() {
    let trigger = WorkflowTrigger::Webhook {
        path: "/webhook".to_string(),
    };

    if let WorkflowTrigger::Webhook { path } = trigger {
        assert_eq!(path, "/webhook");
    } else {
        panic!("Expected Webhook trigger");
    }
}

#[test]
fn test_workflow_trigger_manual() {
    let trigger = WorkflowTrigger::Manual;
    if let WorkflowTrigger::Manual = trigger {
        // Manual is a unit variant
    } else {
        panic!("Expected Manual trigger");
    }
}

#[test]
fn test_workflow_trigger_serialization() {
    let trigger = WorkflowTrigger::Cron {
        expression: "0 * * * *".to_string(),
    };

    let json = to_value(&trigger).unwrap();
    let deserialized: WorkflowTrigger = from_value(json).unwrap();

    if let WorkflowTrigger::Cron { expression } = deserialized {
        assert_eq!(expression, "0 * * * *");
    } else {
        panic!("Expected Cron trigger");
    }
}

#[test]
fn test_condition_operator_serialization() {
    let op = ConditionOperator::Eq;
    let json = to_value(op).unwrap();
    assert_eq!(json, "Eq");

    let deserialized: ConditionOperator = from_value(json).unwrap();
    assert_eq!(deserialized, ConditionOperator::Eq);
}

#[test]
fn test_all_condition_operators() {
    let operators = vec![
        ConditionOperator::Eq,
        ConditionOperator::Ne,
        ConditionOperator::Gt,
        ConditionOperator::Lt,
        ConditionOperator::Contains,
        ConditionOperator::Matches,
    ];

    for op in &operators {
        let json = to_value(op).unwrap();
        let deserialized: ConditionOperator = from_value(json).unwrap();
        assert_eq!(op, &deserialized);
    }
}

#[test]
fn test_condition_basic() {
    let condition = Condition {
        field: "status".to_string(),
        operator: ConditionOperator::Eq,
        value: serde_json::json!("success"),
    };

    assert_eq!(condition.field, "status");
    assert_eq!(condition.operator, ConditionOperator::Eq);
}

#[test]
fn test_condition_serialization() {
    let condition = Condition {
        field: "count".to_string(),
        operator: ConditionOperator::Gt,
        value: serde_json::json!(10),
    };

    let json = to_value(&condition).unwrap();
    let deserialized: Condition = from_value(json).unwrap();
    assert_eq!(deserialized.field, condition.field);
    assert_eq!(deserialized.operator, condition.operator);
}

#[test]
fn test_condition_complex_operators() {
    // Test And operator
    let and_op = ConditionOperator::And(vec![
        Condition {
            field: "x".to_string(),
            operator: ConditionOperator::Gt,
            value: serde_json::json!(0),
        },
        Condition {
            field: "y".to_string(),
            operator: ConditionOperator::Lt,
            value: serde_json::json!(100),
        },
    ]);

    let json = to_value(&and_op).unwrap();
    let deserialized: ConditionOperator = from_value(json).unwrap();
    if let ConditionOperator::And(conditions) = deserialized {
        assert_eq!(conditions.len(), 2);
    } else {
        panic!("Expected And operator");
    }

    // Test Or operator
    let or_op = ConditionOperator::Or(vec![]);
    let json = to_value(&or_op).unwrap();
    let deserialized: ConditionOperator = from_value(json).unwrap();
    if let ConditionOperator::Or(conditions) = deserialized {
        assert!(conditions.is_empty());
    } else {
        panic!("Expected Or operator");
    }

    // Test Not operator
    let not_op = ConditionOperator::Not(Box::new(Condition {
        field: "active".to_string(),
        operator: ConditionOperator::Eq,
        value: serde_json::json!(false),
    }));

    let json = to_value(&not_op).unwrap();
    let deserialized: ConditionOperator = from_value(json).unwrap();
    if let ConditionOperator::Not(_) = deserialized {
        // Successfully deserialized Not operator
    } else {
        panic!("Expected Not operator");
    }
}
