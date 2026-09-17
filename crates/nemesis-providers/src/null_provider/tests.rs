//! NullProvider 测试（独立文件，遵循测试不内联纪律）。

use super::*;

#[tokio::test]
async fn test_chat_returns_honest_unconfigured_error() {
    let p = NullProvider::new(String::new());
    let err = p
        .chat(&[], &[], "any", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { provider, message } => {
            assert_eq!(provider, "null");
            assert!(message.contains("未配置模型"), "message: {}", message);
            assert!(message.contains("Dashboard"), "message: {}", message);
        }
        other => panic!("expected Unknown, got {:?}", other),
    }
}

#[tokio::test]
async fn test_chat_appends_factory_reason_when_present() {
    let p = NullProvider::new("no API key configured for provider: zhipu".to_string());
    let err = p
        .chat(&[], &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Unknown { provider, message } => {
            assert_eq!(provider, "null");
            assert!(
                message.contains("no API key configured"),
                "message: {}",
                message
            );
            assert!(message.contains("未配置模型"), "message: {}", message);
        }
        other => panic!("expected Unknown, got {:?}", other),
    }
}

#[tokio::test]
async fn test_chat_stream_yields_error_then_ends() {
    let p = NullProvider::new(String::new());
    let mut rx = p.chat_stream(&[], &[], "m", &ChatOptions::default());
    let first = rx.recv().await.expect("must yield one error chunk");
    assert!(first.is_err());
    assert!(
        rx.recv().await.is_none(),
        "channel must end after the error"
    );
}

#[test]
fn test_metadata() {
    let p = NullProvider::new(String::new());
    assert_eq!(p.name(), "null");
    assert_eq!(p.default_model(), "");
    let _dyn: Arc<dyn LLMProvider> = null_provider("x");
}
