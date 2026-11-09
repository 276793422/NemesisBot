//! pipe_cap 回归锁（横扫角度 B）。
//!
//! 锁三件事：限内逐字节保真、超限丢头保尾 + 诚实截断标记、空管道空结果。
//! async/sync 两形态同语义。

use super::{PIPE_CAPTURE_CAP_BYTES, drain_capped_async, drain_capped_sync};
use std::io::Cursor;

/// 构造确定性输入：0..n 循环填充（无 RNG，断言可精确复算）。
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

#[test]
fn sync_under_cap_is_verbatim() {
    let data = pattern(100_000); // < 1 MiB
    let out = drain_capped_sync(Cursor::new(&data));
    assert_eq!(out, data);
}

#[test]
fn sync_empty_pipe_is_empty() {
    let out = drain_capped_sync(Cursor::new(Vec::<u8>::new()));
    assert!(out.is_empty());
}

#[test]
fn sync_over_cap_keeps_tail_with_marker() {
    let data = pattern(PIPE_CAPTURE_CAP_BYTES + 4096);
    let out = drain_capped_sync(Cursor::new(&data));
    // 体积有界：标记 + 恰好 cap 字节保留。
    assert!(out.len() <= PIPE_CAPTURE_CAP_BYTES + 128);
    // 诚实标记：丢弃量 = 总量 - 保留量。
    let marker = String::from_utf8_lossy(&out[..out.len().min(96)]).to_string();
    assert!(
        marker.starts_with("[output capture capped: first 4096 bytes dropped"),
        "marker missing/prefix wrong: {marker}"
    );
    // 保尾语义：保留段与输入尾部逐字节一致。
    let kept = &out[out.len() - PIPE_CAPTURE_CAP_BYTES..];
    assert_eq!(kept, &data[data.len() - PIPE_CAPTURE_CAP_BYTES..]);
}

#[tokio::test]
async fn async_under_cap_is_verbatim() {
    let data = pattern(50_000);
    let out = drain_capped_async(Cursor::new(&data)).await;
    assert_eq!(out, data);
}

#[tokio::test]
async fn async_over_cap_keeps_tail_with_marker() {
    let data = pattern(PIPE_CAPTURE_CAP_BYTES + 8192 * 2);
    let out = drain_capped_async(Cursor::new(&data)).await;
    assert!(out.len() <= PIPE_CAPTURE_CAP_BYTES + 128);
    let kept = &out[out.len() - PIPE_CAPTURE_CAP_BYTES..];
    assert_eq!(kept, &data[data.len() - PIPE_CAPTURE_CAP_BYTES..]);
    assert!(out.starts_with(b"[output capture capped: first"));
}

#[tokio::test]
async fn async_empty_pipe_is_empty() {
    let out = drain_capped_async(Cursor::new(Vec::<u8>::new())).await;
    assert!(out.is_empty());
}

// ---------------------------------------------------------------------------
// run_capped_with_timeout（BUG-20261110-038 回归锁）
// ---------------------------------------------------------------------------

/// 大输出（>OS 管道缓冲 ~64KB）不假超时：修复前形态是「退出后才排干」，
/// 子进程在 ~64KB 处阻塞在 write、永不退出 → 撞 deadline 假超时。修复后
/// 运行期并发排干，大输出命令照常拿到完整（限内）结果。超时给 30s——
/// 修复失效时本测试自然红（30s 后 Err → panic），正常路径 <2s。
#[test]
fn run_capped_big_output_no_hang() {
    use super::run_capped_with_timeout;
    use std::process::Command;
    use std::time::{Duration, Instant};

    // 2500 行 × ~104B ≈ 260KB，稳超 64KB 管道缓冲。
    let line = "A".repeat(100);
    let started = Instant::now();
    let out = if cfg!(windows) {
        // cmd /C 单行 for /L（命令行直跑 %i；@echo 抑制命令回显只留输出）。
        let script = format!("for /L %i in (1,1,2500) do @echo {}", line);
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(&script);
        run_capped_with_timeout(cmd, Duration::from_secs(30))
    } else {
        let script = format!(
            "i=0; while [ $i -lt 2500 ]; do printf '%s\\n' '{}'; i=$((i+1)); done",
            line
        );
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&script);
        run_capped_with_timeout(cmd, Duration::from_secs(30))
    };
    let out = out.expect("大输出命令必须正常完成（修复后不被背压卡死）");
    assert!(
        out.status.success(),
        "exit 非零: {:?} stderr={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    // 260KB < 1MiB cap → 全量保真（无截断标记）。
    assert!(
        out.stdout.len() >= 250_000,
        "stdout 应 ≥250KB，实际 {}",
        out.stdout.len()
    );
    assert!(
        !out.stdout.starts_with(b"[output capture capped"),
        "未超 cap 不应有截断标记"
    );
    // 未撞超时（修复失效时这里已是 30s 后的 Err 分支，到不了）。
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "耗时 {:?}——疑似又被背压卡死",
        started.elapsed()
    );
}

/// 超时语义：真挂住的子进程在 deadline 强杀，返回 Err 且不悬挂
/// （drain 线程随 EOF 收束，join 必然完成）。
#[test]
fn run_capped_timeout_kills_and_returns_err() {
    use super::run_capped_with_timeout;
    use std::process::Command;
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let out = if cfg!(windows) {
        // ping 直接做被杀目标（不套 cmd /C）：TerminateProcess 杀 ping
        // 自身 → 管道写端关闭 → drain 线程 EOF 收束。套 cmd 会留孤儿
        // ping 持写端——那是实现已用收尾窗口兜底的场景，测试不混入。
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"]);
        run_capped_with_timeout(cmd, Duration::from_secs(2))
    } else {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        run_capped_with_timeout(cmd, Duration::from_secs(2))
    };
    assert!(out.is_err(), "真挂住的子进程必须超时返回 Err");
    assert!(
        out.err().unwrap().contains("超时被强杀"),
        "错误信息应带超时定性"
    );
    // join 已收束（否则本测试悬挂）；墙钟 ≈ timeout 量级。
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "强杀后应在秒级返回，实际 {:?}",
        started.elapsed()
    );
}
