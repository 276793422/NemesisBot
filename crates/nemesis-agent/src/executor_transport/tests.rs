//! `executor_transport` 单测（宿主侧载体）：stderr 环形尾。wire 字节契约
//! / call_id / duplex 交换原语已随协议本体迁入 `nemesis-exec-protocol`
//! 薄 crate（P5 底座），真子进程 stdio 往返由 `remote_executor_tool`
//! 既有集成测试覆盖。

use super::*;

#[test]
fn stderr_tail_ring_cap_and_snapshot() {
    let tail = StderrTail::default();
    for i in 0..(STDERR_TAIL_CAP as i32 + 5) {
        tail.push_line(format!("line-{i}"));
    }
    let snap = tail.snapshot();
    let lines: Vec<&str> = snap.split(" | ").collect();
    // 环形：恰好 CAP 行，最旧的 0..4 被逐出。
    assert_eq!(lines.len(), STDERR_TAIL_CAP);
    assert_eq!(lines[0], "line-5");
    assert_eq!(
        lines[lines.len() - 1],
        format!("line-{}", STDERR_TAIL_CAP as i32 + 4)
    );
}

#[test]
fn stderr_tail_empty_snapshot_placeholder() {
    let tail = StderrTail::default();
    assert_eq!(tail.snapshot(), "<empty>");
}
