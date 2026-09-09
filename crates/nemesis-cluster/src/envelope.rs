//! Swarm M3 统一信封（impl-plan §5.1，D5 裁决）。
//!
//! 所有集群 RPC 业务 payload 统一形状（与 WSAPI `{module, cmd, reqId, data}`
//! 同构）：`{v, ns, op, corr_id, ok, error, body}`。RPC 层只注册一个
//! `nb_bus` action，信封内 `ns/op` 路由——加功能不加 action。
//!
//! - `ns`：命名空间（board = 看板/讨论区；将来 file / task / forge…）
//! - `op`：点号分层操作名（board.comment.post / board.wake.post / board.sync）
//! - 未知 ns/op 一律统一错误（向前兼容天然成立）
//! - 解析 struct 全带 `#[serde(default)]`（项目惯例）

use serde::{Deserialize, Serialize};

/// 当前信封版本。
pub const ENVELOPE_VERSION: u8 = 1;

/// `nb_bus` 统一 RPC action 名（master/worker 同名注册）。
pub const NB_BUS_ACTION: &str = "nb_bus";

/// 统一错误码（跨 ns 复用；未知错误一律 internal）。
pub mod error_code {
    pub const BAD_ENVELOPE: &str = "bad_envelope";
    pub const UNKNOWN_NS: &str = "unknown_ns";
    pub const UNKNOWN_OP: &str = "unknown_op";
    pub const VALIDATION: &str = "validation";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const QUOTA_EXHAUSTED: &str = "quota_exhausted";
    pub const DUPLICATE: &str = "duplicate";
    pub const UNAVAILABLE: &str = "unavailable";
    pub const INTERNAL: &str = "internal";
}

/// 统一错误位。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnvelopeError {
    pub code: String,
    pub message: String,
}

impl EnvelopeError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

/// 上行请求信封（master 收到的形状）。
///
/// `client_msg_id` 在 body 内（发送方生成的 uuid，上行幂等键）；信封层
/// 不感知它——幂等是 board ns 的落库语义，不是传输语义。
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Envelope {
    /// 信封版本（非 [`ENVELOPE_VERSION`] → bad_envelope）。
    pub v: u8,
    /// 命名空间。
    pub ns: String,
    /// 点号分层操作名。
    pub op: String,
    /// 多轮交互关联 ID（透传回响应）。
    pub corr_id: String,
    /// 业务载荷（ns/op 各自定义 schema）。
    pub body: serde_json::Value,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            v: ENVELOPE_VERSION,
            ns: String::new(),
            op: String::new(),
            corr_id: String::new(),
            body: serde_json::Value::Null,
        }
    }
}

/// 下行响应信封（请求方收到的形状）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(default)]
pub struct EnvelopeResponse {
    pub v: u8,
    pub ns: String,
    pub op: String,
    pub corr_id: String,
    pub ok: bool,
    pub error: Option<EnvelopeError>,
    pub body: serde_json::Value,
}

impl EnvelopeResponse {
    pub fn success(req: &Envelope, body: serde_json::Value) -> Self {
        Self {
            v: ENVELOPE_VERSION,
            ns: req.ns.clone(),
            op: req.op.clone(),
            corr_id: req.corr_id.clone(),
            ok: true,
            error: None,
            body,
        }
    }

    pub fn failure(req: &Envelope, error: EnvelopeError) -> Self {
        Self {
            v: ENVELOPE_VERSION,
            ns: req.ns.clone(),
            op: req.op.clone(),
            corr_id: req.corr_id.clone(),
            ok: false,
            error: Some(error),
            body: serde_json::Value::Null,
        }
    }

    /// 序列化为 RPC handler 返回的 JSON（handler 约定返回 Value）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({"v": ENVELOPE_VERSION, "ok": false,
                "error": {"code": error_code::INTERNAL, "message": "serialize failed"}})
        })
    }
}

/// 解析并校验上行信封。版本不符 / 缺 ns/op → bad_envelope。
pub fn parse_envelope(payload: &serde_json::Value) -> Result<Envelope, EnvelopeError> {
    let env: Envelope = serde_json::from_value(payload.clone())
        .map_err(|e| EnvelopeError::new(error_code::BAD_ENVELOPE, format!("malformed: {e}")))?;
    if env.v != ENVELOPE_VERSION {
        return Err(EnvelopeError::new(
            error_code::BAD_ENVELOPE,
            format!("unsupported envelope version: {} (want {})", env.v, ENVELOPE_VERSION),
        ));
    }
    if env.ns.is_empty() || env.op.is_empty() {
        return Err(EnvelopeError::new(
            error_code::BAD_ENVELOPE,
            "missing ns or op",
        ));
    }
    Ok(env)
}

/// 取 body 内的上行幂等键（无/空 = 不做幂等——下行 op 不需要）。
pub fn client_msg_id(body: &serde_json::Value) -> Option<&str> {
    body.get("client_msg_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests;
