//! 有界管道排干（BUG 2026-10-29 横扫角度 B：子进程输出无界入内存）。
//!
//! 「agent 执行命令 → 收集 stdout/stderr」路径的排干函数原为裸
//! `read_to_end` 全量入内存，显示层截断发生在读取完成之后——子进程输出
//! 速率与生命周期不受我方控制，`yes` 类高吞吐输出在超时窗口内可把宿主打
//! 进 OOM（shell.rs / middleware.rs / loop_tools.rs / guest tools /
//! lsp install / exec_workspace / vm_bridge 七处同族，详见 BUG 文档）。
//!
//! 本模块是修复后的单一真相源：捕获上限对齐 `nemesis-board/src/anchor.rs`
//! D10-4 先例（`PIPE_CAPTURE_CAP_BYTES` = 1 MiB），**丢头保尾**有界读循环
//! ——读取不停、子进程不被背压卡死（不会把大输出的正常命令变成假超时），
//! 超限在结果头部加诚实截断标记。

use std::collections::VecDeque;

/// 单管道捕获上限（对齐 anchor.rs D10-4 同名常量语义）。
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

#[cfg(test)]
mod tests;
