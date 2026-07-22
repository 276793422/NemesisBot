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
    /// HTTP client.
    client: reqwest::Client,
    /// Maximum response size in bytes.
    max_size: usize,
    /// Request timeout.
    timeout: Duration,
}

impl WebFetchTool {
    /// Create a new web fetch tool with default settings.
    pub fn new() -> Self {
        Self::with_options(DEFAULT_TIMEOUT_SECS, DEFAULT_MAX_SIZE)
    }

    /// Create with custom timeout and size limit.
    pub fn with_options(timeout_secs: u64, max_size: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .user_agent("NemesisBot/1.0")
            .build()
            .unwrap_or_default();

        Self {
            client,
            max_size,
            timeout: Duration::from_secs(timeout_secs),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
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
                    return ToolResult::error(&format!(
                        "HTTP {} {}",
                        status.as_u16(),
                        status.canonical_reason().unwrap_or("Unknown")
                    ));
                }

                // Read body with size limit
                match response.bytes().await {
                    Ok(bytes) => {
                        if bytes.len() > max_size {
                            return ToolResult::error(&format!(
                                "response too large: {} bytes (limit: {})",
                                bytes.len(),
                                max_size
                            ));
                        }
                        let content = String::from_utf8_lossy(&bytes);
                        ToolResult::success(&content)
                    }
                    Err(e) => ToolResult::error(&format!("failed to read response body: {}", e)),
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

/// Validate URL to prevent SSRF attacks.
fn validate_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {}", e))?;

    if let Some(host) = parsed.host_str() {
        // Block common private/local addresses
        let blocked_hosts = [
            "localhost",
            "127.0.0.1",
            "0.0.0.0",
            "169.254.169.254", // Cloud metadata endpoint
            "[::1]",
        ];
        let lower = host.to_lowercase();
        for blocked in &blocked_hosts {
            if lower == *blocked {
                return Err(format!(
                    "access to '{}' is not allowed (SSRF protection)",
                    host
                ));
            }
        }

        // Block private IP ranges
        if lower.starts_with("10.")
            || lower.starts_with("192.168.")
            || lower.starts_with("172.16.")
            || lower.starts_with("172.17.")
            || lower.starts_with("172.18.")
            || lower.starts_with("172.19.")
            || lower.starts_with("172.2")
            || lower.starts_with("172.3")
        {
            return Err(format!(
                "access to private IP '{}' is not allowed (SSRF protection)",
                host
            ));
        }
    }

    Ok(())
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
}

impl DuckDuckGoSearchProvider {
    /// Create a new DuckDuckGo search provider.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_default();
        Self { client }
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
            if url_clean.contains("uddg=") {
                if let Some(decoded) = url_decode_query_param(&url_clean, "uddg") {
                    url_clean = decoded;
                }
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

impl Default for DuckDuckGoSearchProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchProvider for DuckDuckGoSearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<String, String> {
        let url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding(&query)
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        let body = resp
            .text()
            .await
            .map_err(|e| format!("failed to read response: {}", e))?;

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
}

impl BraveSearchProvider {
    /// Create a new Brave search provider with the given API key.
    pub fn new(api_key: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            api_key: api_key.to_string(),
            client,
        }
    }
}

#[async_trait]
impl SearchProvider for BraveSearchProvider {
    async fn search(&self, query: &str, count: usize) -> Result<String, String> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding(query),
            count
        );

        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
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
    pub fn new(api_key: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            api_key: api_key.to_string(),
            client,
        }
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
        if opts.perplexity_enabled {
            if let Some(ref key) = opts.perplexity_api_key {
                if !key.is_empty() {
                    let max = if opts.perplexity_max_results > 0 {
                        opts.perplexity_max_results
                    } else {
                        5
                    };
                    return Some(Self {
                        provider: Box::new(PerplexitySearchProvider::new(key)),
                        max_results: max,
                    });
                }
            }
        }

        if opts.brave_enabled {
            if let Some(ref key) = opts.brave_api_key {
                if !key.is_empty() {
                    let max = if opts.brave_max_results > 0 {
                        opts.brave_max_results
                    } else {
                        5
                    };
                    return Some(Self {
                        provider: Box::new(BraveSearchProvider::new(key)),
                        max_results: max,
                    });
                }
            }
        }

        if opts.duckduckgo_enabled {
            let max = if opts.duckduckgo_max_results > 0 {
                opts.duckduckgo_max_results
            } else {
                5
            };
            return Some(Self {
                provider: Box::new(DuckDuckGoSearchProvider::new()),
                max_results: max,
            });
        }

        None
    }

    /// Create a search tool with DuckDuckGo (no API key needed).
    pub fn with_duckduckgo() -> Self {
        Self {
            provider: Box::new(DuckDuckGoSearchProvider::new()),
            max_results: 5,
        }
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
            // Basic percent-decoding
            let mut result = String::new();
            let mut chars = encoded_value.chars();
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
            return Some(result);
        }
    }
    None
}

/// Internal URL parsing module for SSRF validation.
mod url {
    /// Minimal URL parser for SSRF validation (avoids adding another dependency).
    pub struct Url {
        pub host: Option<String>,
    }

    impl Url {
        pub fn parse(raw: &str) -> Result<Self, String> {
            // Very simple parsing: extract host from http(s)://host/path
            let stripped = raw
                .strip_prefix("http://")
                .or_else(|| raw.strip_prefix("https://"))
                .ok_or("not an HTTP URL")?;

            let host_part = stripped.split('/').next().unwrap_or("");
            // Remove port
            let host = host_part.split(':').next().unwrap_or("");
            if host.is_empty() {
                return Err("no host in URL".to_string());
            }

            Ok(Self {
                host: Some(host.to_string()),
            })
        }

        pub fn host_str(&self) -> Option<&str> {
            self.host.as_deref()
        }
    }
}

#[cfg(test)]
mod tests;
