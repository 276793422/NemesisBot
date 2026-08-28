//! models.dev model catalog (U16 sixth batch).
//!
//! Fetches the models.dev public catalog (`/api.json`), extracts per-model
//! `limit.context` (input token capacity) and `limit.output` (max output
//! tokens), and caches it so `model add` can auto-fill `context_window` /
//! `max_output_tokens` for catalog hits — offline/intranet deployments reuse
//! the cache without network.
//!
//! 缓存路径唯一真相源 = `nemesis_path::models_catalog_cache_path`
//! （`<home>/workspace/data/models_catalog.json`）。nemesis-web 的 models
//! handler 与本 crate 共用该函数（nemesis-web 不能依赖 binary crate，共享
//! 路径收在 nemesis-path）；读取入口自动 rename 2026-08-28 前的 home 根
//! 旧位置文件（legacy 迁移）。
//!
//! api.json shape (verified against the models.dev repo schema, packages/
//! core/src/schema.ts + generate.ts):
//!
//! ```json
//! {
//!   "<provider_id>": {
//!     "id": "...", "name": "...",
//!     "models": {
//!       "<model_id>": { "name": "...", "family": "...",
//!                       "limit": { "context": 400000, "output": 128000 } }
//!     }
//!   }
//! }
//! ```
//!
//! Our `model add --model vendor/name` keys match `provider_id/model_id`
//! directly (e.g. `openai/gpt-5.2`, `anthropic/claude-...`). Unknown fields
//! are ignored — the API evolves, parsing must not break (goal §八).

use std::collections::HashMap;
use std::path::Path;

/// Primary endpoint (per models.dev docs; same URL the Pi ecosystem's
/// generate-models.ts fetches).
pub const API_URL: &str = "https://models.dev/api.json";

/// Mirror fallback: the site's api.json is a BUILD ARTIFACT (dist/_api.json,
/// not in the repo), so jsDelivr/gh mirrors 404 for it — but the repo root
/// `models.json` (OpenRouter sync cache, `{data: [{id, context_length, ...}]}`
/// shape) IS in-repo and mirrorable. It carries ids + context lengths (no
/// output limits), enough for context_window auto-fill. Field names differ
/// from api.json — `parse_models_json` handles this shape.
pub const API_MIRROR_URL: &str =
    "https://cdn.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json";

/// One flattened catalog entry: provider/model → limits.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CatalogEntry {
    /// `provider_id/model_id` — matches our `model add --model` key shape.
    pub key: String,
    /// Input token capacity (api.json `limit.context`).
    pub context_window: u64,
    /// Max output tokens (api.json `limit.output`) when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    /// Model family (api.json `family`) when declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
}

/// The on-disk cache shape: `{ "version": 1, "fetched_at": <rfc3339>, "entries": [...] }`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Catalog {
    pub version: u32,
    pub fetched_at: String,
    pub entries: Vec<CatalogEntry>,
}

/// Parse the repo's `models.json` (OpenRouter sync cache) — the mirror
/// shape: `{"data": [{"id": "anthropic/claude-opus-4.7-fast",
/// "context_length": 1000000, "top_provider": {"max_completion_tokens": N},
/// ...}, ...]}`. Ids are `vendor/model` already. `~`-prefixed ids are
/// OpenRouter aliases — skipped (duplicate of the canonical entry).
pub fn parse_models_json(raw: &str) -> Result<Vec<CatalogEntry>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}"))?;
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "expected {data: [...]}" .to_string())?;
    let mut entries = Vec::new();
    for m in arr {
        let Some(id) = m.get("id").and_then(|i| i.as_str()) else {
            continue;
        };
        if id.starts_with('~') || !id.contains('/') {
            continue; // alias or non-vendor/model id
        }
        let Some(ctx) = m.get("context_length").and_then(|c| c.as_u64()) else {
            continue;
        };
        if ctx == 0 {
            continue;
        }
        // Output cap: top_provider.max_completion_tokens when declared.
        let max_out = m
            .get("top_provider")
            .and_then(|tp| tp.get("max_completion_tokens"))
            .and_then(|x| x.as_u64());
        entries.push(CatalogEntry {
            key: id.to_string(),
            context_window: ctx,
            max_output_tokens: max_out,
            family: None,
        });
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    entries.dedup_by(|a, b| a.key == b.key);
    Ok(entries)
}

/// Parse whichever shape the body is (api.json preferred; falls back to the
/// repo models.json shape). A shape-parse that yields ZERO entries counts as
/// a miss (live-observed: the mirror body also parses as a top-level object,
/// so lenient zero-entry successes would silently shadow the right parser).
pub fn parse_any(raw: &str) -> Result<Vec<CatalogEntry>, String> {
    for parser in [parse_api_json, parse_models_json] {
        if let Ok(entries) = parser(raw) {
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }
    Err("no entries extracted from either api.json or models.json shape".to_string())
}

/// Parse the raw api.json payload into catalog entries. Pure — unit-tested
/// with fixtures, no network. Returns `Err` when the payload parses as JSON
/// but has a non-object top level. NOTE: an empty-but-valid provider map is
/// NOT an error here — [`parse_any`] is responsible for treating a zero-entry
/// api.json result as a wrong-shape payload and trying the mirror parser
/// (live-observed: the mirror body also parses as a top-level object, so a
/// lenient Ok(vec![]) from this fn would silently shadow it).
pub fn parse_api_json(raw: &str) -> Result<Vec<CatalogEntry>, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = v
        .as_object()
        .ok_or_else(|| "expected a top-level object of providers".to_string())?;
    // Require at least one provider with a models map — distinguishes the
    // api.json shape from any other top-level object (e.g. models.json).
    let has_provider = obj
        .values()
        .any(|pv| pv.get("models").and_then(|m| m.as_object()).is_some());
    if !has_provider {
        return Err("no provider entries found (wrong shape?)".to_string());
    }
    let mut entries = Vec::new();
    for (pid, pv) in obj {
        let Some(models) = pv.get("models").and_then(|m| m.as_object()) else {
            continue; // provider without models — skip, not an error
        };
        for (mid, mv) in models {
            let Some(limit) = mv.get("limit") else {
                continue; // no declared limits — nothing to offer
            };
            let Some(ctx) = limit.get("context").and_then(|c| c.as_u64()) else {
                continue;
            };
            if ctx == 0 {
                continue;
            }
            let key = format!("{}/{}", pid, mid);
            entries.push(CatalogEntry {
                key,
                context_window: ctx,
                max_output_tokens: limit.get("output").and_then(|o| o.as_u64()),
                family: mv
                    .get("family")
                    .and_then(|f| f.as_str())
                    .map(|s| s.to_string()),
            });
        }
    }
    // Stable order (provider+model id) so the cache file diffs cleanly.
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(entries)
}

/// Load the cached catalog from disk. `Err` only on a present-but-unparsable
/// file (corrupt cache should be loud); a missing file is `Ok(None)`.
///
/// 缓存路径唯一真相源 = `nemesis_path::models_catalog_cache_path`
/// （`<home>/workspace/data/models_catalog.json`；2026-08-28 从 home 根迁入，
/// 读取时自动 rename 旧位置文件）。本 crate 与 nemesis-web 均经该函数取路径，
/// 禁止各自拼 join。
pub fn load_cache(home_dir: &Path) -> Result<Option<Catalog>, String> {
    let path = nemesis_path::models_catalog_cache_path(home_dir);
    nemesis_path::migrate_legacy_models_catalog_cache(home_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let c: Catalog =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    Ok(Some(c))
}

/// Persist the catalog to disk (atomic-ish: write temp then rename).
/// 成功后 best-effort 清掉 home 根的 legacy 缓存（读路径有 rename 迁移，
/// 但存量部署若只跑 catalog-update 则旧文件会永久滞留成孤儿）。
pub fn save_cache(home_dir: &Path, entries: Vec<CatalogEntry>) -> Result<(), String> {
    let path = nemesis_path::models_catalog_cache_path(home_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let cat = Catalog {
        version: 1,
        fetched_at: chrono::Local::now().to_rfc3339(),
        entries,
    };
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(&cat).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&tmp, body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {e}"))?;
    let _ = std::fs::remove_file(nemesis_path::legacy_models_catalog_cache_path(home_dir));
    Ok(())
}

/// Look up a `vendor/model` key in the cache (exact match; the key shapes are
/// identical by construction).
pub fn lookup<'a>(catalog: &'a Catalog, model_key: &str) -> Option<&'a CatalogEntry> {
    catalog.entries.iter().find(|e| e.key == model_key)
}

/// Fetch the catalog over HTTP. MUST be called via
/// `tokio::task::spawn_blocking` from async contexts (the reqwest blocking
/// client builds a nested runtime that panics on drop inside async —
/// observed live 2026-08-23). Tries the primary URL, falls back to the
/// jsDelivr mirror. 30s budget each.
pub fn fetch_http_blocking() -> Result<Vec<CatalogEntry>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("nemesisbot-catalog/1.0")
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let mut last_err = String::new();
    for url in catalog_endpoints() {
        match client.get(url).send() {
            Ok(resp) if resp.status().is_success() => match resp.text() {
                Ok(body) => match parse_any(&body) {
                    Ok(entries) => return Ok(entries),
                    Err(e) => last_err = format!("{url}: {e}"),
                },
                Err(e) => last_err = format!("{url}: body read: {e}"),
            },
            Ok(resp) => last_err = format!("{url}: HTTP {}", resp.status()),
            Err(e) => last_err = format!("{url}: {e}"),
        }
    }
    Err(format!("all catalog endpoints failed — last: {last_err}"))
}

/// Endpoint pair with an optional test seam.
///
/// `NEMESISBOT_CATALOG_API_URL`（未设置时与 `[API_URL, API_MIRROR_URL]` 完全
/// 一致）替换主端点，使 fetch 的成功/坏 body/非 2xx 臂能对本地 mock 确定性
/// 测试（此前 URL 常量硬编码，这些臂只能靠真网络或留豁免）。
fn catalog_endpoints() -> [&'static str; 2] {
    match std::env::var("NEMESISBOT_CATALOG_API_URL") {
        Ok(url) if !url.is_empty() => {
            // Leak 一次把 String 钉成 'static：该函数仅在 CLI 单次运行中调用，
            // 泄漏量 = 一个短字符串，可忽略。
            let leaked: &'static str = Box::leak(url.into_boxed_str());
            [leaked, API_MIRROR_URL]
        }
        _ => [API_URL, API_MIRROR_URL],
    }
}

/// Async wrapper: run [`fetch_http_blocking`] on the blocking pool.
pub async fn fetch_http() -> Result<Vec<CatalogEntry>, String> {
    tokio::task::spawn_blocking(fetch_http_blocking)
        .await
        .map_err(|e| format!("join: {e}"))?
}

/// Build a catalog from a pre-parsed entries map (test/factory helper).
// 仅测试调用（非测试 bin 构建无调用方）。无条件 allow 消警：Windows 上此前
// 容忍警告作基线；Linux rustc 1.95 渲染该多行警告会 ICE（StyledBuffer 越界），
// 直接不产生警告则两端都干净。
#[allow(dead_code)]
pub fn catalog_from(entries: Vec<CatalogEntry>) -> Catalog {
    Catalog {
        version: 1,
        fetched_at: chrono::Local::now().to_rfc3339(),
        entries,
    }
}

/// Reverse map for diagnostics: family → keys (unused now, kept for the
/// `model catalog` listing UX).
// 同上：仅测试调用；无条件 allow 消警（含 Windows），同时避开 Linux 渲染 ICE。
#[allow(dead_code)]
pub fn by_family(catalog: &Catalog) -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    for e in &catalog.entries {
        let f = e.family.clone().unwrap_or_else(|| "(none)".to_string());
        m.entry(f).or_default().push(e.key.clone());
    }
    m
}

#[cfg(test)]
mod tests;
