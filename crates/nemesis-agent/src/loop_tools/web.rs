// ---------------------------------------------------------------------------
// 机械拆分：本文件内容由 loop_tools.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

/// Configuration for web search providers.
#[derive(Debug, Clone)]
pub struct WebSearchConfig {
    /// Brave Search API key.
    pub brave_api_key: Option<String>,
    /// Brave Search max results.
    pub brave_max_results: usize,
    /// Brave Search enabled.
    pub brave_enabled: bool,
    /// DuckDuckGo max results.
    pub duckduckgo_max_results: usize,
    /// DuckDuckGo enabled.
    pub duckduckgo_enabled: bool,
    /// Perplexity API key.
    pub perplexity_api_key: Option<String>,
    /// Perplexity max results.
    pub perplexity_max_results: usize,
    /// Perplexity enabled.
    pub perplexity_enabled: bool,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            brave_api_key: None,
            brave_max_results: 5,
            brave_enabled: false,
            duckduckgo_max_results: 5,
            duckduckgo_enabled: true,
            perplexity_api_key: None,
            perplexity_max_results: 5,
            perplexity_enabled: false,
        }
    }
}

/// Web search tool that queries search engines.
///
/// Supports multiple providers with a configurable fallback chain:
/// Brave -> DuckDuckGo -> Perplexity.
pub struct WebSearchTool {
    config: WebSearchConfig,
}

impl WebSearchTool {
    /// Create a new web search tool with the given configuration.
    pub fn new(config: WebSearchConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn description(&self) -> String {
        "Search the web for current information. Returns titles, URLs, and snippets from search results.".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"Search query string"}},"required":["query"]})
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let query = extract_search_query(args)?;

        // Try providers in order of preference.
        if self.config.brave_enabled && self.config.brave_api_key.is_some() {
            return self.search_brave(&query).await;
        }

        if self.config.duckduckgo_enabled {
            return self.search_duckduckgo(&query).await;
        }

        if self.config.perplexity_enabled && self.config.perplexity_api_key.is_some() {
            return self.search_perplexity(&query).await;
        }

        Err("No search provider configured. Enable at least one search provider.".to_string())
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

impl WebSearchTool {
    #[allow(dead_code)]
    pub(super) fn extract_query(&self, args: &str) -> Result<String, String> {
        extract_search_query(args)
    }

    pub(super) async fn search_brave(&self, query: &str) -> Result<String, String> {
        let api_key = match &self.config.brave_api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => return Err("Brave API key not configured".to_string()),
        };
        let count = self.config.brave_max_results;

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding(query),
            count
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &api_key)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

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

    async fn search_duckduckgo(&self, query: &str) -> Result<String, String> {
        let count = self.config.duckduckgo_max_results;

        let url = format!("https://html.duckduckgo.com/html/?q={}", urlencoding(query));

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| expand_error("request failed", &e))?;

        let html = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        // Extract results from DDG HTML
        let link_re = regex::Regex::new(
            r#"<a[^>]*class="[^"]*result__a[^"]*"[^>]*href="([^"]+)"[^>]*>([\s\S]*?)</a>"#,
        )
        .map_err(|e| format!("regex error: {}", e))?;

        let link_captures: Vec<_> = link_re.captures_iter(&html).take(count + 5).collect();
        if link_captures.is_empty() {
            return Ok(format!(
                "No results found or extraction failed. Query: {}",
                query
            ));
        }

        let snippet_re = regex::Regex::new(r#"<a class="result__snippet[^"]*".*?>([\s\S]*?)</a>"#)
            .map_err(|e| format!("regex error: {}", e))?;

        let snippet_captures: Vec<_> = snippet_re.captures_iter(&html).take(count + 5).collect();
        let tag_re = regex::Regex::new(r"<[^>]+>").map_err(|e| format!("regex error: {}", e))?;
        let strip_tags =
            |content: &str| -> String { tag_re.replace_all(content, "").trim().to_string() };

        let mut lines = vec![format!("Results for: {} (via DuckDuckGo)", query)];
        let max_items = link_captures.len().min(count);

        for i in 0..max_items {
            let caps = &link_captures[i];
            let url_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let title = strip_tags(caps.get(2).map(|m| m.as_str()).unwrap_or(""));

            let mut url_clean = url_str.to_string();
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

    pub(super) async fn search_perplexity(&self, query: &str) -> Result<String, String> {
        let api_key = match &self.config.perplexity_api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => return Err("Perplexity API key not configured".to_string()),
        };
        let count = self.config.perplexity_max_results;

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

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("failed to create HTTP client: {}", e))?;

        let resp = client
            .post("https://api.perplexity.ai/chat/completions")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", api_key))
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

/// Extract search query from tool arguments.
pub(super) fn extract_search_query(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
        && let Some(query) = val.get("query").and_then(|v| v.as_str())
    {
        return Ok(query.to_string());
    }
    // Fallback: treat the entire argument as a query.
    Ok(args.trim().to_string())
}

/// Extract the "name" argument from tool arguments.
pub(super) fn extract_name_arg(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
        && let Some(name) = val.get("name").and_then(|v| v.as_str())
    {
        return Ok(name.to_string());
    }
    // Fallback: treat the entire argument as a name.
    Ok(args.trim().to_string())
}

/// Simple percent-encoding for URL parameters (query strings).
pub(super) fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push('%');
                result.push_str(&format!("{:02X}", byte));
            }
        }
    }
    result
}

/// Decode a query parameter value from a URL that contains query params.
/// For example, extract "uddg" from "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com"
pub(super) fn url_decode_query_param(url: &str, param: &str) -> Option<String> {
    let prefix = format!("{}=", param);
    // Find the parameter in the URL
    for part in url
        .split('&')
        .chain(url.split('?').skip(1).flat_map(|s| s.split('&')))
    {
        if let Some(val) = part.strip_prefix(&prefix) {
            return Some(percent_decode(val));
        }
    }
    None
}

/// Simple percent-decoding.
pub(super) fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }
    result
}

/// web_fetch 手动重定向循环的跳数上限。严于 reqwest
/// 默认的 10 跳：每跳都要过 SSRF 闸 + 重新建连，5 跳足够覆盖正常业务
/// 跳转，更长的链几乎总是开放重定向滥用或循环。
const WEB_FETCH_MAX_REDIRECTS: usize = 5;

/// web_fetch 请求 UA（伪装浏览器以最大化站点兼容，沿用旧实现原值）。
const WEB_FETCH_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Web fetch tool that downloads content from URLs.
pub struct WebFetchTool {
    /// Maximum response body size in bytes.
    pub max_size: usize,
    /// workspace 根（spill 存档用）。空 = 无 spill 落点
    /// （超限时退回旧「截断+注记」行为，诚实不静默）。生产注册处必填；
    /// 旧测试构造路径不带（断言截断注记的用例不受扰）。
    workspace: String,
    /// SSRF 闸宿主（SecurityPlugin）。重定向改为手动
    /// 循环后**每一跳**都重新过闸（旧实现自动跟随 ≤10 跳、闸只查首跳，
    /// `302 → 内网` 直接绕闸）。None（feature 裁剪 / 插件未注入）时跳数
    /// 上限仍生效，仅不做内网校验。
    #[cfg(feature = "security")]
    pub(crate) ssrf: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub(crate) ssrf: Option<()>,
}

impl WebFetchTool {
    /// Create a new web fetch tool with the given maximum response size.
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            workspace: String::new(),
            ssrf: None,
        }
    }

    /// 设置 workspace 根（超限全文存档到 `{workspace}/logs/spill/`）。
    pub fn with_workspace(mut self, workspace: &str) -> Self {
        self.workspace = workspace.to_string();
        self
    }

    /// 为本跳 URL 构建请求 client（SSRF 闸逐跳复查）。
    ///
    /// 三态（与图片 URL 预取 / 同一真相源 `ssrf::Guard`）：
    /// - `Err` = 闸拦截（回环/内网/链路本地/元数据/用户 CIDR/非 http(s)）
    ///   → 整个请求报错；
    /// - `Ok(ips)` 非空 = 钉死 DNS 的专用 client（防 rebinding TOCTOU）；
    /// - `Ok(ips)` 空 = 闸关闭 / host 白名单直通 → 用 plain client。
    ///
    /// 无闸（feature 裁剪 / 插件未注入）直通 plain client——跳数上限仍生效。
    fn hop_client(&self, url: &str, plain: &reqwest::Client) -> Result<reqwest::Client, String> {
        #[cfg(feature = "security")]
        if let Some(guard) = self.ssrf.as_deref().and_then(|p| p.ssrf_guard()) {
            return match guard.resolve_and_validate_collect(url) {
                Ok(ips) if !ips.is_empty() => {
                    Ok(crate::image_attach::build_pinned_no_redirect_client(
                        url,
                        &ips,
                        WEB_FETCH_UA,
                        std::time::Duration::from_secs(10),
                        std::time::Duration::from_secs(60),
                    )
                    .unwrap_or_else(|| plain.clone()))
                }
                Ok(_) => Ok(plain.clone()),
                Err(e) => Err(format!("SSRF blocked: {}", e)),
            };
        }
        let _ = url;
        Ok(plain.clone())
    }

    /// 响应体下载 + HTML 提取（原 execute 内联管线； 拆出以便重定向
    /// 循环收敛到最终 URL 后调用。`url` 报告的是**最终** URL，消息文本
    /// 格式与旧实现字节一致）。
    async fn read_body(
        &self,
        resp: reqwest::Response,
        url: &str,
        session_key: &str,
    ) -> Result<String, String> {
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

        // Download guard against truly huge responses only (OOM / bandwidth
        // abuse). The user-facing bound is on the *extracted* text below, so
        // large real pages (e.g. baidu.com ~680KB HTML) succeed instead of
        // failing at the download stage. Previously this checked `max_size`
        // (50KB) on the raw body, which rejected almost every real webpage.
        const DOWNLOAD_LIMIT: usize = 10 * 1024 * 1024; // 10 MB
        if body.len() > DOWNLOAD_LIMIT {
            return Err(format!(
                "Response too large: {} bytes (download limit: {} bytes)",
                body.len(),
                DOWNLOAD_LIMIT
            ));
        }

        let html = looks_like_html(&content_type, &body);
        let text = if html {
            let extracted = extract_html_text(&String::from_utf8_lossy(&body));
            if extracted.is_empty() {
                return Ok(format!(
                    "Content from {} ({} bytes, HTML with no extractable text)",
                    url,
                    body.len()
                ));
            }
            extracted
        } else {
            String::from_utf8_lossy(&body).trim().to_string()
        };

        self.bound_reply(text, url, &content_type, body.len(), html, session_key)
    }

    /// 超 `max_size` 的提取文本不再「截断+注记」，而是全文存档到
    /// workspace spill 目录 + 回灌 2KB preview + locator（同款语义——
    /// read_file/grep 可检索全量）。存档失败（无 workspace/磁盘错误）退回
    /// 旧「截断+注记」行为，诚实注明。低于限值时回灌格式与旧实现字节一致。
    fn bound_reply(
        &self,
        text: String,
        url: &str,
        content_type: &str,
        body_len: usize,
        was_html: bool,
        session_key: &str,
    ) -> Result<String, String> {
        let label = if was_html {
            "extracted text"
        } else {
            content_type
        };
        if text.len() <= self.max_size {
            return Ok(format!(
                "Content from {} ({} bytes, {}):\n{}",
                url, body_len, label, text
            ));
        }

        let (preview, total) = preview_chars(&text, WEB_FETCH_PREVIEW_CHARS);
        if self.workspace.is_empty() {
            let (cut, truncated) = truncate_str(&text, self.max_size);
            let note = if truncated {
                format!(" truncated to {} bytes", self.max_size)
            } else {
                String::new()
            };
            return Ok(format!(
                "Content from {} ({} bytes, {}{}):\n{}",
                url, body_len, label, note, cut
            ));
        }

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_default();
        let host = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "url".to_string());
        let spill_root = nemesis_path::resolve_spill_dir_in_workspace(Path::new(&self.workspace));
        match crate::spill::save_tool_output(
            &text,
            &spill_root,
            session_key,
            &stamp,
            &format!("web_fetch_{host}"),
        ) {
            Ok(path) => Ok(format!(
                "Content from {} ({} bytes, {} {} chars, showing first {}):\n{}\n[内容过大已完整保存到：{}。可用 read_file 工具按 offset/limit 分段读取该文件，或用 grep 工具在其中检索关键词。]",
                url,
                body_len,
                label,
                total,
                preview.chars().count(),
                preview,
                path.display()
            )),
            Err(e) => {
                tracing::warn!("[WebFetchTool] spill save failed: {e}");
                let (cut, truncated) = truncate_str(&text, self.max_size);
                let note = if truncated {
                    format!(" truncated to {} bytes", self.max_size)
                } else {
                    String::new()
                };
                Ok(format!(
                    "Content from {} ({} bytes, {}{}):\n{}",
                    url, body_len, label, note, cut
                ))
            }
        }
    }
}

/// web_fetch 超限回灌的 preview 字符预算（与 spill 层
/// `SPILL_PREVIEW_CHARS` 同量级）。
const WEB_FETCH_PREVIEW_CHARS: usize = 2000;

/// 取前 `max_chars` 个字符（char-boundary 安全），返回
/// `(preview, 总字符数)`。
pub(super) fn preview_chars(s: &str, max_chars: usize) -> (String, usize) {
    let total = s.chars().count();
    (s.chars().take(max_chars).collect(), total)
}

/// HTML 判定——Content-Type 含 `text/html`，或 Content-Type 未标明时
/// 按内容兜底（trim 后小写前缀 `<!doctype html` / `<html`；只看前 1KB，
/// 避免为判定扫描整个 body）。
pub(super) fn looks_like_html(content_type: &str, body: &[u8]) -> bool {
    if content_type.to_ascii_lowercase().contains("text/html") {
        return true;
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(1024)]);
    let head = head.trim_start().to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html")
}

/// HTML → 可读文本（html2text 渲染：script/style 剥除、链接/列表/
/// 标题结构保留为 markdown 形态；替代旧 regex 剥标签管线——旧管线把
/// 全页压成单行、链接 URL 丢失、表格/列表结构全毁）。80 列渲染宽度。
/// 渲染错误（html5ever 极端容错，实践中几乎不可达）诚实降级为空 →
/// 调用方回「HTML with no extractable text」。
pub(super) fn extract_html_text(html: &str) -> String {
    match html2text::from_read(html.as_bytes(), 80) {
        Ok(rendered) => rendered
            .lines()
            .map(str::trim_end)
            .collect::<Vec<&str>>()
            .join("\n")
            .trim()
            .to_string(),
        Err(e) => {
            tracing::warn!("[WebFetchTool] html2text render failed: {e}");
            String::new()
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn description(&self) -> String {
        "Fetch a URL and extract readable content. Use this to get weather info, news, articles, or any web content.".to_string()
    }

    // vault：外呼抓取参与 web_fetch 类别限额。
    fn limit_categories(&self) -> &[&str] {
        &["web_fetch"]
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type":"object",
            "properties":{
                "url":{"type":"string","description":"URL to fetch"},
                "headers":{
                    "type":"object",
                    "description":"Optional HTTP headers sent with the FIRST request only (never forwarded on redirects). Values must be strings; use vault:<alias> references for credentials so the raw value never enters context or history.",
                    "additionalProperties":{"type":"string"}
                }
            },
            "required":["url"]
        })
    }

    // vault：headers 槽位承载凭据别名（模型传 `vault:<alias>`，
    // dispatch 最内层改写真值——历史/预览只见别名）。
    fn credential_arg_keys(&self) -> &[&str] {
        &["headers"]
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let start_url = extract_url(args)?;
        let headers = extract_headers(args)?;

        // 手动重定向循环替代 reqwest 默认自动跟随——
        // 旧实现用默认 client（≤10 跳自动跟随），SSRF 闸只查首跳 URL，
        // `302 → 内网` 直接绕闸。现在每一跳都重新过闸 + 可钉死 DNS；
        // 跳数上限 5；3xx 无 Location / 相对 Location 都诚实处理。
        let plain_client = crate::image_attach::build_no_redirect_client(
            WEB_FETCH_UA,
            std::time::Duration::from_secs(10),
            std::time::Duration::from_secs(60),
        );

        let mut current = start_url;
        let mut hops: usize = 0;
        // 自定义 headers 只上**首跳**——Authorization 等跟随重定向会
        // 泄漏给 302 指向的第三方目标（凭据外泄面）。
        let mut first_hop = true;
        loop {
            let client = self.hop_client(&current, &plain_client)?;
            let mut req = client.get(&current);
            if first_hop && !headers.is_empty() {
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
            }
            first_hop = false;
            let resp = req
                .send()
                .await
                .map_err(|e| expand_error("request failed", &e))?;

            let status = resp.status();
            if status.is_redirection() {
                hops += 1;
                if hops > WEB_FETCH_MAX_REDIRECTS {
                    return Err(format!(
                        "too many redirects (limit {}), stopped at {}",
                        WEB_FETCH_MAX_REDIRECTS, current
                    ));
                }
                let location = resp
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        format!(
                            "HTTP {} redirect without Location header at {}",
                            status, current
                        )
                    })?
                    .to_string();
                let base = reqwest::Url::parse(&current)
                    .map_err(|e| format!("invalid URL '{}': {}", current, e))?;
                current = base
                    .join(&location)
                    .map_err(|e| format!("invalid redirect target '{}': {}", location, e))?
                    .to_string();
                continue;
            }

            if !status.is_success() {
                return Err(format!("HTTP {} for {}", status, current));
            }

            return self.read_body(resp, &current, &context.session_key).await;
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

/// Truncate a string to at most `max_bytes`, cutting on a UTF-8 char boundary
/// (never mid-character). Returns `(truncated_string, was_truncated)`.
pub(super) fn truncate_str(s: &str, max_bytes: usize) -> (String, bool) {
    if s.len() <= max_bytes {
        return (s.to_string(), false);
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

/// Format an error with its full source chain. reqwest collapses the real
/// cause (TLS / DNS / connect / certificate) into an opaque "error sending
/// request"; this surfaces the underlying reason for diagnosis.
fn expand_error(prefix: &str, e: &dyn std::error::Error) -> String {
    let mut msg = format!("{}: {}", prefix, e);
    let mut cur = e.source();
    while let Some(s) = cur {
        msg.push_str(&format!(" | {}", s));
        cur = s.source();
    }
    msg
}

/// Extract URL from tool arguments.
pub(super) fn extract_url(args: &str) -> Result<String, String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args)
        && let Some(url) = val.get("url").and_then(|v| v.as_str())
    {
        return Ok(url.to_string());
    }
    Ok(args.trim().to_string())
}

/// 解析 + 校验 `headers` 参数（首跳自定义头）。诚实拒绝面：
/// - 非 JSON / 无 headers 键 = 空表（向后兼容旧调用形态）；
/// - 值非字符串 / 头名或值非法（reqwest 层会 panic，必须前置校验）；
/// - 逐跳协议头 denylist（host/content-length/connection/transfer-encoding
///   ——由传输层自管，手工注入属请求走私面）；
/// - 数量 >16 或单值 >4KB（滥用面钳制）。
pub(super) fn extract_headers(args: &str) -> Result<Vec<(String, String)>, String> {
    let val = match serde_json::from_str::<serde_json::Value>(args) {
        Ok(v) => v,
        Err(_) => return Ok(Vec::new()),
    };
    let Some(obj) = val.get("headers").and_then(|h| h.as_object()) else {
        return Ok(Vec::new());
    };
    if obj.len() > 16 {
        return Err(format!(
            "too many headers ({} > 16): the headers parameter is for a handful of auth/routing headers, not bulk injection",
            obj.len()
        ));
    }
    let mut out = Vec::with_capacity(obj.len());
    for (k, v) in obj {
        let name = reqwest::header::HeaderName::try_from(k.as_str())
            .map_err(|e| format!("invalid header name '{k}': {e}"))?;
        // 逐跳协议头 denylist（小写归一后比对；HeaderName 本就小写化）。
        if matches!(
            name.as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        ) {
            return Err(format!(
                "header '{k}' is managed by the transport layer and cannot be overridden"
            ));
        }
        let Some(value) = v.as_str() else {
            return Err(format!(
                "header '{k}' must be a string value (got {})",
                v_type_name(v)
            ));
        };
        if value.len() > 4096 {
            return Err(format!(
                "header '{k}' value too large ({} > 4096 bytes)",
                value.len()
            ));
        }
        if !value.is_ascii() {
            return Err(format!(
                "header '{k}' value must be visible ASCII (HTTP/1.1 header constraint)"
            ));
        }
        // ASCII 校验后 try_from 实际不可达失败——保留错误链防御未来改动。
        reqwest::header::HeaderValue::try_from(value.to_string())
            .map_err(|e| format!("invalid header value for '{k}': {e}"))?;
        out.push((name.to_string(), value.to_string()));
    }
    Ok(out)
}

/// headers 值类型的人读名（错误文案用）。
fn v_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
        serde_json::Value::String(_) => "string",
    }
}

// ===========================================================================
// Cluster RPC tool
// ===========================================================================
