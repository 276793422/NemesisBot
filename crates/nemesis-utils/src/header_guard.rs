//! 请求头防线（BUG-20261122-038；语义勘误 BUG-20261123-013）。
//!
//! **reqwest 真实语义（探针钉死，见 tests::header_probe）**：reqwest 0.12.x
//! 的 `RequestBuilder::header()` / `bearer_auth()` 对非法头名/头值**不是
//! panic**——`header_sensitive` Err 臂把错误收集进 builder（poison 形态），
//! `send()` 立即返回 `Err("builder error: ...")`，网络零触达。038 原记录
//! 「Err 臂 expect panic」是对旧版 reqwest 的过时记忆，已在 vendored 源码
//! 核实推翻（request.rs `header_sensitive`：`error = Some(error::builder(..))`
//! → `send()` 的 `Pending::new_err`）。
//!
//! 防线的真实价值 = **把首请求时才爆的匿名 builder error 上提为构造期
//! 点名报错**：builder error 文案只有 "invalid HTTP header value"，不说是
//! 哪个字段哪个头——粘贴带换行的 api_key 表现为「模型请求全挂但看不出
//! 为什么」，这正是 038 观察到的病象（错误叙事当时误判为 panic 伪装
//! Timeout；真形态 = 每 401 前先生成 builder error，排障方向被带偏）。
//! 上提到构造咽喉点后：字段名 + 违规 byte 码直接进错误信息，排障一步到位。
//!
//! 消毒点（`nemesis-providers::factory::create_provider`，全部生产 lane 的
//! 唯一装配点）：
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
            "{field} 含非法头字符（byte 0x{b:02x}；不 trim 的话首个请求会在 \
             send() 报不点名字段的 builder error）\
             ——请检查该字段是否从文件/剪贴板粘贴带了换行"
        )),
        None => Ok(trimmed.to_string()),
    }
}

/// 校验单个头对是否可安全落 reqwest 请求（BUG-20261123-013 抽出）。
///
/// 语义 = `HeaderName::from_str` / `HeaderValue::from_str` 精确语义（与
/// reqwest builder 内部校验同源，不复制词表防漂移）。Err 文案**点名头名、
/// 绝不带头值**（头值可携带凭据，SEC-002）。消费方：factory 的头对过滤
/// （合法对透传）、MCP http_transport 解析后值校验（非法对 WARN 跳过）
/// 等一切「动态数据 → 请求头」位点。
#[cfg(feature = "http")]
pub fn validate_header_pair(name: &str, value: &str) -> Result<(), String> {
    use std::str::FromStr;
    if let Err(e) = reqwest::header::HeaderName::from_str(name) {
        return Err(format!("头名 {name:?} 非法: {e}"));
    }
    if let Err(e) = reqwest::header::HeaderValue::from_str(value) {
        return Err(format!(
            "头 {name} 的值非法（控制字符/换行；reqwest 会报不点名的 \
             builder error）：{e}；值不回显"
        ));
    }
    Ok(())
}

/// 自定义头对过滤（http 门控：reqwest 不进未启用方依赖图，同 http_cap）。
///
/// 校验委托 [`validate_header_pair`]（单一真相源），非法对丢弃 + WARN：
/// 头名落日志、头值**永不**落日志（自定义头常携带凭据，SEC-002）。合法对
/// 原样透传（含大小写——HeaderName::from_str 对大写合法且 wire 侧小写化
/// 由 http 栈统一处理，与直传行为字节一致）。
#[cfg(feature = "http")]
pub fn filter_valid_header_pairs(
    lane: &str,
    headers: &std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(k, v)| match validate_header_pair(k, v) {
            Ok(()) => Some((k.clone(), v.clone())),
            Err(reason) => {
                // 值不落日志：validate_header_pair 的 Err 文案已按 SEC-002
                // 只含头名与校验器错误，不含头值。
                tracing::warn!(lane, header_name = %k, reason, "自定义头对非法已丢弃");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
