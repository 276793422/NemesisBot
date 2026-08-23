//! Unit tests for LSP framing / JSON-RPC classification / response parsing.

use super::*;

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

#[test]
fn encode_produces_content_length_header() {
    let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "m"});
    let bytes = encode(&msg);
    let body = serde_json::to_vec(&msg).unwrap();
    let prefix = format!("Content-Length: {}\r\n\r\n", body.len());
    assert_eq!(&bytes[..prefix.len()], prefix.as_bytes());
    assert_eq!(&bytes[prefix.len()..], &body[..]);
}

#[test]
fn decoder_handles_split_and_batched_frames() {
    let a = serde_json::json!({"id": 1});
    let b = serde_json::json!({"method": "n"});
    let mut stream = encode(&a);
    stream.extend_from_slice(&encode(&b));

    // Feed one byte at a time to prove incremental handling.
    let mut dec = FrameDecoder::new();
    let mut got = Vec::new();
    for byte in &stream {
        dec.push(&[*byte]);
        while let Some(msg) = dec.next_message().unwrap() {
            got.push(msg);
        }
    }
    assert_eq!(got, vec![a, b]);
}

#[test]
fn decoder_accepts_lowercase_content_length() {
    let body = br#"{"x":1}"#;
    let raw = format!("content-length: {}\r\n\r\n", body.len());
    let mut dec = FrameDecoder::new();
    dec.push(raw.as_bytes());
    dec.push(body);
    let msg = dec.next_message().unwrap().expect("message complete");
    assert_eq!(msg, serde_json::json!({"x": 1}));
}

#[test]
fn decoder_errors_without_content_length() {
    let mut dec = FrameDecoder::new();
    dec.push(b"Some-Header: 1\r\n\r\n{}");
    assert!(dec.next_message().is_err());
}

#[test]
fn decoder_returns_none_on_partial_body() {
    let body = br#"{"a":123}"#;
    let raw = format!("Content-Length: {}\r\n\r\n", body.len());
    let mut dec = FrameDecoder::new();
    dec.push(raw.as_bytes());
    dec.push(&body[..3]);
    assert!(dec.next_message().unwrap().is_none());
    dec.push(&body[3..]);
    assert!(dec.next_message().unwrap().is_some());
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

#[test]
fn classify_response_server_request_and_notification() {
    let resp = serde_json::json!({"jsonrpc":"2.0","id":7,"result":{"ok":true}});
    assert_eq!(
        classify(&resp, 7),
        Incoming::Response(resp.clone())
    );
    // Response to a different id (stale) → skippable.
    assert!(matches!(classify(&resp, 8), Incoming::Notification { .. }));

    let req = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"workspace/configuration"});
    assert_eq!(
        classify(&req, 7),
        Incoming::ServerRequest {
            id: serde_json::json!(2),
            method: "workspace/configuration".to_string()
        }
    );

    let notif = serde_json::json!({"jsonrpc":"2.0","method":"$/progress"});
    assert_eq!(
        classify(&notif, 7),
        Incoming::Notification {
            method: "$/progress".to_string()
        }
    );
}

#[test]
fn default_server_response_configuration_is_empty_array() {
    assert_eq!(
        default_server_response("workspace/configuration"),
        serde_json::json!([])
    );
    assert_eq!(default_server_response("anything/else"), serde_json::Value::Null);
}

#[test]
fn transient_error_detection() {
    // Spec transient codes (retry-safe).
    assert!(is_transient_error(&serde_json::json!({"code": -32800, "message": "RequestCancelled"})));
    assert!(is_transient_error(&serde_json::json!({"code": -32801, "message": "content modified"})));
    // Real errors must NOT be retried blindly.
    assert!(!is_transient_error(&serde_json::json!({"code": -32603, "message": "internal"})));
    assert!(!is_transient_error(&serde_json::json!({"code": -32601, "message": "method not found"})));
    assert!(!is_transient_error(&serde_json::json!({"message": "no code"})));
}

// ---------------------------------------------------------------------------
// URI round-trips
// ---------------------------------------------------------------------------

#[test]
fn uri_round_trip_plain_paths() {
    // Windows backslashes normalize to forward slashes on the way back
    // (Rust accepts both when opening; display consistency matters more).
    for (p, expected) in [
        ("/home/u/repo/src/lib.rs", "/home/u/repo/src/lib.rs"),
        ("C:\\u\\repo\\src\\lib.rs", "C:/u/repo/src/lib.rs"),
    ] {
        let uri = path_to_uri(std::path::Path::new(p));
        assert!(uri.starts_with("file:///"), "{uri}");
        assert_eq!(uri_to_path(&uri), expected);
    }
}

#[test]
fn uri_round_trip_spaces_and_cjk() {
    // Space + Chinese chars must survive as percent-encoded UTF-8 and come
    // back identical (paths in this project frequently contain CJK).
    let p = "/tmp/项目 目录/lib.rs";
    let uri = path_to_uri(std::path::Path::new(p));
    assert!(uri.contains("%20"), "space should be encoded: {uri}");
    assert!(uri.contains("%E9%A1%B9"), "CJK should be encoded: {uri}");
    assert_eq!(uri_to_path(&uri), p);
}

#[test]
fn uri_to_path_strips_drive_slash_and_decodes() {
    assert_eq!(uri_to_path("file:///C:/x%20y/a.rs"), "C:/x y/a.rs");
    assert_eq!(uri_to_path("file:///home/u/a.rs"), "/home/u/a.rs");
    // Tolerant passthrough for unencoded URIs some servers emit.
    assert_eq!(uri_to_path("file:///home/u/a b.rs"), "/home/u/a b.rs");
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn loc_json(uri: &str, line: u64, ch: u64) -> serde_json::Value {
    serde_json::json!({
        "uri": uri,
        "range": {"start": {"line": line, "character": ch}, "end": {"line": line, "character": ch + 3}}
    })
}

#[test]
fn parse_locations_all_spec_shapes() {
    // null
    assert!(parse_locations(&serde_json::Value::Null).is_empty());
    // single Location object
    let one = loc_json("file:///a.rs", 3, 8);
    assert_eq!(
        parse_locations(&one),
        vec![Loc { path: "/a.rs".into(), line: 3, character: 8 }]
    );
    // Location array
    let arr = serde_json::json!([loc_json("file:///a.rs", 1, 0), loc_json("file:///b.rs", 2, 4)]);
    assert_eq!(parse_locations(&arr).len(), 2);
    // LocationLink array (targetUri/targetRange)
    let link = serde_json::json!([{
        "targetUri": "file:///c.rs",
        "targetRange": {"start": {"line": 9, "character": 5}, "end": {"line": 9, "character": 9}},
        "targetSelectionRange": {"start": {"line": 9, "character": 5}, "end": {"line": 9, "character": 9}},
    }]);
    assert_eq!(
        parse_locations(&link),
        vec![Loc { path: "/c.rs".into(), line: 9, character: 5 }]
    );
    // locationless entries are skipped, not fatal
    let mixed = serde_json::json!([loc_json("file:///a.rs", 0, 0), {"uri": "file:///no-range.rs"}]);
    assert_eq!(parse_locations(&mixed).len(), 1);
}

#[test]
fn parse_hover_all_contents_shapes() {
    // MarkupContent (markdown)
    let md = serde_json::json!({"contents": {"kind": "markdown", "value": "fn foo()"}});
    assert_eq!(parse_hover(&md), "fn foo()");
    // MarkedString string
    let s = serde_json::json!({"contents": "plain docs"});
    assert_eq!(parse_hover(&s), "plain docs");
    // MarkedString object {language, value}
    let code = serde_json::json!({"contents": {"language": "rust", "value": "fn foo() {}"}});
    assert_eq!(parse_hover(&code), "```rust\nfn foo() {}\n```");
    // array mix
    let arr = serde_json::json!({"contents": ["intro", {"language": "rust", "value": "let x"}]});
    assert_eq!(parse_hover(&arr), "intro\n\n```rust\nlet x\n```");
    // null / missing contents
    assert_eq!(parse_hover(&serde_json::Value::Null), "");
    assert_eq!(parse_hover(&serde_json::json!({})), "");
}
