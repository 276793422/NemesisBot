#![cfg(windows)]
//! M2 实弹集成（总控 §10 M2 验收，场景 B）：真实 spawn dummy-monitor →
//! attach_and_schedule → poll 调度（on_change + notify）→ 报警到达事件汇
//! （≤3s）→ 重复报警限频（60s 窗口内只发一次）→ capture 回流落盘 →
//! 一次性快照读 → detach 停调度。
//!
//! 前置与降级纪律同 ziwei_m1_live：payload 内嵌缺席 / dummy 构建失败 =
//! 诚实 SKIP。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use nemesis_ziwei::capture::CaptureStore;
use nemesis_ziwei::scheduler::{ZiweiEvent, attach_and_schedule};
use nemesis_ziwei::session::SessionManager;
use nemesis_ziwei_profile::Profile;

const DUMMY_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/dummy-targets"
);

fn dummy_exe(arch: &str, name: &str) -> Option<PathBuf> {
    let p = Path::new(DUMMY_ROOT)
        .join("bin")
        .join(arch)
        .join(format!("{name}.exe"));
    if p.is_file() {
        return Some(p);
    }
    let manifest = Path::new(DUMMY_ROOT).join(name).join("Cargo.toml");
    let built = Command::new("cargo")
        .args(["build", "--release"])
        .arg("--manifest-path")
        .arg(&manifest)
        .status();
    match built {
        Ok(s) if s.success() && p.is_file() => Some(p),
        other => {
            eprintln!("SKIP: {name} 现场构建失败（{other:?}）——集成前置不满足");
            None
        }
    }
}

/// spawn dummy-monitor 并解析 stdout 契约行（PID= / STATE_ADDR=）。
fn spawn_monitor(exe: &Path, extra: &[&str]) -> (Child, u32, u64) {
    let mut child = Command::new(exe)
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn dummy-monitor");
    let mut pid = 0u32;
    let mut addr = 0u64;
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => panic!("dummy-monitor 提前退出"),
            Ok(_) => {
                if let Some(v) = line.trim().strip_prefix("PID=") {
                    pid = v.parse().expect("PID 解析");
                }
                if let Some(v) = line.trim().strip_prefix("STATE_ADDR=") {
                    addr = u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("addr 解析");
                }
                if pid != 0 && addr != 0 {
                    break;
                }
            }
            Err(e) => panic!("读 stdout 失败: {e}"),
        }
    }
    // 剩余 tick 行防管道堵
    std::thread::spawn(move || {
        let mut rest = String::new();
        loop {
            rest.clear();
            match reader.read_line(&mut rest) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    (child, pid, addr)
}

/// 场景 B profile：MonitorState poll 工具 + ALERT 报警条件。
fn monitor_profile(profile_id: &str) -> Profile {
    serde_json::from_str(&format!(
        r#"{{
        "schema_version": 1,
        "profile_id": "{profile_id}",
        "name": "集成-监视 dummy",
        "created_at": "2026-10-22T00:00:00Z",
        "last_verified_at": "2026-10-22T00:00:00Z",
        "status": "active",
        "target": {{"exe_name": "dummy-monitor.exe", "machine": "x64", "verify_policy": "hash_or_version"}},
        "attach": {{"mode": "attach"}},
        "verification": {{"on_attach": ["file_sha256"]}},
        "data_structures": [
            {{
                "name": "MonitorState",
                "size": 80,
                "fields": [
                    {{"offset": 0, "name": "magic", "type": "u32", "decode": "hex"}},
                    {{"offset": 4, "name": "status", "type": "u32"}},
                    {{"offset": 8, "name": "tick", "type": "u64"}},
                    {{"offset": 16, "name": "message", "type": "wchar[32]", "decode": "utf16"}}
                ]
            }}
        ],
        "tools": [
            {{
                "tool_name": "mon_state",
                "description": "读取监视器状态",
                "input_schema": null,
                "action": {{
                    "primitive": "poll_report",
                    "addr": "struct:MonitorState",
                    "mode": "on_change",
                    "interval_secs": 1,
                    "notify": {{"on": "message==MONITOR_ALERT", "channel": "outbound"}}
                }},
                "run_policy": "auto"
            }}
        ]
    }}"#
    ))
    .expect("profile JSON")
}

fn temp_ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-m2-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 场景 B 主链路：翻转 → 报警到达（≤3s/轮询窗）→ 限频 → capture → invoke。
#[tokio::test(flavor = "multi_thread")]
async fn alert_to_sink_with_rate_limit_and_capture() {
    let Some(exe) = dummy_exe("x64", "dummy-monitor") else {
        return;
    };
    // 400ms 翻转：轮询窗 1s 内状态必变（on_change 每窗触发，ALERT 每 3 tick
    // 一遇）——限频窗口内应只发一次报警。
    let (mut child, pid, state_addr) = spawn_monitor(&exe, &["--interval-ms=400"]);
    let ws = temp_ws("alert");
    let mgr = std::sync::Arc::new(SessionManager::new(&ws, None, 2));
    let profile = monitor_profile("zw-m2-alert");

    let alerts: std::sync::Arc<Mutex<Vec<ZiweiEvent>>> = Default::default();
    let sink_alerts = alerts.clone();
    mgr.set_event_sink(std::sync::Arc::new(move |ev: ZiweiEvent| {
        sink_alerts.lock().unwrap().push(ev);
    }));

    let attach = tokio::time::timeout(
        Duration::from_secs(60),
        attach_and_schedule(
            &mgr,
            &profile,
            pid,
            [("MonitorState".to_string(), state_addr)]
                .into_iter()
                .collect(),
        ),
    )
    .await;
    let session = match attach {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let _ = child.kill();
            panic!("attach_and_schedule 失败: {e}");
        }
        Err(_) => {
            let _ = child.kill();
            panic!("attach 超时（60s）");
        }
    };
    assert_eq!(session.state().as_str(), "ready");

    // 验收：状态翻转 → 报警 ≤3s 到达（轮询窗 1s，ALERT 每 3 tick 一遇，
    // 10s 预算覆盖慢机抖动）。
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !alerts.lock().unwrap().is_empty() || std::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    {
        let got = alerts.lock().unwrap();
        assert!(!got.is_empty(), "10s 内未收到报警（调度/条件链路断裂）");
        let ZiweiEvent::Alert {
            profile_id,
            profile_name,
            tool,
            expr,
            snapshot,
        } = &got[0];
        assert_eq!(profile_id, "zw-m2-alert");
        assert_eq!(profile_name, "集成-监视 dummy");
        assert_eq!(tool, "mon_state");
        assert_eq!(expr, "message==MONITOR_ALERT");
        assert_eq!(snapshot["message"], "MONITOR_ALERT");
        println!(
            "M2 报警到达：tool={tool} expr={expr} tick={}",
            snapshot["tick"]
        );
    }

    // 限频：60s 窗口内重复命中只发一次（dummy 持续翻转，无限制会有多条）。
    tokio::time::sleep(Duration::from_secs(4)).await;
    {
        let got = alerts.lock().unwrap();
        assert_eq!(
            got.len(),
            1,
            "限频失效：4s 窗口内收到 {} 条报警（应恰 1 条）",
            got.len()
        );
    }

    // capture 回流：on_change 每窗落一条（≥2 条 = 调度照常、报警限频只限通知）。
    let cap = CaptureStore::new(&ws);
    let records = cap.read_recent("zw-m2-alert", 100);
    assert!(
        records.len() >= 2,
        "capture 应 ≥2 条（实得 {}）",
        records.len()
    );
    assert_eq!(records.last().unwrap()["kind"], "poll");

    // invoke 语义（一次性快照读）——与 WSAPI/agent 子工具同源路径。
    let ds = &profile.data_structures[0];
    let snap = mgr
        .read_struct("zw-m2-alert", state_addr, ds)
        .await
        .expect("invoke read");
    assert_eq!(snap["magic"], "0x4d4f4e31");
    assert!(
        snap["message"].as_str().unwrap().starts_with("MONITOR_"),
        "message 应为 MONITOR_*：{:?}",
        snap["message"]
    );

    // detach：poll 任务 abort + 卸载干净。
    mgr.detach("zw-m2-alert").await.expect("detach");
    assert_eq!(mgr.session_count(), 0);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(child.try_wait().unwrap().is_none(), "目标不应随卸载退出");
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

/// 结构地址未解析（空 struct_addrs）→ 工具诚实停用（warn 一次，无报警、
/// 不炸会话；读路径照常可用——「停用」只指调度）。
#[tokio::test(flavor = "multi_thread")]
async fn unresolved_struct_addr_disables_poll_honestly() {
    let Some(exe) = dummy_exe("x64", "dummy-monitor") else {
        return;
    };
    let (mut child, pid, _addr) = spawn_monitor(&exe, &["--interval-ms=400"]);
    let ws = temp_ws("noaddr");
    let mgr = std::sync::Arc::new(SessionManager::new(&ws, None, 2));
    let profile = monitor_profile("zw-m2-noaddr");
    let alerts: std::sync::Arc<Mutex<Vec<ZiweiEvent>>> = Default::default();
    let sink_alerts = alerts.clone();
    mgr.set_event_sink(std::sync::Arc::new(move |ev: ZiweiEvent| {
        sink_alerts.lock().unwrap().push(ev);
    }));

    let session = tokio::time::timeout(
        Duration::from_secs(60),
        attach_and_schedule(&mgr, &profile, pid, Default::default()),
    )
    .await
    .expect("attach 超时")
    .expect("attach 失败");
    assert_eq!(session.state().as_str(), "ready");

    // 无地址 → 调度器不开环：3s 内零报警（无 panic、无假报警）。
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        alerts.lock().unwrap().is_empty(),
        "地址未解析不应有任何报警"
    );

    mgr.detach("zw-m2-noaddr").await.expect("detach");
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}
