//! Executor wire 协议薄 crate（P5 底座，2026-10-17）。
//!
//! 「gateway ↔ executor 子进程」之间一行一 JSON 的 wire 协议（v1/v2 双形态
//! 与 call_id 回显）、传输抽象（`ExecutorStream` trait）与交换原语的
//! **单一真相源**。原实现住在 `nemesis-agent/src/executor_transport.rs`
//! （P3 T1），P5 桌面级 VM 需要第三个消费方——guest agent（VM 内执行体）
//! ——而 guest crate 不可能依赖 nemesis-agent（整包依赖图太重），所以协议
//! 本体抽薄到这里（零 nemesis 依赖），两侧同源消费，防两端口径漂移。
//!
//! 消费方：
//! - `nemesis-agent::executor_transport`：re-export 本 crate（现有调用点
//!   零改动），并继续持有 stdio/persistent 会话等宿主侧载体。
//! - `nemesis-guest`（P5）：guest agent 的协议循环（musl 静态二进制 +
//!   宿主双栖直跑）。
//!
//! 协议版本纪律（与 P3 一致）：
//! - **v1**（现状）：`{tool,args,context}` → `{ok,result,error}`。per-call
//!   路径继续产出 v1 字节（request 不带 `call_id`/`v` 键——serde skip），
//!   现有测试与外部断言零迁移。
//! - **v2**：`{v:2, call_id, tool, args, context}` →
//!   `{v:2, call_id, ok, result, error}`。persistent 会话与 VM 通道恒走
//!   v2；`call_id` 是在途调用幂等（journal replay）与响应匹配的键。
//! - 子进程侧双解析：v1/v2 同循环接受（同一二进制自 spawn 自，不存在
//!   生产混布；双解析是为了测试与逐批演进的平滑性）。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt};

/// v2 协议版本号（request/response 的 `v` 字段值；v1 不写该键，反序列化
/// 缺省 0）。
pub const PROTOCOL_V2: u32 = 2;

// ---------------------------------------------------------------------------
// Wire structs（两侧单一真相源）
// ---------------------------------------------------------------------------

/// Gateway → executor 请求（序列化形态，借用字段）。
///
/// `v` / `call_id` 在空值时**不写键**（`skip_serializing_if`）：per-call
/// 路径产出与历史逐字节一致的 v1 线形态；persistent 路径填 v2 值。
#[derive(Serialize)]
pub struct ExecutorRequestRef<'a> {
    #[serde(skip_serializing_if = "is_zero")]
    pub v: u32,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub call_id: &'a str,
    pub tool: &'a str,
    /// 原始 tool-args JSON 串——LLM 产出的原文，本地 `Tool::execute` 的
    /// `args: &str` 原样接收。
    pub args: &'a str,
    /// 序列化后的 gateway 请求上下文（`async_callback` 经 `#[serde(skip)]`
    /// 丢弃）。
    pub context: serde_json::Value,
}

/// Gateway → executor 请求（反序列化形态，子进程侧用；两侧共用字段名防
/// 镜像漂移）。
#[derive(Deserialize)]
pub struct ExecutorRequestOwned {
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub call_id: String,
    pub tool: String,
    pub args: String,
    pub context: serde_json::Value,
}

/// Executor → gateway 响应。v1 形态不带 `v`/`call_id`（反序列化缺省）；
/// v2 恒带。序列化侧同样空 `call_id` 不写键（per-call 响应字节不变）。
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct ExecutorResponseWire {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub v: u32,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    pub call_id: String,
    pub ok: bool,
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub error: String,
}

impl ExecutorResponseWire {
    /// 纯错误响应（engage 失败 / 解析失败等工具循环外的失败臂）。
    pub fn error(error: &str) -> Self {
        Self {
            ok: false,
            error: error.to_string(),
            ..Default::default()
        }
    }

    /// 是否 v2 请求（`v` 字段钉 2 且 call_id 非空——两条件都满足才走
    /// 幂等账路径，防半填形态误入）。
    pub fn is_v2(&self) -> bool {
        self.v == PROTOCOL_V2 && !self.call_id.is_empty()
    }
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

// ---------------------------------------------------------------------------
// call_id 生成（gateway 侧；进程内唯一即可——journal 以「进程生命周期 +
// 文件」为账本，跨 gateway 重启的 call_id 冲突不存在：wake 后 journal 里
// 的历史条目按 call_id 精确匹配，新调用生成新 id）
// ---------------------------------------------------------------------------

static CALL_ID_SEQ: AtomicU64 = AtomicU64::new(1);

/// 生成新 call_id：`c-{millis}-{seq}`。seq 进程内单调，millis 提供跨重启
/// 区分度（同毫秒内 seq 兜底唯一性）。
pub fn new_call_id() -> String {
    let seq = CALL_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("c-{millis}-{seq}")
}

// ---------------------------------------------------------------------------
// v2 请求行构造（supervisor / guest 测试 / VM 通道共用）
// ---------------------------------------------------------------------------

/// 构造 v2 请求行（含尾部换行）。supervisor 日常调用/replay 与 VM 通道
/// 共用——单一真相源。
pub fn build_v2_request_line(
    tool: &str,
    args: &str,
    context: &serde_json::Value,
    call_id: &str,
) -> Result<String, String> {
    let request = ExecutorRequestRef {
        v: PROTOCOL_V2,
        call_id,
        tool,
        args,
        context: context.clone(),
    };
    let mut line =
        serde_json::to_string(&request).map_err(|e| format!("serialize executor request: {e}"))?;
    line.push('\n');
    Ok(line)
}

// ---------------------------------------------------------------------------
// Transport trait（stdio / 具名管道 / vsock / NAT-TCP 统一接口）
// ---------------------------------------------------------------------------

/// executor 字节传输的最小接口。任何 `AsyncRead + AsyncWrite + Unpin +
/// Send` 的对象自动实现（blanket impl）——stdio 双工、Windows 具名管道
/// （tokio `NamedPipeServer/Client` 原生满足）、P5 的 TCP/vsock 流同理。
pub trait ExecutorStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> ExecutorStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

/// 拆分后的读写半（persistent 会话同时持有：BufReader 跨请求保留缓冲，
/// 写半独立 flush——单对象双工必须先 split 才能同时读写）。
pub type BoxStreamRead = Box<dyn AsyncRead + Unpin + Send>;
pub type BoxStreamWrite = Box<dyn AsyncWrite + Unpin + Send>;

/// 把双工流拆成 owned 读写半。
pub fn split_duplex<S: ExecutorStream + 'static>(stream: S) -> (BoxStreamRead, BoxStreamWrite) {
    let (r, w) = tokio::io::split(stream);
    (Box::new(r), Box::new(w))
}

// ---------------------------------------------------------------------------
// TransportError
// ---------------------------------------------------------------------------

/// 传输层错误（persistent 会话面）。超时与 EOF 分列——EOF = 子进程退出
/// （supervisor 的崩溃重建/回退判定依据），Timeout = 子进程活着但没回话。
#[derive(Debug)]
pub enum TransportError {
    Timeout,
    /// 流关闭（子进程退出/管道断开），无响应。
    Eof,
    Io(std::io::Error),
    /// 线格式/协议违规（坏 JSON、call_id 不匹配等）。
    Protocol(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "executor transport timed out"),
            Self::Eof => write!(f, "executor transport closed without a response"),
            Self::Io(e) => write!(f, "executor transport io error: {e}"),
            Self::Protocol(m) => write!(f, "executor transport protocol error: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

// ---------------------------------------------------------------------------
// 交换原语（一行请求 → 一行响应；per-call / persistent / VM 通道共用）
// ---------------------------------------------------------------------------

/// 序列化请求并写一行（含换行 + flush）。
pub async fn send_request_line(
    writer: &mut (dyn AsyncWrite + Unpin + Send),
    request_line: &str,
) -> Result<(), TransportError> {
    writer
        .write_all(request_line.as_bytes())
        .await
        .map_err(TransportError::Io)?;
    writer.flush().await.map_err(TransportError::Io)
}

/// 读一行响应（带超时）并解析。`expected_call_id` 非空时校验响应的
/// `call_id` 匹配（journal replay 后乱序/串话防线——虽然 persistent 会话
/// 内严格逐请求应答，校验是把契约钉进代码而不是钉进注释）。
pub async fn read_response<R>(
    reader: &mut R,
    timeout: Duration,
    expected_call_id: Option<&str>,
) -> Result<ExecutorResponseWire, TransportError>
where
    R: tokio::io::AsyncBufRead + Unpin + Send,
{
    let mut line = String::new();
    let read = tokio::time::timeout(timeout, reader.read_line(&mut line)).await;
    match read {
        Err(_) => return Err(TransportError::Timeout),
        Ok(Err(e)) => return Err(TransportError::Io(e)),
        Ok(Ok(0)) => return Err(TransportError::Eof),
        Ok(Ok(_)) => {}
    }
    let line = line.trim_end_matches(['\n', '\r']);
    let resp: ExecutorResponseWire = serde_json::from_str(line)
        .map_err(|e| TransportError::Protocol(format!("bad response line: {e}")))?;
    if let Some(expected) = expected_call_id
        && !expected.is_empty()
        && resp.call_id != expected
    {
        return Err(TransportError::Protocol(format!(
            "response call_id mismatch: expected {expected}, got {}",
            resp.call_id
        )));
    }
    Ok(resp)
}

/// 会话内一次调用的传输原语（无 Child 依赖——duplex 可测）。
pub async fn call_over_halves(
    writer: &mut (dyn AsyncWrite + Unpin + Send),
    reader: &mut tokio::io::BufReader<BoxStreamRead>,
    request_line: &str,
    expected_call_id: Option<&str>,
    timeout: Duration,
) -> Result<ExecutorResponseWire, TransportError> {
    send_request_line(writer, request_line).await?;
    read_response(reader, timeout, expected_call_id).await
}

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------
