//! J2a（2026-09-04）：web_fetch 手动重定向循环 + SSRF 逐跳复查测试。
//!
//! 用 raw TCP 服务器手写 HTTP/1.1 响应（不经任何 HTTP 框架），覆盖：
//! - ≤5 跳重定向跟随 + 最终 URL 报告（无闸路径）
//! - 自循环重定向在跳数上限处诚实拒绝
//! - 相对 Location 解析（`Url::join`）
//! - 非成功状态在**最终** URL 上报错（旧实现报首跳 URL）
//! - security feature：SSRF 闸先于任何连接执行（拦截后连接数 = 0）
//! - security feature：guard 白名单短路 + 回环 IP 拦截的双语义
//!   （完整 socket 链路无法经 `SecurityPlugin` 构造——其 config 不透传
//!   allowed_hosts 且无 setter，故对 `Guard` 直接断言输入输出）

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::WebFetchTool;
use crate::context::RequestContext;
use crate::r#loop::Tool;

// ---------------------------------------------------------------------------
// raw HTTP/1.1 测试服务器
// ---------------------------------------------------------------------------

/// 按路径返回预置响应的 raw 服务器。返回 `(base_url, 连接计数)`。
fn spawn_raw_server(routes: Vec<(&'static str, String)>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind raw test server");
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let routes: HashMap<String, String> = routes
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    let c2 = counter.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            c2.fetch_add(1, Ordering::SeqCst);
            // 读到请求头结束（\r\n\r\r\n）为止；测试请求都无 body 且很小。
            let mut buf = [0u8; 4096];
            let mut got = 0;
            loop {
                match stream.read(&mut buf[got..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        got += n;
                        if buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                        if got == buf.len() {
                            break;
                        }
                    }
                }
            }
            let req = String::from_utf8_lossy(&buf[..got]);
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let route = path.split('?').next().unwrap_or("/").to_string();
            let resp = routes.get(&route).cloned().unwrap_or_else(not_found);
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{}", port), counter)
}

fn redirect(to: &str) -> String {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        to
    )
}

fn ok(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn not_found() -> String {
    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
}

async fn run_fetch(tool: &WebFetchTool, url: &str) -> Result<String, String> {
    let ctx = RequestContext::new("web", "chat1", "user1", "sess1");
    tool.execute(&serde_json::json!({ "url": url }).to_string(), &ctx)
        .await
}

// ---------------------------------------------------------------------------
// 无闸路径（重定向循环行为）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn follows_redirect_chain_within_limit() {
    let (base, counter) = spawn_raw_server(vec![("/a", redirect("/b")), ("/b", ok("J2A_FINAL"))]);
    let tool = WebFetchTool::new(50000);
    let out = run_fetch(&tool, &format!("{}/a", base))
        .await
        .expect("chain within limit should succeed");
    assert!(out.contains("Content from"), "got: {out}");
    assert!(
        out.contains(&format!("{}/b", base)),
        "should report the final URL, got: {out}"
    );
    assert!(out.contains("J2A_FINAL"), "got: {out}");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn rejects_self_redirect_loop_at_hop_limit() {
    let (base, counter) = spawn_raw_server(vec![("/x", redirect("/x"))]);
    let tool = WebFetchTool::new(50000);
    let err = run_fetch(&tool, &format!("{}/x", base))
        .await
        .expect_err("self loop must be rejected");
    assert!(err.contains("too many redirects"), "got: {err}");
    // 首跳 + 5 次跟随 = 6 个连接，第 6 个 302 触发上限拒绝。
    assert_eq!(counter.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn resolves_relative_location_header() {
    let (base, counter) =
        spawn_raw_server(vec![("/r", redirect("/abs")), ("/abs", ok("REL_BODY"))]);
    let tool = WebFetchTool::new(50000);
    let out = run_fetch(&tool, &format!("{}/r", base))
        .await
        .expect("relative Location should resolve against current URL");
    assert!(out.contains("REL_BODY"), "got: {out}");
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn http_error_reported_at_final_url() {
    let (base, _counter) = spawn_raw_server(vec![("/e", redirect("/f")), ("/f", not_found())]);
    let tool = WebFetchTool::new(50000);
    let err = run_fetch(&tool, &format!("{}/e", base))
        .await
        .expect_err("404 at the final hop must surface");
    assert!(err.contains("HTTP 404 Not Found"), "got: {err}");
    assert!(
        err.contains(&format!("{}/f", base)),
        "error should cite the final URL, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// security 路径（SSRF 闸逐跳复查）
// ---------------------------------------------------------------------------

#[cfg(feature = "security")]
mod ssrf_gate_tests {
    use super::*;

    fn plugin() -> Arc<nemesis_security::pipeline::SecurityPlugin> {
        Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
            nemesis_security::pipeline::SecurityPluginConfig {
                ssrf_enabled: true,
                ..Default::default()
            },
        ))
    }

    #[tokio::test]
    async fn ssrf_blocks_loopback_first_hop_before_connecting() {
        let (base, counter) = spawn_raw_server(vec![("/a", ok("SHOULD_NEVER_ARRIVE"))]);
        let mut tool = WebFetchTool::new(50000);
        tool.ssrf = Some(plugin());
        let err = run_fetch(&tool, &format!("{}/a", base))
            .await
            .expect_err("loopback first hop must be blocked");
        assert!(err.contains("SSRF blocked"), "got: {err}");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "gate must run before any connection is made"
        );
    }

    /// 白名单放行首跳 + 回环重定向目标被拦：hop_decision 的两个 guard 输入
    /// 语义直接对 `Guard` 断言（`SecurityPlugin` 无法构造带白名单的 guard）。
    /// 「hop_client 每跳真的调用了 guard」由 ssrf_blocks_loopback_first_hop
    /// 证明——两者组合即闭合「白名单首跳 → 回环目标被拦」的绕闸漏洞语义。
    #[test]
    fn guard_allowlist_semantics_behind_hop_client() {
        let guard = nemesis_security::ssrf::Guard::new(nemesis_security::ssrf::SsrfConfig {
            allowed_hosts: vec!["localhost".to_string()],
            ..Default::default()
        })
        .expect("default config parses");
        // 白名单 host 短路放行（首跳 localhost 会走的分支，含绕过回环检查）。
        assert!(
            guard
                .resolve_and_validate_collect("http://localhost:1/start")
                .is_ok(),
            "allowlisted host must short-circuit to allowed"
        );
        // 回环 IP 直 parse 被拦（重定向目标 127.0.0.1 会走的分支）。
        assert!(
            guard
                .resolve_and_validate_collect("http://127.0.0.1:1/deny")
                .is_err(),
            "loopback IP must be blocked"
        );
    }
}

// ---------------------------------------------------------------------------
// J2b (2026-09-06)：HTML 提取（html2text）+ spill 对齐截断
// ---------------------------------------------------------------------------

/// 从回灌文本中抠出 spill 文件路径（locator 行形如
/// `[内容过大已完整保存到：<path>。可用 read_file …]`——路径以句号结尾）。
fn j2b_locator_path(reply: &str) -> String {
    reply
        .lines()
        .find_map(|l| l.split("已完整保存到：").nth(1))
        .and_then(|rest| rest.split('。').next())
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .expect("locator path in reply")
}

/// text/html 形态的 200 响应。
fn ok_html(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK
Content-Type: text/html; charset=utf-8
Content-Length: {}
Connection: close

{}",
        body.len(),
        body
    )
}

#[test]
fn j2b_looks_like_html_detection_matrix() {
    let body = b"<html><body>x</body></html>";
    // Content-Type 命中（body 无关）。
    assert!(super::looks_like_html("text/html; charset=utf-8", body));
    // Content-Type 未标明 / 撒谎 → 内容前缀兜底（大小写不敏感）。
    assert!(super::looks_like_html("", body));
    assert!(super::looks_like_html(
        "application/octet-stream",
        b"<!DOCTYPE html>..."
    ));
    assert!(super::looks_like_html(
        "application/octet-stream",
        b"  <HTML>"
    ));
    // 纯文本 / JSON 不误判。
    assert!(!super::looks_like_html("application/json", b"{\"a\":1}"));
    assert!(!super::looks_like_html(
        "text/plain",
        b"just words <not html>"
    ));
    assert!(!super::looks_like_html("", b""));
}

#[test]
fn j2b_extract_html_strips_chrome_and_keeps_structure() {
    let page = concat!(
        "<html><head><style>.x{color:red}</style>",
        "<script>var tracked_script = 1;</script></head>",
        "<body><h1>Title Here</h1>",
        "<ul><li>alpha item</li><li>beta item</li></ul>",
        "<p>See <a href=\"https://example.com/docs\">Docs</a> &amp; more.</p>",
        "</body></html>"
    );
    let out = super::extract_html_text(page);
    assert!(out.contains("Title Here"), "got: {out}");
    // script/style 内容剥除。
    assert!(!out.contains("tracked_script"), "got: {out}");
    assert!(!out.contains("color:red"), "got: {out}");
    // 列表结构保留（两行分立，不再压成单行空格串）。
    let alpha_line = out
        .lines()
        .find(|l| l.contains("alpha item"))
        .expect("alpha line");
    let beta_line = out
        .lines()
        .find(|l| l.contains("beta item"))
        .expect("beta line");
    assert_ne!(
        alpha_line, beta_line,
        "list items must be separate lines: {out}"
    );
    // 链接 URL 保留（旧 regex 管线会把 URL 完全丢掉）。
    assert!(out.contains("example.com"), "got: {out}");
    // 实体解码。
    assert!(out.contains('&'), "got: {out}");
}

#[tokio::test]
async fn j2b_oversize_text_spills_full_and_previews() {
    let ws = tempfile::tempdir().expect("tempdir");
    let body = format!(
        "PREVIEW_MARKER_HEAD\n{}\nTAIL_MARKER_END",
        "X".repeat(60000)
    );
    let (base, _c) = spawn_raw_server(vec![("/big", ok(&body))]);
    let tool = WebFetchTool::new(50000).with_workspace(ws.path().to_str().unwrap());
    let out = run_fetch(&tool, &format!("{}/big", base))
        .await
        .expect("fetch should succeed");

    // 回灌 = preview + locator；尾部内容不进回灌。
    assert!(out.contains("PREVIEW_MARKER_HEAD"), "got (tail): {out}");
    assert!(
        !out.contains("TAIL_MARKER_END"),
        "preview must not include tail"
    );
    assert!(out.contains("已完整保存到"), "got (tail): {out}");
    assert!(out.contains("read_file"), "locator hint missing");

    // 全文落盘（spill 文件含头尾标记；路径来自回灌文本）。
    let path = j2b_locator_path(&out);
    let archived = std::fs::read_to_string(&path).expect("spill file readable");
    assert!(archived.contains("PREVIEW_MARKER_HEAD"), "head missing");
    assert!(
        archived.contains("TAIL_MARKER_END"),
        "full text missing tail"
    );
}

#[tokio::test]
async fn j2b_oversize_without_workspace_falls_back_to_truncate() {
    let body = "Y".repeat(60000);
    let (base, _c) = spawn_raw_server(vec![("/big", ok(&body))]);
    let tool = WebFetchTool::new(4096); // 无 workspace → 旧「截断+注记」
    let out = run_fetch(&tool, &format!("{}/big", base))
        .await
        .expect("fetch should succeed");
    assert!(out.contains("truncated to 4096 bytes"), "got (tail): {out}");
    assert!(!out.contains("已完整保存到"), "no spill without workspace");
}

#[tokio::test]
async fn j2b_under_limit_reply_has_no_spill_note() {
    let (base, _c) = spawn_raw_server(vec![("/small", ok("hello world"))]);
    let tool = WebFetchTool::new(50000).with_workspace("/nonexistent-ws-j2b");
    let out = run_fetch(&tool, &format!("{}/small", base))
        .await
        .expect("fetch should succeed");
    assert!(
        out.contains(
            "bytes, text/plain):
hello world"
        ),
        "under-limit wording changed, got: {out}"
    );
    assert!(!out.contains("已完整保存到"));
}

#[tokio::test]
async fn j2b_html_oversize_extracts_then_spills_clean_text() {
    let ws = tempfile::tempdir().expect("tempdir");
    let para =
        "<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor.</p>"
            .repeat(220);
    let page = format!(
        "<html><head><script>var hidden_from_reply = 1;</script></head><body><h1>Big Page</h1>{para}<a href=\"https://example.com/ref\">ref</a></body></html>"
    );
    let (base, _c) = spawn_raw_server(vec![("/page", ok_html(&page))]);
    let tool = WebFetchTool::new(4096).with_workspace(ws.path().to_str().unwrap());
    let out = run_fetch(&tool, &format!("{}/page", base))
        .await
        .expect("fetch should succeed");

    // 提取生效：script 不出现在回灌，正文出现在回灌。
    assert!(!out.contains("hidden_from_reply"), "got (tail): {out}");
    assert!(out.contains("Big Page"), "got (tail): {out}");
    // 超限 → spill。
    assert!(out.contains("已完整保存到"), "got (tail): {out}");
    let path = j2b_locator_path(&out);
    let archived = std::fs::read_to_string(&path).expect("spill file readable");
    assert!(archived.contains("Big Page"));
    assert!(archived.contains("Lorem ipsum"));
    assert!(
        archived.contains("example.com"),
        "link URL kept in extraction"
    );
    assert!(
        !archived.contains("hidden_from_reply"),
        "script must stay out of spill"
    );
}
