//! Provider 成功响应体有帽读取（BUG-20261115-024）。
//!
//! BUG-20261113-164 修了错误体（`http_cap::read_error_body` 256KB 截断），
//! BUG-20261101-273 / BUG-20261107-005 修了下载数据面，但 8 个 provider
//! lane 的**成功体**仍是裸 `resp.text().await`——reqwest 无默认体上限，
//! 失控/恶意端点可在 200 状态码下把 GB 级 body 灌满进程内存（600s 墙钟
//! 超时在快管路上不约束体积；流式 SSE 读循环有 deadline 兜底=不同量级，
//! 唯独「非流式成功体一次读全」面零防护）。
//!
//! 本模块是 provider 成功体的统一入口：`read_success_body` 走
//! `http_cap::read_body_capped_typed`（超限在读入中途断开，内存峰值=已
//! 到达前缀），`to_failover_error` 把分型错误归入 [`FailoverError`]——
//! 超限 = `Unknown` + `"response body exceeds limit"` 前缀锚（可重试，
//! 坏端点换下一家，对齐 2026-10-13 诚实化配套同一精神），传输失败照旧
//! `transport_error`。
//!
//! 合法响应体量级：LLM JSON 补全 ≤ 数 MB（128K ctx + max_output，8x
//! 余量钉 64MB 帽）。刻意不帽的面见 BUG_FIX 横向排查节（images.rs b64
//! 已文档化决策、SSE 增量累积面）。

use crate::failover::{FailoverError, transport_error};

/// 单次成功响应体上限：64MB。
///
/// 依据：全量上下文（~128K tokens ≈ 512KB UTF-8）+ 大 max_output +
/// tool_calls/reasoning 冗余，合法响应 ≤ ~8MB；8x 余量容中转网关打包
/// 差额，仍把恶意灌流限制在「读入中途断开」的常数内存内。
pub const RESPONSE_BODY_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// 成功体读取错误（分型视图，转发 [`nemesis_utils::http_cap::BodyReadError`]）。
#[derive(Debug)]
pub enum SuccessBodyError {
    /// 传输读失败（连接中断/超时等）——归 `transport_error`（既有语义不变）。
    Transport(reqwest::Error),
    /// 超帽，读取中途断开——独立错误类。
    ExceedsLimit { cap: u64 },
}

/// 读 provider 成功响应体（有帽，String 视图）。
pub async fn read_success_body(resp: reqwest::Response) -> Result<String, SuccessBodyError> {
    let bytes = nemesis_utils::http_cap::read_body_capped_typed(resp, RESPONSE_BODY_LIMIT_BYTES)
        .await
        .map_err(|e| match e {
            nemesis_utils::http_cap::BodyReadError::Transport(t) => SuccessBodyError::Transport(t),
            nemesis_utils::http_cap::BodyReadError::ExceedsLimit { cap } => {
                SuccessBodyError::ExceedsLimit { cap }
            }
        })?;
    // reqwest .text() 本就是 lossy 解码（charset 检测 + 替换序列）；LLM
    // JSON 体必须 UTF-8，lossy 与既有行为对齐（非法序列不炸、不伪装）。
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// 分型错误 → [`FailoverError`]（provider 位点统一的 map_err 端点）。
///
/// - `Transport` → `transport_error`（真超时→Timeout；连接失败→Unknown +
///   "connect error" 前缀，与全 lane 口径一致）。
/// - `ExceedsLimit` → `Unknown` + `"response body exceeds limit"` 前缀锚
///   （`is_retriable` 据此轮转 fallback——端点失控是端点问题，换下一家）。
///   model 编进 message（`Unknown` 无 model 槽位，对齐既有约定）。
pub fn to_failover_error(provider: &str, model: &str, e: SuccessBodyError) -> FailoverError {
    match e {
        SuccessBodyError::Transport(t) => transport_error(provider, model, &t),
        SuccessBodyError::ExceedsLimit { cap } => FailoverError::Unknown {
            provider: provider.to_string(),
            message: format!(
                "response body exceeds limit (model {model}): >{cap} bytes; aborted mid-read"
            ),
        },
    }
}
