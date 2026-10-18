//! Executor transport 抽象（P3 T1；P5 起协议本体住薄 crate）。
//!
//! 职责分界：
//! - **`nemesis-exec-protocol` 薄 crate**（零 nemesis 依赖）：wire 协议
//!   （一行一 JSON、v1/v2 双形态、call_id 回显）、`ExecutorStream` 传输
//!   抽象、交换原语、`new_call_id`。本模块 re-export（现有调用点零改动）；
//!   guest agent（P5 VM 内执行体）同源消费同一 crate——单一真相源，防
//!   两端口径漂移。
//! - 本模块：宿主侧载体——stdio 双工（[`ChildStdio`]）、stderr 环形尾
//!   （[`StderrTail`]）、**常驻会话**（[`PersistentExecutorSession`]，一个
//!   长驻子进程跑多次工具调用，persistent 模式的 gateway 侧载体）。
//! - [`crate::remote_executor_tool`]：per-call 通道（每次调用 spawn-退出，
//!   Layer 1/2 现状语义，字节不变）；persistent 不可用时的回退路径。
//! - `nemesisbot::exec_worker`：子进程侧循环（v1/v2 双解析 + call_id 回显
//!   + journal 幂等账）。

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader};

// 协议本体（wire structs / Transport trait / 交换原语 / TransportError /
// call_id / v2 请求行构造）单一真相源在薄 crate——这里 re-export 保兼容。
pub use nemesis_exec_protocol::{
    BoxStreamRead, BoxStreamWrite, ExecutorRequestOwned, ExecutorRequestRef, ExecutorResponseWire,
    ExecutorStream, PROTOCOL_V2, TransportError, build_v2_request_line, call_over_halves,
    new_call_id, read_response, send_request_line, split_duplex,
};

// ---------------------------------------------------------------------------
// ChildStdio——stdio 双工适配（宿主侧载体；经 blanket impl 满足 ExecutorStream）
// ---------------------------------------------------------------------------

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

/// 常驻执行体会话（gateway 侧载体）：持有子进程句柄 + 拆分后的传输半 +
/// stderr 排水。生命周期归本模块的调用方（nemesisbot 的 supervisor）管辖；
/// 协议与传输原语在薄 crate，这里只承载宿主侧句柄。
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

    /// 构造 v2 请求行（supervisor 日常调用/replay 共用；实现在薄 crate）。
    pub fn build_v2_request_line(
        tool: &str,
        args: &str,
        context: &serde_json::Value,
        call_id: &str,
    ) -> Result<String, String> {
        build_v2_request_line(tool, args, context, call_id)
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
