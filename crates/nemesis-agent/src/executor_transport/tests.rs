//! `executor_transport` 单测（宿主侧载体）：stderr 环形尾。wire 字节契约
//! / call_id / duplex 交换原语已随协议本体迁入 `nemesis-exec-protocol`
//! 薄 crate（底座），真子进程 stdio 往返由 `remote_executor_tool`
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

// BUG-20261122-020：排水必须带帽行读——executor 子进程 stderr 是任意被
// 执行命令的输出（不受我方控制），无换行巨行在裸 next_line 的无界积累
// 里打爆宿主内存。截断行带注记、游标对齐下一行行首、EOF 收尾。
#[tokio::test]
async fn stderr_drain_truncates_oversize_lines_and_keeps_following_lines() {
    use tokio::io::AsyncWriteExt;
    let (mut writer, server) = tokio::io::duplex(128 * 1024);
    let sink = StderrTail::default();
    let drain_jh = tokio::spawn(StderrTail::drain(server, sink.clone()));
    // 毒丸行 80KB > 64KB 帽：截断保留前 64KB + 注记。
    let poison = "x".repeat(80 * 1024);
    writer.write_all(poison.as_bytes()).await.unwrap();
    writer.write_all(b"\n").await.unwrap();
    writer.write_all(b"after line\n").await.unwrap();
    writer.write_all(b"tail line\n").await.unwrap();
    drop(writer); // EOF
    drain_jh.await.unwrap();

    let snap = sink.snapshot();
    assert!(
        snap.contains("...[line truncated]"),
        "超帽行必须带截断注记: {snap}"
    );
    assert!(
        snap.contains("after line"),
        "游标必须对齐毒丸行之后的下一行: {snap}"
    );
    assert!(snap.contains("tail line"), "后续行必须照常进环形尾: {snap}");
}
