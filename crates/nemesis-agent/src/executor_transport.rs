//! Executor transport abstraction（P3 T1）。
//!
//! 把「gateway ↔ executor 子进程」之间的字节通路从两种硬编码形态（stdio /
//! Windows 具名管道）抽成统一 trait，并把 wire 协议升级到 **v2**（带
//! `call_id` 双向回显）——这是 P5 VM 后端（vsock / NAT-TCP 传输）的前置：
//! 新传输只需实现 [`AsyncRead`] + [`AsyncWrite`]，协议层与生命周期层零改动。
//!
//! 职责分界：
//! - 本模块：协议（一行一 JSON）、传输抽象、**常驻会话**（一个长驻子进程
//!   跑多次工具调用，persistent 模式的 gateway 侧载体）。
//! - [`crate::remote_executor_tool`]：per-call 通道（每次调用 spawn-退出，
//!   Layer 1/2 现状语义，字节不变）；persistent 不可用时的回退路径。
//! - `nemesisbot::exec_worker`：子进程侧循环（v1/v2 双解析 + call_id 回显
//!   + journal 幂等账）。
//!
//! 协议版本纪律：
//! - **v1**（现状）：`{tool,args,context}` → `{ok,result,error}`。per-call
//!   路径继续产出 v1 字节（request 不带 `call_id`/`v` 键——serde skip），
//!   现有测试与外部断言零迁移。
//! - **v2**：`{v:2, call_id, tool, args, context}` →
//!   `{v:2, call_id, ok, result, error}`。persistent 会话恒走 v2；
//!   `call_id` 是在途调用幂等（journal replay）与响应匹配的键。
//! - 子进程侧双解析：v1/v2 同循环接受（同一二进制自 spawn 自，不存在
//!   生产混布；双解析是为了测试与逐批演进的平滑性）。

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

/// v2 协议版本号（request/response 的 `v` 字段值；v1 不写该键，反序列化
/// 缺省 0）。
pub const PROTOCOL_V2: u32 = 2;

// ---------------------------------------------------------------------------
// Wire structs（两侧单一真相源——原先 gateway / exec_worker 各镜像一份）
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
    /// 序列化后的 [`crate::context::RequestContext`]（`async_callback` 经
    /// `#[serde(skip)]` 丢弃）。
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
// Transport trait（P5 前置：stdio / 具名管道 / vsock / NAT-TCP 统一接口）
// ---------------------------------------------------------------------------

/// executor 字节传输的最小接口。任何 `AsyncRead + AsyncWrite + Unpin +
/// Send` 的对象自动实现（blanket impl）——stdio 双工（[`ChildStdio`]）、
/// Windows 具名管道（tokio `NamedPipeServer/Client` 原生满足）、P5 的
/// vsock/TCP 流同理。
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

/// 子进程 stdio 双工：把 `ChildStdin` + `ChildStdout` 拼成一个
/// [`ExecutorStream`] 对象（tokio 没有现成的 unsplit；手写 poll 委托）。
pub struct ChildStdio {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl ChildStdio {
    pub fn new(stdin: tokio::process::ChildStdin, stdout: tokio::process::ChildStdout) -> Self {
        Self { stdin, stdout }
    }
}

impl AsyncRead for ChildStdio {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChildStdio {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        // shutdown = 关 stdin（EOF 信号，子进程循环退出语义与 per-call 一致）。
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
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
// 交换原语（一行请求 → 一行响应；per-call 与 persistent 共用）
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

// ---------------------------------------------------------------------------
// StderrTail——子进程 stderr 后台排水 + 环形尾（诊断面，per-call 与
// persistent 共用单一真相源；原 RemoteExecutorTool::drain_stderr 收编于此）
// ---------------------------------------------------------------------------

/// stderr 环形尾容量（行数）。
pub const STDERR_TAIL_CAP: usize = 20;

/// 子进程 stderr 的后台排水句柄。排水防止 ~4KB 管道缓冲写满后子进程阻塞；
/// 行照旧 `debug!` 全量落日志，环形尾保留最近 [`STDERR_TAIL_CAP`] 行供
/// 失败臂拼接（静默死亡=panic/OOM/启动失败时唯一的诊断面）。
#[derive(Clone, Default)]
pub struct StderrTail {
    tail: Arc<Mutex<VecDeque<String>>>,
}

impl StderrTail {
    /// 从子进程 stderr 起排水任务。stderr 未接管（None）= 空尾。
    pub fn spawn(stderr: Option<tokio::process::ChildStderr>) -> Self {
        let this = Self::default();
        if let Some(stderr) = stderr {
            let sink = this.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!("[executor stderr] {line}");
                    sink.push_line(line);
                }
            });
        }
        this
    }

    /// 推一行进环形尾（满则逐出最旧）。
    pub fn push_line(&self, line: String) {
        let mut buf = self.tail.lock().unwrap_or_else(|e| e.into_inner());
        if buf.len() >= STDERR_TAIL_CAP {
            buf.pop_front();
        }
        buf.push_back(line);
    }

    /// 快照（单行拼接；空 = 子进程没写 stderr → `<empty>`）。
    pub fn snapshot(&self) -> String {
        let buf = self.tail.lock().unwrap_or_else(|e| e.into_inner());
        if buf.is_empty() {
            "<empty>".to_string()
        } else {
            buf.iter().cloned().collect::<Vec<_>>().join(" | ")
        }
    }
}

// ---------------------------------------------------------------------------
// PersistentExecutorSession——一个长驻子进程跑多次工具调用
// ---------------------------------------------------------------------------

/// 会话内一次调用的传输原语（无 Child 依赖——duplex 可测）。
pub async fn call_over_halves(
    writer: &mut (dyn AsyncWrite + Unpin + Send),
    reader: &mut BufReader<BoxStreamRead>,
    request_line: &str,
    expected_call_id: Option<&str>,
    timeout: Duration,
) -> Result<ExecutorResponseWire, TransportError> {
    send_request_line(writer, request_line).await?;
    read_response(reader, timeout, expected_call_id).await
}

/// 常驻执行体会话（gateway 侧载体）：持有子进程句柄 + 拆分后的传输半 +
/// stderr 排水。生命周期归本模块的调用方（nemesisbot 的 supervisor）管辖；
/// 这里只提供传输与协议原语。
///
/// 会话内严格逐请求应答（不并发——工具调用语义串行，幂等账由子进程侧
/// journal + supervisor 的 pending 集合承载）。
pub struct PersistentExecutorSession {
    pub child: tokio::process::Child,
    writer: BoxStreamWrite,
    reader: BufReader<BoxStreamRead>,
    stderr: StderrTail,
}

impl PersistentExecutorSession {
    /// 从 stdio 子进程装配（transport=stdio；子进程必须以 piped 三件套
    /// spawn）。stderr 接管转后台排水。
    pub fn from_stdio_child(mut child: tokio::process::Child) -> Result<Self, String> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "executor child has no stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "executor child has no stdout".to_string())?;
        let stderr = StderrTail::spawn(child.stderr.take());
        let (read, write) = split_duplex(ChildStdio::new(stdin, stdout));
        Ok(Self {
            child,
            reader: BufReader::new(read),
            writer: write,
            stderr,
        })
    }

    /// 从管道传输装配（stdio 是 null——字节通路走具名管道；管道两端都是
    /// AsyncRead+AsyncWrite，由调用方 split 后传入）。
    pub fn from_pipe_parts(
        child: tokio::process::Child,
        stderr: StderrTail,
        read: BoxStreamRead,
        write: BoxStreamWrite,
    ) -> Self {
        Self {
            child,
            reader: BufReader::new(read),
            writer: write,
            stderr,
        }
    }

    /// 发一行 v2 请求、收一行响应（协议原语）。`request_line` 由调用方
    /// 构造（supervisor 对 replay 场景要原样重发历史请求行，所以这里收
    /// 原始行而不是结构体）。
    pub async fn call_raw(
        &mut self,
        request_line: &str,
        expected_call_id: Option<&str>,
        timeout: Duration,
    ) -> Result<ExecutorResponseWire, TransportError> {
        call_over_halves(
            self.writer.as_mut(),
            &mut self.reader,
            request_line,
            expected_call_id,
            timeout,
        )
        .await
    }

    /// 构造 v2 请求行（supervisor 日常调用/replay 共用）。
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
        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("serialize executor request: {e}"))?;
        line.push('\n');
        Ok(line)
    }

    /// 子进程是否仍活着（true = 存活；false = 已退出——watchdog 据此触发
    /// 崩溃重建）。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// stderr 尾快照（失败诊断拼接）。
    pub fn stderr_snapshot(&self) -> String {
        self.stderr.snapshot()
    }

    /// 优雅关停：整条流双半 drop（EOF 信号）→ 等 `grace` → 仍活着则 kill
    /// → 收尸。sleep 语义（进程退出，状态全在 home/journal 落盘）走这里。
    ///
    /// 注意必须**两个半都 drop**：管道传输的 split 半共享底层 server 对象
    /// （BiLock），只关写半对端看不到 EOF；stdio 路径 ChildStdio 随半俱亡
    /// 时 stdin 关闭，EOF 语义等价于显式 shutdown。
    pub async fn shutdown(self, grace: Duration) {
        let Self {
            mut child,
            writer,
            reader,
            ..
        } = self;
        drop(writer);
        drop(reader);
        match tokio::time::timeout(grace, child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
