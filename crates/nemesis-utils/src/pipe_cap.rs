//! 有界管道排干（横扫角度 B：子进程输出无界入内存）。
//!
//! 「agent 执行命令 → 收集 stdout/stderr」路径的排干函数原为裸
//! `read_to_end` 全量入内存，显示层截断发生在读取完成之后——子进程输出
//! 速率与生命周期不受我方控制，`yes` 类高吞吐输出在超时窗口内可把宿主打
//! 进 OOM（shell.rs / middleware.rs / loop_tools.rs / guest tools /
//! lsp install / exec_workspace / vm_bridge 七处同族，详见 BUG 文档）。
//!
//! 本模块是修复后的统一定义：捕获上限对齐 `nemesis-board/src/anchor.rs`
//! 先例（`PIPE_CAPTURE_CAP_BYTES` = 1 MiB），**丢头保尾**有界读循环
//! ——读取不停、子进程不被背压卡死（不会把大输出的正常命令变成假超时），
//! 超限在结果头部加诚实截断标记。

use std::collections::VecDeque;
use std::process::Stdio;

/// 单管道捕获上限（对齐 anchor.rs 同名常量语义）。
pub const PIPE_CAPTURE_CAP_BYTES: usize = 1 << 20;

const CHUNK: usize = 8192;

/// 丢头保尾累积器：超限剪头（记录丢弃量），EOF 后统一产出。
struct TailBuf {
    buf: VecDeque<u8>,
    dropped: u64,
}

impl TailBuf {
    fn new() -> Self {
        Self {
            buf: VecDeque::new(),
            dropped: 0,
        }
    }

    fn push(&mut self, chunk: &[u8]) {
        self.buf.extend(chunk);
        if self.buf.len() > PIPE_CAPTURE_CAP_BYTES {
            let drop = self.buf.len() - PIPE_CAPTURE_CAP_BYTES;
            self.buf.drain(..drop);
            self.dropped += drop as u64;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.dropped == 0 {
            return self.buf.make_contiguous().to_vec();
        }
        let marker = format!(
            "[output capture capped: first {} bytes dropped (1 MiB cap)]\n",
            self.dropped
        );
        let mut out = Vec::with_capacity(marker.len() + self.buf.len());
        out.extend_from_slice(marker.as_bytes());
        out.extend_from_slice(self.buf.make_contiguous());
        out
    }
}

/// 同步排干（`std::process` 子进程管道）。读错误按 EOF 收尾（与旧
/// `let _ = read_to_end` 的容错语义一致：排干是尽力而为的收集动作）。
pub fn drain_capped_sync<R: std::io::Read>(mut pipe: R) -> Vec<u8> {
    let mut tb = TailBuf::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => tb.push(&chunk[..n]),
        }
    }
    tb.finish()
}

/// 同步运行子进程（双管道并发排干 + 超时强杀）。
///
/// spawn 后**立即**把 stdout/stderr 移交两个排干线程（[`drain_capped_sync`]
/// 的读阻塞即消费，子进程退出 → EOF → 线程自然收束），主线程照旧
/// `try_wait` 轮询 + deadline 强杀，最后 join 拿结果。强杀路径管道写端
/// 随进程消亡关闭，两排干线程必然 EOF 收束，join 不悬。
///
/// 背景（BUG-20261110-038）：此前 exec_workspace / vm_bridge 的同款形态
/// 是「`try_wait` 返回 Some（子进程已退出）之后」才排干——运行期间管道
/// 无人消费，输出超过 OS 管道缓冲（~64KB）即子进程阻塞在 write、永不
/// 退出，`try_wait` 恒 None → 撞 deadline 假超时强杀，本模块文档承诺的
/// 「不被背压卡死」在调用侧失效。
///
/// 返回：`Ok(Output)` = 子进程退出（含非零码）；`Err` = spawn 失败 /
/// 等待错误 / 超时强杀（错误信息带形态）。
pub fn run_capped_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    // 排干线程经 channel 回传结果（而非 JoinHandle）：Windows kill 只杀
    // 直接子进程，孤儿孙进程（cmd /C ping 的 ping 等）可继续持有管道写端
    // ——EOF 不来，无条件 join 会悬挂到孤儿自然退出。收尾窗口内收不到就
    // detach（EOF 后线程自然收束）按空输出收尾，调用方不被孙进程寿命绑架
    // （「排干是尽力而为的收集动作」语义的延伸）。
    const JOIN_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
    let (stdout_tx, stdout_rx) = std::sync::mpsc::channel();
    let (stderr_tx, stderr_rx) = std::sync::mpsc::channel();
    let stdout_t = child.stdout.take().map(|p| {
        std::thread::spawn(move || {
            let _ = stdout_tx.send(drain_capped_sync(p));
        })
    });
    let stderr_t = child.stderr.take().map(|p| {
        std::thread::spawn(move || {
            let _ = stderr_tx.send(drain_capped_sync(p));
        })
    });
    let deadline = std::time::Instant::now() + timeout;
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break false,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break true;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => return Err(format!("wait: {e}")),
        }
    };
    let stdout = stdout_rx.recv_timeout(JOIN_GRACE).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(JOIN_GRACE).unwrap_or_default();
    // 句柄 drop = detach：孤儿管道场景下线程在 EOF 后自然收束，不泄漏。
    drop(stdout_t);
    drop(stderr_t);
    if timed_out {
        return Err("子进程超时被强杀".into());
    }
    let status = child
        .try_wait()
        .map_err(|e| format!("wait: {e}"))?
        .ok_or_else(|| "子进程状态丢失".to_string())?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// 异步排干（`tokio::process` 子进程管道）。容错语义同 [`drain_capped_sync`]。
pub async fn drain_capped_async<R: tokio::io::AsyncRead + Unpin>(mut pipe: R) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut tb = TailBuf::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => tb.push(&chunk[..n]),
        }
    }
    tb.finish()
}

/// 异步排干 + 旁路观测（2026-11-08 子进程实时输出与完成唤醒）：与
/// [`drain_capped_async`] 同一累积路径（返回值逐字节一致），每读到原始
/// chunk 额外调一次 `on_chunk`（同步回调，不得阻塞——回调里做节流/截窗，
/// 不做 IO）。exec 执行中输出流的 tee 用：观测面零侵入返回值路径。
pub async fn drain_capped_async_tee<R, F>(mut pipe: R, mut on_chunk: F) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
    F: FnMut(&[u8]),
{
    use tokio::io::AsyncReadExt;
    let mut tb = TailBuf::new();
    let mut chunk = [0u8; CHUNK];
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                on_chunk(&chunk[..n]);
                tb.push(&chunk[..n]);
            }
        }
    }
    tb.finish()
}

#[cfg(test)]
mod tests;
