//! WS 帧编解码——反向桥服务端 WS 长泵模式的转发层（一期批次一）。
//!
//! 场景：浏览器与设备 web server 之间的 WS 升级经桥转发。升级握手完成后，
//! 浏览器侧由 axum 持有（`axum::extract::ws::Message` 抽象），而设备侧的
//! conn_data 帧载荷是本机 web server 写出的**原始 ws 帧字节序列**（服务端
//! 帧：unmasked）。服务端必须做一次帧格式转换：
//!
//! - 浏览器 → 设备：axum Message → **client 帧**（必须 mask——RFC 6455
//!   规定客户端帧必须带掩码，否则设备侧 tungstenite 直接协议违规断连）
//! - 设备 → 浏览器：原始字节流按 ws 帧边界解析 → axum Message
//!
//! **为何不用 tungstenite 的 Frame API**：tokio-tungstenite 0.26 的底层
//! Frame 接口经历重构且面向内部 writer，直接依赖其私有形态不稳定——ws
//! 帧头本身极简（RFC 6455 §5.2），手写 ~70 行更可控。**这不违反「桥不
//! 解析内容」原则**：桥搬运的载荷字节原样进出，帧头是转发层自加自减的
//! 信封（与响应头解析同类——转发必需，不碰业务内容）。

use axum::extract::ws::Message;

/// 浏览器 → 设备：把 axum Message 编码为 client 形态 ws 帧（masked）。
pub fn encode_client_frame(msg: &Message) -> Vec<u8> {
    let (opcode, payload): (u8, &[u8]) = match msg {
        Message::Text(t) => (0x1, t.as_str().as_bytes()),
        Message::Binary(b) => (0x2, b),
        Message::Ping(p) => (0x9, p),
        Message::Pong(p) => (0xA, p),
        Message::Close(_) => (0x8, &[]),
    };
    build_frame(opcode, payload, true)
}

/// 设备 → 浏览器：从字节缓冲头部解析**一个** server 帧（unmasked）。
/// 返回 (消息, 消耗字节数)；缓冲不足一个完整帧 → None。
pub fn parse_server_frame(buf: &[u8]) -> Option<(Message, usize)> {
    if buf.len() < 2 {
        return None;
    }
    let fin = buf[0] & 0x80 != 0;
    let opcode = buf[0] & 0x0F;
    let masked = buf[1] & 0x80 != 0;
    let mut len = (buf[1] & 0x7F) as usize;
    let mut pos = 2;
    if len == 126 {
        if buf.len() < pos + 2 {
            return None;
        }
        len = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
        pos += 2;
    } else if len == 127 {
        if buf.len() < pos + 8 {
            return None;
        }
        let mut b8 = [0u8; 8];
        b8.copy_from_slice(&buf[pos..pos + 8]);
        len = u64::from_be_bytes(b8) as usize;
        pos += 8;
    }
    // server 帧不该 masked（RFC 6455）；防御式处理：有 mask 也吃掉 4 字节。
    let mask = if masked {
        if buf.len() < pos + 4 {
            return None;
        }
        let m = [buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]];
        pos += 4;
        Some(m)
    } else {
        None
    };
    if buf.len() < pos + len {
        return None;
    }
    let mut payload = buf[pos..pos + len].to_vec();
    let consumed = pos + len;
    if !fin {
        // 分片帧：dashboard 无分片大帧场景（axum ws 单帧上限内），不完整
        // 支持分片重组——遇中间分片按整体丢弃 + 上层 WARN（由调用方记录）。
        // 返回一个占位 Text 避免协议错乱，消耗字节保证流不卡死。
        return Some((Message::Text(String::new().into()), consumed));
    }
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i % 4];
        }
    }
    let msg = match opcode {
        0x1 => match String::from_utf8(payload) {
            Ok(t) => Message::Text(t.into()),
            Err(_) => Message::Binary(Vec::new().into()),
        },
        0x2 => Message::Binary(payload.into()),
        0x9 => Message::Ping(payload.into()),
        0xA => Message::Pong(payload.into()),
        0x8 => Message::Close(None),
        _ => Message::Binary(payload.into()),
    };
    Some((msg, consumed))
}

/// 构造一个 ws 帧。`client_role=true` 时加随机掩码（RFC 6455：客户端帧
/// 必须掩码）。
fn build_frame(opcode: u8, payload: &[u8], client_role: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(payload.len() + 14);
    buf.push(0x80 | opcode); // FIN + opcode（不做分片）
    let mask_bit = if client_role { 0x80u8 } else { 0u8 };
    let len = payload.len();
    if len < 126 {
        buf.push(mask_bit | len as u8);
    } else if len <= u16::MAX as usize {
        buf.push(mask_bit | 126);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(mask_bit | 127);
        buf.extend_from_slice(&(len as u64).to_be_bytes());
    }
    if client_role {
        // 掩码 key 随机（不加密，仅协议要求）；掩码 key 可预测不构成
        // 安全问题——它本来就不是安全机制。
        let key: [u8; 4] = rand_mask_key();
        buf.extend_from_slice(&key);
        let start = buf.len();
        buf.extend_from_slice(payload);
        let end = buf.len();
        for (i, b) in buf[start..end].iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    } else {
        buf.extend_from_slice(payload);
    }
    buf
}

/// 生成 4 字节掩码 key（std-only：从两个独立熵源混合；非安全场景够用）。
fn rand_mask_key() -> [u8; 4] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let addr = &nanos as *const u64 as usize;
    let mixed = nanos ^ ((addr as u64).rotate_left(17)) ^ (std::process::id() as u64) << 32;
    mixed.to_le_bytes()[..4].try_into().unwrap_or([0; 4])
}

// AGT 覆盖率批次（2026-09-25）：encode 的 Binary 臂 + 126 编码臂、parse 的
// 16 位长度截断/完整、64 位截断、FIN=0 占位、text 非法 UTF-8 兜底、Pong、
// 保留 opcode 兜底。豁免（build_frame server_role 死臂）见 agt_tests 头注。
#[cfg(test)]
mod agt_tests;
