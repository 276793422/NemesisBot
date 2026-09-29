//! 出站 HTTP 代理（deny-by-default）。
//!
//! 安全形态（对齐宿主 SSRF 闸精神）：
//! - manifest `permissions.egress` 无 allowlist → 能力整体关闭（no-permission）；
//! - allowlist **精确域名匹配**（大小写不敏感；`host:port` 形态钉端口，
//!   无端口 = 任意端口；不做子域名泛化——通配必须显式逐条列出）；
//! - `allow-private` 默认 false：解析出的 IP 落私网/回环/链路本地/ULA → 拒绝；
//! - **DNS 钉地址**：解析一次，reqwest `.resolve()` 钉死——请求期 DNS 重绑
//!   （rebinding）不成立；重定向 `Policy::none()`（跟重定向绕 allowlist 不成立）；
//! - 响应体超限（默认 1MB）截断丢弃（status/headers 照常回传 + budget-exceeded
//!   语义留待 v2 结构化；v1 截断即截断，诚实无注记通道——body 是字节面）。

use crate::limits::PluginLimits;

/// 出站请求（crate 内中立形态；两 world bindgen record 互转）。
#[derive(Debug, Clone)]
pub struct EgressRequest {
    /// HTTP 方法（GET/POST/…；非法值拒绝）。
    pub method: String,
    /// 绝对 URL（http/https）。
    pub url: String,
    /// 请求头（宿主追加 User-Agent；Authorization 里的 vault 值由 guest 负责）。
    pub headers: Vec<(String, String)>,
    /// 请求体。
    pub body: Option<Vec<u8>>,
}

/// 出站响应（中立形态）。
#[derive(Debug, Clone)]
pub struct EgressResponse {
    /// 状态码。
    pub status: u16,
    /// 响应头。
    pub headers: Vec<(String, String)>,
    /// 响应体（超限已截断丢弃 → 空体；status 照常）。
    pub body: Vec<u8>,
}

/// 出站策略（从 manifest permissions 物化）。
#[derive(Debug, Clone, Default)]
pub struct EgressPolicy {
    /// 能力总开关（allowlist 非空才开）。
    pub enabled: bool,
    /// 精确域名/域名:端口 allowlist。
    pub allowlist: Vec<String>,
    /// 允许私网/回环目标（默认 false；测试场景对 127.0.0.1 显式开）。
    pub allow_private: bool,
}

impl EgressPolicy {
    /// 从 manifest permissions 构建（allowlist 空 = enabled=false）。
    #[must_use]
    pub fn from_permissions(allowlist: Vec<String>, allow_private: bool) -> Self {
        let enabled = !allowlist.is_empty();
        Self {
            enabled,
            allowlist: allowlist.into_iter().map(|s| s.to_lowercase()).collect(),
            allow_private,
        }
    }
}

/// 判断 IP 是否私网段（allow_private=false 时拒绝目标）。
///
/// pub：私网判定原语（v4-mapped/NAT64 归一、ULA/link-local/CGNAT 等段表）
/// 独立成面供边界测试钉死——绕过面回归（H1）不允许只靠 e2e 兜。
#[must_use]
pub fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_private_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // IPv4-mapped（::ffff:0:0/96）与 NAT64（64:ff9b::/96）内嵌的
            // IPv4 归一回 v4 判定——DNS 返回 v4-mapped AAAA 时裸 v6 检查
            // 五项全不命中（非 loopback/ULA/link-local），是私网闸的真实
            // 绕过面（2026-09-29 交付审查 H1，rebinding 威胁模型内）。
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(v4);
            }
            let seg = v6.segments();
            if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6].iter().all(|s| *s == 0) {
                return is_private_v4(std::net::Ipv4Addr::new(
                    (seg[6] >> 8) as u8,
                    seg[6] as u8,
                    (seg[7] >> 8) as u8,
                    seg[7] as u8,
                ));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (seg[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation 2001:db8::/32
        }
    }
}

/// IPv4 私网段（含 0.0.0.0/8「本网络」保留段——不只是单址 0.0.0.0）。
fn is_private_v4(v4: std::net::Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_unspecified()
        || v4.is_documentation()
        || v4.octets()[0] == 0 // 0.0.0.0/8
        || v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64 // CGNAT 100.64/10
        || v4.octets()[0] == 169 && v4.octets()[1] == 254
}

/// allowlist 精确匹配（host 大小写不敏感；条目可带 :port 钉端口）。
fn allowlisted(policy: &EgressPolicy, host: &str, port: u16) -> bool {
    let host_lc = host.to_lowercase();
    for entry in &policy.allowlist {
        if let Some((eh, ep)) = entry.rsplit_once(':')
            && ep.chars().all(|c| c.is_ascii_digit())
            && !eh.contains(':')
        {
            if let Ok(p) = ep.parse::<u16>()
                && eh == host_lc
                && p == port
            {
                return true;
            }
        } else if *entry == host_lc {
            return true;
        }
    }
    false
}

/// 执行一次代理请求（异步；宿主侧经 tokio Handle block_on 调用）。
pub(crate) async fn proxy_request(
    policy: &EgressPolicy,
    limits: &PluginLimits,
    req: EgressRequest,
) -> Result<EgressResponse, String> {
    proxy_request_inner(policy, limits, req).await
}

async fn proxy_request_inner(
    policy: &EgressPolicy,
    limits: &PluginLimits,
    req: EgressRequest,
) -> Result<EgressResponse, String> {
    if !policy.enabled {
        return Err("egress not granted (manifest permissions.egress empty)".into());
    }
    let url = url::Url::parse(&req.url).map_err(|e| format!("bad url: {e}"))?;
    let scheme = url.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("scheme not allowed: {scheme}"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| "url has no host".to_string())?
        .trim_matches(['[', ']'])
        .to_string();
    let port = url
        .port_or_known_default()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    if !allowlisted(policy, &host, port) {
        return Err(format!("host not in egress allowlist: {host}:{port}"));
    }

    // DNS 解析 + 私网闸 + 钉地址（rebinding 防御）。
    let addr_host = format!("{host}:{port}");
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&addr_host)
        .await
        .map_err(|e| format!("dns resolution failed for {host}: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err(format!("dns resolution returned no addresses for {host}"));
    }
    let mut pinned: Option<std::net::SocketAddr> = None;
    for a in &addrs {
        if !policy.allow_private && is_private_ip(a.ip()) {
            return Err(format!(
                "target resolves to private address (allow_private=false): {}",
                a.ip()
            ));
        }
        if pinned.is_none() {
            pinned = Some(*a);
        }
    }
    let pinned = pinned.expect("addrs non-empty");

    let method = reqwest::Method::from_bytes(req.method.to_uppercase().as_bytes())
        .map_err(|e| format!("bad method: {e}"))?;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_millis(limits.egress_timeout_ms))
        // DNS 钉地址：连接目标固定为解析结果，请求期 rebinding 不成立；
        // TLS SNI 仍用真实域名（resolve 只改连接目标），证书校验照常。
        .resolve(&host, pinned)
        .build()
        .map_err(|e| format!("client build: {e}"))?;

    let mut out = client.request(method, &req.url);
    for (k, v) in &req.headers {
        // 宿主保留头不允许 guest 覆写（连接语义头）。
        let kl = k.to_lowercase();
        if matches!(
            kl.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding" | "upgrade"
        ) {
            continue;
        }
        out = out.header(k, v);
    }
    if !req
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
    {
        out = out.header("User-Agent", "NemesisBot-WasmPlugin");
    }
    if let Some(body) = req.body {
        out = out.body(body);
    }

    let resp = out
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    // 响应体上限：先看 content-length，再流式读取超限截断。
    let declared = resp.content_length();
    if let Some(n) = declared
        && n > limits.egress_body_max_bytes as u64
    {
        return Ok(EgressResponse {
            status,
            headers,
            body: Vec::new(),
        });
    }
    let mut body = Vec::with_capacity(declared.unwrap_or(0).min(64 * 1024) as usize);
    let mut stream = resp.bytes_stream();
    use futures::StreamExt as _;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("body read: {e}"))?;
        if body.len() + chunk.len() > limits.egress_body_max_bytes {
            // 超限：丢弃已读 + 终止（诚实截断；status/headers 已回传）。
            body.clear();
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok(EgressResponse {
        status,
        headers,
        body,
    })
}
