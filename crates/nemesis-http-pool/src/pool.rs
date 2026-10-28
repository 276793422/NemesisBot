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
    /// 不跟随重定向的伴生 client（BUG-HUNT R36）。reqwest 默认策略自动
    /// 跟随 ≤10 跳——「先过 SSRF 闸再发请求」的调用面闸只覆盖显式 URL，
    /// 对端 302 指向内网/元数据端点即整条绕闸（web_fetch J2a 修过的同类
    /// 形态）。守卫类下载面走 [`Self::download_file_capped_strict`]；
    /// 普通面维持 [`Self::client`] 语义不变。
    no_redirect_client: reqwest::Client,
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
        let no_redirect_client = reqwest::Client::builder()
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(config.max_connections_per_host)
            .pool_idle_timeout(config.idle_timeout)
            .user_agent(&config.user_agent)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();

        Self {
            config,
            client,
            no_redirect_client,
        }
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
        retry_request(
            self.config.max_retries,
            self.config.retry_delay,
            || self.client.get(url).send(),
            retryable_get,
        )
        .await
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
        retry_request(
            self.config.max_retries,
            self.config.retry_delay,
            || {
                let body = body.clone();
                async move { self.client.post(url).json(&body).send().await }
            },
            retryable_connect_only,
        )
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
    ///
    /// 语义注记（BUG-HUNT R36）：本方法用默认 client，**跟随重定向**。
    /// 调用方若先过了 SSRF 闸（闸只覆盖显式 URL，逐跳不重放），必须改用
    /// [`Self::download_file_capped_strict`]。
    pub async fn download_file_capped(
        &self,
        url: &str,
        dest_path: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        self.download_file_capped_on(&self.client, url, dest_path, max_bytes)
            .await
    }

    /// [`Self::download_file_capped`] 的不跟随重定向版（BUG-HUNT R36）：
    /// reqwest 默认策略自动跟随 ≤10 跳，而「先过 SSRF 闸再发请求」的调用
    /// 面（board 资产拉取等）闸只查显式 URL——对端 302 指向
    /// metadata/内网端点即整条绕闸。本方法遇 3xx 直接按 `HTTP <status>`
    /// 报错，跳转裁决权交还调用方（与 web_fetch J2a 手动逐跳过闸同一纪律；
    /// 构成不安全跳转时由上层错误路径自然处理，无 second request 落地）。
    pub async fn download_file_capped_strict(
        &self,
        url: &str,
        dest_path: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        self.download_file_capped_on(&self.no_redirect_client, url, dest_path, max_bytes)
            .await
    }

    /// 下载实现的单一真相源：`max_bytes` 双闸（Content-Length 预检 +
    /// 流式硬上限）+ 全量缓冲后落盘。**落盘是裸 `std::fs::write` 非原子**
    /// ——原子性由调用方负责（board 资产面走 `.part` 临时名 + finalize
    /// 改名；R180 修正：旧注释宣称「原子落盘」与实现不符）。重定向行为
    /// 由传入的 client 决定（两个公开入口的唯一差异点）。
    async fn download_file_capped_on(
        &self,
        client: &reqwest::Client,
        url: &str,
        dest_path: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("download request: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        if let Some(len) = resp.content_length()
            && len > max_bytes
        {
            return Err(format!(
                "download too large: Content-Length {len} > cap {max_bytes}"
            ));
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

/// GET（幂等）的可重试判定：连接级失败 + 请求超时。
fn retryable_get(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout()
}

/// POST（非幂等）的可重试判定：只认连接级失败——请求尚未发出，天然幂等。
/// 请求超时可能在服务端已接收并处理后触发，重试 = 双提交（webhook/通知
/// 类端点双投递）。
fn retryable_connect_only(e: &reqwest::Error) -> bool {
    e.is_connect()
}

/// Execute a request with retry logic (total attempts = 1 + `max_retries`;
/// backoff = `retry_delay` × 2^attempt).
///
/// BUG-20261025-61：此前 `max_retries`/`retry_delay` 硬编码（3 / 500ms
/// 基数），[`PoolConfig`] 的两个配置项声名从未消费（配置 max_retries=0
/// 关重试完全无效）。现由调用方传入实际消费；重试面按请求幂等性分派
/// （[`retryable_get`] / [`retryable_connect_only`]）。
async fn retry_request<F, Fut, P>(
    max_retries: u32,
    retry_delay: Duration,
    mut f: F,
    retryable: P,
) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
    P: Fn(&reqwest::Error) -> bool,
{
    let mut last_err = None;

    for attempt in 0..=max_retries {
        match f().await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                if retryable(&e) {
                    last_err = Some(e);
                    if attempt < max_retries {
                        tokio::time::sleep(retry_delay * 2u32.pow(attempt)).await;
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
