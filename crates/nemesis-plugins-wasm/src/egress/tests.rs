use super::*;
use std::collections::BTreeMap;

fn policy(entries: &[&str], allow_private: bool) -> EgressPolicy {
    EgressPolicy::from_permissions(
        entries.iter().map(|s| s.to_string()).collect(),
        allow_private,
    )
}

// -- 策略物化 -------------------------------------------------------------

#[test]
fn from_permissions_empty_disables() {
    let p = policy(&[], false);
    assert!(!p.enabled);
    assert!(p.allowlist.is_empty());
    assert!(!p.allow_private);
    assert!(p.proxy.is_none());
    // deny-by-default：能力关 = 请求直接拒绝。
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt
        .block_on(proxy_request(
            &p,
            &PluginLimits::default(),
            EgressRequest {
                method: "GET".into(),
                url: "http://api.example.com/".into(),
                headers: vec![],
                body: None,
            },
        ))
        .err()
        .unwrap_or_else(|| panic!("denied"));
    assert!(err.contains("egress not granted"));
}

#[test]
fn from_permissions_lowercases_entries() {
    let p = policy(&["API.Example.COM"], false);
    assert!(p.enabled);
    assert_eq!(p.allowlist, vec!["api.example.com"]);
}

// -- token 渲染 ------------------------------------------------------------

#[test]
fn render_tokens_literals_pass_through_and_malformed_dropped() {
    let cfg = BTreeMap::new();
    let out = render_egress_tokens(
        &[
            "api.example.com".to_string(),
            "{{broken".to_string(),
            "prefix-{{config.k}}".to_string(),
            "{{notconfig.k}}".to_string(),
        ],
        &cfg,
    );
    assert_eq!(out, vec!["api.example.com".to_string()]);
}

#[test]
fn render_tokens_resolves_config_values() {
    let mut cfg = BTreeMap::new();
    cfg.insert("host".to_string(), " self-hosted.example.com ".to_string());
    cfg.insert("url".to_string(), "http://box.lan:9000".to_string());
    cfg.insert("url_no_port".to_string(), "https://box2.lan".to_string());
    cfg.insert("empty".to_string(), "  ".to_string());
    let out = render_egress_tokens(
        &[
            "{{config.host}}".to_string(),
            "{{config.url}}".to_string(),
            "{{config.url_no_port}}".to_string(),
            "{{config.empty}}".to_string(),
            "{{config.ghost}}".to_string(),
            "{{config.ipv6}}".to_string(),
            "{{config.badurl}}".to_string(),
        ],
        &cfg,
    );
    // 普通值 trim 后整条目；URL 形态提取 host[:port]；空值/缺席丢弃。
    assert_eq!(
        out,
        vec![
            "self-hosted.example.com".to_string(),
            "box.lan:9000".to_string(),
            "box2.lan".to_string()
        ]
    );
    // IPv6 带显式端口（host 含冒号）= 条目语法无法表达 → 丢弃从严。
    let mut cfg6 = BTreeMap::new();
    cfg6.insert("ipv6".to_string(), "http://[::1]:8080".to_string());
    cfg6.insert("badurl".to_string(), "://not a url".to_string());
    let out = render_egress_tokens(
        &[
            "{{config.ipv6}}".to_string(),
            "{{config.badurl}}".to_string(),
        ],
        &cfg6,
    );
    assert!(out.is_empty());
}

// -- 私网判定（边界回归钉死面）---------------------------------------------

#[test]
fn is_private_ip_v4_segments() {
    use std::net::Ipv4Addr;
    for ip in [
        Ipv4Addr::LOCALHOST,               // 127/8
        Ipv4Addr::new(10, 0, 0, 1),        // 10/8
        Ipv4Addr::new(172, 16, 0, 1),      // 172.16/12
        Ipv4Addr::new(192, 168, 1, 1),     // 192.168/16
        Ipv4Addr::new(169, 254, 3, 4),     // link-local
        Ipv4Addr::new(255, 255, 255, 255), // broadcast
        Ipv4Addr::new(0, 0, 0, 0),         // unspecified
        Ipv4Addr::new(0, 1, 2, 3),         // 0.0.0.0/8 整段
        Ipv4Addr::new(192, 0, 2, 9),       // documentation 192.0.2/24
        Ipv4Addr::new(198, 51, 100, 7),    // documentation
        Ipv4Addr::new(203, 0, 113, 5),     // documentation
        Ipv4Addr::new(100, 64, 1, 1),      // CGNAT 100.64/10
        Ipv4Addr::new(100, 127, 255, 255), // CGNAT 上界
    ] {
        assert!(is_private_ip(std::net::IpAddr::V4(ip)), "{ip}");
    }
    // 公网段不误伤。
    for ip in [
        Ipv4Addr::new(8, 8, 8, 8),
        Ipv4Addr::new(1, 1, 1, 1),
        Ipv4Addr::new(100, 128, 0, 1),
        Ipv4Addr::new(100, 63, 255, 255),
    ] {
        assert!(!is_private_ip(std::net::IpAddr::V4(ip)), "{ip}");
    }
}

#[test]
fn is_private_ip_v6_segments_and_nested_v4() {
    use std::net::Ipv6Addr;
    assert!(is_private_ip(std::net::IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert!(is_private_ip(std::net::IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    assert!(is_private_ip(std::net::IpAddr::V6(Ipv6Addr::new(
        0xfc00, 0, 0, 0, 0, 0, 0, 1
    ))));
    assert!(is_private_ip(std::net::IpAddr::V6(Ipv6Addr::new(
        0xfe80, 0, 0, 0, 0, 0, 0, 1
    ))));
    assert!(is_private_ip(std::net::IpAddr::V6(Ipv6Addr::new(
        0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
    ))));
    // v4-mapped 内嵌私网 → 归一 v4 判定。
    assert!(is_private_ip(std::net::IpAddr::V6(
        "::ffff:192.168.1.1".parse().expect("v4-mapped")
    )));
    // v4-mapped 内嵌公网 → 不判私。
    assert!(!is_private_ip(std::net::IpAddr::V6(
        "::ffff:8.8.8.8".parse().expect("v4-mapped")
    )));
    // NAT64 内嵌私网/公网。
    assert!(is_private_ip(std::net::IpAddr::V6(
        "64:ff9b::192.168.0.1".parse().expect("nat64")
    )));
    assert!(!is_private_ip(std::net::IpAddr::V6(
        "64:ff9b::808:808".parse().expect("nat64 8.8.8.8")
    )));
    // 公网 v6。
    assert!(!is_private_ip(std::net::IpAddr::V6(Ipv6Addr::new(
        0x2606, 0x4700, 0, 0, 0, 0, 0, 4700
    ))));
}

// -- allowlist 匹配（条目三分形态）-----------------------------------------

#[test]
fn allowlisted_exact_and_case_insensitive() {
    let p = policy(&["api.example.com"], false);
    assert!(allowlisted(&p, "api.example.com", 443));
    assert!(allowlisted(&p, "API.example.COM", 80));
    assert!(!allowlisted(&p, "evil.example.com", 443));
    // 子域名不泛化。
    assert!(!allowlisted(&p, "x.api.example.com", 443));
}

#[test]
fn allowlisted_pinned_ports() {
    let p = policy(&["api.example.com:8443"], false);
    assert!(allowlisted(&p, "api.example.com", 8443));
    assert!(!allowlisted(&p, "api.example.com", 443));
}

#[test]
fn allowlisted_ipv6_forms() {
    // bracketed 钉端口 / bracketed 任意端口 / 裸 IPv6 任意端口。
    let p = policy(&["[::1]:8080", "[2001:db8::9]", "::1"], false);
    assert!(allowlisted(&p, "::1", 8080));
    assert!(allowlisted(&p, "::1", 9999)); // 裸条目任意端口
    assert!(allowlisted(&p, "2001:db8::9", 1234)); // bracketed 任意端口
    assert!(allowlisted(&p, "2001:db8::9", 8080)); // 同上：任意端口都命中
    // bracketed 畸形条目：整串比较不命中（不 panic）。
    let p = policy(&["[broken", "[::1]junk"], false);
    assert!(!allowlisted(&p, "::1", 8080));
}

#[test]
fn allowlisted_port_parse_failure_does_not_shortcircuit() {
    // 端口 >u16 解析失败：该条目不命中也不短路——后续裸条目照常尝试。
    let p = policy(&["api.example.com:99999", "api.example.com"], false);
    assert!(allowlisted(&p, "api.example.com", 80));
    // host:port 与裸 IPv6 混合：`::1:8080` 整体是合法地址 → 裸处理。
    let p = policy(&["::1:8080"], false);
    assert!(allowlisted(&p, "::1:8080", 443));
    assert!(!allowlisted(&p, "::1", 8080));
}

// -- 请求侧闸（错误臂全覆盖；成功臂走本地 TCP 服务）-------------------------

fn req(method: &str, url: &str) -> EgressRequest {
    EgressRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![],
        body: None,
    }
}

#[test]
fn proxy_request_rejects_bad_url_and_scheme_and_nonallowlisted() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let p = policy(&["api.example.com"], false);
    rt.block_on(async {
        let err = proxy_request(&p, &PluginLimits::default(), req("GET", "not a url"))
            .await
            .err()
            .unwrap_or_else(|| panic!("bad url"));
        assert!(err.contains("bad url"));
        let err = proxy_request(
            &p,
            &PluginLimits::default(),
            req("GET", "ftp://api.example.com/x"),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("bad scheme"));
        assert!(err.contains("scheme not allowed"));
        let err = proxy_request(
            &p,
            &PluginLimits::default(),
            req("GET", "http://other.example.com/"),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("not allowlisted"));
        assert!(err.contains("host not in egress allowlist"));
    });
}

#[test]
fn proxy_request_rejects_private_target_when_not_allowed() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // allowlist 钉 127.0.0.1 但 allow_private=false → 私网闸拦截。
    let p = policy(&["127.0.0.1"], false);
    rt.block_on(async {
        let err = proxy_request(
            &p,
            &PluginLimits::default(),
            req("GET", "http://127.0.0.1:1/x"),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("private target"));
        assert!(err.contains("private address"), "{err}");
    });
}

/// 本地 HTTP 服务：回一行 HTTP/1.1 200 + Content-Length + body。
fn spawn_http_server(body: &'static [u8]) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::task::spawn_blocking(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut s = stream;
            use std::io::{Read, Write};
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body);
        }
    });
    (addr, handle)
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_request_roundtrip_local_server() {
    let body: &'static [u8] = b"hello egress";
    let (addr, server) = spawn_http_server(body);
    let entry = format!("127.0.0.1:{}", addr.port());
    let p = policy(&[entry.as_str()], true);
    let mut limits = PluginLimits::default();
    limits.egress_timeout_ms = 5_000;
    let resp = proxy_request(
        &p,
        &limits,
        EgressRequest {
            method: "post".into(), // 小写方法 → 大写归一
            url: format!("http://127.0.0.1:{}/data", addr.port()),
            headers: vec![
                ("X-Custom".to_string(), "v1".to_string()),
                // 保留头：宿主不允许 guest 覆写（发出去也必须被剥掉）。
                ("Connection".to_string(), "evil".to_string()),
            ],
            body: Some(b"payload".to_vec()),
        },
    )
    .await
    .expect("roundtrip");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, body);
    assert!(
        resp.headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "text/plain")
    );
    // 显式 User-Agent 头不被默认覆盖（无 UA 时宿主默认补——两形态都合法）。
    server.await.expect("server joins");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_request_truncates_oversize_declared_body() {
    // 声明超限 Content-Length → 直接丢弃响应体（status/headers 照常）。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let big: &'static u64 = Box::leak(Box::new(
        u64::try_from(PluginLimits::default().egress_body_max_bytes).expect("usize fits") + 1,
    ));
    let liar = tokio::task::spawn_blocking(move || {
        if let Ok((stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut s = stream;
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head =
                format!("HTTP/1.1 200 OK\r\nContent-Length: {big}\r\nConnection: close\r\n\r\n");
            let _ = s.write_all(head.as_bytes());
        }
    });
    let mut limits = PluginLimits::default();
    limits.egress_timeout_ms = 5_000;
    let p = policy(&[format!("127.0.0.1:{}", addr.port()).as_str()], true);
    let resp = proxy_request(
        &p,
        &limits,
        req("GET", &format!("http://127.0.0.1:{}/big", addr.port())),
    )
    .await
    .expect("declared oversize handled");
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_empty(), "声明超限 → 丢弃响应体");
    liar.await.expect("server joins");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_request_truncates_streamed_oversize_body() {
    // chunked 编码实发超限字节（Content-Length 形态会被 reqwest 按声明
    // 截断流——无声明长度，流式读超限截断路径才可达）。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let limit = PluginLimits::default().egress_body_max_bytes;
    let liar = tokio::task::spawn_blocking(move || {
        if let Ok((stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut s = stream;
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Transfer-Encoding: chunked\r\n",
                "Connection: close\r\n\r\n",
            );
            let _ = s.write_all(head.as_bytes());
            let chunk = vec![b'x'; 64 * 1024];
            let mut sent = 0usize;
            while sent <= limit {
                let frame = format!("{:x}\r\n", chunk.len());
                if s.write_all(frame.as_bytes()).is_err()
                    || s.write_all(&chunk).is_err()
                    || s.write_all(b"\r\n").is_err()
                {
                    break;
                }
                sent += chunk.len();
            }
            let _ = s.write_all(b"0\r\n\r\n");
        }
    });
    let mut limits = PluginLimits::default();
    limits.egress_timeout_ms = 5_000;
    let p = policy(&[format!("127.0.0.1:{}", addr.port()).as_str()], true);
    let resp = proxy_request(
        &p,
        &limits,
        req("GET", &format!("http://127.0.0.1:{}/lie", addr.port())),
    )
    .await
    .expect("streamed oversize handled");
    assert_eq!(resp.status, 200);
    assert!(resp.body.is_empty(), "流式超限 → 已读丢弃 + 截断");
    liar.await.expect("server joins");
}

#[test]
fn proxy_request_invalid_proxy_url_fails() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut p = policy(&["api.example.com"], false);
    p.proxy = Some(":://not-a-proxy".to_string());
    rt.block_on(async {
        let err = proxy_request(
            &p,
            &PluginLimits::default(),
            req("GET", "http://api.example.com/"),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("invalid proxy"));
        assert!(err.contains("proxy"), "{err}");
    });
}

#[test]
fn proxy_request_dns_failure_is_honest_error() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    // RFC 6761 保留 TLD：解析必失败（即便意外解析到公网，请求本身也会失败
    // ——Err 断言在两条路径下都成立）。
    let p = policy(&["nonexistent.invalid"], false);
    rt.block_on(async {
        let err = proxy_request(
            &p,
            &PluginLimits::default(),
            req("GET", "http://nonexistent.invalid/x"),
        )
        .await
        .err()
        .unwrap_or_else(|| panic!("dns failure"));
        assert!(!err.is_empty());
    });
}
