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
    assert_eq!(classify(&resp, 7), Incoming::Response(resp.clone()));
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
    assert_eq!(
        default_server_response("anything/else"),
        serde_json::Value::Null
    );
}

#[test]
fn transient_error_detection() {
    // Spec transient codes (retry-safe).
    assert!(is_transient_error(
        &serde_json::json!({"code": -32800, "message": "RequestCancelled"})
    ));
    assert!(is_transient_error(
        &serde_json::json!({"code": -32801, "message": "content modified"})
    ));
    // Real errors must NOT be retried blindly.
    assert!(!is_transient_error(
        &serde_json::json!({"code": -32603, "message": "internal"})
    ));
    assert!(!is_transient_error(
        &serde_json::json!({"code": -32601, "message": "method not found"})
    ));
    assert!(!is_transient_error(
        &serde_json::json!({"message": "no code"})
    ));
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
// URI 规范化键（2026-09-05 C3 实机闭环验证根修）
// ---------------------------------------------------------------------------

#[test]
fn uri_key_unifies_whatwg_normal_forms() {
    // Windows 盘符大小写（rust-analyzer 的 url crate 把 `C:` 规范成 `c:`
    // 回显）必须落到同一个键上——record/lookup 双端靠它精确匹配。
    // 跨平台纯字符串级：POSIX 宿主上同样归一（不依赖路径语义）。
    assert_eq!(uri_key("file:///C:/u/a.rs"), "file:///c:/u/a.rs");
    assert_eq!(uri_key("file:///C:/u/a.rs"), uri_key("file:///c:/u/a.rs"));
    // 规范形自身幂等。
    assert_eq!(uri_key("file:///c:/u/a.rs"), "file:///c:/u/a.rs");
    assert_eq!(uri_key("file:///u/a.rs"), "file:///u/a.rs");
    // 非 file scheme / 解析失败原样返回（绝不 panic）。
    assert_eq!(uri_key("http://x/a.rs"), "http://x/a.rs");
    assert_eq!(uri_key("not a uri at all"), "not a uri at all");
}

#[test]
fn path_to_uri_matches_server_normalized_form() {
    // 闭环真不变量：本端发射形态经 uri_key 归一后，必须与「WHATWG 规范化
    // 服务器」的回显形态**同键**。发射与回显字面不必一致（2026-09-05 实
    // 测：本机 rust-url from_file_path 产出大写盘符，真 rust-analyzer 回
    // 显小写）——键归一兜住两端，这才是 publishDiagnostics 记录/查询精
    // 确匹配的实际依赖。
    let p = std::env::temp_dir().join("uri_norm_probe").join("a.rs");
    let uri = path_to_uri(&p);
    // 模拟 rust-analyzer 系服务器的回显：file:///X:/ → file:///x:/。
    let b = uri.as_bytes();
    let echoed = if uri.starts_with("file:///")
        && b.len() >= 10
        && b[8].is_ascii_alphabetic()
        && b[9] == b':'
    {
        format!(
            "{}{}{}",
            &uri[..8],
            (b[8] as char).to_ascii_lowercase(),
            &uri[9..]
        )
    } else {
        uri.clone()
    };
    assert_eq!(
        uri_key(&uri),
        uri_key(&echoed),
        "发射键必须与服务器回显键一致: emit={uri} echo={echoed}"
    );
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
        vec![Loc {
            path: "/a.rs".into(),
            line: 3,
            character: 8
        }]
    );
    // Location array
    let arr = serde_json::json!([
        loc_json("file:///a.rs", 1, 0),
        loc_json("file:///b.rs", 2, 4)
    ]);
    assert_eq!(parse_locations(&arr).len(), 2);
    // LocationLink array (targetUri/targetRange)
    let link = serde_json::json!([{
        "targetUri": "file:///c.rs",
        "targetRange": {"start": {"line": 9, "character": 5}, "end": {"line": 9, "character": 9}},
        "targetSelectionRange": {"start": {"line": 9, "character": 5}, "end": {"line": 9, "character": 9}},
    }]);
    assert_eq!(
        parse_locations(&link),
        vec![Loc {
            path: "/c.rs".into(),
            line: 9,
            character: 5
        }]
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

// ===========================================================================
// S1 补测（2026-08-26）：FrameDecoder::default()；uri_to_path 小写十六进制 /
// 非法十六进制回退；parse_hover 无 value 字段的对象
// ===========================================================================

#[test]
fn s1_frame_decoder_default_is_empty() {
    let mut d = FrameDecoder::default();
    assert!(d.next_message().unwrap().is_none());
    d.push(b"Content-Length: 2\r\n\r\n{}");
    assert_eq!(d.next_message().unwrap().unwrap(), serde_json::json!({}));
}

#[test]
fn s1_uri_to_path_lowercase_hex_decodes() {
    // Lowercase hex digits take the b'a'..=b'f' arm: %2f → '/'.
    assert_eq!(uri_to_path("file:///a%2fb%2fc.rs"), "/a/b/c.rs");
    assert_eq!(uri_to_path("file:///x%5e"), "/x^");
}

#[test]
fn s1_uri_to_path_invalid_hex_passes_through_literally() {
    // %zz is not a valid escape: the '%' and the following bytes are pushed
    // as-is (tolerant decoding for servers that echo raw characters).
    assert_eq!(uri_to_path("file:///a%zzb.rs"), "/a%zzb.rs");
    // Trailing percent at end of string (i+2 < len fails) also passes through.
    assert_eq!(uri_to_path("file:///tail%"), "/tail%");
}

#[test]
fn s1_parse_hover_object_without_value_is_empty() {
    // {kind: "plaintext"} has neither language+value nor value → "".
    assert_eq!(
        parse_hover(&serde_json::json!({"contents": {"kind": "plaintext"}})),
        ""
    );
}

// ===========================================================================
// C2 补测（devtool-upgrade 阶段 2）：publishDiagnostics 解析
// ===========================================================================

#[test]
fn c2_parse_publish_diagnostics_full_and_defaults() {
    let params = serde_json::json!({
        "uri": "file:///a/b.go",
        "diagnostics": [
            {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 7}},
             "severity": 2, "source": "go build", "message": "undefined: x"},
            {"range": {"start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 1}},
             "message": "syntax error"}
        ]
    });
    let (uri, diags) = parse_publish_diagnostics(&params).unwrap();
    assert_eq!(uri, "file:///a/b.go");
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].range_start, (1, 2));
    assert_eq!(diags[0].range_end, (1, 7));
    assert_eq!(diags[0].severity, 2);
    assert_eq!(diags[0].source.as_deref(), Some("go build"));
    assert_eq!(diags[0].message, "undefined: x");
    // 缺 severity → 规范默认 1（Error）；缺 source → None。
    assert_eq!(diags[1].severity, 1);
    assert_eq!(diags[1].source, None);
    assert_eq!(diags[1].message, "syntax error");
}

#[test]
fn c2_parse_publish_diagnostics_tolerant() {
    // 缺 uri / 缺 diagnostics / 形状错 → None（不炸）。
    assert!(parse_publish_diagnostics(&serde_json::json!({})).is_none());
    assert!(parse_publish_diagnostics(&serde_json::json!({"uri": "file:///x.go"})).is_none());
    assert!(parse_publish_diagnostics(&serde_json::json!({"uri": 3, "diagnostics": []})).is_none());
    assert!(
        parse_publish_diagnostics(&serde_json::json!({"uri": "file:///x.go", "diagnostics": "no"}))
            .is_none()
    );
    // 空列表合法（服务器用它清诊断）。
    let empty = parse_publish_diagnostics(&serde_json::json!({
        "uri": "file:///x.go", "diagnostics": []
    }))
    .unwrap();
    assert!(empty.1.is_empty());
    // 个别畸形条目（无 range / 无 message）跳过，不拖垮整批。
    let mixed = parse_publish_diagnostics(&serde_json::json!({
        "uri": "file:///x.go",
        "diagnostics": [
            {"message": "no range"},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}},
            {"range": {"start": {"line": 2, "character": 1}, "end": {"line": 2, "character": 2}},
             "message": "ok"}
        ]
    }))
    .unwrap();
    assert_eq!(mixed.1.len(), 1);
    assert_eq!(mixed.1[0].message, "ok");
    // severity 越界回落默认 1。
    let weird = parse_publish_diagnostics(&serde_json::json!({
        "uri": "file:///x.go",
        "diagnostics": [{"range": {"start": {"line": 0, "character": 0},
                                   "end": {"line": 0, "character": 0}},
                         "severity": 9, "message": "m"}]
    }))
    .unwrap();
    assert_eq!(weird.1[0].severity, 1);
}

// ===========================================================================
// C7 补测（devtool-upgrade 阶段 6）：WorkspaceEdit / CodeAction / 位置换算 /
// TextEdit 应用
// ===========================================================================

fn te(sl: u64, sc: u64, el: u64, ec: u64, new_text: &str) -> serde_json::Value {
    serde_json::json!({
        "range": {"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}},
        "newText": new_text
    })
}

#[test]
fn c7_parse_workspace_edit_changes_map() {
    let result = serde_json::json!({
        "changes": {
            "file:///a.rs": [te(0, 4, 0, 7, "bar"), te(3, 0, 3, 3, "bar")],
            "file:///b.go": [te(1, 0, 1, 2, "y")]
        }
    });
    let mut edits = parse_workspace_edit(&result);
    edits.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].path, "/a.rs");
    assert_eq!(edits[0].edits.len(), 2);
    assert_eq!(edits[0].edits[0].range_start, (0, 4));
    assert_eq!(edits[0].edits[0].new_text, "bar");
    assert_eq!(edits[1].path, "/b.go");
}

#[test]
fn c7_parse_workspace_edit_document_changes() {
    // spec 3.16 documentChanges 形态（rust-analyzer 常用）。
    let result = serde_json::json!({
        "documentChanges": [
            {"textDocument": {"uri": "file:///a.rs", "version": 7},
             "edits": [te(0, 0, 0, 3, "z")]}
        ]
    });
    let edits = parse_workspace_edit(&result);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].path, "/a.rs");
    assert_eq!(edits[0].edits[0].new_text, "z");
}

#[test]
fn c7_parse_workspace_edit_tolerant() {
    assert!(parse_workspace_edit(&serde_json::Value::Null).is_empty());
    assert!(parse_workspace_edit(&serde_json::json!({})).is_empty());
    // 两种形态同时出现都收（规范允许双发）。
    let both = serde_json::json!({
        "changes": {"file:///a.rs": [te(0, 0, 0, 1, "x")]},
        "documentChanges": [
            {"textDocument": {"uri": "file:///b.rs"}, "edits": [te(1, 1, 1, 1, "y")]}
        ]
    });
    assert_eq!(parse_workspace_edit(&both).len(), 2);
    // 畸形条目跳过不炸：edits 非数组 / TextEdit 缺 newText。
    let broken = serde_json::json!({
        "changes": {"file:///a.rs": "not-an-array"},
        "documentChanges": [
            {"textDocument": {"uri": "file:///b.rs"}, "edits": [{"range": {}}]},
            {"textDocument": {"uri": "file:///c.rs"}, "edits": [te(0, 0, 0, 1, "ok")]}
        ]
    });
    let got = parse_workspace_edit(&broken);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].path, "/c.rs");
}

#[test]
fn c7_parse_code_actions_full_and_degraded() {
    let result = serde_json::json!([
        {"title": "Import `std::io`", "kind": "quickfix", "isPreferred": true,
         "edit": {"changes": {}}},
        {"title": "Wrap in Ok", "kind": "quickfix"},
        {"title": "legacy command", "command": {"title": "legacy command"}}
    ]);
    let actions = parse_code_actions(&result);
    assert_eq!(actions.len(), 3);
    assert_eq!(actions[0].title, "Import `std::io`");
    assert_eq!(actions[0].kind.as_deref(), Some("quickfix"));
    assert!(actions[0].has_edit);
    assert!(actions[0].is_preferred);
    assert!(!actions[1].has_edit);
    assert!(!actions[1].is_preferred);
    // legacy Command 形态：只有 title，kind/edit 缺省。
    assert_eq!(actions[2].title, "legacy command");
    assert_eq!(actions[2].kind, None);
    assert!(!actions[2].has_edit);
    // null / 无 title 条目 → 空或跳过。
    assert!(parse_code_actions(&serde_json::Value::Null).is_empty());
    assert!(parse_code_actions(&serde_json::json!([{"kind": "quickfix"}])).is_empty());
}

#[test]
fn c7_position_to_byte_ascii_and_boundaries() {
    let content = "hello\nworld\n";
    assert_eq!(position_to_byte(content, 0, 0).unwrap(), 0);
    assert_eq!(position_to_byte(content, 0, 3).unwrap(), 3);
    // EOL 插入点（'o' 与 '\n' 之间）合法。
    assert_eq!(position_to_byte(content, 0, 5).unwrap(), 5);
    assert_eq!(position_to_byte(content, 1, 2).unwrap(), 8);
    // 末尾空行（最后一个 '\n' 之后）。
    assert_eq!(position_to_byte(content, 2, 0).unwrap(), 12);
    // 无终止换行的最后一行：EOF 落点合法。
    let no_nl = "ab\ncd";
    assert_eq!(position_to_byte(no_nl, 1, 2).unwrap(), 5);
    // 越界：行 / 列都诚实报错（不 clamp——错位会腐蚀编辑）。
    assert!(position_to_byte(content, 9, 0).is_err());
    assert!(position_to_byte(content, 0, 6).is_err());
    assert!(position_to_byte(content, 1, 99).is_err());
    // 空文档。
    assert_eq!(position_to_byte("", 0, 0).unwrap(), 0);
    assert!(position_to_byte("", 1, 0).is_err());
}

#[test]
fn c7_position_to_byte_utf16_columns() {
    // CJK：BMP 内 = 1 UTF-16 单位 = 3 UTF-8 字节。
    let cjk = "中文\nx";
    assert_eq!(position_to_byte(cjk, 0, 1).unwrap(), 3);
    assert_eq!(position_to_byte(cjk, 0, 2).unwrap(), 6);
    assert_eq!(position_to_byte(cjk, 1, 0).unwrap(), 7);
    // (1,1) = 'x' 之后 = EOF。
    assert_eq!(position_to_byte(cjk, 1, 1).unwrap(), 8);
    // astral（emoji = 2 UTF-16 单位）：列按单位数、字节按 UTF-8 落点。
    let emoji = "😀x\ny";
    assert_eq!(position_to_byte(emoji, 0, 2).unwrap(), 4);
    assert_eq!(position_to_byte(emoji, 0, 3).unwrap(), 5);
    // 落在代理对中间 = 诚实报错。
    assert!(position_to_byte(emoji, 0, 1).is_err());
}

#[test]
fn c7_apply_text_edits_basic_shapes() {
    let content = "fn foo() {\n    bar();\n}\n";
    // 单替换。
    let repl = vec![TextEdit {
        range_start: (1, 4),
        range_end: (1, 7),
        new_text: "baz".into(),
    }];
    assert_eq!(
        apply_text_edits(content, &repl).unwrap(),
        "fn foo() {\n    baz();\n}\n"
    );
    // 纯插入（start==end）。
    let ins = vec![TextEdit {
        range_start: (0, 0),
        range_end: (0, 0),
        new_text: "//! doc\n".into(),
    }];
    assert_eq!(
        apply_text_edits(content, &ins).unwrap(),
        "//! doc\nfn foo() {\n    bar();\n}\n"
    );
    // 删除（newText 空）。
    let del = vec![TextEdit {
        range_start: (1, 0),
        range_end: (2, 0),
        new_text: String::new(),
    }];
    assert_eq!(apply_text_edits(content, &del).unwrap(), "fn foo() {\n}\n");
    // 空 edits = 原文不变。
    assert_eq!(apply_text_edits(content, &[]).unwrap(), content);
}

#[test]
fn c7_apply_text_edits_order_and_overlap() {
    // 乱序输入（服务器不保证有序）→ 按 byte 位置倒序应用。
    let content = "let aa = aa + aa;\n";
    let edits = vec![
        TextEdit {
            range_start: (0, 14),
            range_end: (0, 16),
            new_text: "zz".into(),
        },
        TextEdit {
            range_start: (0, 4),
            range_end: (0, 6),
            new_text: "bb".into(),
        },
        TextEdit {
            range_start: (0, 9),
            range_end: (0, 11),
            new_text: "cc".into(),
        },
    ];
    assert_eq!(
        apply_text_edits(content, &edits).unwrap(),
        "let bb = cc + zz;\n"
    );
    // 相邻不重叠（前一个 end == 后一个 start）合法。
    let adjacent = vec![
        TextEdit {
            range_start: (0, 0),
            range_end: (0, 3),
            new_text: "X".into(),
        },
        TextEdit {
            range_start: (0, 3),
            range_end: (0, 6),
            new_text: "Y".into(),
        },
    ];
    assert_eq!(apply_text_edits("abcdef", &adjacent).unwrap(), "XY");
    // 重叠范围 = 拒绝（坏服务器的嵌套 range 不能静默合并出损坏文本）。
    let overlap = vec![
        TextEdit {
            range_start: (0, 0),
            range_end: (0, 5),
            new_text: "x".into(),
        },
        TextEdit {
            range_start: (0, 3),
            range_end: (0, 6),
            new_text: "y".into(),
        },
    ];
    assert!(apply_text_edits("abcdef", &overlap).is_err());
    // 越界位置 = 拒绝。
    let oob = vec![TextEdit {
        range_start: (5, 0),
        range_end: (5, 1),
        new_text: "x".into(),
    }];
    assert!(apply_text_edits("abcdef", &oob).is_err());
    // end < start = 拒绝（防御畸形 range）。
    let reversed = vec![TextEdit {
        range_start: (0, 3),
        range_end: (0, 1),
        new_text: "x".into(),
    }];
    assert!(apply_text_edits("abcdef", &reversed).is_err());
    // CJK 内容上跨行替换（UTF-16 列 → UTF-8 字节落点）。
    let cjk = "中文\n中文";
    let cross = vec![TextEdit {
        range_start: (0, 1),
        range_end: (1, 1),
        new_text: "X".into(),
    }];
    assert_eq!(apply_text_edits(cjk, &cross).unwrap(), "中X文");
}

#[test]
fn c7_diagnostic_to_json_shape() {
    let d = Diagnostic {
        range_start: (1, 2),
        range_end: (1, 7),
        severity: 2,
        source: Some("rustc".into()),
        message: "unused variable".into(),
    };
    let j = d.to_json();
    assert_eq!(j["range"]["start"]["line"], 1);
    assert_eq!(j["range"]["start"]["character"], 2);
    assert_eq!(j["range"]["end"]["line"], 1);
    assert_eq!(j["range"]["end"]["character"], 7);
    assert_eq!(j["severity"], 2);
    assert_eq!(j["source"], "rustc");
    assert_eq!(j["message"], "unused variable");
    // source=None → JSON null（codeAction context 规范允许）。
    let d2 = Diagnostic { source: None, ..d };
    assert!(d2.to_json()["source"].is_null());
}
