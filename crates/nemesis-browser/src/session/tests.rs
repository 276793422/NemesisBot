//! session 辅助函数单测（纯本机、无浏览器依赖）。

use super::{kill_process, pid_targetable, process_alive, tail_file};

#[test]
fn tail_file_reads_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("log.txt");
    std::fs::write(&p, "l1\nl2\nl3\nl4\nl5\n").unwrap();
    assert_eq!(tail_file(&p, 2), "l4\nl5");
    assert_eq!(tail_file(&p, 10), "l1\nl2\nl3\nl4\nl5");
}

#[test]
fn tail_file_missing_is_empty() {
    assert_eq!(tail_file(std::path::Path::new("Z:/no/such.log"), 5), "");
}

#[test]
fn tail_file_non_utf8_lossy_not_empty() {
    // F30 回归锁（2026-11-15 五审）：中文 Windows 上浏览器 stderr 是本地
    // 代码页（GBK）——旧 read_to_string 严格 UTF-8 读取 Err → 尾部静默变
    // 空串，诊断现场整段丢失。字节读 + lossy 后 ASCII 骨架（路径/端口/
    // 错误码）保真。GBK 字节 =「中文错误 port 9222」。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("stderr.log");
    let gbk: &[u8] = &[
        0xD6, 0xD0, 0xCE, 0xC4, 0xB4, 0xED, 0xCE, 0xF3, b' ', b'p', b'o', b'r', b't', b' ', b'9',
        b'2', b'2', b'2',
    ];
    std::fs::write(&p, gbk).unwrap();
    let tail = tail_file(&p, 10);
    assert!(tail.contains("port 9222"), "{tail:?}");
}

#[test]
fn summary_projects_alive_dead_state() {
    // F27 回归锁（2026-11-15 五审）：summary 的 alive 字段随 exited 旗标
    // 翻转——用户手关窗口后 reaper ms 级置位，前端「已退出」渲染数据源。
    use crate::driver::DriverKind;
    let s = super::BrowserSession::test_with("bT", DriverKind::Edge, false, 1);
    assert_eq!(s.summary()["alive"], serde_json::Value::Bool(true));
    s.exited.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(s.summary()["alive"], serde_json::Value::Bool(false));
}

#[test]
fn process_alive_current_pid() {
    // 本测试进程自己必然活着。
    assert!(process_alive(std::process::id()));
}

#[test]
fn process_alive_bogus_pid() {
    // PID u32::MAX 不可能存在（Windows 32 位 pid 上限远低于此）。
    assert!(!process_alive(u32::MAX));
}

#[test]
fn kill_process_bogus_pid_is_noop() {
    // 对不存在的 pid 强杀必须无害（不 panic、不误伤）。
    kill_process(u32::MAX);
}

// --- pid=0 守卫（F36，2026-11-15 六审）---

#[test]
fn process_alive_zero_pid_is_false() {
    // 0 号是保留值：Unix kill(0,0) 语义=「探测本进程组」恒真（必假活）；
    // spawn 竞态 unwrap_or(0) 与收养会话都不许被误判。
    assert!(!process_alive(0));
}

#[test]
fn kill_process_zero_pid_is_noop() {
    // Unix `kill(0, SIGKILL)` = 信号发给整个进程组 = 网关自己被杀。守卫
    // 必须 no-op（不 panic；Windows taskkill /PID 0 无害但 Linux 是组语义）。
    kill_process(0);
}

// --- 孤儿窗口收养（F37，2026-11-15 六审）---

/// 假 CDP HTTP 端点（收养探针打 GET /json/version；先读请求再写响应——
/// 带未读数据 close 时 Windows 发 RST 冲掉响应，见 F28 坑位实录）。
fn spawn_canned_version_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let resp: &[u8] =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut drain = [0u8; 2048];
            let _ = std::io::Read::read(&mut s, &mut drain);
            let _ = std::io::Write::write_all(&mut s, resp);
        }
    });
    port
}

#[tokio::test]
async fn spawn_adopts_orphan_browser_via_live_port_file() {
    // F37 回归锁：端口文件在 + 端点活 = 不 spawn 直接收养（pid=0、端口
    // 来自文件、预清理不删文件）。executable 是不存在的路径——收养臂不
    // 触碰它，spawn 臂才会「启动浏览器」失败。
    use crate::cdp::DEVTOOLS_PORT_FILE;
    use crate::driver::DriverKind;

    let port = spawn_canned_version_server();
    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join(DEVTOOLS_PORT_FILE),
        format!("{port}\n/devtools/browser/x\n"),
    )
    .unwrap();

    let s = super::BrowserSession::spawn(
        "bA".to_string(),
        1,
        DriverKind::Edge,
        std::path::PathBuf::from("Z:/no/such/browser.exe"),
        &profile_root,
        "human",
    )
    .await
    .expect("活端口文件应触发收养而非 spawn");
    assert_eq!(s.port, port, "收养会话端口应来自端口文件");
    assert_eq!(
        s.pid, 0,
        "收养会话 pid 未知 = 0（强杀兜底被 F36 守卫 no-op）"
    );
    assert!(s.is_alive().await, "端点活 = 会话活");
    // 端口文件必须仍在（收养臂不得触发预清理）。
    assert!(profile_dir.join(DEVTOOLS_PORT_FILE).exists());
}

#[tokio::test]
async fn spawn_with_dead_port_file_falls_through_to_launch() {
    // 死端点（端口 1 无监听）= 不收养，回落正常 spawn 路径——本例
    // executable 无效，报「启动浏览器」错证明走的是 spawn 臂。
    use crate::cdp::DEVTOOLS_PORT_FILE;
    use crate::driver::DriverKind;

    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    std::fs::write(
        profile_dir.join(DEVTOOLS_PORT_FILE),
        "1\n/devtools/browser/x\n",
    )
    .unwrap();

    let Err(err) = super::BrowserSession::spawn(
        "bB".to_string(),
        1,
        DriverKind::Edge,
        std::path::PathBuf::from("Z:/no/such/browser.exe"),
        &profile_root,
        "human",
    )
    .await
    else {
        panic!("死端口文件不应收养（必须回落 spawn 臂）");
    };
    assert!(err.contains("启动浏览器"), "{err}");
}

// --- relaunch 收养 + 孤儿收尸 + stderr 语境（2026-11-23 修复 A/B/C）---

#[test]
fn stderr_tail_context_fresh_write_has_no_stale_note() {
    // 本次进程有写入（mtime 晚于 spawn 起点）→ 正常尾部，不带历史注记。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("stderr.log");
    std::fs::write(&p, "recent boom\n").unwrap();
    let spawn_start = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let ctx = super::stderr_tail_context(&p, spawn_start);
    assert!(ctx.contains("stderr 尾部"), "{ctx:?}");
    assert!(ctx.contains("recent boom"));
    assert!(!ctx.contains("历史日志"), "{ctx:?}");
}

#[test]
fn stderr_tail_context_stale_write_notes_history() {
    // 修复 C 回归锁：append 累积的旧日志（mtime 早于 spawn 起点）必须带
    // 「历史日志」注记——11-23 现场：11-17 的 ERROR 尾部被当成 11-23 进程
    // 的临终输出，误导诊断方向。
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("stderr.log");
    std::fs::write(&p, "ancient error\n").unwrap();
    {
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
            .unwrap();
    }
    let ctx = super::stderr_tail_context(&p, std::time::SystemTime::now());
    assert!(ctx.contains("历史日志"), "{ctx:?}");
    assert!(ctx.contains("ancient error"), "{ctx:?}");
}

#[test]
fn stderr_tail_context_absent_log_is_honest() {
    // 日志文件不存在（进程零输出且文件未建）= 诚实「零写入」，不空串糊弄。
    let ctx = super::stderr_tail_context(
        std::path::Path::new("Z:/no/such/stderr.log"),
        std::time::SystemTime::now(),
    );
    assert!(ctx.contains("零写入"), "{ctx:?}");
}

/// 写完 fresh DevToolsActivePort 后（可选延时命令）退出的 Windows 批处理
/// （进程级回归锁的「Edge relaunch」替身：留下端口文件后退出）。
/// `delay_cmd` 传 "" = 写完立即退（raw 先于轮询置位 → 终裁收养路径）；
/// 传 ping 等 = 装配后才退（wait task 终裁路径）。
// 仅 Windows 测试调用（下方全部调用点 cfg(windows)）；无门时 Linux
// `--all-targets` 编译本文件即零调用者 → dead_code 堵 clippy 闸
// （BUG-20261125-064）。
#[cfg(windows)]
fn write_relay_bat(
    profile_dir: &std::path::Path,
    port: u16,
    delay_cmd: &str,
) -> std::path::PathBuf {
    let bat = profile_dir.join("relay.bat");
    // @echo off = 零 stderr；> 重定向 = fresh mtime。
    let content = format!(
        "@echo off\r\n> \"{}\" (echo {}\r\necho /devtools/browser/relay)\r\n{}\r\n",
        profile_dir.join(crate::cdp::DEVTOOLS_PORT_FILE).display(),
        port,
        delay_cmd
    );
    std::fs::write(&bat, content).unwrap();
    bat
}

#[cfg(windows)]
#[tokio::test]
async fn spawn_process_exit_with_fresh_port_file_is_adopted() {
    // 修复 A 回归锁（11-23 现场）：进程退出 ≠ 启动失败。进程写了 fresh
    // 端口文件再退出（Edge relaunch 接力成功形态）→ 终裁读文件 → 收养
    // 装配（pid=0、会话活、端口来自文件）。
    use crate::driver::DriverKind;

    let port = spawn_canned_version_server();
    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    let bat = write_relay_bat(&profile_dir, port, "");

    let s = super::BrowserSession::spawn(
        "bR".to_string(),
        1,
        DriverKind::Edge,
        bat,
        &profile_root,
        "human",
    )
    .await
    .expect("进程退出但 fresh 端口文件在 = relaunch 接力成功，应收养");
    assert_eq!(s.port, port, "收养端口来自接力进程写的端口文件");
    assert_eq!(s.pid, 0, "接力进程 pid 未知 = 0（F36 禁杀守卫）");
    assert!(s.is_alive().await, "端点活 = 会话活");
}

#[cfg(windows)]
#[tokio::test]
async fn spawn_process_exit_after_ready_same_port_stays_alive() {
    // 修复 A 终裁臂回归锁：装配成功（正常态）后原始进程才退出（relaunch
    // 慢于轮询的形态）——wait task 终裁探装配端口：canned 端点仍活 = 同
    // 端口接力 → 转探针不置死当旗标，会话保持可用投影（不误杀）。
    use crate::driver::DriverKind;

    let port = spawn_canned_version_server();
    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    // 写文件（轮询 ~250ms 读到 → 装配）→ ping ~2s（装配后才退出）。
    let bat = write_relay_bat(&profile_dir, port, "ping -n 3 127.0.0.1 > nul");

    let s = super::BrowserSession::spawn(
        "bK".to_string(),
        1,
        DriverKind::Edge,
        bat,
        &profile_root,
        "human",
    )
    .await
    .expect("端口文件在且端点活 = 装配成功");
    // 等 bat 退出 + wait task 终裁探端口（bat ~2s + 终裁即时）。
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert!(
        s.is_alive().await,
        "装配后进程退出但端点活 = 终裁应转探针，不得误杀投影"
    );
    assert_eq!(s.summary()["alive"], serde_json::Value::Bool(true));
}

#[cfg(windows)]
#[tokio::test]
async fn spawn_process_exit_without_port_file_fails_fast() {
    // 修复 A 加速器语义回归锁：进程退出 + 无端口文件 = 快速失败（不等满
    // 120s——退出信号的价值就在这）；cmd 报错写 stderr（fresh）→ 尾部
    // 进报错无历史注记。
    use crate::driver::DriverKind;

    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    std::fs::create_dir_all(&profile_root).unwrap();
    let t0 = std::time::Instant::now();
    let Err(err) = super::BrowserSession::spawn(
        "bF".to_string(),
        1,
        DriverKind::Edge,
        std::path::PathBuf::from("cmd"),
        &profile_root,
        "human",
    )
    .await
    else {
        panic!("进程退出且无端口文件 = 必须失败");
    };
    assert!(err.contains("提前退出"), "{err}");
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(30),
        "退出信号应加速失败（实际 {:?}）",
        t0.elapsed()
    );
}

#[cfg(windows)]
#[tokio::test]
async fn spawn_exit_with_fresh_port_but_dead_endpoint_fails_and_cleans() {
    // 修复 A 终裁 + 修复 B 收尸回归锁：文件在但端点死（接力进程没起来）
    // = wait_for_endpoint 失败 → Err + 树杀兜底诚实跑过（无匹配进程 0 杀）。
    use crate::driver::DriverKind;

    let dir = tempfile::tempdir().unwrap();
    let profile_root = dir.path().join("browser");
    let profile_dir = profile_root.join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    // 死端口（1 无监听）：bat 写出 fresh 文件后立即退出。
    let bat = write_relay_bat(&profile_dir, 1, "");

    let Err(err) = super::BrowserSession::spawn(
        "bD".to_string(),
        1,
        DriverKind::Edge,
        bat,
        &profile_root,
        "human",
    )
    .await
    else {
        panic!("端口文件在但端点死 = 必须失败");
    };
    assert!(!err.is_empty(), "{err}");
}

#[cfg(windows)]
#[test]
fn kill_profile_tree_kills_matching_process() {
    // 修复 B 回归锁：命令行含专用 profile 路径的进程被树杀；不含的不伤。
    // 活靶用 PowerShell（cmd REM 形态实测立即退出不可靠）——CommandLine
    // 含 profile 路径的睡眠进程；系统里其他 powershell 不含该路径不受扰。
    let dir = tempfile::tempdir().unwrap();
    let profile_dir = dir.path().join("edge");
    std::fs::create_dir_all(&profile_dir).unwrap();
    use std::os::windows::process::CommandExt;
    let mut child = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("$x='{}'; Start-Sleep -Seconds 30", profile_dir.display()),
        ])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .expect("powershell 活靶 spawn");
    std::thread::sleep(std::time::Duration::from_millis(1500)); // 等 CommandLine 注册
    assert!(child.try_wait().unwrap().is_none(), "活靶应在树杀前存活");
    super::kill_profile_tree(&profile_dir);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut gone = false;
    while std::time::Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            gone = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    assert!(gone, "树杀后活靶应退出");
    // 无匹配目录 = 诚实 0 杀不 panic（空转 smoke）。
    let empty = tempfile::tempdir().unwrap();
    super::kill_profile_tree(empty.path());
}

// BUG-20261128-009：pid 资格守卫平台契约钉死（Unix 堵 -1 广播别名 /
// Windows 保回绕高段 u32 全空间可达）。
#[test]
fn pid_targetable_rejects_zero() {
    assert!(!pid_targetable(0));
}

#[cfg(windows)]
#[test]
fn pid_targetable_windows_accepts_high_u32_pids() {
    // Windows PID 按 4 递增回绕整个 u32 空间，[2^31, 2^32) 高段是真实
    // 存在的进程（长 uptime 高翻新机器可见）；tasklist /FI "PID eq" 与
    // taskkill /PID 都吃完整 u32——守卫不得用 i32 负数别名假判死。
    assert!(pid_targetable(3_000_000_000));
    assert!(pid_targetable(u32::MAX));
}

#[cfg(not(windows))]
#[test]
fn pid_targetable_unix_rejects_negative_alias() {
    // u32::MAX 折 i32 = -1：kill(-1, 0) 探测恒真 / kill(-1, 9) 屠杀全体。
    assert!(!pid_targetable(u32::MAX));
    assert!(pid_targetable(1));
}

#[test]
fn process_alive_self_pid_is_alive() {
    // 守卫不被误扩的端到端锚：本进程 pid 必须可达（Windows 走 tasklist，
    // Unix 走 kill(pid, 0)）。
    assert!(process_alive(std::process::id()));
}

// --- 会话级四动作（2026-11-28 所有权批）：close_tab 匹配裁决 + foreign_tabs ---

use super::{MatchOutcome, match_tab, tab_list_row};
use crate::cdp::TargetInfo;

fn tab(id: &str, title: &str, url: &str) -> TargetInfo {
    TargetInfo {
        id: id.to_string(),
        target_type: "page".to_string(),
        title: title.to_string(),
        url: url.to_string(),
        attached: false,
    }
}

#[test]
fn match_tab_three_states_case_insensitive() {
    // 三态 + 大小写不敏感子串（target_id/url/title 三字段都参与；0/多命中
    // 绝不猜——与跨域 iframe「URL 子串 + 清单即发现机制」同款纪律）。
    let tabs = vec![
        tab("AAA111", "文本页", "http://127.0.0.1:1/text"),
        tab("BBB222", "表单页", "http://127.0.0.1:1/form"),
    ];
    // 唯一命中：URL 子串（含路径）/title/id 三路都可命中。
    match match_tab(&tabs, "/form") {
        MatchOutcome::Unique(t) => assert_eq!(t.id, "BBB222"),
        _ => panic!("/form 必须唯一命中"),
    }
    match match_tab(&tabs, "表单页") {
        MatchOutcome::Unique(t) => assert_eq!(t.id, "BBB222"),
        _ => panic!("title 命中失败"),
    }
    match match_tab(&tabs, "bbb") {
        MatchOutcome::Unique(t) => assert_eq!(t.id, "BBB222"),
        _ => panic!("id 大小写不敏感命中失败"),
    }
    // 大写 needle 打小写 URL（不敏感双向）。
    match match_tab(&tabs, "/TEXT") {
        MatchOutcome::Unique(t) => assert_eq!(t.id, "AAA111"),
        _ => panic!("大写 needle 命中失败"),
    }
    // 0 命中。
    assert!(matches!(
        match_tab(&tabs, "no-such-page"),
        MatchOutcome::NotFound
    ));
    // 多命中：同域前缀两个 tab 都含。
    match match_tab(&tabs, "127.0.0.1") {
        MatchOutcome::Ambiguous(hits) => assert_eq!(hits.len(), 2),
        _ => panic!("同域前缀必须判多命中"),
    }
}

#[test]
fn tab_list_row_carries_all_three_fields() {
    // 错误清单行 = title + url + 完整 target_id（同标题同址时模型需要 id
    // 才能拼出唯一子串）。
    let row = tab_list_row(&tab("XYZ789", "标题", "http://h/p"));
    assert!(row.contains("标题"), "{row}");
    assert!(row.contains("http://h/p"), "{row}");
    assert!(row.contains("XYZ789"), "{row}");
}

/// foreign_tabs 三臂（close_browser 连坐半闸的 tab 半闸）：
/// ① 人面（None）恒空清单（免检，不触 driver）；② list_tabs 失败（实例
/// 死透）= 空清单半闸放行（实例闸仍由 manager 把守）；③ 有 caller 时
/// 无主 tab 也算异己（用户手动开的不能被 bot 连坐扫掉）——③ 需要 list_tabs
/// 活体，归 edge_live 所有权闭环；此处锁 ①② 两臂。
#[tokio::test]
async fn foreign_tabs_human_face_empty_and_dead_driver_fail_open() {
    let s = super::BrowserSession::test_with("bFT", crate::detect::DriverKind::Edge, true, 0);
    // ① 人面：免检（不触 driver——死实例也不拦人面）。
    assert!(s.foreign_tabs(None).await.is_empty());
    // ② bot 面 + 死 driver：list_tabs Err → 空清单放行（fail-open 半闸）。
    assert!(s.foreign_tabs(Some("root-a")).await.is_empty());
}
