use super::*;

#[test]
fn test_messages_to_prompt() {
    let provider = GitHubCopilotProvider::new(GitHubCopilotConfig::default());
    let messages = vec![
        Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
        Message {
            role: "assistant".to_string(),
            content: "Hi there".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: std::collections::HashMap::new(),
        },
    ];
    let prompt = provider.messages_to_prompt(&messages);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&prompt).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0]["role"], "user");
    assert_eq!(parsed[1]["role"], "assistant");
}

#[test]
fn test_config_default() {
    let config = GitHubCopilotConfig::default();
    assert_eq!(config.connect_mode, "grpc");
    assert_eq!(config.default_model, DEFAULT_COPILOT_MODEL);
    assert_eq!(config.timeout_secs, 120);
}

#[test]
fn test_default_model_constant() {
    assert_eq!(DEFAULT_COPILOT_MODEL, "gpt-4.1");
}

// -- Additional tests --

#[test]
fn test_github_copilot_config_serialization_roundtrip() {
    let config = GitHubCopilotConfig {
        uri: "https://copilot.example.com".into(),
        connect_mode: "http".into(),
        default_model: "gpt-4".into(),
        timeout_secs: 60,
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: GitHubCopilotConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.uri, "https://copilot.example.com");
    assert_eq!(back.connect_mode, "http");
    assert_eq!(back.default_model, "gpt-4");
    assert_eq!(back.timeout_secs, 60);
}

#[test]
fn test_github_copilot_config_deserialization_defaults() {
    let json = r#"{}"#;
    let config: GitHubCopilotConfig = serde_json::from_str(json).unwrap();
    assert!(config.uri.is_empty());
    assert!(config.connect_mode.is_empty());
    assert!(config.default_model.is_empty());
    assert_eq!(config.timeout_secs, 120); // from serde default
}

#[test]
fn test_github_copilot_config_deserialization_with_timeout() {
    let json = r#"{"timeout_secs": 30}"#;
    let config: GitHubCopilotConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.timeout_secs, 30);
}

#[test]
fn test_messages_to_prompt_single_message() {
    let provider = GitHubCopilotProvider::new(GitHubCopilotConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let prompt = provider.messages_to_prompt(&messages);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&prompt).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["role"], "user");
    assert_eq!(parsed[0]["content"], "Hello");
}

#[test]
fn test_messages_to_prompt_empty() {
    let provider = GitHubCopilotProvider::new(GitHubCopilotConfig::default());
    let messages: Vec<Message> = vec![];
    let prompt = provider.messages_to_prompt(&messages);
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&prompt).unwrap();
    assert!(parsed.is_empty());
}

#[test]
fn test_provider_name_and_default_model() {
    let provider = GitHubCopilotProvider::new(GitHubCopilotConfig::default());
    assert_eq!(provider.name(), "github-copilot");
    assert_eq!(provider.default_model(), DEFAULT_COPILOT_MODEL);
}

#[test]
fn test_config_default_values() {
    let config = GitHubCopilotConfig::default();
    assert!(config.uri.is_empty());
    assert_eq!(config.connect_mode, "grpc");
    assert_eq!(config.default_model, DEFAULT_COPILOT_MODEL);
    assert_eq!(config.timeout_secs, 120);
}
