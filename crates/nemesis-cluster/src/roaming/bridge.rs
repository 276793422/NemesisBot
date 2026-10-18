//! A1 — iroh bi-stream ↔ 集群帧协议桥接（P2 组件层）。
//!
//! 现有 TCP 路径的帧协议组件（`transport::frame`：4B 长度前缀 + AEAD）与
//! 底层 IO 解耦——`AsyncFrameReader<R>`/`write_frame_async<W>` 泛型于
//! `AsyncRead`/`AsyncWrite`。iroh 的 bi-stream（QUIC 双向流）实现了这两个
//! trait，因此桥接是**薄封装**：同一套帧字节格式 + 同一套 AEAD token 语义，
//! 换个 IO 载体而已——LAN TCP 与跨网 iroh 的帧层字节兼容。
//!
//! 信任分层照旧：iroh EndpointId = 传输身份（QUIC-TLS 握手即验身），
//! AEAD token = 应用授权（本桥每帧照常加解密——语义独立于传输加密，
//! iroh 的 TLS 不替代 token 校验）。

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::transport::frame::{
    AES_KEY_SIZE, AsyncFrameReader, decrypt_frame, derive_key, encrypt_frame, write_frame_async,
};

/// iroh 地址前缀（`iroh:<64位小写hex>`）——dial 分派依据。
pub const IROH_ADDR_PREFIX: &str = "iroh:";

/// 是否 iroh 形态地址（前缀判断；P2 dial 分派第一闸）。
pub fn is_roaming_address(addr: &str) -> bool {
    addr.trim_start().starts_with(IROH_ADDR_PREFIX)
}

/// `iroh:<hex>[@<ip:port>]` → [`iroh::EndpointAddr`]（剥前缀 + hex 解码；
/// 非法 loud 报错）。
///
/// 两种形态：
/// - `iroh:<hex>`：仅 node_id——依赖发现层供址（N0 预设的 DNS/relay）。
/// - `iroh:<hex>@<socket>`：钉直连地址——Minimal 预设（无发现）/ 受限网络
///   / loopback 测试的诚实出口（`EndpointAddr::with_ip_addr`）。
pub fn parse_roaming_address(addr: &str) -> Result<iroh::EndpointAddr, String> {
    let s = addr.trim();
    let rest = s
        .strip_prefix(IROH_ADDR_PREFIX)
        .ok_or_else(|| format!("非 iroh 形态地址（缺少 `{IROH_ADDR_PREFIX}` 前缀）: {addr}"))?;
    let (hex_part, socket_part) = match rest.split_once('@') {
        Some((h, sock)) => {
            if sock.trim().is_empty() {
                return Err(format!("iroh 地址 `@` 后缺直连地址: {addr}"));
            }
            (h, Some(sock.trim()))
        }
        None => (rest, None),
    };
    let node_id = super::endpoint::node_id_from_hex(hex_part)?;
    let base = iroh::EndpointAddr::from(node_id);
    match socket_part {
        Some(sock) => {
            let sa: std::net::SocketAddr = sock
                .parse()
                .map_err(|e| format!("iroh 直连地址非法（应为 ip:port）: {sock}: {e}"))?;
            Ok(base.with_ip_addr(sa))
        }
        None => Ok(base),
    }
}

// ---------------------------------------------------------------------------
// IrohBiStream — (SendStream, RecvStream) → 单一 AsyncRead + AsyncWrite
// ---------------------------------------------------------------------------

/// iroh bi-stream 组合器：QUIC 的收/发是两个独立类型（`RecvStream: AsyncRead`、
/// `SendStream: AsyncWrite`），而集群连接引擎（`TcpConn` 读写循环）吃的是
/// **单一** `AsyncRead + AsyncWrite` 流。此类型把二者拼成一个，使 `TcpConn`
/// 原样跑在 iroh 流上——帧协议/AEAD/空闲监测全部复用，字节语义零分叉。
pub struct IrohBiStream {
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
}

impl IrohBiStream {
    /// 拼装（`connect`/`accept_bi`/`open_bi` 产出的对）。
    pub fn new(send: iroh::endpoint::SendStream, recv: iroh::endpoint::RecvStream) -> Self {
        Self { send, recv }
    }
}

impl tokio::io::AsyncRead for IrohBiStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for IrohBiStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        // SendStream 的固有 poll_write 返回自家 WriteError（Rust 方法解析固有
        // 优先于 trait）——与 noq 自身 AsyncWrite impl 同款 `Into::into` 转发。
        std::pin::Pin::new(&mut self.send)
            .poll_write(cx, buf)
            .map(|r| r.map_err(std::convert::Into::into))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // QUIC 流没有 flush 概念（写即发）；frames 语义由上层写入方决定。
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // `write_half.shutdown()`（TcpConn 写循环收尾）映射为 QUIC finish：
        // 本端不再发数据，对端读到 EOF——与 TCP shutdown 语义对齐。
        let _ = self.send.finish();
        std::task::Poll::Ready(Ok(()))
    }
}

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
