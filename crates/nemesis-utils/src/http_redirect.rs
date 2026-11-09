//! 跨主机重定向终止策略（凭据随重定向泄漏防线）。
//!
//! reqwest 默认策略自动跟随 ≤10 跳，跨主机重定向只剥标准四头
//! （authorization / cookie / proxy-authorization / www-authenticate）；
//! `x-api-key`（anthropic lane）、`X-Acs-Dingtalk-Access-Token` 等自定义
//! 凭据头与 URL path 里的 token（telegram `/bot<token>/…`）都会原样带到
//! 重定向目标主机——模型 API / 通道上游返回（或被劫持后返回）一个 302，
//! 长效凭据就静默流向第三个主机。带凭据外呼的 client 统一换用
//! [`no_cross_host_redirect_policy`]：**同源重定向保留**（CDN/网关合规
//! 场景不断链），**跨源终止**（`attempt.stop()` 把 3xx 响应当最终响应
//! 透传，上层按非成功响应诚实处理——凭据不离开配置的主机）。
//!
//! 消费面（BUG-20261109-014 修复）：nemesis-providers 六 lane 全部
//! builder + nemesis-channels 三个 builder（base.rs 共享面 + matrix/
//! signal 独立面）。web_fetch / 图片预取等 SSRF 面已有 no-redirect +
//! 逐跳过闸的专用链路（image_attach::build_no_redirect_client），不属
//! 本防线职责。

use reqwest::redirect::{Attempt, Policy};

/// 跳数上限（与 reqwest 默认 `Policy::limited(10)` 同值——同源跳链行为
/// 不变，只新增跨源终止这一道）。
pub const MAX_REDIRECT_HOPS: usize = 10;

/// 重定向裁决纯函数（测试面）：`true` = 跟随，`false` = 终止。
/// 终止条件：跳数耗尽（`hops >= MAX_REDIRECT_HOPS`，与 reqwest 默认
/// 上限同语义）或目标 origin 与首跳 origin 不同；origin 键缺席（URL
/// 解析失败）fail-closed 终止。
///
/// origin 键 = `scheme://host:port`（[`origin_key`]）——同主机不同端口
/// 的服务是不同凭据接收面，一并拦（从紧方向，同源网关场景无端口漂移
/// 的实际需求）。
pub fn cross_host_redirect_decision(
    first_origin: Option<&str>,
    target_origin: Option<&str>,
    hops: usize,
) -> bool {
    if hops >= MAX_REDIRECT_HOPS {
        return false;
    }
    match (first_origin, target_origin) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// URL 的 origin 键（scheme://host:port；host 或端口缺席 = None——
/// fail-closed 由裁决函数终停）。
pub(crate) fn origin_key(url: &reqwest::Url) -> Option<String> {
    Some(format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str()?,
        url.port_or_known_default()?
    ))
}

/// 带凭据外呼 client 的重定向策略（单一真相源）：同源跟随 +
/// 跳数上限 10，跨源 `stop()`（3xx 透传为最终响应）。消费方只需在
/// builder 链上加 `.redirect(no_cross_host_redirect_policy())`。
pub fn no_cross_host_redirect_policy() -> Policy {
    Policy::custom(|attempt: Attempt| {
        let first_origin = attempt.previous().first().and_then(origin_key);
        let target_origin = origin_key(attempt.url());
        if cross_host_redirect_decision(
            first_origin.as_deref(),
            target_origin.as_deref(),
            attempt.previous().len(),
        ) {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

#[cfg(test)]
mod tests;
