//! pipe_cap 回归锁（BUG 2026-10-29 横扫角度 B）。
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
