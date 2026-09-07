//! 价目表在线同步（A2，2026-08-31）：LiteLLM 主源拉取 → 解析 → 落分层价目表。
//!
//! 设计契约（对齐 goal 硬约束）：**断网/解析失败降级 = 保留现有表继续用旧
//! 数据**——失败时只刷新 meta（`record_failed_fetch`），绝不因价目表同步
//! 问题拖垮计价链路。ETag 增量请求（304 NotModified = 表已是最新）。

use std::sync::OnceLock;

use nemesis_data::{PRICE_MIRROR_URLS, PricingMeta, PricingStore, parse_litellm_json};

const USER_AGENT: &str = concat!("NemesisBot/", env!("CARGO_PKG_VERSION"));
const FETCH_TIMEOUT_SECS: u64 = 60;

/// 同步结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PricingSyncResult {
    /// `false` = 304 NotModified（表已是最新，未替换）。
    pub updated: bool,
    pub entry_count: usize,
    pub source_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .build()
            .expect("pricing sync http client")
    })
}

/// 拉取最新价目表并整体替换下载层。
///
/// - `url = Some(u)`：**只拉 `u`**（用户显式指定 = 用户最清楚可达端点，
///   不做镜像兜底）。
/// - `url = None`：走 [`PRICE_MIRROR_URLS`] 镜像链（官方 raw → jsdelivr
///   CDN ×2），按序尝试、命中即止——`raw.githubusercontent.com` 在受限
///   网络下常被干扰，镜像链让更新在国内网络也能成功。
///
/// ETag 只在「待拉 URL == 上次成功来源」时附带（不同镜像/CDN 的 etag 互
/// 不通用，张冠李戴会拿到错误 304）。失败（网络/解析/空表）→
/// `record_failed_fetch` 刷 meta + 返回 Err（各镜像错误拼接），**旧表
/// 保持不动**。304 → `Ok(updated: false)`。
pub async fn fetch_and_replace(
    store: &PricingStore,
    url: Option<&str>,
) -> Result<PricingSyncResult, String> {
    match url {
        Some(u) => try_fetch_url(store, u).await,
        // 测试经 `fetch_chain` 注入本地服务器 URL——镜像链永不硬编码进
        // 测试（受限网络下不打外网）。
        None => fetch_chain(store, PRICE_MIRROR_URLS).await,
    }
}

/// 镜像链逐条尝试（[`PRICE_MIRROR_URLS`] 缺省；测试可注入本地 URL 表）。
/// 任一条成功（含 304）即返回；全失败 → Err（各镜像错误拼接）。
pub(crate) async fn fetch_chain(
    store: &PricingStore,
    urls: &[&str],
) -> Result<PricingSyncResult, String> {
    let mut errors = Vec::new();
    for candidate in urls {
        match try_fetch_url(store, candidate).await {
            Ok(r) => return Ok(r),
            Err(e) => errors.push(format!("{candidate}: {e}")),
        }
    }
    Err(errors.join("; "))
}

/// 单 URL 拉取 + 替换下载层（镜像链的每一条与用户显式 URL 共用此路径）。
async fn try_fetch_url(store: &PricingStore, url: &str) -> Result<PricingSyncResult, String> {
    let mut req = http_client().get(url);
    // ETag 增量：仅当该 etag 来自同一个 URL 时才附带——不同镜像的 etag
    // 语义不通，跨 URL 带旧 etag 可能换来错误的 304。
    let meta = store.meta();
    if meta.source_url.as_deref() == Some(url)
        && let Some(etag) = &meta.etag
    {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let stored_etag = meta.etag.clone();

    let resp = req
        .send()
        .await
        .map_err(|e| format!("价目表下载失败（保留旧表）: {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(PricingSyncResult {
            updated: false,
            entry_count: store.list_downloaded().map(|v| v.len()).unwrap_or(0),
            source_url: url.to_string(),
            etag: stored_etag,
        });
    }

    if !resp.status().is_success() {
        let msg = format!("价目表下载失败（保留旧表）: HTTP {}", resp.status());
        let _ = store.record_failed_fetch(url);
        return Err(msg);
    }

    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    // 不走 resp.text()（按响应头 charset 解码，被干扰的响应头会让完好的
    // 字节也解码失败——真机实证的 "error decoding response body" 根因）；
    // LiteLLM 表是 UTF-8 JSON，直取字节。
    let raw_bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("价目表读取失败（保留旧表）: {e}"))?;
    let raw = String::from_utf8_lossy(&raw_bytes);

    let entries = parse_litellm_json(&raw)?;
    let entry_count = entries.len();

    store
        .replace_downloaded(
            entries,
            PricingMeta {
                etag,
                fetched_at: Some(chrono::Local::now().timestamp()),
                source_url: Some(url.to_string()),
                entry_count,
            },
        )
        .map_err(|e| format!("价目表落盘失败: {e}"))?;

    tracing::info!(entry_count, url = %url, "[PricingSync] 价目表已更新");
    Ok(PricingSyncResult {
        updated: true,
        entry_count,
        source_url: url.to_string(),
        etag: store.meta().etag,
    })
}

#[cfg(test)]
mod tests;
