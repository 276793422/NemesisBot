//! 价目表在线同步（A2，2026-08-31）：LiteLLM 主源拉取 → 解析 → 落分层价目表。
//!
//! 设计契约（对齐 goal 硬约束）：**断网/解析失败降级 = 保留现有表继续用旧
//! 数据**——失败时只刷新 meta（`record_failed_fetch`），绝不因价目表同步
//! 问题拖垮计价链路。ETag 增量请求（304 NotModified = 表已是最新）。

use std::sync::OnceLock;

use nemesis_data::{PricingMeta, PricingStore, LITELLM_PRICE_URL, parse_litellm_json};

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

/// 从 `url`（缺省 = LiteLLM 官方 raw 地址）拉取最新价目表并整体替换下载层。
///
/// 失败（网络/解析/空表）→ `record_failed_fetch` 刷 meta + 返回 Err，**旧表
/// 保持不动**。304 → `Ok(updated: false)`。
pub async fn fetch_and_replace(store: &PricingStore, url: Option<&str>) -> Result<PricingSyncResult, String> {
    let url = url.unwrap_or(LITELLM_PRICE_URL).to_string();

    let mut req = http_client().get(&url);
    // ETag 增量：上次成功下载的 etag 还有效 → 上游 304。
    if let Some(etag) = store.meta().etag {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("价目表下载失败（保留旧表）: {e}"))?;

    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        let etag = store.meta().etag;
        return Ok(PricingSyncResult {
            updated: false,
            entry_count: store.list_downloaded().map(|v| v.len()).unwrap_or(0),
            source_url: url,
            etag,
        });
    }

    if !resp.status().is_success() {
        let msg = format!("价目表下载失败（保留旧表）: HTTP {}", resp.status());
        let _ = store.record_failed_fetch(&url);
        return Err(msg);
    }

    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("价目表读取失败（保留旧表）: {e}"))?;

    let entries = parse_litellm_json(&raw)?;
    let entry_count = entries.len();

    store
        .replace_downloaded(
            entries,
            PricingMeta {
                etag,
                fetched_at: Some(chrono::Local::now().timestamp()),
                source_url: Some(url.clone()),
                entry_count,
            },
        )
        .map_err(|e| format!("价目表落盘失败: {e}"))?;

    tracing::info!(entry_count, url = %url, "[PricingSync] 价目表已更新");
    Ok(PricingSyncResult {
        updated: true,
        entry_count,
        source_url: url,
        etag: store.meta().etag,
    })
}

#[cfg(test)]
mod tests;
