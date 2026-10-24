//! A1 — 集群帧协议组件（QUIC 插件化 M3 后宿主侧仅存的桥件）。
//!
//! 现有 TCP 路径的帧协议组件（`transport::frame`：4B 长度前缀 + AEAD）与
//! 底层 IO 解耦——`AsyncFrameReader<R>`/`write_frame_async<W>` 泛型于
//! `AsyncRead`/`AsyncWrite`。QUIC 插件化 M3：iroh bi-stream 本体已随端点
//! 迁入 plugin_roaming（`IrohBiStream` 组合器与地址解析一并退役），宿主侧
//! 帧收发载体换 loopback TCP（`TcpStream` 直实现两 trait）——[`RoamingConn`]
//! 原样复用：同一套帧字节格式 + 同一套 AEAD token 语义，LAN TCP 与跨网
//! 漫游的帧层字节兼容。
//!
//! 信任分层照旧：iroh EndpointId = 传输身份（QUIC-TLS 握手即验身，裁决在
//! 插件 accept 闸），AEAD token = 应用授权（本桥每帧照常加解密——语义
//! 独立于传输加密，iroh 的 TLS 不替代 token 校验）。

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::transport::frame::{
    AES_KEY_SIZE, AsyncFrameReader, decrypt_frame, derive_key, encrypt_frame, write_frame_async,
};

/// iroh 地址前缀（`iroh:<64位小写hex>`）——单一真相源在 lib.rs（无 feature
/// 门，announce 接收侧地址仲裁同样需要）；此处 re-export 保路径兼容。
pub use crate::{IROH_ADDR_PREFIX, is_roaming_address};

// ---------------------------------------------------------------------------
// 帧桥连接
// ---------------------------------------------------------------------------

/// iroh bi-stream 上的帧连接（send = QUIC SendStream，recv = RecvStream）。
///
/// 帧格式与 TCP 路径逐字节一致：`[4B BE 长度][载荷]`；配置了 token 时载荷
/// 为 `[12B nonce][密文+tag]`（AES-256-GCM，token SHA-256 派生密钥）。
/// 泛型于 IO 使 duplex 单测无需真实 iroh 端点。
pub struct RoamingConn<S, R> {
    send: S,
    reader: AsyncFrameReader<R>,
    key: [u8; AES_KEY_SIZE],
    /// token 是否启用（空 token = 明文帧；TCP 路径同语义）。
    token_on: bool,
}

impl<S: AsyncWrite + Unpin, R: AsyncRead + Unpin> RoamingConn<S, R> {
    /// 建桥（token 派生 AEAD 密钥；空 token = 明文帧——与 TCP 路径同语义）。
    pub fn new(send: S, recv: R, auth_token: &str) -> Self {
        let token_on = !auth_token.is_empty();
        Self {
            send,
            reader: AsyncFrameReader::new(recv),
            key: derive_key(auth_token),
            token_on,
        }
    }

    /// 发一帧（有 token = 先加密）。
    pub async fn send_frame(&mut self, payload: &[u8]) -> Result<(), String> {
        let wire = if self.token_enabled() {
            encrypt_frame(payload, &self.key)?
        } else {
            payload.to_vec()
        };
        write_frame_async(&mut self.send, &wire)
            .await
            .map_err(|e| format!("iroh 帧写失败: {e}"))
    }

    /// 收一帧（有 token = 解密；返回明文载荷）。
    pub async fn recv_frame(&mut self) -> Result<Vec<u8>, String> {
        let wire = self
            .reader
            .read_frame()
            .await
            .map_err(|e| format!("iroh 帧读失败: {e}"))?;
        if self.token_enabled() {
            decrypt_frame(&wire, &self.key)
        } else {
            Ok(wire)
        }
    }

    /// JSON 便利发送。
    pub async fn send_json<T: Serialize>(&mut self, value: &T) -> Result<(), String> {
        let bytes = serde_json::to_vec(value).map_err(|e| format!("JSON 序列化失败: {e}"))?;
        self.send_frame(&bytes).await
    }

    /// JSON 便利接收。
    pub async fn recv_json<T: DeserializeOwned>(&mut self) -> Result<T, String> {
        let bytes = self.recv_frame().await?;
        serde_json::from_slice(&bytes).map_err(|e| format!("JSON 反序列化失败: {e}"))
    }

    /// token 是否启用。
    fn token_enabled(&self) -> bool {
        self.token_on
    }
}

#[cfg(test)]
mod tests;
