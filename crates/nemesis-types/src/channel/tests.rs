use super::*;

#[test]
fn test_media_attachment_serialize_as_string() {
    // When media_type is empty and data is None, should serialize as plain string.
    let ma = MediaAttachment {
        media_type: String::new(),
        url: "https://example.com/image.png".to_string(),
        data: None,
    };
    let json = serde_json::to_string(&ma).unwrap();
    assert_eq!(json, "\"https://example.com/image.png\"");
}

#[test]
fn test_media_attachment_serialize_as_object() {
    // When media_type is set, should serialize as object.
    let ma = MediaAttachment {
        media_type: "image".to_string(),
        url: "https://example.com/img.png".to_string(),
        data: None,
    };
    let json = serde_json::to_string(&ma).unwrap();
    assert!(json.contains("\"type\":\"image\""));
    assert!(json.contains("\"url\""));
}

#[test]
fn test_media_attachment_deserialize_from_string() {
    // Go sends []string, so we need to accept plain strings.
    let json = "\"https://example.com/file.mp4\"";
    let ma: MediaAttachment = serde_json::from_str(json).unwrap();
    assert_eq!(ma.url, "https://example.com/file.mp4");
    assert_eq!(ma.media_type, "");
    assert!(ma.data.is_none());
}

#[test]
fn test_media_attachment_deserialize_from_object() {
    let json = r#"{"type":"image","url":"https://example.com/img.png"}"#;
    let ma: MediaAttachment = serde_json::from_str(json).unwrap();
    assert_eq!(ma.media_type, "image");
    assert_eq!(ma.url, "https://example.com/img.png");
}

#[test]
fn test_inbound_message_with_string_media() {
    // Simulates Go's []string media format.
    let json = r#"{
        "channel": "web",
        "sender_id": "user1",
        "chat_id": "chat1",
        "content": "hello",
        "media": ["https://example.com/a.png", "https://example.com/b.png"],
        "session_key": "sess1",
        "correlation_id": ""
    }"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.media.len(), 2);
    assert_eq!(msg.media[0].url, "https://example.com/a.png");
    assert_eq!(msg.media[1].url, "https://example.com/b.png");
}

#[test]
fn test_inbound_message_with_object_media() {
    let json = r#"{
        "channel": "web",
        "sender_id": "user1",
        "chat_id": "chat1",
        "content": "hello",
        "media": [{"type":"image","url":"https://example.com/img.png"}],
        "session_key": "sess1",
        "correlation_id": ""
    }"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.media.len(), 1);
    assert_eq!(msg.media[0].media_type, "image");
    assert_eq!(msg.media[0].url, "https://example.com/img.png");
}

// --- OutboundMessage ---

#[test]
fn test_outbound_message_new() {
    let msg = OutboundMessage::new("web", "chat1", "hello");
    assert_eq!(msg.channel, "web");
    assert_eq!(msg.chat_id, "chat1");
    assert_eq!(msg.content, "hello");
    assert_eq!(msg.message_type, "");
}

#[test]
fn test_outbound_message_with_type() {
    let msg = OutboundMessage::with_type("rpc", "chat2", "response", "history");
    assert_eq!(msg.channel, "rpc");
    assert_eq!(msg.chat_id, "chat2");
    assert_eq!(msg.content, "response");
    assert_eq!(msg.message_type, "history");
}

#[test]
fn test_outbound_message_serialize() {
    let msg = OutboundMessage::new("web", "chat1", "hello");
    let json = serde_json::to_string(&msg).unwrap();
    // message_type is serialized as "type" due to rename
    assert!(json.contains("\"type\":\"\""));
    assert!(json.contains("\"channel\":\"web\""));
    assert!(json.contains("\"chat_id\":\"chat1\""));
    assert!(json.contains("\"content\":\"hello\""));
}

#[test]
fn test_outbound_message_with_type_serialize() {
    let msg = OutboundMessage::with_type("web", "chat1", "hello", "history");
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"type\":\"history\""));
}

#[test]
fn test_outbound_message_deserialize() {
    let json = r#"{"channel":"discord","chat_id":"ch42","content":"hi","type":""}"#;
    let msg: OutboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.channel, "discord");
    assert_eq!(msg.chat_id, "ch42");
    assert_eq!(msg.content, "hi");
    assert_eq!(msg.message_type, "");
}

#[test]
fn test_outbound_message_deserialize_missing_type() {
    // message_type has #[serde(default)], so missing field => ""
    let json = r#"{"channel":"web","chat_id":"c1","content":"test"}"#;
    let msg: OutboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.message_type, "");
}

#[test]
fn test_outbound_message_roundtrip() {
    let msg = OutboundMessage::with_type("rpc", "c1", "content", "history");
    let json = serde_json::to_string(&msg).unwrap();
    let msg2: OutboundMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.channel, msg2.channel);
    assert_eq!(msg.chat_id, msg2.chat_id);
    assert_eq!(msg.content, msg2.content);
    assert_eq!(msg.message_type, msg2.message_type);
}

#[test]
fn test_outbound_message_clone() {
    let msg = OutboundMessage::new("web", "c1", "hello");
    let msg2 = msg.clone();
    assert_eq!(msg.channel, msg2.channel);
    assert_eq!(msg.content, msg2.content);
}

// --- InboundMessage additional tests ---

#[test]
fn test_inbound_message_basic_fields() {
    let json = r#"{
        "channel": "telegram",
        "sender_id": "user42",
        "chat_id": "chat99",
        "content": "test message",
        "media": [],
        "session_key": "sess_abc",
        "correlation_id": "corr_123"
    }"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.channel, "telegram");
    assert_eq!(msg.sender_id, "user42");
    assert_eq!(msg.chat_id, "chat99");
    assert_eq!(msg.content, "test message");
    assert!(msg.media.is_empty());
    assert_eq!(msg.session_key, "sess_abc");
    assert_eq!(msg.correlation_id, "corr_123");
}

#[test]
fn test_inbound_message_metadata_default() {
    let json = r#"{
        "channel": "web",
        "sender_id": "u1",
        "chat_id": "c1",
        "content": "hi",
        "media": [],
        "session_key": "s1",
        "correlation_id": ""
    }"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();
    assert!(msg.metadata.is_empty());
}

#[test]
fn test_inbound_message_metadata_present() {
    let json = r#"{
        "channel": "discord",
        "sender_id": "u1",
        "chat_id": "c1",
        "content": "hi",
        "media": [],
        "session_key": "s1",
        "correlation_id": "",
        "metadata": {"guild_id": "g1", "peer_kind": "discord"}
    }"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.metadata.get("guild_id"), Some(&"g1".to_string()));
    assert_eq!(msg.metadata.get("peer_kind"), Some(&"discord".to_string()));
}

#[test]
fn test_inbound_message_mixed_media() {
    let json = r#"{
        "channel": "web",
        "sender_id": "u1",
        "chat_id": "c1",
        "content": "mixed",
        "media": [
            "https://example.com/a.png",
            {"type":"video","url":"https://example.com/vid.mp4"},
            {"type":"audio","url":"https://example.com/audio.mp3","data":"base64data"}
        ],
        "session_key": "s1",
        "correlation_id": ""
    }"#;
    let msg: InboundMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.media.len(), 3);
    // First: plain string => no type
    assert_eq!(msg.media[0].media_type, "");
    assert_eq!(msg.media[0].url, "https://example.com/a.png");
    assert!(msg.media[0].data.is_none());
    // Second: object with type
    assert_eq!(msg.media[1].media_type, "video");
    assert!(msg.media[1].data.is_none());
    // Third: object with type and data
    assert_eq!(msg.media[2].media_type, "audio");
    assert_eq!(msg.media[2].data, Some("base64data".to_string()));
}

#[test]
fn test_inbound_message_roundtrip() {
    let msg = InboundMessage {
        channel: "web".to_string(),
        sender_id: "u1".to_string(),
        chat_id: "c1".to_string(),
        content: "hello".to_string(),
        media: vec![],
        session_key: "sk1".to_string(),
        correlation_id: "cid1".to_string(),
        metadata: std::collections::HashMap::new(),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let msg2: InboundMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.channel, msg2.channel);
    assert_eq!(msg.sender_id, msg2.sender_id);
    assert_eq!(msg.content, msg2.content);
    assert_eq!(msg.correlation_id, msg2.correlation_id);
}

// --- MediaAttachment edge cases ---

#[test]
fn test_media_attachment_serialize_with_data() {
    let ma = MediaAttachment {
        media_type: "image".to_string(),
        url: "https://example.com/img.png".to_string(),
        data: Some("base64data".to_string()),
    };
    let json = serde_json::to_string(&ma).unwrap();
    assert!(json.contains("\"data\":\"base64data\""));
    assert!(json.contains("\"type\":\"image\""));
}

#[test]
fn test_media_attachment_serialize_with_data_no_type() {
    // media_type empty but data is Some => object form
    let ma = MediaAttachment {
        media_type: String::new(),
        url: "https://example.com/img.png".to_string(),
        data: Some("base64data".to_string()),
    };
    let json = serde_json::to_string(&ma).unwrap();
    // Should be object because data is Some (not None)
    assert!(json.starts_with("{"));
}

#[test]
fn test_media_attachment_serialize_empty_url_with_type() {
    let ma = MediaAttachment {
        media_type: "image".to_string(),
        url: String::new(),
        data: None,
    };
    let json = serde_json::to_string(&ma).unwrap();
    // media_type is set, so object form
    assert!(json.starts_with("{"));
}

#[test]
fn test_media_attachment_serialize_empty_url_empty_type() {
    // media_type empty, data None, url empty => condition !self.url.is_empty() is false => object form
    let ma = MediaAttachment {
        media_type: String::new(),
        url: String::new(),
        data: None,
    };
    let json = serde_json::to_string(&ma).unwrap();
    // Should serialize as object because url is empty (the plain-string shortcut requires non-empty url)
    assert!(json.starts_with("{"));
}

#[test]
fn test_media_attachment_deserialize_object_with_data() {
    let json = r#"{"type":"image","url":"https://example.com/img.png","data":"base64data"}"#;
    let ma: MediaAttachment = serde_json::from_str(json).unwrap();
    assert_eq!(ma.media_type, "image");
    assert_eq!(ma.url, "https://example.com/img.png");
    assert_eq!(ma.data, Some("base64data".to_string()));
}

#[test]
fn test_media_attachment_deserialize_object_missing_type() {
    let json = r#"{"url":"https://example.com/img.png"}"#;
    let ma: MediaAttachment = serde_json::from_str(json).unwrap();
    assert_eq!(ma.media_type, "");
    assert_eq!(ma.url, "https://example.com/img.png");
    assert!(ma.data.is_none());
}

#[test]
fn test_media_attachment_clone() {
    let ma = MediaAttachment {
        media_type: "image".to_string(),
        url: "https://example.com/img.png".to_string(),
        data: Some("data".to_string()),
    };
    let ma2 = ma.clone();
    assert_eq!(ma.media_type, ma2.media_type);
    assert_eq!(ma.url, ma2.url);
    assert_eq!(ma.data, ma2.data);
}

#[test]
fn test_media_attachment_debug() {
    let ma = MediaAttachment {
        media_type: "image".to_string(),
        url: "https://example.com/img.png".to_string(),
        data: None,
    };
    let dbg = format!("{:?}", ma);
    assert!(dbg.contains("image"));
    assert!(dbg.contains("https://example.com/img.png"));
}

// --- ChannelContent ---

#[test]
fn test_channel_content_text() {
    let cc = ChannelContent::Text("hello".to_string());
    let json = serde_json::to_string(&cc).unwrap();
    assert!(json.contains("Text"));
    let cc2: ChannelContent = serde_json::from_str(&json).unwrap();
    assert!(matches!(cc2, ChannelContent::Text(s) if s == "hello"));
}

#[test]
fn test_channel_content_markdown() {
    let cc = ChannelContent::Markdown("# Hello".to_string());
    let json = serde_json::to_string(&cc).unwrap();
    let cc2: ChannelContent = serde_json::from_str(&json).unwrap();
    assert!(matches!(cc2, ChannelContent::Markdown(s) if s == "# Hello"));
}

#[test]
fn test_channel_content_html() {
    let cc = ChannelContent::Html("<b>bold</b>".to_string());
    let json = serde_json::to_string(&cc).unwrap();
    let cc2: ChannelContent = serde_json::from_str(&json).unwrap();
    assert!(matches!(cc2, ChannelContent::Html(s) if s == "<b>bold</b>"));
}

#[test]
fn test_channel_content_clone() {
    let cc = ChannelContent::Text("hello".to_string());
    let cc2 = cc.clone();
    assert!(matches!(cc2, ChannelContent::Text(s) if s == "hello"));
}

// --- ChannelUser ---

#[test]
fn test_channel_user_basic() {
    let user = ChannelUser {
        user_id: "u1".to_string(),
        username: "testuser".to_string(),
        display_name: None,
        is_admin: false,
    };
    assert_eq!(user.user_id, "u1");
    assert_eq!(user.username, "testuser");
    assert!(user.display_name.is_none());
    assert!(!user.is_admin);
}

#[test]
fn test_channel_user_with_display_name() {
    let user = ChannelUser {
        user_id: "u2".to_string(),
        username: "john".to_string(),
        display_name: Some("John Doe".to_string()),
        is_admin: true,
    };
    assert_eq!(user.display_name, Some("John Doe".to_string()));
    assert!(user.is_admin);
}

#[test]
fn test_channel_user_serialize_deserialize() {
    let user = ChannelUser {
        user_id: "u1".to_string(),
        username: "testuser".to_string(),
        display_name: Some("Test User".to_string()),
        is_admin: false,
    };
    let json = serde_json::to_string(&user).unwrap();
    let user2: ChannelUser = serde_json::from_str(&json).unwrap();
    assert_eq!(user.user_id, user2.user_id);
    assert_eq!(user.username, user2.username);
    assert_eq!(user.display_name, user2.display_name);
    assert_eq!(user.is_admin, user2.is_admin);
}

#[test]
fn test_channel_user_clone() {
    let user = ChannelUser {
        user_id: "u1".to_string(),
        username: "test".to_string(),
        display_name: None,
        is_admin: false,
    };
    let user2 = user.clone();
    assert_eq!(user.user_id, user2.user_id);
}

#[test]
fn test_channel_user_debug() {
    let user = ChannelUser {
        user_id: "u1".to_string(),
        username: "test".to_string(),
        display_name: None,
        is_admin: false,
    };
    let dbg = format!("{:?}", user);
    assert!(dbg.contains("u1"));
}
