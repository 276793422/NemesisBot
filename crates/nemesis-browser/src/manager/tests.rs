//! manager 纯逻辑单测：选择解析 / 事件环形缓冲 / 日志查询。

use super::{BrowserEvent, BrowserManager, BrowserPrefs, cleanup_stale_port_file};
use crate::cdp::DEVTOOLS_PORT_FILE;
use crate::detect::DriverKind;
use std::path::PathBuf;

fn manager_with(home: PathBuf, prefs: BrowserPrefs) -> BrowserManager {
    BrowserManager::new(home, Box::new(move || prefs.clone()), None)
}

// --- resolve_browser：显式覆盖 > 配置 > 自动解析 ---

#[test]
fn resolve_executable_override_missing_path_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    let err = m
        .resolve_browser(None, Some("Z:/no/such/browser.exe"))
        .unwrap_err();
    assert!(err.contains("不存在"), "{err}");
}

#[test]
fn resolve_executable_override_exists() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("fake-browser.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    let (kind, p) = m
        .resolve_browser(None, Some(exe.to_str().unwrap()))
        .unwrap();
    assert_eq!(kind, DriverKind::Custom);
    assert_eq!(p, exe);
}

#[test]
fn resolve_kind_unknown_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    let err = m.resolve_browser(Some("netscape"), None).unwrap_err();
    assert!(err.contains("未知浏览器 kind"), "{err}");
}

#[test]
fn resolve_kind_firefox_is_phase2_err() {
    // Firefox 二期诚实报错（D14：枚举可见不可驱动，不进默认序）。
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    let err = m.resolve_browser(Some("firefox"), None).unwrap_err();
    assert!(err.contains("二期"), "{err}");
}

#[test]
fn resolve_config_kind_pinned() {
    let dir = tempfile::tempdir().unwrap();
    // 本机必有 Edge（Windows 系统组件）；非 Windows 上该断言退化为探测结果，
    // 测试只在 windows 下跑严格断言。
    let mut prefs = BrowserPrefs::default();
    prefs.kind = "edge".to_string();
    let m = manager_with(dir.path().to_path_buf(), prefs);
    let r = m.resolve_browser(None, None);
    if cfg!(windows) {
        let (kind, _p) = r.unwrap();
        assert_eq!(kind, DriverKind::Edge);
    } else {
        // 非 Windows 允许「未检出」错误（诚实路径）。
        let _ = r;
    }
}

// --- 事件环形缓冲与日志查询 ---

#[test]
fn event_to_json_shape() {
    let ev = BrowserEvent {
        ts_ms: 1700000000000,
        session_id: "b1".into(),
        action: "spawn".into(),
        target: "edge".into(),
        ok: true,
        detail: "fine".into(),
        duration_ms: 42,
    };
    let v = ev.to_json();
    assert_eq!(v["session_id"], "b1");
    assert_eq!(v["action"], "spawn");
    assert_eq!(v["ok"], true);
    assert_eq!(v["duration_ms"], 42);
}

#[test]
fn logs_ring_records_and_filters() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    m.record_event("b1", "spawn", "edge", true, "ok", 10);
    m.record_event("b2", "spawn", "chrome", false, "fail", 20);
    m.record_event("b1", "close", "", true, "done", 5);

    let all = m.logs(None, 10);
    assert_eq!(all.len(), 3);
    // 最新在前（rev）。
    assert_eq!(all[0]["action"], "close");

    let only_b1 = m.logs(Some("b1"), 10);
    assert_eq!(only_b1.len(), 2);
    assert!(only_b1.iter().all(|e| e["session_id"] == "b1"));

    // limit 生效。
    assert_eq!(m.logs(None, 1).len(), 1);
}

// --- 端口文件清理辅助 ---

#[test]
fn cleanup_stale_port_file_removes_only_that_file() {
    let dir = tempfile::tempdir().unwrap();
    let pf = dir.path().join(DEVTOOLS_PORT_FILE);
    std::fs::write(&pf, "1234\n/x\n").unwrap();
    let keep = dir.path().join("keep.txt");
    std::fs::write(&keep, b"x").unwrap();

    cleanup_stale_port_file(dir.path());
    assert!(!pf.exists());
    assert!(keep.exists());
    // 幂等：文件不存在再清不炸。
    cleanup_stale_port_file(dir.path());
}

// --- 缺省会话选择（F28，2026-11-15 五审：空/全死/死活混合三臂）---

/// 假活 HTTP 端点（is_alive 的探测臂打 GET /json/version；canned 应答
/// `{}` 即可过解析）。
fn spawn_canned_version_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let resp: &[u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            // 必须先读掉请求再写响应（F28 锁的稳定性前提）：带着未读数据
            // close 时 Windows 发 RST 而非 FIN，会冲掉排队中的响应 → 客户端
            // read 撞 os error 10053（WSAECONNABORTED）。读干净后 close 走
            // 优雅 FIN，响应字节完达。
            let mut drain = [0u8; 2048];
            let _ = std::io::Read::read(&mut s, &mut drain);
            let _ = std::io::Write::write_all(&mut s, resp);
            // drop = 关闭（http_version 一次性探测，无 keep-alive 需求）。
        }
    });
    port
}

#[tokio::test]
async fn pick_default_session_empty_dead_and_alive_arms() {
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());

    // ① 空：诚实报错（不是 panic 不是脏默认）。
    let Err(e1) = m.pick_default_session().await else {
        panic!("空注册表必须 Err");
    };
    assert!(e1.contains("无浏览器会话"), "{e1}");

    // ② 全死（手关窗口后注册表残留死会话）：诚实报错——旧实现（工具侧
    // 局部循环同语义）已对；本锁钉住 manager 单一真相源不回退。
    m.sessions
        .lock()
        .push(crate::session::BrowserSession::test_with(
            "bd",
            DriverKind::Edge,
            true,
            0,
        ));
    let Err(e2) = m.pick_default_session().await else {
        panic!("全死注册表必须 Err");
    };
    assert!(e2.contains("均已退出"), "{e2}");

    // ③ 死 + 活：跳过死会话选中活的（canned server 撑探测臂）。
    let port = spawn_canned_version_server();
    m.sessions
        .lock()
        .push(crate::session::BrowserSession::test_with(
            "ba",
            DriverKind::Chrome,
            false,
            port,
        ));
    let s = m.pick_default_session().await.unwrap();
    assert_eq!(s.id, "ba");
}
