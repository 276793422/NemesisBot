use super::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

fn make_watcher() -> (FsWatcher, tokio::sync::mpsc::Receiver<RawFsEvent>) {
    let (tx, rx) = tokio::sync::mpsc::channel(1024);
    (FsWatcher::new(tx).unwrap(), rx)
}

fn roots(paths: &[&str]) -> BTreeSet<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[tokio::test]
async fn lifecycle_rearm_diff_and_shutdown() {
    let (mut w, _rx) = make_watcher();
    // 差分挂监听：只新增/只移除，重挂幂等
    w.rearm(&roots(&["."]));
    w.rearm(&roots(&["."]));
    assert_eq!(w.armed.len(), 1);
    let dir = tempfile::tempdir().unwrap();
    w.rearm(&roots(&[".", dir.path().to_str().unwrap()]));
    assert_eq!(w.armed.len(), 2);
    w.rearm(&roots(&["."]));
    assert_eq!(w.armed.len(), 1);
    w.shutdown();
    assert!(w.watcher.is_none());
    assert!(w.armed.is_empty());
    w.shutdown(); // 幂等
}

#[tokio::test]
async fn fs_event_flows_through_channel() {
    let (mut w, mut rx) = make_watcher();
    let dir = tempfile::tempdir().unwrap();
    w.rearm(&roots(&[dir.path().to_str().unwrap()]));
    // 真实落一个文件 → Create 事件应经 channel 到达（Access 类事件已被过滤）
    std::fs::write(dir.path().join("hello.txt"), "x").unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("5s 内未收到 fs 事件（notify 语义异常）")
        .expect("channel 提前关闭");
    assert!(got.path.ends_with("hello.txt"), "实际收到: {}", got.path);
    assert!(got.at_ms > 0);
    w.shutdown();
}

#[test]
fn now_ms_monotonic_enough() {
    let a = now_ms();
    let b = now_ms();
    assert!(b >= a);
    assert!(b > 1_700_000_000_000); // 2023 后（系统时钟 sanity）
}
