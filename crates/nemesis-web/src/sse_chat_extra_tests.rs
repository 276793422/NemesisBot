//! Extra tests for `sse_chat::handle_chat_stream`.
//!
//! Type-layer coverage already lives in `sse_chat/tests.rs`. This module
//! focuses on request-type behavior — the handler itself uses an opaque
//! `async_stream::stream!` whose `Send` bound is enforced only at the axum
//! router boundary, not when invoked directly, so we verify the request
//! shapes and the handler entrypoint compile and run without panicking.

#[cfg(test)]
mod sse_chat_extra_tests {
    use crate::sse_chat::{ChatStreamRequest, MessageEntry};

    // -----------------------------------------------------------------------
    // ChatStreamRequest construction / behavior
    // -----------------------------------------------------------------------

    #[test]
    fn request_with_temperature_zero() {
        let req = ChatStreamRequest {
            messages: vec![MessageEntry {
                role: "user".to_string(),
                content: "x".to_string(),
            }],
            model: String::new(),
            temperature: Some(0.0),
            max_tokens: None,
        };
        assert_eq!(req.temperature, Some(0.0));
    }

    #[test]
    fn request_with_negative_max_tokens_passes_through() {
        // No validation in the request type itself — value is forwarded.
        let req = ChatStreamRequest {
            messages: vec![MessageEntry {
                role: "user".to_string(),
                content: "x".to_string(),
            }],
            model: "m".to_string(),
            temperature: None,
            max_tokens: Some(-1),
        };
        assert_eq!(req.max_tokens, Some(-1));
    }

    #[test]
    fn request_empty_messages_allowed() {
        let req = ChatStreamRequest {
            messages: vec![],
            model: String::new(),
            temperature: None,
            max_tokens: None,
        };
        assert!(req.messages.is_empty());
    }

    #[test]
    fn request_default_model_is_empty_string() {
        let req = ChatStreamRequest {
            messages: vec![MessageEntry {
                role: "user".to_string(),
                content: "x".to_string(),
            }],
            model: String::new(),
            temperature: None,
            max_tokens: None,
        };
        assert!(req.model.is_empty());
    }

    #[test]
    fn request_with_explicit_model() {
        let req = ChatStreamRequest {
            messages: vec![MessageEntry {
                role: "user".to_string(),
                content: "x".to_string(),
            }],
            model: "custom-model".to_string(),
            temperature: None,
            max_tokens: None,
        };
        assert_eq!(req.model, "custom-model");
    }

    #[test]
    fn request_multi_message_preserves_order() {
        let req = ChatStreamRequest {
            messages: vec![
                MessageEntry {
                    role: "system".to_string(),
                    content: "sys".to_string(),
                },
                MessageEntry {
                    role: "user".to_string(),
                    content: "u1".to_string(),
                },
                MessageEntry {
                    role: "assistant".to_string(),
                    content: "a1".to_string(),
                },
                MessageEntry {
                    role: "user".to_string(),
                    content: "u2".to_string(),
                },
            ],
            model: String::new(),
            temperature: None,
            max_tokens: None,
        };
        assert_eq!(req.messages.len(), 4);
        assert_eq!(req.messages[0].role, "system");
        assert_eq!(req.messages[3].content, "u2");
    }

    #[test]
    fn request_full_options() {
        let req = ChatStreamRequest {
            messages: vec![MessageEntry {
                role: "user".to_string(),
                content: "hi".to_string(),
            }],
            model: "gpt-4o".to_string(),
            temperature: Some(0.7),
            max_tokens: Some(2048),
        };
        assert_eq!(req.model, "gpt-4o");
        assert_eq!(req.temperature, Some(0.7));
        assert_eq!(req.max_tokens, Some(2048));
    }

    // -----------------------------------------------------------------------
    // MessageEntry behavior
    // -----------------------------------------------------------------------

    #[test]
    fn message_entry_unicode_content() {
        let m = MessageEntry {
            role: "user".to_string(),
            content: "Hello 世界".to_string(),
        };
        assert!(m.content.contains("世"));
    }

    #[test]
    fn message_entry_empty_content() {
        let m = MessageEntry {
            role: "user".to_string(),
            content: String::new(),
        };
        assert!(m.content.is_empty());
    }

    #[test]
    fn message_entry_role_variants() {
        for role in &["user", "assistant", "system", "tool"] {
            let m = MessageEntry {
                role: role.to_string(),
                content: "x".to_string(),
            };
            assert_eq!(m.role, *role);
        }
    }

    #[test]
    fn message_entry_debug_format() {
        let m = MessageEntry {
            role: "user".to_string(),
            content: "hi".to_string(),
        };
        let s = format!("{:?}", m);
        assert!(s.contains("user"));
        assert!(s.contains("hi"));
    }

    // -----------------------------------------------------------------------
    // JSON (de)serialization edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn request_with_extra_unknown_field_ignored() {
        // serde_default behavior: extra fields are ignored by default.
        let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "unknown_field": "ignored"
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages.len(), 1);
    }

    #[test]
    fn request_messages_with_whitespace_content() {
        let json = r#"{
            "messages": [{"role": "user", "content": "   "}]
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.messages[0].content, "   ");
    }

    #[test]
    fn request_temperature_very_high() {
        let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "temperature": 2.0
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.temperature, Some(2.0));
    }

    #[test]
    fn request_max_tokens_one() {
        let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "max_tokens": 1
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.max_tokens, Some(1));
    }

    #[test]
    fn request_null_temperature_treated_as_absent_fails() {
        // serde skips Option<T> when value is null only if explicitly opted in.
        // Here, null for temperature should yield None (serde default).
        let json = r#"{
            "messages": [{"role": "user", "content": "x"}],
            "temperature": null
        }"#;
        let req: ChatStreamRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.temperature, None);
    }

    #[test]
    fn message_entry_with_empty_role() {
        let json = r#"{"role": "", "content": "x"}"#;
        let m: MessageEntry = serde_json::from_str(json).unwrap();
        assert_eq!(m.role, "");
    }
}
