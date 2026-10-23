#![cfg(windows)]
//! M1 实弹集成（总控 §10 M1 验收）：真实 spawn dummy 目标 → 注入 payload →
//! 管道握手（PID/令牌/协议/架构四重校验）→ read_mem + profile 结构解码 →
//! unload 自卸 → 目标进程存活确认。
//!
//! 前置与降级纪律（CLAUDE.md 环境敏感速查）：
//! - payload 由 build.rs 内嵌（plugins/ziwei-agent-dll/bin 双 DLL 缺席 =
//!   诚实报错路径，见 `payload_missing_is_honest`）；
//! - dummy exe 缺席 → 测试内现场 `cargo build --release`（一次 ~10s）；
//!   构建失败 → 诚实 SKIP（eprintln + return，不算失败）；
//! - 外来进程抢连 / 会话上限 / WOW64 x86 链路各有独立用例。

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nemesis_ziwei::pipe::{self, PipeError};
use nemesis_ziwei::session::{AttachError, SessionManager};
use nemesis_ziwei_profile::{DataStructure, FieldDecode, FieldDef, FieldType, Profile};

const DUMMY_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-tools/dummy-targets"
);

/// 取 dummy exe（缺席现场构建；失败 = SKIP）。
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

/// spawn dummy 并解析 stdout 契约行 → (child, pid, data_addr)。
/// 两种契约行都认：dummy-draw=DATA_ADDR / dummy-monitor=STATE_ADDR。
#[allow(unused_assignments)]
fn spawn_dummy(exe: &Path, extra: &[&str]) -> (Child, u32, u64) {
    let mut child = Command::new(exe)
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // kill 后 dummy println 炸管的噪音不入测试输出
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn dummy");
    let mut pid = 0u32;
    let mut data_addr = 0u64;
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => panic!("dummy 提前退出"),
            Ok(_) => {
                if let Some(v) = line.trim().strip_prefix("PID=") {
                    pid = v.parse().expect("PID 解析");
                }
                for marker in ["DATA_ADDR=", "STATE_ADDR="] {
                    if let Some(v) = line.trim().strip_prefix(marker) {
                        data_addr =
                            u64::from_str_radix(v.trim_start_matches("0x"), 16).expect("addr 解析");
                    }
                }
                if pid != 0 && data_addr != 0 {
                    break; // 契约行齐了
                }
            }
            Err(e) => panic!("读 dummy stdout 失败: {e}"),
        }
    }
    // 读完剩余契约行（不阻塞：dummy 循环打印 tick，保留 reader 防管道堵）
    std::thread::spawn(move || {
        let mut rest = String::new();
        let mut r = reader;
        loop {
            rest.clear();
            match r.read_line(&mut rest) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    });
    assert!(pid != 0 && data_addr != 0, "stdout 契约不完整");
    (child, pid, data_addr)
}

/// 构造指向 dummy-draw 的 profile（V1 结构与 dummy 同源）。
fn draw_profile(profile_id: &str) -> Profile {
    serde_json::from_str(&format!(
        r#"{{
        "schema_version": 1,
        "profile_id": "{profile_id}",
        "name": "集成-绘图 dummy",
        "created_at": "2026-10-22T00:00:00Z",
        "last_verified_at": "2026-10-22T00:00:00Z",
        "status": "active",
        "target": {{"exe_name": "dummy-draw.exe", "machine": "x64", "verify_policy": "hash_or_version"}},
        "attach": {{"mode": "attach"}},
        "verification": {{"on_attach": ["file_sha256"]}},
        "data_structures": [
            {{
                "name": "DataHeader",
                "size": 16,
                "fields": [
                    {{"offset": 0, "name": "magic", "type": "u32", "decode": "hex"}},
                    {{"offset": 4, "name": "layout_version", "type": "u32"}},
                    {{"offset": 8, "name": "count", "type": "u32"}},
                    {{"offset": 12, "name": "capacity", "type": "u32"}}
                ]
            }},
            {{
                "name": "DataRecordV1",
                "size": 40,
                "fields": [
                    {{"offset": 0, "name": "id", "type": "u32"}},
                    {{"offset": 8, "name": "value", "type": "i64"}},
                    {{"offset": 16, "name": "label", "type": "char[16]", "decode": "utf8"}},
                    {{"offset": 32, "name": "flags", "type": "u32", "decode": "hex"}}
                ]
            }}
        ]
    }}"#
    ))
    .expect("profile JSON")
}

fn ft(spec: &str) -> FieldType {
    FieldType::parse(spec).unwrap()
}

fn header_struct() -> DataStructure {
    DataStructure {
        name: "DataHeader".into(),
        size: 16,
        fields: vec![
            FieldDef {
                offset: 0,
                name: "magic".into(),
                type_: ft("u32"),
                decode: Some(FieldDecode::Hex),
            },
            FieldDef {
                offset: 4,
                name: "layout_version".into(),
                type_: ft("u32"),
                decode: None,
            },
            FieldDef {
                offset: 8,
                name: "count".into(),
                type_: ft("u32"),
                decode: None,
            },
            FieldDef {
                offset: 12,
                name: "capacity".into(),
                type_: ft("u32"),
                decode: None,
            },
        ],
    }
}

fn record_struct() -> DataStructure {
    DataStructure {
        name: "DataRecordV1".into(),
        size: 40,
        fields: vec![
            FieldDef {
                offset: 0,
                name: "id".into(),
                type_: ft("u32"),
                decode: None,
            },
            FieldDef {
                offset: 8,
                name: "value".into(),
                type_: ft("i64"),
                decode: None,
            },
            FieldDef {
                offset: 16,
                name: "label".into(),
                type_: ft("char[16]"),
                decode: Some(FieldDecode::Utf8),
            },
            FieldDef {
                offset: 32,
                name: "flags".into(),
                type_: ft("u32"),
                decode: Some(FieldDecode::Hex),
            },
        ],
    }
}

/// 隔离工作区（payload first-use 落盘点）。
fn temp_ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "ziwei-it-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test(flavor = "multi_thread")]
async fn x64_attach_handshake_read_decode_detach() {
    let Some(exe) = dummy_exe("x64", "dummy-draw") else {
        return;
    };
    let (mut child, pid, data_addr) = spawn_dummy(&exe, &["--tick-ms=200"]);
    let ws = temp_ws("x64");

    let mgr = SessionManager::new(&ws, None, 4);
    let profile = draw_profile("zw-it-x64");

    // attach（注入+握手，60s 预算——CI 慢机注入/握手窗口内必完成）
    let attach = tokio::time::timeout(Duration::from_secs(60), mgr.attach(&profile, pid)).await;
    let session = match attach {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let _ = child.kill();
            panic!("attach 失败（注入/握手链路）: {e}");
        }
        Err(_) => {
            let _ = child.kill();
            panic!("attach 超时（60s）");
        }
    };
    assert_eq!(session.state().as_str(), "ready");
    assert_eq!(session.arch, "x64");
    assert_eq!(mgr.session_count(), 1);

    // read_mem + 解码：DataHeader（V1 魔法 0x5A6B7C8D / count=5 / cap=16）
    let hdr = mgr
        .read_struct(&profile.profile_id, data_addr, &header_struct())
        .await
        .expect("read header");
    assert_eq!(hdr["magic"], "0x5a6b7c8d", "V1 魔法锚");
    assert_eq!(hdr["layout_version"], 1);
    assert_eq!(hdr["count"], 5);
    assert_eq!(hdr["capacity"], 16);

    // record[0]：id=0 / label=REC_00（值随 tick 变化，只断静态字段）
    let rec0 = mgr
        .read_struct(&profile.profile_id, data_addr + 16, &record_struct())
        .await
        .expect("read record0");
    assert_eq!(rec0["id"], 0);
    assert_eq!(rec0["label"], "REC_00");
    assert_eq!(rec0["flags"], "0x1");

    // record[4]：id=4 / label=REC_04
    let rec4 = mgr
        .read_struct(
            &profile.profile_id,
            data_addr + 16 + 4 * 40,
            &record_struct(),
        )
        .await
        .expect("read record4");
    assert_eq!(rec4["id"], 4);
    assert_eq!(rec4["label"], "REC_04");

    // 数据活性：dummy 推程按 tick%6 轮转（record 0 首推在 tick 6，200ms 档
    // ~1.2s）——等 1.6s 保证至少一轮，value 必然离开初值 1000（delta 1..3）。
    let v1 = rec0["value"].as_i64().unwrap();
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let rec0b = mgr
        .read_struct(&profile.profile_id, data_addr + 16, &record_struct())
        .await
        .expect("read record0 again");
    let v2 = rec0b["value"].as_i64().unwrap();
    assert_ne!(v1, v2, "value 应被推程改写（{v1}→{v2}）");

    // detach（unload → payload FreeLibraryAndExitThread 自卸）
    let msg = tokio::time::timeout(Duration::from_secs(20), mgr.detach(&profile.profile_id))
        .await
        .expect("detach 超时")
        .expect("detach 失败");
    assert!(msg.contains("已卸载"), "{msg}");
    assert_eq!(mgr.session_count(), 0);

    // 目标进程应仍然存活（卸载不影响宿主）
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "目标进程不应随卸载退出"
    );
    let _ = child.kill();
    let _ = child.wait();

    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

#[tokio::test(flavor = "multi_thread")]
async fn x64_attach_twice_rejected() {
    let Some(exe) = dummy_exe("x64", "dummy-draw") else {
        return;
    };
    let (mut child, pid, _addr) = spawn_dummy(&exe, &["--tick-ms=500"]);
    let ws = temp_ws("twice");
    let mgr = SessionManager::new(&ws, None, 4);
    let profile = draw_profile("zw-it-twice");

    let session = tokio::time::timeout(Duration::from_secs(60), mgr.attach(&profile, pid))
        .await
        .expect("attach 超时")
        .expect("attach 失败");
    let err = mgr.attach(&profile, pid).await.unwrap_err();
    assert!(matches!(err, AttachError::AlreadyAttached(_)), "{err}");
    let _ = session; // 会话保活到测试尾

    mgr.detach(&profile.profile_id).await.expect("detach");
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

/// 外来进程抢连：管道 server 对非目标 PID 的连入必须 PidMismatch 硬拒
/// （GetNamedPipeClientProcessId 是 win32 查询，不信任对端自报）。
#[tokio::test]
async fn foreign_process_pipe_rejected() {
    let pipe_name = format!(
        r"\\.\pipe\ZiWei_it_foreign_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );
    let server = pipe::create_server(&pipe_name).expect("create server");
    let hs = tokio::spawn(pipe::finish_handshake(server, "tok123", 999_999_999, "x64"));

    // 本进程 = 外来者：打开管道即触发 server 侧 PID 校验
    let _client = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&pipe_name)
        .expect("外来连入");
    let res = tokio::time::timeout(Duration::from_secs(10), hs)
        .await
        .unwrap();
    match res.expect("handshake task") {
        Err(PipeError::PidMismatch { got, expect }) => {
            assert_eq!(expect, 999_999_999);
            assert_eq!(got, std::process::id(), "连入方应被识别为本进程");
        }
        Err(other) => panic!("应 PidMismatch，实得 {other}"),
        Ok(_) => panic!("外来连入握手不应成功"),
    }
}

/// payload 缺席（payload_dir 指向空目录）→ 诚实 IO 报错（不回退、不静默）。
#[tokio::test]
async fn payload_missing_is_honest() {
    let ws = temp_ws("missing");
    let empty_dir = ws.join("no-payload-here");
    std::fs::create_dir_all(&empty_dir).unwrap();
    let mgr = SessionManager::new(&ws, Some(empty_dir.to_string_lossy().to_string()), 2);
    let profile = draw_profile("zw-it-missing");
    // 用本进程 pid（存活）确保走到 payload 解析这一步
    let err = mgr.attach(&profile, std::process::id()).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("payload"), "诚实报错应含 payload 字样：{msg}");
}

/// WOW64 x86 全链路：32 位 dummy → x86 payload 注入 → 握手 → 读 → 卸载。
/// 前置：SysWOW64（x64 Windows）+ x86 payload 内嵌 + x86 dummy 在场。
#[tokio::test(flavor = "multi_thread")]
async fn x86_wow64_attach_read_detach() {
    if !Path::new(r"C:\Windows\SysWOW64\kernel32.dll").is_file() {
        eprintln!("SKIP: SysWOW64 不在场（非 x64 Windows）");
        return;
    }
    let Some(exe) = dummy_exe("x86", "dummy-draw") else {
        return;
    };
    let (mut child, pid, data_addr) = spawn_dummy(&exe, &["--tick-ms=200"]);
    let ws = temp_ws("x86");

    let mgr = SessionManager::new(&ws, None, 4);
    let mut profile = draw_profile("zw-it-x86");
    profile.target.machine = nemesis_ziwei_profile::Machine::X86;

    let attach = tokio::time::timeout(Duration::from_secs(60), mgr.attach(&profile, pid)).await;
    let session = match attach {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            let _ = child.kill();
            panic!("x86 attach 失败（WOW64 链路）: {e}");
        }
        Err(_) => {
            let _ = child.kill();
            panic!("x86 attach 超时（60s）");
        }
    };
    assert_eq!(session.arch, "x86");

    let hdr = mgr
        .read_struct(&profile.profile_id, data_addr, &header_struct())
        .await
        .expect("x86 read header");
    assert_eq!(hdr["magic"], "0x5a6b7c8d");
    assert_eq!(hdr["count"], 5);

    let rec0 = mgr
        .read_struct(&profile.profile_id, data_addr + 16, &record_struct())
        .await
        .expect("x86 read record0");
    assert_eq!(rec0["id"], 0);
    assert_eq!(rec0["label"], "REC_00");

    mgr.detach(&profile.profile_id).await.expect("x86 detach");
    assert_eq!(mgr.session_count(), 0);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "x86 目标不应随卸载退出"
    );
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}

/// 10 分钟 soak（#[ignore]——`cargo test -p nemesis-ziwei --test ziwei_m1_live
/// -- --ignored` 显式触发）：attach 后持续轮询，验证长时驻留不撕裂。
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn soak_10min() {
    let Some(exe) = dummy_exe("x64", "dummy-monitor") else {
        return;
    };
    let (mut child, pid, state_addr) = spawn_dummy(&exe, &["--interval-ms=1000"]);
    let ws = temp_ws("soak");
    let mgr = SessionManager::new(&ws, None, 2);
    let profile: Profile = serde_json::from_str(
        r#"{
        "schema_version": 1,
        "profile_id": "zw-soak",
        "name": "soak",
        "created_at": "2026-10-22T00:00:00Z",
        "last_verified_at": "2026-10-22T00:00:00Z",
        "status": "active",
        "target": {"exe_name": "dummy-monitor.exe", "machine": "x64", "verify_policy": "hash_or_version"},
        "attach": {"mode": "attach"},
        "verification": {"on_attach": ["file_sha256"]}
    }"#,
    )
    .unwrap();
    let ds = DataStructure {
        name: "MonitorState".into(),
        size: 80,
        fields: vec![
            FieldDef {
                offset: 0,
                name: "magic".into(),
                type_: ft("u32"),
                decode: Some(FieldDecode::Hex),
            },
            FieldDef {
                offset: 4,
                name: "status".into(),
                type_: ft("u32"),
                decode: None,
            },
            FieldDef {
                offset: 8,
                name: "tick".into(),
                type_: ft("u64"),
                decode: None,
            },
            FieldDef {
                offset: 16,
                name: "message".into(),
                type_: ft("wchar[32]"),
                decode: Some(FieldDecode::Utf16),
            },
        ],
    };
    let session = tokio::time::timeout(Duration::from_secs(60), mgr.attach(&profile, pid))
        .await
        .expect("attach 超时")
        .expect("attach 失败");
    let start = std::time::Instant::now();
    let mut last_tick = 0u64;
    let mut polls = 0u64;
    while start.elapsed() < Duration::from_secs(600) {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let v = mgr
            .read_struct(&profile.profile_id, state_addr, &ds)
            .await
            .expect("soak read");
        let tick = v["tick"].as_u64().unwrap();
        assert!(tick >= last_tick, "tick 回退（{last_tick}→{tick}）");
        assert_eq!(v["magic"], "0x4d4f4e31");
        last_tick = tick;
        polls += 1;
    }
    assert!(last_tick > 300, "10 分钟应有 300+ tick，实得 {last_tick}");
    mgr.detach(&profile.profile_id).await.expect("soak detach");
    println!(
        "soak 完成：{polls} 次轮询，tick={last_tick}，state={}",
        session.state().as_str()
    );
    let _ = child.kill();
    let _ = child.wait();
    let _ = nemesis_utils::fsutil::remove_dir_all_retry(&ws);
}
