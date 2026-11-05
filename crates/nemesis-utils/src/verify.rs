//! 恒时 token 比较。
//!
//! XOR 折叠常数时间形态（纵深防御设计取舍）：长度相等 +
//! 逐字节 XOR 折叠 == 0 ≡ `==`，但不因内容提前短路。不引 subtle 依赖。
//! 理论时序侧信道在真实网络抖动噪声下现实门槛高——修它是把纵深防御
//! 决策补齐到全覆盖，不是堵已知洞。
//!
//! 历史：本函数原住 nemesis-web `api_handlers.rs`（引入，REST 面），
//! 把 web crate 内三条 WS 升级闸换到本函数；
//! 发现 355 横扫半径未含 nemesis-channels（其 websocket 通道升级闸仍是裸
//! `==`）→ 本函数下沉到 nemesis-utils 供跨 crate 单源消费，web 侧
//! `api_handlers::verify_token` 路径保持 re-export 兼容。

/// Constant-time(ish) token comparison: `true` iff `token == expected`.
///
/// - `expected` 为空 → `true`（调用方语义：「未配置 token = 不设闸」）；
///   需要强制非空的调用方自行前置 `!expected.is_empty()` 闸。
/// - 长度不等 → `false`（长度本身可从流量观测，非秘密）。
/// - 长度相等 → 逐字节 XOR 折叠，比较次数与内容无关。
pub fn verify_token(token: &str, expected: &str) -> bool {
    if expected.is_empty() {
        return true;
    }
    if token.len() != expected.len() {
        return false;
    }
    token
        .bytes()
        .zip(expected.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests;
