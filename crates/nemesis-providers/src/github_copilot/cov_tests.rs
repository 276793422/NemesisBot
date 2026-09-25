// github_copilot.rs 覆盖率补充测试（send 失败 → 诚实 Timeout 105-107）。

use super::*;
use crate::types::{ChatOptions, Message};

/// 不可达 uri → send 失败映射 Timeout（105-107）。
#[tokio::test]
async fn chat_dead_uri_returns_timeout() {
    let provider = GitHubCopilotProvider::new(GitHubCopilotConfig {
        uri: "http://127.0.0.1:1".to_string(),
        connect_mode: String::new(),
        default_model: "cov-model".to_string(),
        timeout_secs: 5,
    });
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Timeout { provider: p, .. } => assert_eq!(p, "github-copilot"),
        other => panic!("预期 Timeout，得到 {other:?}"),
    }
}
