//! LiteLLM 价目表过滤/合并（纯函数模块，**无 crate 内部依赖**）。
//!
//! 同一份代码两个消费者（单一真相源，改一处两边生效）：
//! - [`crate::pricing_lite`]（lib 内）：复用 [`LiteLLMEntry`] 反序列化 +
//!   URL 常量；
//! - `build.rs`（编译脚本，`#[path]` include）：下载 → [`filter_litellm_table`]
//!   → [`merge_extras`] → 写 `$OUT_DIR/model_prices_embedded.json`。
//!
//! 因此本文件**禁止**出现 `crate::` 路径或对本 crate 其他模块的任何依赖
//! ——build.rs 的模块树里没有它们。
//!
//! 内嵌表格式 = LiteLLM 原始形状（`model_name → entry` 的扁平 map，
//! per-token 计价），与运行时下载层完全同构——解析只有
//! [`crate::parse_litellm_json`] 一条路径。

use serde::{Deserialize, Serialize};

/// LiteLLM 表的默认下载地址（raw 直拉；ETag 增量）。运行时可通过
/// CLI `--url` / WSAPI `url` 参数覆盖（镜像场景——受限网络下指向可达的
/// 镜像端点）。
pub const LITELLM_PRICE_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// 下载镜像链（按序尝试，命中即止）。首条 = 官方 raw 地址；jsdelivr 两
/// 条 CDN 镜像服务 `raw.githubusercontent.com` 被干扰的受限网络。编译期
/// （build.rs）与运行时（`pricing_sync`）共用。
pub const PRICE_MIRROR_URLS: &[&str] = &[
    LITELLM_PRICE_URL,
    "https://fastly.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json",
    "https://cdn.jsdelivr.net/gh/BerriAI/litellm@main/model_prices_and_context_window.json",
];

/// 内嵌/下载表的最小条目数校验阈值：真表数千条，显著低于此 = 代理劫持
/// 页 / 截断 payload / 传错文件，宁可降级保旧表。
pub const MIN_FILTERED_ENTRIES: usize = 100;

/// 宽容 i64：上游 token 数字段偶发浮点形态（如 2M 窗口模型的
/// `"max_input_tokens": 2000000.0"`），严格 i64 会让**整表**解析失败
/// （编译期内嵌与运行时下载同炸——真机数据实证）。整型直取、浮点截断、
/// 其余形态 → None（缺失语义，诚实降级）。
fn de_i64_lenient<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    let v = match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i)
            } else {
                n.as_f64().map(|f| f as i64)
            }
        }
        _ => None,
    };
    Ok(v)
}

/// LiteLLM 表条目（宽容反序列化：未知字段一律忽略——LiteLLM 每个版本都
/// 会加字段，旧解析器吃新表不炸；序列化只写本结构字段，自动丢弃我们用
/// 不到的 supports_*/deprecation_date 等，内嵌体积从 2.3MB 压到 ~0.6MB）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LiteLLMEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost_per_token: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(
        default,
        deserialize_with = "de_i64_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_input_tokens: Option<i64>,
    #[serde(
        default,
        deserialize_with = "de_i64_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_tokens: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub litellm_provider: Option<String>,
    /// 同义别名（LiteLLM 上游无此字段，恒 None；精选补充层用它保留
    /// `deepseek/deepseek-chat` 等 provider 限定名）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
}

/// 条目是否被价目表收录（与 [`crate::parse_litellm_json`] 的收录规则一致）：
/// 只收 chat/completion，双基础价齐全且有限才收。
fn entry_accepted(e: &LiteLLMEntry) -> bool {
    match e.mode.as_deref() {
        Some("chat") | Some("completion") => {}
        _ => return false,
    }
    match (e.input_cost_per_token, e.output_cost_per_token) {
        (Some(i), Some(o)) => i.is_finite() && o.is_finite(),
        _ => false,
    }
}

/// 过滤 LiteLLM 原始表：解析 → 只留收录条目 → 按类型化条目重序列化
/// （丢弃未知字段）。`raw` 必须是顶层 map；解析失败或 0 条收录 → Err。
pub fn filter_litellm_table(raw: &str) -> Result<String, String> {
    let map: std::collections::BTreeMap<String, LiteLLMEntry> =
        serde_json::from_str(raw).map_err(|e| format!("LiteLLM price table parse failed: {e}"))?;
    let kept: std::collections::BTreeMap<&String, &LiteLLMEntry> =
        map.iter().filter(|(_, e)| entry_accepted(e)).collect();
    if kept.is_empty() {
        return Err(
            "LiteLLM price table filtered to 0 chat entries — wrong file or shape?".to_string(),
        );
    }
    serde_json::to_string(&kept).map_err(|e| format!("filtered table serialize failed: {e}"))
}

/// 把精选补充层合并进过滤表：**只补缺失键**（上游已有的条目以上游为
/// 权威），幂等（对已合并的表再跑一遍结果不变）。上游把 GLM/Qwen/Kimi
/// 等裸名键换成 provider 前缀键后，这层保证 `zhipu/glm-4.7` 等裸名配置
/// 仍可查（bare-suffix 依赖裸名键存在）。
pub fn merge_extras(filtered_json: &str, extras_json: &str) -> Result<String, String> {
    let mut map: std::collections::BTreeMap<String, LiteLLMEntry> = serde_json::from_str(
        filtered_json,
    )
    .map_err(|e| format!("filtered table parse failed: {e}"))?;
    let extras: std::collections::BTreeMap<String, LiteLLMEntry> =
        serde_json::from_str(extras_json)
            .map_err(|e| format!("extras table parse failed: {e}"))?;
    for (k, v) in extras {
        map.entry(k).or_insert(v);
    }
    serde_json::to_string(&map).map_err(|e| format!("merged table serialize failed: {e}"))
}

/// 解析 + 条目数校验（[`MIN_FILTERED_ENTRIES`]）。返回收录条目数。
pub fn validate_filtered_table(filtered_json: &str) -> Result<usize, String> {
    let map: std::collections::BTreeMap<String, LiteLLMEntry> =
        serde_json::from_str(filtered_json).map_err(|e| format!("table parse failed: {e}"))?;
    let n = map.values().filter(|e| entry_accepted(e)).count();
    if n < MIN_FILTERED_ENTRIES {
        return Err(format!(
            "filtered table has only {n} entries (threshold {MIN_FILTERED_ENTRIES}) — \
             likely a proxy junk page or truncated payload"
        ));
    }
    Ok(n)
}

#[cfg(test)]
mod tests;
