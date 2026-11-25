//! Web tools - fetch URL content and search the web.
//!
//! Provides `WebFetchTool` for retrieving URL content via HTTP GET,
//! `WebSearchTool` for web search with multiple provider backends
//! (DuckDuckGo, Brave, Perplexity), and search provider traits.

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::time::Duration;

/// Default maximum response size (1 MB).
const DEFAULT_MAX_SIZE: usize = 1024 * 1024;

/// Default request timeout (30 seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Web fetch tool - retrieves URL content via HTTP GET.
pub struct WebFetchTool {
    /// HTTP client（pub(crate) 供 tests 子模块直连验证重定向策略——
    /// execute 全链的 validate_url 会拦一切本地 host，网络行为测试
    /// 只能绕开闸直测 client）。
    pub(crate) client: reqwest::Client,
    /// Maximum response size in bytes.
    max_size: usize,
    /// Request timeout.
    timeout: Duration,
}

impl WebFetchTool {
    /// Create a new web fetch tool with default settings.
    pub fn new() -> Result<Self, String> {
        Self::with_options(DEFAULT_TIMEOUT_SECS, DEFAULT_MAX_SIZE)
    }

    /// Create with custom timeout and size limit.
    ///
    /// 构建失败 fail-closed（BUG-20261124-014，对齐 BUG-20261123-028 横向扫
    /// 同文件 DDG/Brave/Perplexity 三 provider 先例）：不再
    /// `unwrap_or_default()` 静默回落默认 client——那会同时丢 30s 超时与
    /// `Policy::none()` 禁重定向策略，而 `validate_url` 只在首跳执行，
    /// 默认 client 跟随 302 → 内网即绕过 SSRF 首跳闸。
    pub fn with_options(timeout_secs: u64, max_size: usize) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("NemesisBot/1.0")
            // BUG-20261113-155：不跟随重定向。reqwest 默认 Policy 会静默跟随
            // 最多 10 跳，而 validate_url 只在首跳 URL 上执行——公网恶意站
            // 302 → http://127.0.0.1 / 169.254.169.254 即绕过 SSRF 闸。禁跟
            // 随后 3xx 原样返回，execute 把 Location 带进错误消息（见下方
            // 3xx 分支），模型自行 fetch 新地址时重新过闸——每一跳都校验。
            // 与 image_attach URL 预取 / plugins-wasm egress / http-pool /
            // release_fetch 等安全敏感出口的 Policy::none 惯例一致。
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| {
                format!(
                    "WebFetch client build failed (refusing default client \
                     without timeout + no-redirect guard): {e}"
                )
            })?;

        Ok(Self {
            client,
            max_size,
            timeout: Duration::from_secs(timeout_secs),
        })
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch the content of a URL via HTTP GET"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 30)"
                },
                "max_size": {
                    "type": "integer",
                    "description": "Maximum response size in bytes (default: 1048576)"
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let url = match args["url"].as_str() {
            Some(u) => u,
            None => return ToolResult::error("missing 'url' argument"),
        };

        // Basic URL validation
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return ToolResult::error("URL must start with http:// or https://");
        }

        // Check for SSRF - block private/local addresses
        if let Err(e) = validate_url(url) {
            return ToolResult::error(&e);
        }

        // Override timeout if specified
        let request_timeout = args["timeout"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(self.timeout);

        // Override max size if specified
        let max_size = args["max_size"].as_u64().unwrap_or(self.max_size as u64) as usize;

        // Fetch the URL
        let result = tokio::time::timeout(request_timeout, self.client.get(url).send()).await;

        match result {
            Ok(Ok(response)) => {
                let status = response.status();
                if !status.is_success() {
                    // 重定向禁跟随（BUG-20261113-155）：3xx 带 Location 回给
                    // 模型——它可自行 fetch 新地址，新 URL 重新过 validate_url
                    // 首跳闸；不回 Location 模型就看不到跳转去向。
                    if status.is_redirection()
                        && let Some(loc) = response
                            .headers()
                            .get(reqwest::header::LOCATION)
                            .and_then(|v| v.to_str().ok())
                    {
                        return ToolResult::error(&redirect_refetch_message(status, loc));
                    }
                    return ToolResult::error(&format!(
                        "HTTP {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown")
                    ));
                }

                // Read body with size limit.
                // BUG-20261107-005：旧形态 `bytes()` 全量读入内存后才核对
                // max_size——SSRF 闸只挡内网，公网恶意站可在核对前把任意
                // 大 body 灌进内存。换用共享流式累计闸，超限在读入中途即断。
                match nemesis_utils::http_cap::read_body_capped(response, max_size as u64).await {
                    Ok(bytes) => {
                        let content = String::from_utf8_lossy(&bytes);
                        ToolResult::success(&content)
                    }
                    Err(e) => ToolResult::error(&e),
                }
            }
            Ok(Err(e)) => ToolResult::error(&format!("request failed: {}", e)),
            Err(_) => ToolResult::error(&format!(
                "request timed out after {}s",
                request_timeout.as_secs()
            )),
        }
    }
}

/// 重定向禁跟随（BUG-20261113-155）的 3xx 错误消息：状态码 + 跳转去向
/// 一并回给模型（它可自行 fetch 新地址，新 URL 重新过 validate_url 首跳闸）。
fn redirect_refetch_message(status: reqwest::StatusCode, location: &str) -> String {
    format!(
        "HTTP {} {} (redirected, not followed — re-fetch this URL to continue: {})",
        status.as_u16(),
        status.canonical_reason().unwrap_or("Unknown"),
        location
    )
}

/// Validate URL to prevent SSRF attacks（字面量闸）。
///
/// 职责边界：URL 解析形态正确性 + **字面量**形态（IP 直接量 / 域名字面量）
/// 的内网段判定。诚实边界：域名解析到内网 IP（DNS rebinding / 私有 DNS /
/// mDNS .local）不在此闸能力内——那是 DNS 级闸的职责（nemesis-security
/// Layer 6 `ssrf::Guard::resolve_and_validate`，生产默认配置下 web_fetch 的
/// url 参数经 `extract_url` 臂过 Layer 6）。本闸价值：security.enabled=false
/// 降级配置下的唯一闸 + Layer 6 前的快速失败回显。
///
/// 修复历史（BUG-20261125-028）：旧实现自写极简解析器（`split('/').next()`
/// + `split(':').next()` 抠 host），六形态绕过：
///
/// - ① userinfo：`http://x@127.0.0.1/` → host 被抠成 `x@127.0.0.1`，blocked
///   表精确匹配永不命中，reqwest 真实解析后连的就是 127.0.0.1；
/// - ② IPv6 括号：`[::1]` → `split(':')` 得 `[`，blocked 表的 `[::1]` 永不
///   命中——旧 tests.rs 曾把此形态钉成「documented behavior」；
/// - ③ 非点分十进制 IPv4：`2130706433` / `0x7f000001` / `0177.0.0.1` 均语义
///   等价 127.0.0.1，字符串匹配全部放行；
/// - ④ 尾点域名：`localhost.` ≠ `localhost` 精确匹配放行；
/// - ⑤ link-local 段缺失：blocked 表只精确挡 `169.254.169.254`，段内其余
///   `169.254.0.0/16` 全放行；
/// - ⑥ 私有段误伤：`starts_with("172.2")`/`("172.3")` 把公网
///   `172.32.0.0/12` 之外的 `172.200/172.32-39` 等一并拦掉。
///
/// 现用 url crate 真实解析（WHATWG 语义）：userinfo 在解析期与 host 分离
/// （①结构性消灭）；IP 字面量规范化为 typed `Host::Ipv4/Ipv6`（②③消灭）；
/// 段判定走 std 分类语义 + resolver 同段位保留段（⑤⑥消灭）；域名走小写 +
/// 单尾点剥离后匹配（④消灭）。
fn validate_url(raw: &str) -> Result<(), String> {
    let parsed = url::Url::parse(raw).map_err(|e| format!("invalid URL: {}", e))?;

    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "scheme '{}' is not allowed (http/https only)",
            scheme
        ));
    }

    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => validate_ip_literal(std::net::IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => validate_ip_literal(std::net::IpAddr::V6(ip)),
        Some(url::Host::Domain(domain)) => {
            // WHATWG 允许尾点形态（"localhost." 与 "localhost" 同解析目标），
            // 单尾点剥离后匹配；多尾点是非法域名（url crate 解析期已拒）。
            let lower = domain.strip_suffix('.').unwrap_or(domain).to_lowercase();
            if lower == "localhost"
                || lower.ends_with(".localhost")
                || lower == "localhost.localdomain"
            {
                return Err(format!(
                    "access to '{}' is not allowed (SSRF protection)",
                    domain
                ));
            }
            Ok(())
        }
        None => Err("no host in URL".to_string()),
    }
}

/// IP 字面量内网段判定（与 nemesis-security `resolver.rs` 同段位的字面量
/// 闸子集；Layer 6 是权威 DNS 级闸，见 [`validate_url`] 职责边界）。
fn validate_ip_literal(ip: std::net::IpAddr) -> Result<(), String> {
    use std::net::IpAddr;

    // IPv4-mapped IPv6（:ffff:0:0/96）在双栈 socket 上按内嵌 IPv4 连接，
    // 必须先归一再判定（std is_loopback 对映射形态全漏）。
    let mut normalized = match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    };
    // RFC 4291 IPv4-compatible（::a.b.c.d）折叠为内嵌 IPv4（::7f00:1 语义
    // 上命名的就是 127.0.0.1）；:: 与 ::1 各由专属分类承担。
    if let IpAddr::V6(v6) = normalized
        && let Some(v4) = ipv4_compatible_of(v6)
    {
        normalized = IpAddr::V4(v4);
    }

    let blocked = match &normalized {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_private()
                || v4.is_link_local() // 169.254.0.0/16（含 metadata 169.254.169.254）
                || o[0] == 0 // 0.0.0.0/8 current network
                || o[0] >= 224 // multicast 224/4 + reserved 240/4 + broadcast
                || (o[0] == 100 && (64..=127).contains(&o[1])) // RFC 6598 CGNAT
                || (o[0] == 192 && o[1] == 0 && o[2] == 2) // RFC 5735 TEST-NET-1
                || (o[0] == 198 && o[1] == 51 && o[2] == 100) // TEST-NET-2
                || (o[0] == 203 && o[1] == 0 && o[2] == 113) // TEST-NET-3
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // RFC 2544 benchmarking
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || (s[0] & 0xfe00) == 0xfc00 // RFC 4193 ULA fc00::/7
                || (s[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || s[0] == 0x2002 // 6to4 2002::/16（内嵌 v4 隧道形态，从严整段封锁）
                || (s[0] == 0x0064 && s[1] == 0xff9b) // RFC 6052 NAT64 64:ff9b::/96
                || (s[0] == 0x2001 && s[1] == 0x0db8) // RFC 3849 documentation
        }
    };

    if blocked {
        Err(format!(
            "access to '{}' is not allowed (SSRF protection)",
            normalized
        ))
    } else {
        Ok(())
    }
}

/// RFC 4291 IPv4-compatible IPv6（前六组全零、尾两节非零）折叠为内嵌
/// IPv4（`::1` loopback 由 `is_loopback` 承担，显式排除；与
/// nemesis-security `resolver::ipv4_compatible_of` 同语义）。
fn ipv4_compatible_of(v6: std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let [a, b, c, d, e, f, hi, lo] = v6.segments();
    if (a, b, c, d, e, f) == (0, 0, 0, 0, 0, 0) && (hi != 0 || lo > 1) {
        return Some(std::net::Ipv4Addr::new(
            (hi >> 8) as u8,
            (hi & 0xff) as u8,
            (lo >> 8) as u8,
            (lo & 0xff) as u8,
        ));
    }
    None
}

/// Search provider trait - abstracts over different search backends.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Execute a search query and return formatted results.
    async fn search(&self, query: &str, count: usize) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// DuckDuckGo HTML search provider (zero API key required)
// ---------------------------------------------------------------------------

/// DuckDuckGo search provider using HTML scraping.
pub struct DuckDuckGoSearchProvider {
    client: reqwest::Client,
    /// 查询端点根（默认官方 html 端点；测试侧可注入本地 mock——BUG-20261115-017 回归锁）。
    base_url: String,
}

impl DuckDuckGoSearchProvider {
    /// Create a new DuckDuckGo search provider.
    ///
    /// 构建失败 fail-closed（BUG-20261123-028 横向扫）：不再
    /// `unwrap_or_default()` 静默回落默认 client（丢 10s 超时 + 丢跨源
    /// 重定向策略）。查询正文（用户搜索词）会随 307/308 保方法转发到
    /// 重定向目标主机——与 Perplexity provider（BUG-20261115-045 同挂
    /// `no_cross_host_redirect_policy`）对齐；DDG 无凭据头但用户查询
    /// 内容同样是出站面。
    pub fn new() -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
            .build()
            .map_err(|e| {
                format!(
                    "DuckDuckGo search client build failed (refusing default client \
                     without timeout + cross-host redirect guard): {e}"
                )
            })?;
        Ok(Self {
            client,
            base_url: "https://html.duckduckgo.com/html/".to_string(),
        })
    }

    /// 测试专用：注入本地 mock 端点（生产路径不可达硬编码官方端点，回归锁需要可控服务器）。
    #[cfg(test)]
    fn with_base_url(base_url: &str) -> Self {
        let mut p = Self::new().expect("test client build");
        p.base_url = base_url.to_string();
        p
    }

    /// Extract search results from DuckDuckGo HTML response.
    fn extract_results(&self, html: &str, count: usize, query: &str) -> Result<String, String> {
        let link_re = regex::Regex::new(
            r#"<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)</a>"#,
        )
        .map_err(|e| format!("regex error: {}", e))?;

        let matches = link_re.find_iter(html).take(count + 5).collect::<Vec<_>>();
        if matches.is_empty() {
            // Try a more lenient fallback: any href with result class nearby
            return Ok(format!(
                "No results found or extraction failed. Query: {}",
                query
            ));
        }

        let full_captures: Vec<_> = link_re.captures_iter(html).take(count + 5).collect();

        let snippet_re = regex::Regex::new(r#"<a class="result__snippet[^"]*".*?>([\s\S]*?)</a>"#)
            .map_err(|e| format!("regex error: {}", e))?;

        let snippet_captures: Vec<_> = snippet_re.captures_iter(html).take(count + 5).collect();

        let tag_re = regex::Regex::new(r"<[^>]+>").map_err(|e| format!("regex error: {}", e))?;
        let strip_tags =
            |content: &str| -> String { tag_re.replace_all(content, "").trim().to_string() };

        let mut lines = vec![format!("Results for: {} (via DuckDuckGo)", query)];

        let max_items = full_captures.len().min(count);
        for i in 0..max_items {
            let caps = &full_captures[i];
            let url_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let title = strip_tags(caps.get(2).map(|m| m.as_str()).unwrap_or(""));

            let mut url_clean = url_str.to_string();
            // Decode uddg parameter if present (DuckDuckGo redirect URL)
            if url_clean.contains("uddg=")
                && let Some(decoded) = url_decode_query_param(&url_clean, "uddg")
            {
                url_clean = decoded;
            }

            lines.push(format!("{}. {}\n   {}", i + 1, title, url_clean));

            if i < snippet_captures.len() {
                let snippet =
                    strip_tags(snippet_captures[i].get(1).map(|m| m.as_str()).unwrap_or(""));
                if !snippet.is_empty() {
                    lines.push(format!("   {}", snippet));
                }
            }
        }

        Ok(lines.join("\n"))
    }
}

// `Default` 已随 new() -> Result fail-closed（BUG-20261123-028 横向扫）
// 一并移除——Default 无法表达失败，保留会诱导 unwrap_or_default 式静默
// 降级回归。调用方显式处理 Err（from_opts 降级跳过 + warn）。

#[async_trait]
impl SearchProvider for DuckDuckGoSearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<String, String> {
        let url = format!("{}?q={}", self.base_url, urlencoding(query));

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        // BUG-20261115-017：非 2xx 必须报错而非进解析——html.duckduckgo.com
        // 的 403 异常页/429 限流页（反爬常态）此前被当普通 HTML 提取，稳定
        // 落「No results found or extraction failed」假象：agent 把「传输被拒」
        // 误读为「网上没有」。错误体前缀 200 字符（char 边界安全）喂回
        // agent 供定性（限流 vs 拒绝 vs 端点变更）。
        if !status.is_success() {
            let preview: String = body.chars().take(200).collect();
            return Err(format!("DuckDuckGo HTTP {status}: {preview}"));
        }

        self.extract_results(&body, count, query)
    }
}

// ---------------------------------------------------------------------------
// Brave search provider (API key required)
// ---------------------------------------------------------------------------

/// Brave search provider using the Brave Search API.
pub struct BraveSearchProvider {
    api_key: String,
    client: reqwest::Client,
    /// 查询端点根（默认官方 API；测试侧可注入本地 mock——BUG-20261115-017 回归锁）。
    base_url: String,
}

impl BraveSearchProvider {
    /// Create a new Brave search provider with the given API key.
    ///
    /// 构建失败 fail-closed（BUG-20261123-028 横向扫）：不再
    /// `unwrap_or_default()` 静默回落默认 client——那会同时丢掉 10s 超时
    /// 与凭据随重定向防线。本请求带 `X-Subscription-Token`（自定义凭据头，
    /// reqwest 默认策略跨主机**不剥**，BUG-20261115-045 实录），fail-open
    /// 即凭据出站面。
    pub fn new(api_key: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            // BUG-20261115-045：X-Subscription-Token 是自定义凭据头——reqwest
            // 默认重定向策略跨主机只剥 authorization/cookie 等标准四头，自定义
            // 头会原样带到 302 目标主机。换同源跟随 + 跨源终止策略（与
            // nemesis-providers 六 lane / channels 三 builder 同源纪律，见
            // nemesis_utils::http_redirect）。
            .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
            .build()
            .map_err(|e| {
                format!(
                    "Brave search client build failed (refusing default client \
                     without timeout + cross-host redirect guard): {e}"
                )
            })?;
        Ok(Self {
            api_key: api_key.to_string(),
            client,
            base_url: "https://api.search.brave.com/res/v1/web/search".to_string(),
        })
    }

    /// 测试专用：注入本地 mock 端点。
    #[cfg(test)]
    fn with_base_url(api_key: &str, base_url: &str) -> Self {
        let mut p = Self::new(api_key).expect("test client build");
        p.base_url = base_url.to_string();
        p
    }
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<String, String> {
        let url = format!("{}?q={}&count={}", self.base_url, urlencoding(query), count);

        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        // BUG-20261115-017：非 2xx 必须报错而非进解析。Brave 4xx/429 的 JSON
        // 错误体（如 {"error":[{"id":"RATE_LIMITED"}]}）没有 "web" 键——下方
        // `#[serde(default)]` typed struct 会把它静默解析成「零结果成功」，
        // 配额耗尽被伪装成「网上没有结果」，agent 无从触发重试/降级。
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        if !status.is_success() {
            let preview: String = body.chars().take(200).collect();
            return Err(format!("Brave HTTP {status}: {preview}"));
        }

        #[derive(serde::Deserialize)]
        struct SearchResult {
            title: String,
            url: String,
            #[serde(default)]
            description: String,
        }

        #[derive(serde::Deserialize, Default)]
        struct WebResults {
            #[serde(default)]
            results: Vec<SearchResult>,
        }

        #[derive(serde::Deserialize)]
        struct SearchResponse {
            #[serde(default)]
            web: WebResults,
        }

        let search_resp: SearchResponse =
            serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {}", e))?;

        if search_resp.web.results.is_empty() {
            return Ok(format!("No results for: {}", query));
        }

        let mut lines = vec![format!("Results for: {}", query)];
        for (i, item) in search_resp.web.results.iter().take(count).enumerate() {
            lines.push(format!("{}. {}\n   {}", i + 1, item.title, item.url));
            if !item.description.is_empty() {
                lines.push(format!("   {}", item.description));
            }
        }

        Ok(lines.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Perplexity search provider (API key required)
// ---------------------------------------------------------------------------

/// Perplexity search provider using the Perplexity chat completions API.
pub struct PerplexitySearchProvider {
    api_key: String,
    client: reqwest::Client,
}

impl PerplexitySearchProvider {
    /// Create a new Perplexity search provider with the given API key.
    ///
    /// 构建失败 fail-closed（BUG-20261123-028 横向扫）：不再
    /// `unwrap_or_default()` 静默回落默认 client——那会同时丢掉 30s 超时
    /// 与凭据随重定向防线（BUG-20261115-045 挂上的
    /// `no_cross_host_redirect_policy`）。本请求带
    /// `Authorization: Bearer`，凭据外呼面 fail-open 不可接受。
    pub fn new(api_key: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            // BUG-20261115-045：凭据外呼统一走跨源终止策略（Authorization
            // 虽在 reqwest 默认剥离清单内，但显式策略不再依赖「记得哪些头
            // 会被剥」——日后换自定义凭据头时不再裸奔）。
            .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
            .build()
            .map_err(|e| {
                format!(
                    "Perplexity search client build failed (refusing default client \
                     without timeout + cross-host redirect guard): {e}"
                )
            })?;
        Ok(Self {
            api_key: api_key.to_string(),
            client,
        })
    }
}

#[async_trait]
impl SearchProvider for PerplexitySearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<String, String> {
        let payload = serde_json::json!({
            "model": "sonar",
            "messages": [
                {
                    "role": "system",
                    "content": "You are a search assistant. Provide concise search results with titles, URLs, and brief descriptions in the following format:\n1. Title\n   URL\n   Description\n\nDo not add extra commentary."
                },
                {
                    "role": "user",
                    "content": format!("Search for: {}. Provide up to {} relevant results.", query, count)
                }
            ],
            "max_tokens": 1000
        });

        let resp = self
            .client
            .post("https://api.perplexity.ai/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        if !status.is_success() {
            return Err(format!("Perplexity API error: {}", body));
        }

        #[derive(serde::Deserialize)]
        struct Message {
            content: String,
        }

        #[derive(serde::Deserialize)]
        struct Choice {
            message: Message,
        }

        #[derive(serde::Deserialize)]
        struct SearchResponse {
            #[serde(default)]
            choices: Vec<Choice>,
        }

        let search_resp: SearchResponse =
            serde_json::from_str(&body).map_err(|e| format!("failed to parse response: {}", e))?;

        if search_resp.choices.is_empty() {
            return Ok(format!("No results for: {}", query));
        }

        Ok(format!(
            "Results for: {} (via Perplexity)\n{}",
            query, search_resp.choices[0].message.content
        ))
    }
}

// ---------------------------------------------------------------------------
// WebSearchTool - configurable multi-provider search
// ---------------------------------------------------------------------------

/// Configuration options for the web search tool.
#[derive(Debug, Clone, Default)]
pub struct WebSearchToolOptions {
    /// Brave Search API key.
    pub brave_api_key: Option<String>,
    /// Whether Brave search is enabled.
    pub brave_enabled: bool,
    /// Maximum results for Brave (default 5).
    pub brave_max_results: usize,
    /// DuckDuckGo enabled (default true since no key required).
    pub duckduckgo_enabled: bool,
    /// Maximum results for DuckDuckGo (default 5).
    pub duckduckgo_max_results: usize,
    /// Perplexity API key.
    pub perplexity_api_key: Option<String>,
    /// Whether Perplexity search is enabled.
    pub perplexity_enabled: bool,
    /// Maximum results for Perplexity (default 5).
    pub perplexity_max_results: usize,
}

/// Web search tool - searches the web using configured search providers.
///
/// Supports three backends with priority order: Perplexity > Brave > DuckDuckGo.
/// DuckDuckGo requires no API key and is used as the default fallback.
pub struct WebSearchTool {
    provider: Box<dyn SearchProvider>,
    max_results: usize,
}

impl WebSearchTool {
    /// Create a new web search tool with the given options.
    ///
    /// Provider priority: Perplexity > Brave > DuckDuckGo.
    /// Returns `None` if no provider is enabled.
    pub fn new(opts: &WebSearchToolOptions) -> Option<Self> {
        // Priority: Perplexity > Brave > DuckDuckGo
        if opts.perplexity_enabled
            && let Some(ref key) = opts.perplexity_api_key
            && !key.is_empty()
        {
            let max = if opts.perplexity_max_results > 0 {
                opts.perplexity_max_results
            } else {
                5
            };
            // fail-closed（BUG-20261123-028 横向扫）：构建失败 = Perplexity
            // lane 诚实缺席（warn + 继续探测 Brave/DDG），不静默跑无超时
            // 无重定向防线的默认 client。
            match PerplexitySearchProvider::new(key) {
                Ok(provider) => {
                    return Some(Self {
                        provider: Box::new(provider),
                        max_results: max,
                    });
                }
                Err(e) => {
                    tracing::warn!("[web_search] Perplexity provider 装配失败，跳过: {e}");
                }
            }
        }

        if opts.brave_enabled
            && let Some(ref key) = opts.brave_api_key
            && !key.is_empty()
        {
            let max = if opts.brave_max_results > 0 {
                opts.brave_max_results
            } else {
                5
            };
            // fail-closed（BUG-20261123-028 横向扫）：构建失败 = Brave lane
            // 诚实缺席（warn + 继续探测 DDG）。
            match BraveSearchProvider::new(key) {
                Ok(provider) => {
                    return Some(Self {
                        provider: Box::new(provider),
                        max_results: max,
                    });
                }
                Err(e) => {
                    tracing::warn!("[web_search] Brave provider 装配失败，跳过: {e}");
                }
            }
        }

        if opts.duckduckgo_enabled {
            let max = if opts.duckduckgo_max_results > 0 {
                opts.duckduckgo_max_results
            } else {
                5
            };
            // fail-closed（BUG-20261123-028 横向扫）：client 构建失败 =
            // DDG lane 诚实缺席（warn 留痕 + 继续探测其余 provider），
            // 不静默跑无超时无重定向防线的默认 client。
            match DuckDuckGoSearchProvider::new() {
                Ok(provider) => {
                    return Some(Self {
                        provider: Box::new(provider),
                        max_results: max,
                    });
                }
                Err(e) => {
                    tracing::warn!("[web_search] DuckDuckGo provider 装配失败，跳过: {e}");
                }
            }
        }

        None
    }

    /// Create a search tool with DuckDuckGo (no API key needed).
    pub fn with_duckduckgo() -> Result<Self, String> {
        Ok(Self {
            provider: Box::new(DuckDuckGoSearchProvider::new()?),
            max_results: 5,
        })
    }

    /// Create a search tool with a custom provider.
    pub fn with_provider(provider: Box<dyn SearchProvider>, max_results: usize) -> Self {
        Self {
            provider,
            max_results: if max_results > 0 { max_results } else { 5 },
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for current information. Returns titles, URLs, and snippets from search results."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results (1-10)",
                    "minimum": 1,
                    "maximum": 10
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let query = match args["query"].as_str() {
            Some(q) if !q.trim().is_empty() => q,
            Some(_) => return ToolResult::error("empty search query"),
            None => return ToolResult::error("missing 'query' argument"),
        };

        let count = args["count"]
            .as_u64()
            .map(|c| (c as usize).clamp(1, 10))
            .unwrap_or(self.max_results);

        match self.provider.search(query, count).await {
            Ok(result) => ToolResult::success(&result),
            Err(e) => ToolResult::error(&format!("search failed: {}", e)),
        }
    }
}

// ---------------------------------------------------------------------------
// URL encoding and query parameter utilities
// ---------------------------------------------------------------------------

/// Minimal percent-encoding for query strings (replaces space with +, etc.).
fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Extract a query parameter value from a URL string (handles uddg= style params).
fn url_decode_query_param(url: &str, param: &str) -> Option<String> {
    let prefix = format!("{}=", param);

    // Split on '?' to find the query string, then split on '&'
    let query_part = url.split('?').nth(1).unwrap_or(url);

    for part in query_part.split('&') {
        if let Some(encoded_value) = part.strip_prefix(&prefix) {
            // percent-decoding 按 UTF-8 字节序列整体重组（BUG-20261122-018，
            // 同族先例 oauth.rs url_decode / wechat.rs percent_decode）：
            // 旧形态逐字节 `byte as char` 是 Latin-1 映射——multibyte 序列
            //（`中` = %E4%B8%AD）被拆成三个独立码位产出 mojibake，web_search
            // 的 uddg 解包对一切非 ASCII 目标 URL 产出乱码。字节收集后
            // from_utf8_lossy：合法 UTF-8 正确还原，非法序列 U+FFFD 诚实
            // 降级；`+` 视作空格，非法 % 序列原样保留。
            let bytes = encoded_value.as_bytes();
            let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'%' if i + 2 < bytes.len() => {
                        let hex_part = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                        if let Ok(v) = u8::from_str_radix(hex_part, 16) {
                            out.push(v);
                            i += 3;
                        } else {
                            out.push(b'%');
                            i += 1;
                        }
                    }
                    b'+' => {
                        out.push(b' ');
                        i += 1;
                    }
                    b => {
                        out.push(b);
                        i += 1;
                    }
                }
            }
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
    }
    None
}

// 注：旧版此处有一个自写 `mod url` 极简解析器（「避免加依赖」），同名字
// 遮蔽 url crate——它正是 BUG-20261125-028 六形态绕过的结构性成因（见
// [`validate_url`] doc）。现直接依赖 workspace `url = "2.5"`（reqwest 本就
// 传递依赖它），本文件 `url::` 路径解析到真实 crate。

#[cfg(test)]
mod tests;
