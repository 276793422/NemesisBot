//! Shared HTTP connection pool with retry support, connection reuse, and shared client.

use std::sync::Arc;
use std::time::Duration;

/// Connection pool configuration.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub max_connections_per_host: usize,
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_retries: u32,
    pub retry_delay: Duration,
    pub user_agent: String,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections_per_host: 10,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(90),
            max_retries: 3,
            retry_delay: Duration::from_millis(500),
            user_agent: "NemesisBot/1.0".to_string(),
        }
    }
}

/// Shared HTTP connection pool wrapper around reqwest.
pub struct ConnectionPool {
    config: PoolConfig,
    client: reqwest::Client,
}

impl ConnectionPool {
    /// Create a new connection pool.
    pub fn new(config: PoolConfig) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(config.max_connections_per_host)
            .pool_idle_timeout(config.idle_timeout)
            .user_agent(&config.user_agent)
            .build()
            .unwrap_or_default();

        Self { config, client }
    }

    /// Create with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PoolConfig::default())
    }

    /// Get a reference to the underlying reqwest client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Execute a GET request.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        self.client.get(url).send().await
    }

    /// Execute a GET request with retry.
    pub async fn get_with_retry(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        retry_request(|| self.client.get(url).send()).await
    }

    /// Execute a POST request with JSON body.
    pub async fn post_json(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, reqwest::Error> {
        self.client.post(url).json(body).send().await
    }

    /// Execute a POST request with JSON body and retry.
    pub async fn post_json_with_retry(
        &self,
        url: &str,
        body: serde_json::Value,
    ) -> Result<reqwest::Response, reqwest::Error> {
        retry_request(|| {
            let body = body.clone();
            async move { self.client.post(url).json(&body).send().await }
        })
        .await
    }

    /// Execute a POST request with raw body.
    pub async fn post(
        &self,
        url: &str,
        content_type: &str,
        body: &[u8],
    ) -> Result<reqwest::Response, reqwest::Error> {
        self.client
            .post(url)
            .header("Content-Type", content_type)
            .body(body.to_vec())
            .send()
            .await
    }

    /// Download a file from a URL, writing to the specified path.
    pub async fn download_file(&self, url: &str, dest_path: &str) -> Result<(), String> {
        self.download_file_capped(url, dest_path, u64::MAX).await
    }

    /// [`Self::download_file`] 的有界版（D10-1，2026-10-13 第 6 轮审计）：
    /// `max_bytes` 上限内才落盘——曾 `resp.bytes()` 无界缓冲，恶意/失控
    /// 远端可把任意大响应灌进内存（磁盘弹药）。两道闸：Content-Length
    /// 预检（超限在传输前拒绝）+ 分块累积硬上限（缺/谎报 Content-Length
    /// 的 chunked 响应由实收字节兜底）。`u64::MAX` = 不限（旧语义）。
    pub async fn download_file_capped(
        &self,
        url: &str,
        dest_path: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("download request: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        if let Some(len) = resp.content_length() {
            if len > max_bytes {
                return Err(format!(
                    "download too large: Content-Length {len} > cap {max_bytes}"
                ));
            }
        }

        let mut body: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("read body: {}", e))?;
            if body.len() as u64 + chunk.len() as u64 > max_bytes {
                return Err(format!(
                    "download too large: exceeded cap {max_bytes} while streaming"
                ));
            }
            body.extend_from_slice(&chunk);
        }

        if let Some(parent) = std::path::Path::new(dest_path).parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {}", e))?;
        }

        std::fs::write(dest_path, &body).map_err(|e| format!("write: {}", e))?;

        Ok(())
    }

    /// Get the pool configuration.
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }
}

/// Global shared pool singleton (uses OnceLock for safe static access).
static GLOBAL_POOL: std::sync::OnceLock<Arc<ConnectionPool>> = std::sync::OnceLock::new();

/// Get or create the global shared connection pool.
pub fn shared_pool() -> Arc<ConnectionPool> {
    GLOBAL_POOL
        .get_or_init(|| Arc::new(ConnectionPool::with_defaults()))
        .clone()
}

/// Execute a request with retry logic.
async fn retry_request<F, Fut>(mut f: F) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    let max_retries = 3;
    let mut last_err = None;

    for attempt in 0..=max_retries {
        match f().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                if e.is_connect() || e.is_timeout() {
                    last_err = Some(e);
                    if attempt < max_retries {
                        let delay = Duration::from_millis(500 * (2u64.pow(attempt)));
                        tokio::time::sleep(delay).await;
                    }
                } else {
                    return Err(e);
                }
            }
        }
    }

    Err(last_err.unwrap())
}

#[cfg(test)]
mod tests;
