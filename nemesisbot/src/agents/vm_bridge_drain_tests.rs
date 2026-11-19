//! —— vm_bridge.rs 构建排水（drain_child_stream）韧性伴生测试（零内联纪律）。
//!
//! BUG-20261113-133 横向同族回归锁：子进程 stdout/stderr 排水旧形态
//! `BufReader::lines()` 遇非 UTF-8 行 Err 即停 → 管道写满 → 构建脚本
//! 挂死 → `tokio::join!` 无超时 → IMAGE_JOB 单飞闸永久持有。新契约：
//! lossy 解码收敛，坏行照记（U+FFFD），后续行不丢。

use tokio::io::AsyncWriteExt as _;

use super::vm_bridge::drain_child_stream;

/// 非 UTF-8 行 lossy 收敛且后续行照常。可证伪性：旧 `lines()` 形态
/// next_line 返回 Err → 排水循环停 → "after line" 永不落日志 → 断言
/// 当场抓（机制已独立 scratch 程序实证：tokio lines() 对 0xFF 字节
/// Err(InvalidData, Utf8Error)）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drain_child_stream_survives_invalid_utf8_and_logs_lossy() {
    let tmp = tempfile::tempdir().unwrap();
    let log_path = tmp.path().join("build.log");
    let (writer, reader) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(drain_child_stream(reader, log_path.clone(), "stderr"));

    let mut w = writer;
    w.write_all(b"ok line\n").await.unwrap();
    w.write_all(&[0xff, 0xfe]).await.unwrap();
    w.write_all(b"\n").await.unwrap();
    w.write_all(b"after line\n").await.unwrap();
    w.flush().await.unwrap();
    drop(w); // EOF → 排水结束

    task.await.unwrap();

    let content = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(
        lines,
        vec![
            "[stderr] ok line",
            "[stderr] \u{FFFD}\u{FFFD}",
            "[stderr] after line",
        ],
        "坏行 lossy 收敛 + 后续行不丢（旧 lines() 形态 after 行必丢）: {lines:?}"
    );
}
