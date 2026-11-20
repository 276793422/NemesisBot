//! 代理 URL 日志脱敏（SEC-002 口径）。
//!
//! 代理 URL 可携带 userinfo 凭据（`http://user:pass@host:port`），凭据面
//! 与 api_key 同判（BUG-20261111-068 Debug 脱敏 / BUG-20261114-063 环境面
//! 同族）。日志与错误上下文输出代理值时只允许落 scheme、不落完整 URL。
//!
//! 本模块是全仓同构位点（`reqwest::Proxy::all` 消费面的 Ok/Err 日志臂）
//! 的单一真相源：BUG-20261115-005 修了 channels-telegram、voice、gateway
//! 环境面，providers 三个 lane 与 whatsapp 通道同形位点漏网
//!（BUG-20261120-018 收口于此）——各消费 crate（nemesis-channels /
//! nemesis-providers / nemesis-voice）均已依赖 nemesis-utils，统一引用
//! 本函数消除形态漂移。
//!
//! 注记：`reqwest::Proxy::all` 的错误 Display 不回显 URL 本身
//!（只落解析原因，如 `relative URL without a base`），`{e}` 单独打印安全；
//! 需要脱敏的是我们自己 format 进日志/错误上下文的原值。

/// 代理 URL 日志脱敏：只保留 scheme，其余统一遮蔽为 `<redacted>`。
///
/// - `"http://user:pass@host:port"` → `"http://<redacted>"`
/// - `"socks5h://10.0.0.1:1080"` → `"socks5h://<redacted>"`
/// - trim 后为空 / `"://…"`（scheme 段为空）→ `"<unknown-scheme>"`
///
/// 非 URL 形态（如裸词 `no-scheme-value`）按 005 前例原样当 scheme 段
/// 输出（`no-scheme-value://<redacted>`）——不猜测输入意图，只保证
/// `"://"` 之后的内容永不出现。
pub fn redact_proxy_for_log(proxy_url: &str) -> String {
    let scheme = proxy_url.trim().split("://").next().unwrap_or("");
    if scheme.is_empty() {
        "<unknown-scheme>".to_string()
    } else {
        format!("{scheme}://<redacted>")
    }
}

#[cfg(test)]
mod tests;
