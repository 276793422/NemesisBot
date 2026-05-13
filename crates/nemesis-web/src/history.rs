//! History types for chat history pagination.

use serde::{Deserialize, Serialize};

/// A single message in chat history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// A page of history messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    pub messages: Vec<HistoryMessage>,
    pub has_more: bool,
    pub oldest_index: i64,
    pub total_count: i64,
}

/// History request data payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRequestData {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_index: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_history_page() {
        let page = HistoryPage {
            messages: vec![
                HistoryMessage { role: "user".to_string(), content: "hello".to_string() },
                HistoryMessage { role: "assistant".to_string(), content: "hi".to_string() },
            ],
            has_more: false,
            oldest_index: 0,
            total_count: 2,
        };
        let json = serde_json::to_string(&page).unwrap();
        assert!(json.contains("user"));
    }

    #[test]
    fn test_deserialize_history_page() {
        let json = r#"{"messages":[{"role":"user","content":"hi"}],"has_more":true,"oldest_index":5,"total_count":100}"#;
        let page: HistoryPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].role, "user");
        assert!(page.has_more);
        assert_eq!(page.oldest_index, 5);
        assert_eq!(page.total_count, 100);
    }

    #[test]
    fn test_history_message_serialization() {
        let msg = HistoryMessage {
            role: "system".to_string(),
            content: "welcome".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HistoryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, "system");
        assert_eq!(parsed.content, "welcome");
    }

    #[test]
    fn test_history_page_empty_messages() {
        let page = HistoryPage {
            messages: vec![],
            has_more: false,
            oldest_index: 0,
            total_count: 0,
        };
        let json = serde_json::to_string(&page).unwrap();
        let parsed: HistoryPage = serde_json::from_str(&json).unwrap();
        assert!(parsed.messages.is_empty());
        assert_eq!(parsed.total_count, 0);
    }

    #[test]
    fn test_history_page_many_messages() {
        let messages: Vec<HistoryMessage> = (0..100)
            .map(|i| HistoryMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("message {}", i),
            })
            .collect();
        let page = HistoryPage {
            messages,
            has_more: true,
            oldest_index: 0,
            total_count: 200,
        };
        let json = serde_json::to_string(&page).unwrap();
        let parsed: HistoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.messages.len(), 100);
        assert!(parsed.has_more);
    }

    #[test]
    fn test_history_request_data_serialization() {
        let req = HistoryRequestData {
            request_id: "req-123".to_string(),
            limit: Some(50),
            before_index: Some(100),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: HistoryRequestData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "req-123");
        assert_eq!(parsed.limit, Some(50));
        assert_eq!(parsed.before_index, Some(100));
    }

    #[test]
    fn test_history_request_data_minimal() {
        let req = HistoryRequestData {
            request_id: "r1".to_string(),
            limit: None,
            before_index: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: HistoryRequestData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "r1");
        assert!(parsed.limit.is_none());
        assert!(parsed.before_index.is_none());
    }

    #[test]
    fn test_history_request_data_skip_none_fields() {
        let req = HistoryRequestData {
            request_id: "r1".to_string(),
            limit: None,
            before_index: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("limit"));
        assert!(!json.contains("before_index"));
    }

    #[test]
    fn test_history_message_with_special_chars() {
        let msg = HistoryMessage {
            role: "user".to_string(),
            content: "Hello <b>world</b> & 'friends' \"quoted\"".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HistoryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, msg.content);
    }

    #[test]
    fn test_history_page_roundtrip() {
        let page = HistoryPage {
            messages: vec![
                HistoryMessage { role: "user".to_string(), content: "test".to_string() },
            ],
            has_more: true,
            oldest_index: 42,
            total_count: 999,
        };
        let json = serde_json::to_string_pretty(&page).unwrap();
        let parsed: HistoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.messages.len(), 1);
        assert!(parsed.has_more);
        assert_eq!(parsed.oldest_index, 42);
        assert_eq!(parsed.total_count, 999);
    }

    #[test]
    fn test_history_page_negative_oldest_index() {
        let page = HistoryPage {
            messages: vec![],
            has_more: false,
            oldest_index: -1,
            total_count: 0,
        };
        let json = serde_json::to_string(&page).unwrap();
        let parsed: HistoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.oldest_index, -1);
    }

    // --- Additional history tests ---

    #[test]
    fn test_history_message_clone() {
        let msg = HistoryMessage { role: "user".into(), content: "test".into() };
        let cloned = msg.clone();
        assert_eq!(cloned.role, "user");
        assert_eq!(cloned.content, "test");
    }

    #[test]
    fn test_history_page_clone() {
        let page = HistoryPage {
            messages: vec![HistoryMessage { role: "user".into(), content: "hi".into() }],
            has_more: true,
            oldest_index: 5,
            total_count: 10,
        };
        let cloned = page.clone();
        assert_eq!(cloned.messages.len(), 1);
        assert!(cloned.has_more);
        assert_eq!(cloned.oldest_index, 5);
        assert_eq!(cloned.total_count, 10);
    }

    #[test]
    fn test_history_request_data_clone() {
        let req = HistoryRequestData {
            request_id: "r1".into(),
            limit: Some(10),
            before_index: None,
        };
        let cloned = req.clone();
        assert_eq!(cloned.request_id, "r1");
        assert_eq!(cloned.limit, Some(10));
    }

    #[test]
    fn test_history_message_debug() {
        let msg = HistoryMessage { role: "user".into(), content: "hello".into() };
        let debug_str = format!("{:?}", msg);
        assert!(debug_str.contains("user"));
        assert!(debug_str.contains("hello"));
    }

    #[test]
    fn test_history_page_debug() {
        let page = HistoryPage {
            messages: vec![],
            has_more: false,
            oldest_index: 0,
            total_count: 0,
        };
        let debug_str = format!("{:?}", page);
        assert!(debug_str.contains("has_more"));
    }

    #[test]
    fn test_history_request_data_debug() {
        let req = HistoryRequestData {
            request_id: "r1".into(),
            limit: None,
            before_index: None,
        };
        let debug_str = format!("{:?}", req);
        assert!(debug_str.contains("r1"));
    }

    #[test]
    fn test_history_message_with_unicode_content() {
        let msg = HistoryMessage {
            role: "user".into(),
            content: "Hello! Test".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HistoryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content, "Hello! Test");
    }

    #[test]
    fn test_history_page_with_large_total_count() {
        let page = HistoryPage {
            messages: vec![],
            has_more: true,
            oldest_index: 0,
            total_count: i64::MAX,
        };
        let json = serde_json::to_string(&page).unwrap();
        let parsed: HistoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_count, i64::MAX);
    }

    #[test]
    fn test_history_message_empty_content() {
        let msg = HistoryMessage { role: "system".into(), content: String::new() };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HistoryMessage = serde_json::from_str(&json).unwrap();
        assert!(parsed.content.is_empty());
    }

    #[test]
    fn test_history_request_data_with_only_request_id() {
        let json = r#"{"request_id":"only-id"}"#;
        let req: HistoryRequestData = serde_json::from_str(json).unwrap();
        assert_eq!(req.request_id, "only-id");
        assert!(req.limit.is_none());
        assert!(req.before_index.is_none());
    }

    #[test]
    fn test_history_page_alternating_roles() {
        let messages: Vec<HistoryMessage> = vec![
            HistoryMessage { role: "user".into(), content: "q1".into() },
            HistoryMessage { role: "assistant".into(), content: "a1".into() },
            HistoryMessage { role: "user".into(), content: "q2".into() },
            HistoryMessage { role: "assistant".into(), content: "a2".into() },
        ];
        let page = HistoryPage {
            messages,
            has_more: false,
            oldest_index: 0,
            total_count: 4,
        };
        assert_eq!(page.messages.len(), 4);
        assert_eq!(page.messages[0].role, "user");
        assert_eq!(page.messages[1].role, "assistant");
    }

    #[test]
    fn test_history_request_data_with_limit_only() {
        let req = HistoryRequestData {
            request_id: "r1".into(),
            limit: Some(25),
            before_index: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("limit"));
        assert!(!json.contains("before_index"));
    }

    #[test]
    fn test_history_request_data_with_before_index_only() {
        let req = HistoryRequestData {
            request_id: "r1".into(),
            limit: None,
            before_index: Some(50),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("limit"));
        assert!(json.contains("before_index"));
    }

    #[test]
    fn test_history_page_serialization_format() {
        let page = HistoryPage {
            messages: vec![HistoryMessage { role: "user".into(), content: "hi".into() }],
            has_more: false,
            oldest_index: 0,
            total_count: 1,
        };
        let json = serde_json::to_string_pretty(&page).unwrap();
        assert!(json.contains("\"messages\""));
        assert!(json.contains("\"has_more\""));
        assert!(json.contains("\"oldest_index\""));
        assert!(json.contains("\"total_count\""));
    }

    #[test]
    fn test_history_message_long_content() {
        let long_content = "x".repeat(10000);
        let msg = HistoryMessage { role: "user".into(), content: long_content.clone() };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HistoryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content.len(), 10000);
    }

    #[test]
    fn test_history_page_with_negative_total_count() {
        let page = HistoryPage {
            messages: vec![],
            has_more: false,
            oldest_index: 0,
            total_count: -100,
        };
        let json = serde_json::to_string(&page).unwrap();
        let parsed: HistoryPage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.total_count, -100);
    }

    #[test]
    fn test_history_request_data_with_negative_limit() {
        let req = HistoryRequestData {
            request_id: "r1".into(),
            limit: Some(-5),
            before_index: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: HistoryRequestData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.limit, Some(-5));
    }

    #[test]
    fn test_history_message_with_multiline_content() {
        let msg = HistoryMessage {
            role: "user".into(),
            content: "line1\nline2\nline3".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: HistoryMessage = serde_json::from_str(&json).unwrap();
        assert!(parsed.content.contains('\n'));
        assert_eq!(parsed.content.lines().count(), 3);
    }
}
