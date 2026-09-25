// proto.rs 覆盖率补充测试（path_to_uri 的 Windows 盘符补斜杠臂 202、
// parse_workspace_edit 的 documentChanges 缺 uri 跳过臂 433）。

use super::*;

/// Windows 盘符路径不以 / 开头 → 显式补斜杠（202）。
#[test]
fn path_to_uri_adds_leading_slash_for_drive_paths() {
    let uri = path_to_uri(std::path::Path::new("C:/tmp/x.rs"));
    assert!(uri.starts_with("file:///"), "{uri}");
    assert!(
        uri.contains(":/tmp/x.rs") || uri.contains("%3A/tmp/x.rs"),
        "{uri}"
    );

    // POSIX 形态（已有前导斜杠）不重复补。
    let uri = path_to_uri(std::path::Path::new("/tmp/x.rs"));
    assert_eq!(uri, "file:///tmp/x.rs", "{uri}");
}

/// documentChanges 条目缺 textDocument.uri → 跳过该条（433），其余照收。
#[test]
fn parse_workspace_edit_skips_entries_without_uri() {
    let result = serde_json::json!({
        "documentChanges": [
            {"textDocument": {"version": 1}, "edits": [
                {"range": {"start": {"line": 0, "character": 0},
                           "end": {"line": 0, "character": 1}},
                 "newText": "x"}
            ]},
            {"textDocument": {"uri": "file:///tmp/a.rs"}, "edits": [
                {"range": {"start": {"line": 1, "character": 2},
                           "end": {"line": 1, "character": 3}},
                 "newText": "替换"}
            ]}
        ]
    });
    let out = parse_workspace_edit(&result);
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out[0].path.contains("a.rs"), "{:?}", out[0].path);
    assert_eq!(out[0].edits.len(), 1);
    assert_eq!(out[0].edits[0].new_text, "替换");
    assert_eq!(
        (out[0].edits[0].range_start.0, out[0].edits[0].range_start.1),
        (1, 2)
    );
}
