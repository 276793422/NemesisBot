//! cdp 纯函数单测：状态行/Content-Length 解析、端口文件解析、target 投影。

use super::{extract_content_length, page_targets, parse_status_line, read_devtools_active_port};
use std::path::Path;

// --- parse_status_line ---

#[test]
fn status_line_ok() {
    let (code, reason) = parse_status_line("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n").unwrap();
    assert_eq!(code, 200);
    assert_eq!(reason, "OK");
}

#[test]
fn status_line_multi_word_reason() {
    let (code, reason) =
        parse_status_line("HTTP/1.1 505 HTTP Version Not Supported\r\n\r\n").unwrap();
    assert_eq!(code, 505);
    assert_eq!(reason, "HTTP Version Not Supported");
}

#[test]
fn status_line_no_code() {
    assert!(parse_status_line("HTTP/1.1\r\n\r\n").is_err());
    assert!(parse_status_line("").is_err());
}

// --- extract_content_length ---

#[test]
fn content_length_present() {
    assert_eq!(
        extract_content_length("HTTP/1.1 200 OK\r\nContent-Length: 1234\r\n\r\n").unwrap(),
        1234
    );
    // 小写头名兼容（chromium 实发规范大小写，防御形态）。
    assert_eq!(
        extract_content_length("HTTP/1.1 200 OK\r\ncontent-length: 7\r\n\r\n").unwrap(),
        7
    );
}

#[test]
fn content_length_missing_is_err() {
    // 缺席 = 诚实报错（不做 EOF 猜测——chromium DevTools 端点恒带）。
    assert!(extract_content_length("HTTP/1.1 200 OK\r\nServer: x\r\n\r\n").is_err());
    assert!(extract_content_length("Content-Length: notanumber\r\n\r\n").is_err());
}

// --- F43 响应体预分配上限 ---

#[test]
fn http_body_cap_rejects_overshoot_semantics() {
    // F43（2026-11-16 八审）：cdp_http_raw 按 Content-Length 一次性预分配
    // body——谎报的端点可触发巨量分配。上限 16MiB（正常 /json/list 数百
    // KB 的 50 倍余量）；超限语义 = 诚实报错不分配（检查在 cdp_http_raw
    // 内，此处锁常量量级防手滑改成 usize::MAX）。
    assert_eq!(super::HTTP_BODY_CAP, 16 * 1024 * 1024);
}

// --- read_devtools_active_port ---

#[test]
fn port_file_parse_ok() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("DevToolsActivePort");
    std::fs::write(&p, "41017\n/devtools/browser/abc-123\n").unwrap();
    let (port, ws) = read_devtools_active_port(dir.path()).unwrap();
    assert_eq!(port, 41017);
    assert_eq!(ws, "/devtools/browser/abc-123");
}

#[test]
fn port_file_missing_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_devtools_active_port(dir.path()).is_none());
    assert!(read_devtools_active_port(Path::new("Z:/definitely/not/here")).is_none());
}

#[test]
fn port_file_zero_is_none() {
    // 端口 0 = 未就绪（--remote-debugging-port=0 写文件前不可采信）。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("DevToolsActivePort");
    std::fs::write(&p, "0\n/devtools/browser/x\n").unwrap();
    assert!(read_devtools_active_port(dir.path()).is_none());
}

#[test]
fn port_file_garbage_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("DevToolsActivePort");
    std::fs::write(&p, "not-a-port\n").unwrap();
    assert!(read_devtools_active_port(dir.path()).is_none());
}

// --- page_targets ---

#[test]
fn page_targets_filters_to_pages() {
    let list: serde_json::Value = serde_json::from_str(
        r#"[
        {"id": "t1", "type": "page", "title": "首页", "url": "http://a/", "attached": false},
        {"id": "t2", "type": "iframe", "title": "f", "url": "http://b/", "attached": false},
        {"id": "t3", "type": "page", "title": "b", "url": "http://c/", "attached": true}
    ]"#,
    )
    .unwrap();
    let tabs = page_targets(&list);
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0].id, "t1");
    assert_eq!(tabs[0].title, "首页");
    assert!(!tabs[0].attached);
    assert_eq!(tabs[1].id, "t3");
    assert!(tabs[1].attached);
}

#[test]
fn page_targets_non_array_is_empty() {
    assert!(page_targets(&serde_json::json!({"x": 1})).is_empty());
    assert!(page_targets(&serde_json::Value::Null).is_empty());
}
