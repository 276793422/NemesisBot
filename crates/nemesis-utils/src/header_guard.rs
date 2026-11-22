//! 请求头防线（BUG-20261122-038）：reqwest 的 `.header()` / `bearer_auth()` 对
//! 非法头名/头值是 panic 语义（`HeaderValue::from_str` 拒 \r/\n 等控制字符，
//! reqwest 源码 header_sensitive 对 Err 臂 expect）——api_key 粘贴带尾随换行 /
//! 自定义头多行值会在**首次请求构造**时炸掉请求任务：streaming 链路
//! panic → tx drop → rx None → 上游伪装成可重试 Timeout，根因被吞；非流式
//! 链路直接炸 chat 任务。
//!
//! 防线 = 构造咽喉点（`nemesis-providers::factory::create_provider`，全部
//! 生产 lane 的唯一装配点）一次性消毒：
//! - 凭据面（api_key / account_id）：trim + 控制字符拒绝——`Bearer {key}`
//!   组合值与 x-api-key 原值只要控制字符-free 即保证合法；
//! - 自定义头对（headers map）：按 `HeaderName`/`HeaderValue::from_str`
//!   精确语义拦截，非法对丢弃 + WARN（名落日志、**值永不落日志**——
//!   自定义头可携带凭据，SEC-002 口径）。
//!
//! 单一真相源：头对校验语义直接调 reqwest::header 的 from_str（不复制
//! token/可见字符词表，防漂移）；凭据面是纯字符串扫描（零依赖，任何
//! lane 可用）。

/// 控制字符扫描（凭据面）：无 bytes <0x20（\t 例外——HeaderValue 允许 tab）
/// 或 0x7F 即安全。
fn first_bad_byte(s: &str) -> Option<u8> {
    s.bytes().find(|&b| (b < 0x20 && b != b'\t') || b == 0x7f)
}

/// Trim + 校验将落入请求头的凭据字段（API key / account id）。
///
/// Err = 含控制字符/换行（提示只落违规 byte 码，不带字段内容——字段可能
/// 是凭据）；Ok = trim 后的值（**空串放行**——保留空 key 现网 wire 行为，
/// HttpCompat 空 key 在 resolve_provider_selection 早臂已 loud Err）。
pub fn header_safe_credential(field: &str, raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    match first_bad_byte(trimmed) {
        Some(b) => Err(format!(
            "{field} 含非法头字符（byte 0x{b:02x}；换行/控制字符会使请求构造 panic）\
             ——请检查该字段是否从文件/剪贴板粘贴带了换行"
        )),
        None => Ok(trimmed.to_string()),
    }
}

/// 自定义头对过滤（http 门控：reqwest 不进未启用方依赖图，同 http_cap）。
///
/// 校验语义 = `HeaderName::from_str` / `HeaderValue::from_str`（与 reqwest
/// `.header()` panic 闸同源），非法对丢弃 + WARN：头名落日志、头值**永不**
/// 落日志（自定义头常携带凭据，SEC-002）。合法对原样透传（含大小写——
/// HeaderName::from_str 对大写合法且 wire 侧小写化由 http 栈统一处理，
/// 与直传行为字节一致）。
#[cfg(feature = "http")]
pub fn filter_valid_header_pairs(
    lane: &str,
    headers: &std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    use reqwest::header::{HeaderName, HeaderValue};
    use std::str::FromStr;
    headers
        .iter()
        .filter_map(|(k, v)| {
            if let Err(e) = HeaderName::from_str(k) {
                tracing::warn!(
                    lane,
                    header_name = %k,
                    error = %e,
                    "自定义头名非法已丢弃（原样透传会使请求构造 panic）"
                );
                return None;
            }
            if let Err(e) = HeaderValue::from_str(v) {
                tracing::warn!(
                    lane,
                    header_name = %k,
                    error = %e,
                    "自定义头值含非法字符（控制字符/换行）已丢弃；值不落日志（可携带凭据）"
                );
                return None;
            }
            Some((k.clone(), v.clone()))
        })
        .collect()
}

mod tests;
