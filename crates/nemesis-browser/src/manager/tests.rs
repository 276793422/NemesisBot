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

// --- custom 路径 Gecko 系守卫（2026-11-24：custom 的协议假设是 Chromium
// --- CDP，Gecko 系可执行名必须在解析层诚实拒绝——否则拿着 Chromium 旗标
// --- 启动 Firefox（--user-data-dir 它不认），死在调试端口等待超时）---

#[test]
fn resolve_executable_override_firefox_is_phase2_err() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("firefox.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    let err = m
        .resolve_browser(None, Some(exe.to_str().unwrap()))
        .unwrap_err();
    assert!(err.contains("二期"), "{err}");
    assert!(err.contains("Gecko"), "{err}");
}

#[test]
fn resolve_config_executable_firefox_is_phase2_err() {
    // config browser.executable 臂同闸（设置页自定义路径入口）。
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("firefox.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let mut prefs = BrowserPrefs::default();
    prefs.executable = exe.to_str().unwrap().to_string();
    let m = manager_with(dir.path().to_path_buf(), prefs);
    let err = m.resolve_browser(None, None).unwrap_err();
    assert!(err.contains("二期"), "{err}");
}

#[test]
fn resolve_executable_override_gecko_case_and_forks() {
    // 大小写不敏感 + 分支（LibreWolf 同为 Gecko 系）+ 无扩展名 Linux 形态。
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    for name in ["FIREFOX.EXE", "librewolf.exe", "firefox-bin"] {
        let exe = dir.path().join(name);
        std::fs::write(&exe, b"MZ").unwrap();
        let err = m
            .resolve_browser(None, Some(exe.to_str().unwrap()))
            .unwrap_err();
        assert!(err.contains("二期"), "{name}: {err}");
    }
    // Chromium 分支形态不受误伤（启发式拒绝会误伤五花八门的 exe 名——
    // 本守卫是显式 denylist，名单外照常放行）。
    let ok = dir.path().join("thorium-browser.exe");
    std::fs::write(&ok, b"MZ").unwrap();
    let (kind, _) = m.resolve_browser(None, Some(ok.to_str().unwrap())).unwrap();
    assert_eq!(kind, DriverKind::Custom);
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

// --- 会话级四动作（2026-11-28 所有权批）：挑选偏好 / close 双闸 / 台账投影 ---

use serde_json::json;

use crate::session::BrowserSession;

fn push_test_session(m: &BrowserManager, id: &str, exited: bool, port: u16) {
    m.sessions.lock().push(BrowserSession::test_with(
        id,
        DriverKind::Edge,
        exited,
        port,
    ));
}

/// 三实例死活矩阵注册表（b1=own/root-a、b2=neutral/human、b3=others/root-b）。
/// 注册表序故意把 others 排最前之后再按序 push？不——偏好序是策略不是
/// 注册表巧合：这里按 b3,b2,b1 push（others 在注册表头），own 在尾，
/// 选中 own 证明序来自偏好策略。
fn registry_matrix(
    own_alive: bool,
    neutral_alive: bool,
    others_alive: bool,
    port: u16,
) -> BrowserManager {
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    push_test_session(&m, "b3", !others_alive, port);
    push_test_session(&m, "b2", !neutral_alive, port);
    push_test_session(&m, "b1", !own_alive, port);
    {
        let mut owners = m.owners.lock();
        owners.insert("b1".into(), "root-a".into());
        owners.insert("b2".into(), "human".into());
        owners.insert("b3".into(), "root-b".into());
    }
    m
}

#[tokio::test]
async fn pick_default_session_for_prefers_own_then_neutral_then_others() {
    // 堵洞④偏好序：本会话所启 > human/无主 > 他会话所启（带诚实注记）。
    let port = spawn_canned_version_server();

    // ① 全活：root-a → own（b1）选中，无注记（注册表头是 b3——证明序来自
    // 偏好策略不是注册表巧合）。
    let m = registry_matrix(true, true, true, port);
    let (s, note) = m.pick_default_session_for(Some("root-a")).await.unwrap();
    assert_eq!(s.id, "b1");
    assert!(note.is_none(), "own 选中不得带注记: {note:?}");
    // root-b 同矩阵 → b3 own 选中。
    let (s, note) = m.pick_default_session_for(Some("root-b")).await.unwrap();
    assert_eq!(s.id, "b3");
    assert!(note.is_none(), "无主同链：own 选中无注记");

    // ② own 死 → neutral（b2/human）顶上，仍无注记。
    let m = registry_matrix(false, true, true, port);
    let (s, note) = m.pick_default_session_for(Some("root-a")).await.unwrap();
    assert_eq!(s.id, "b2");
    assert!(note.is_none(), "human/无主选中不得带注记: {note:?}");

    // ③ own + neutral 全死 → others（b3/root-b）末选，带诚实注记。
    let m = registry_matrix(false, false, true, port);
    let (s, note) = m.pick_default_session_for(Some("root-a")).await.unwrap();
    assert_eq!(s.id, "b3");
    let note = note.expect("others 选中必须带注记");
    assert!(note.contains("root-b") && note.contains("b3"), "{note}");

    // ④ 人面（None）：旧行为 = 第一个活会话（b3），不受偏好序影响、无注记。
    let (s, note) = m.pick_default_session_for(None).await.unwrap();
    assert_eq!(s.id, "b3");
    assert!(note.is_none());
}

#[tokio::test]
async fn close_session_bot_gate_wrong_holder_refuses_and_keeps_session() {
    // 双闸①（实例半闸）：非启动者拒关 + 注册表条目保留 + 台账不动。
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    push_test_session(&m, "bG", true, 0);
    m.owners.lock().insert("bG".into(), "root-a".into());

    let err = m.close_session("bG", Some("root-b")).await.unwrap_err();
    assert!(
        err.contains("root-a") && err.contains("只关自己开的"),
        "{err}"
    );
    assert!(m.session("bG").is_some(), "拒关后实例必须仍在注册表");
    assert_eq!(m.owner_of("bG").as_deref(), Some("root-a"));

    // 台账无条目（重启前的旧会话）同样按异己拒绝。
    m.owners.lock().remove("bG");
    let err = m.close_session("bG", Some("root-b")).await.unwrap_err();
    assert!(err.contains("未知会话"), "{err}");
    assert!(m.session("bG").is_some());
}

#[tokio::test]
async fn close_session_human_face_bypasses_and_removes_entry() {
    // 人面（None）免检：Dashboard 关闭语义不变；关后注册表 + 台账双摘。
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    push_test_session(&m, "bH", true, 0);
    m.owners.lock().insert("bH".into(), "human".into());

    let r = m.close_session("bH", None).await.unwrap();
    assert_eq!(r["ok"], json!(true));
    assert!(m.session("bH").is_none(), "关闭后必须摘出注册表");
    assert_eq!(m.owner_of("bH"), None, "关闭后必须摘出台账条目");

    // 不存在的 id：诚实报错（人面 bot 面同文案）。
    let err = m.close_session("bX", None).await.unwrap_err();
    assert!(err.contains("不存在"), "{err}");
}

#[test]
fn sessions_snapshot_and_owner_of_project_owner() {
    // 台账投影：sessions_snapshot 行带 owner；无条目行不带 owner 键；
    // owner_of None = 台账缺席（重启前旧会话语义）。
    let dir = tempfile::tempdir().unwrap();
    let m = manager_with(dir.path().to_path_buf(), BrowserPrefs::default());
    push_test_session(&m, "bS1", true, 0);
    push_test_session(&m, "bS2", true, 0);
    m.owners.lock().insert("bS1".into(), "root-a".into());

    let snap = m.sessions_snapshot();
    assert_eq!(snap[0]["id"], json!("bS1"));
    assert_eq!(snap[0]["owner"], json!("root-a"));
    assert_eq!(snap[1]["id"], json!("bS2"));
    assert!(
        snap[1].get("owner").is_none(),
        "台账无条目行不得投影 owner 键",
    );
    assert_eq!(m.owner_of("bS1").as_deref(), Some("root-a"));
    assert_eq!(m.owner_of("bS2"), None);
}
