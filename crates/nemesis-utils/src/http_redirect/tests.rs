use super::{MAX_REDIRECT_HOPS, cross_host_redirect_decision, origin_key};

#[test]
fn same_origin_follows() {
    assert!(cross_host_redirect_decision(
        Some("https://api.example.com:443"),
        Some("https://api.example.com:443"),
        0
    ));
    // 路径变化不影响 origin 判定（origin 键只含 scheme/host/port）。
    assert!(cross_host_redirect_decision(
        Some("https://api.example.com:443"),
        Some("https://api.example.com:443"),
        3
    ));
}

#[test]
fn cross_origin_stops() {
    // 跨主机。
    assert!(!cross_host_redirect_decision(
        Some("https://api.example.com:443"),
        Some("https://evil.example.net:443"),
        0
    ));
    // 同主机降级明文（https → http）——凭据下行面，拦。
    assert!(!cross_host_redirect_decision(
        Some("https://api.example.com:443"),
        Some("http://api.example.com:80"),
        0
    ));
    // 同主机不同端口——不同服务不同凭据接收面，拦。
    assert!(!cross_host_redirect_decision(
        Some("https://api.example.com:443"),
        Some("https://api.example.com:8443"),
        0
    ));
}

#[test]
fn missing_origin_fails_closed() {
    assert!(!cross_host_redirect_decision(
        None,
        Some("https://a.com:443"),
        0
    ));
    assert!(!cross_host_redirect_decision(
        Some("https://a.com:443"),
        None,
        0
    ));
    assert!(!cross_host_redirect_decision(None, None, 0));
}

#[test]
fn hop_cap_matches_reqwest_default() {
    // 与 reqwest 默认 Policy::limited(10) 同语义（BUG-20261119-191 钉死）：
    // hops = attempt.previous().len()，第 N 次判定 = N（含初始 URL）。
    // reqwest limited 判定是 `len() > max` → 第 10 跳（hops=10）放行，
    // 第 11 次判定（hops=11）终停。旧形态 `hops >= 10` 只放行 9 跳（差一跳）。
    assert!(cross_host_redirect_decision(
        Some("https://a.com:443"),
        Some("https://a.com:443"),
        MAX_REDIRECT_HOPS - 1
    ));
    assert!(cross_host_redirect_decision(
        Some("https://a.com:443"),
        Some("https://a.com:443"),
        MAX_REDIRECT_HOPS
    ));
    assert!(!cross_host_redirect_decision(
        Some("https://a.com:443"),
        Some("https://a.com:443"),
        MAX_REDIRECT_HOPS + 1
    ));
}

#[test]
fn origin_key_covers_default_ports() {
    let u = reqwest::Url::parse("https://api.example.com").unwrap();
    assert_eq!(
        origin_key(&u).as_deref(),
        Some("https://api.example.com:443")
    );
    let u = reqwest::Url::parse("http://api.example.com:8080/x").unwrap();
    assert_eq!(
        origin_key(&u).as_deref(),
        Some("http://api.example.com:8080")
    );
    // 无 host（如 file:///x）→ None → fail-closed。
    let u = reqwest::Url::parse("mailto:x@y.z").unwrap();
    assert_eq!(origin_key(&u), None);
}
