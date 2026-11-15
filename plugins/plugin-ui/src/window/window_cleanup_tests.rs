//! BUG-20261115-010 回归锁：WebView2 数据目录韧性删除行为。
//!
//! 旧实现单发 `remove_dir_all`：controller 未关/浏览器进程异步退出窗口内
//! 句柄被持 → sharing violation → `%TEMP%\nemesisbot-webview2` 每开一次窗
//! 泄漏一个完整 profile（目录名毫秒级唯一）。修复 = drop 序（调用侧）+
//! 退避重试（cleanup_web_context）。本文件钉韧性语义：
//! 1. 无句柄竞争时一次删除成功（正常路径不引入额外延迟）；
//! 2. 句柄持有窗口内失败 → 释放后的下一轮重试成功（单发旧实现 = 目录残留
//!    断言红）。
//!
//! drop 序（webview/web_context 先于 cleanup）需要真窗+WebView2 运行时，
//! 单测不可达——由调用侧两处 `drop(...)` 注释与代码审读保证（诚实边界）。

use std::path::PathBuf;
use std::time::Duration;

/// 隔离 temp 根（pid+毫秒唯一，避免与其他测试/实例互踩）。
fn temp_root(tag: &str) -> PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "pluginui-cleanup-{}-{}-{}",
        tag,
        std::process::id(),
        millis
    ))
}

/// 正常路径：无竞争时一次删除成功（含嵌套内容），NotFound 前提下不误报。
#[test]
fn cleanup_removes_dir_on_first_attempt() {
    let root = temp_root("ok");
    let dir = root.join("dashboard-ok");
    std::fs::create_dir_all(dir.join("EBWebView")).unwrap();
    std::fs::write(dir.join("EBWebView").join("lock"), b"x").unwrap();

    super::cleanup_web_context(&dir);

    assert!(
        !dir.exists(),
        "BUG-20261115-010：无竞争删除必须一次成功: {}",
        dir.display()
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// 韧性路径：独占句柄（Windows share_mode(0) = 禁共享）持 250ms 后释放，
/// 重试循环在释放后的下一轮删除成功。旧单发实现此场景目录残留 → 断言红
/// （区分力：把 cleanup_web_context 换回单发形态本测试必败）。
#[cfg(windows)]
#[test]
fn cleanup_retries_while_handle_held() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::time::Instant;

    let root = temp_root("retry");
    let dir = root.join("approval-retry");
    std::fs::create_dir_all(&dir).unwrap();
    let held = dir.join("EBWebView.lock");
    std::fs::write(&held, b"x").unwrap();

    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&held)
        .expect("独占打开测试句柄");
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        drop(file);
    });

    let start = Instant::now();
    super::cleanup_web_context(&dir);
    let elapsed = start.elapsed();

    releaser.join().unwrap();
    assert!(
        !dir.exists(),
        "BUG-20261115-010：句柄释放后重试删除必须成功（残留 = 无韧性回退）: {}",
        dir.display()
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "重试路径应至少等待过一次退避（实测 {elapsed:?}）"
    );
    let _ = std::fs::remove_dir_all(&root);
}
