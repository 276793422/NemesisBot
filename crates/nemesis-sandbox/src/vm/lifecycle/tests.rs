//! [`lifecycle`] 单测。三组：
//! 1. `adjudicate_accel` 纯函数全组合（G2 门「探针缺席 → Unavailable 单测；
//!    strict+缺席 → 拒绝执行单测」的落点）；
//! 2. token 生成形态；
//! 3. spawn 失败诚实面（不存在的 qemu 二进制）+ 首帧鉴权/工具调用的
//!    **进程内 guest 全链回归**（真实 TCP + 真实 nemesis-guest，无 QEMU——
//!    QEMU 侧真启动裁决归 KVM/WHPX 实机冒烟）。

use std::time::Duration;

use super::*;
use crate::vm::guest_arch::QemuAccel;
use crate::vm::probe::AccelProbe;

fn probe(available: bool, reason: &str) -> AccelProbe {
    AccelProbe {
        available,
        layer: if available { "static" } else { "none" },
        reason: if available {
            String::new()
        } else {
            reason.to_string()
        },
    }
}

// ---------------------------------------------------------------------------
// adjudicate_accel：纯函数全组合
// ---------------------------------------------------------------------------

/// G2 门：探针缺席 + 非 strict = Unavailable（调用方可回落其他执行后端）。
#[test]
fn adjudicate_unavailable_non_strict_gives_unavailable() {
    let kvm = probe(false, "/dev/kvm 不存在");
    let whpx = probe(false, "WinHvPlatform.dll 不可加载");
    let err = adjudicate_accel(false, None, &kvm, &whpx, false, GuestArch::X86_64).unwrap_err();
    assert!(matches!(&err, VmError::Unavailable(_)), "{err:?}");
    assert!(err.to_string().contains("不做 TCG 纯模拟降级"), "{err}");
}

/// G2 门：探针缺席 + strict = StrictReject（fail-closed 拒绝执行而非降级）。
#[test]
fn adjudicate_unavailable_strict_gives_strict_reject() {
    let kvm = probe(false, "/dev/kvm 不存在");
    let whpx = probe(false, "WinHvPlatform.dll 不可加载");
    let err = adjudicate_accel(true, None, &kvm, &whpx, false, GuestArch::X86_64).unwrap_err();
    assert!(matches!(&err, VmError::StrictReject(_)), "{err:?}");
}

/// 显式 hypervisor 且可用 = 直接采信。
#[test]
fn adjudicate_explicit_available_ok() {
    let kvm = probe(true, "");
    let whpx = probe(false, "x");
    for explicit in [QemuAccel::Kvm, QemuAccel::Whpx] {
        let whpx = if explicit == QemuAccel::Whpx {
            probe(true, "")
        } else {
            whpx.clone()
        };
        let got = adjudicate_accel(
            true,
            Some(explicit),
            &kvm,
            &whpx,
            cfg!(windows),
            GuestArch::X86_64,
        )
        .expect("explicit available");
        assert_eq!(got, explicit);
    }
}

/// 显式 hypervisor 缺席：strict 拒绝 / 非 strict 可回落。
#[test]
fn adjudicate_explicit_unavailable_two_modes() {
    let kvm = probe(false, "no kvm");
    let whpx = probe(false, "no whpx");
    let err = adjudicate_accel(
        false,
        Some(QemuAccel::Kvm),
        &kvm,
        &whpx,
        false,
        GuestArch::X86_64,
    )
    .unwrap_err();
    assert!(
        matches!(&err, VmError::Unavailable(m) if m.contains("no kvm")),
        "{err:?}"
    );
    let err = adjudicate_accel(
        true,
        Some(QemuAccel::Kvm),
        &kvm,
        &whpx,
        false,
        GuestArch::X86_64,
    )
    .unwrap_err();
    assert!(
        matches!(&err, VmError::StrictReject(m) if m.contains("no kvm")),
        "{err:?}"
    );
}

/// auto 按宿主 OS 选首选序（两侧探针都可用时：Windows→WHPX，Linux→KVM）。
#[test]
fn adjudicate_auto_prefers_host_os() {
    let ok = probe(true, "");
    for host_windows in [true, false] {
        let expected = if host_windows {
            QemuAccel::Whpx
        } else {
            QemuAccel::Kvm
        };
        let got = adjudicate_accel(false, None, &ok, &ok, host_windows, GuestArch::X86_64)
            .expect("both available");
        assert_eq!(got, expected, "host_windows={host_windows}");
    }
}

/// auto 回落序：首选缺席时落到次选（接口留给跨平台显式覆盖口）。
#[test]
fn adjudicate_auto_falls_to_secondary() {
    let kvm = probe(false, "no kvm");
    let whpx = probe(true, "");
    // Linux 宿主（KVM 优先）但只有 WHPX 可用 → 落 WHPX 而非拒绝。
    let got = adjudicate_accel(false, None, &kvm, &whpx, false, GuestArch::X86_64);
    assert!(matches!(got, Ok(QemuAccel::Whpx)), "{got:?}");
}

/// aarch64 预留档：无论探针/strict 如何，一律诚实 Unavailable「当前不产出」。
#[test]
fn adjudicate_aarch64_is_unavailable() {
    let ok = probe(true, "");
    let err = adjudicate_accel(true, None, &ok, &ok, false, GuestArch::AArch64).unwrap_err();
    assert!(
        matches!(&err, VmError::Unavailable(m) if m.contains("当前不产出")),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// token 生成
// ---------------------------------------------------------------------------

/// token 形态：32 hex；两次生成不同（时间/计数器熵）。
#[test]
fn token_shape_and_uniqueness() {
    let t1 = generate_token();
    let t2 = generate_token();
    assert_eq!(t1.len(), 32, "{t1}");
    assert!(t1.chars().all(|c| c.is_ascii_hexdigit()), "{t1}");
    assert_ne!(t1, t2, "同进程连续两次生成应不同");
}

// ---------------------------------------------------------------------------
// spawn 失败诚实面
// ---------------------------------------------------------------------------

/// 不存在的 qemu 二进制 = Launch 错误（不挂死、不吞错）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_missing_qemu_binary_is_launch_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let params = VmStartParams {
        id: "test-1".into(),
        run_dir: tmp.path().join("run"),
        qemu_bin: tmp.path().join("no-such-qemu-system-x86_64"),
        accel: QemuAccel::Kvm,
        arch: GuestArch::X86_64,
        vcpu: 1,
        ram_mb: 256,
        overlay_path: tmp.path().join("o.qcow2"),
        token: generate_token(),
        shared_host_path: None,
        loadvm_tag: None,
        incoming_file: None,
        display_vnc_port: None,
    };
    let err = VmInstance::start(params).await.unwrap_err();
    assert!(
        matches!(&err, VmError::Launch(m) if m.contains("spawn 失败")),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// wait_qmp_ready 收尸纪律（fake-qmp + 真子进程回归——2026-10-20 第五轮：
// 修前查询瞬断 `?` 直通放弃整轮、坏状态路径不 kill——泄漏的 QEMU 占
// overlay+端口，下轮 ensure_running 双开同一 overlay = qcow2 并发写损坏）
// ---------------------------------------------------------------------------

/// 长寿命替身进程（非 QEMU）：给 wait_qmp_ready 一个可收尸的 child。
fn spawn_sleeper() -> tokio::process::Child {
    let mut cmd = tokio::process::Command::new(if cfg!(target_os = "windows") {
        "ping"
    } else {
        "sleep"
    });
    if cfg!(target_os = "windows") {
        cmd.args(["-n", "30", "127.0.0.1"]);
    } else {
        cmd.arg("30");
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn sleeper")
}

/// 断言失败时也收走替身进程（start_kill 对已退出进程是 no-op）。
struct KillOnDrop(tokio::process::Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.start_kill();
    }
}

/// 坏状态路径（冷启 + paused 非过渡态）必须 kill+wait 后再报错。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_qmp_ready_bad_status_reaps_child() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior(
        "query-status",
        fake_qmp::Behavior::Return(serde_json::json!({
            "status": "paused", "singlestep": false, "running": false
        })),
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut g = KillOnDrop(spawn_sleeper());
    let err = VmInstance::wait_qmp_ready(
        &mut g.0,
        handle.addr(),
        &tmp.path().join("qemu.stderr.log"),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VmError::Launch(m) if m.contains("状态非 running") && m.contains("已终止")),
        "{err:?}"
    );
    // 收尸回归锚：修前此路径直接 return——子进程仍在跑。
    assert!(
        matches!(g.0.try_wait(), Ok(Some(_))),
        "坏状态路径必须收尸（子进程应已退出）"
    );
    let _ = g.0.wait().await; // 完全收账（幂等）
    handle.shutdown().await;
}

/// 查询瞬断（每轮断连）不放弃整轮：继续轮询到 deadline（修前首轮
/// `?` 直通回 Qmp 错误）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_qmp_ready_query_failure_polls_until_deadline() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior("query-status", fake_qmp::Behavior::Reset);
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut g = KillOnDrop(spawn_sleeper());
    let started = std::time::Instant::now();
    let err = VmInstance::wait_qmp_ready_with_budget(
        &mut g.0,
        handle.addr(),
        &tmp.path().join("qemu.stderr.log"),
        false,
        Duration::from_millis(900),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VmError::Launch(m) if m.contains("内未就绪")),
        "瞬断必须继续轮询到 deadline（而非首轮 QMP 错误放弃）: {err:?}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "确实轮询了一阵而非首轮放弃（实际 {:?}）",
        started.elapsed()
    );
    assert!(matches!(g.0.try_wait(), Ok(Some(_))), "deadline 路径收尸");
    let _ = g.0.wait().await;
    handle.shutdown().await;
}

/// 装配闸在 spawn **之前**：逗号路径 = 逗号错误而非 spawn 错误（这里
/// qemu_bin 也不存在——若闸序错了会先报「spawn 失败」）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_rejects_comma_path_before_spawn() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let params = VmStartParams {
        id: "test-1".into(),
        run_dir: tmp.path().join("run"),
        qemu_bin: tmp.path().join("no-such-qemu-system-x86_64"),
        accel: QemuAccel::Kvm,
        arch: GuestArch::X86_64,
        vcpu: 1,
        ram_mb: 256,
        overlay_path: tmp.path().join("o,r.qcow2"),
        token: generate_token(),
        shared_host_path: None,
        loadvm_tag: None,
        incoming_file: None,
        display_vnc_port: None,
    };
    let err = VmInstance::start(params).await.unwrap_err();
    assert!(
        matches!(&err, VmError::Launch(m) if m.contains("逗号") && m.contains("overlay_path")),
        "{err:?}"
    );
}

// ---------------------------------------------------------------------------
// 首帧鉴权 + 工具调用：进程内 guest 全链回归（真实 TCP，无 QEMU）
// ---------------------------------------------------------------------------

use nemesis_guest::GuestAgent;

/// 起一个带 token 的进程内 guest agent 于 127.0.0.1 临时端口，返回地址。
async fn spawn_guest(token: &str) -> (std::net::SocketAddr, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind ephemeral");
    let addr = listener.local_addr().expect("addr");
    let agent = GuestAgent::new(dir.path().to_path_buf()).with_token(token);
    tokio::spawn(async move {
        let _ = agent.serve_tcp(listener).await;
    });
    (addr, dir)
}

/// 正确 token：鉴权通过 + exec 回环（真实 TCP 全链）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guest_agent_auth_and_call_roundtrip() {
    let token = generate_token();
    let (addr, _dir) = spawn_guest(&token).await;
    let mut stream = auth_handshake(addr, &token, Duration::from_secs(5))
        .await
        .expect("auth handshake");
    let resp = guest_call(
        &mut stream,
        "exec",
        r#"{"command":"echo vm-loop"}"#,
        &serde_json::json!({"session_id": "t"}),
        Duration::from_secs(10),
    )
    .await
    .expect("guest call");
    assert!(resp.ok, "{resp:?}");
    assert!(resp.result.contains("vm-loop"), "{resp:?}");
    assert_eq!(resp.v, nemesis_exec_protocol::PROTOCOL_V2);
    assert!(!resp.call_id.is_empty());
}

/// 错误 token：握手被拒（诚实错误带 token mismatch）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guest_agent_wrong_token_refused() {
    let token = generate_token();
    let (addr, _dir) = spawn_guest(&token).await;
    let err = auth_handshake(addr, "wrong-token", Duration::from_secs(5))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VmError::Launch(m) if m.contains("token mismatch")),
        "{err:?}"
    );
}

/// 不可达地址：连接预算超时 = Timeout（不挂死）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_handshake_unreachable_times_out() {
    let started = std::time::Instant::now();
    let addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let err = auth_handshake(addr, "t", Duration::from_millis(500))
        .await
        .unwrap_err();
    assert!(matches!(err, VmError::Timeout | VmError::Io(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "不挂死");
}

/// alloc_loopback_port：返回非常量端口且可绑定。
#[test]
fn alloc_loopback_port_sane() {
    let p1 = alloc_loopback_port().expect("port");
    let p2 = alloc_loopback_port().expect("port");
    assert_ne!(p1, 0);
    assert_ne!(p2, 0);
}

// ---------------------------------------------------------------------------
// sleep→wake 状态机（fake-qmp 驱动——QMP 协议使用正确性 CI 锚；QEMU 真实
// savevm/migrate 行为归实机探针/冒烟，见实施计划附录早验记录）
// ---------------------------------------------------------------------------

/// 睡眠线默认选择随 accel（三探针早验定谳的编码化锚定）：KVM = 快照线；
/// WHPX = None（savevm/migrate 双双挂死，诚实不支持——退路是调用方显式
/// 选 stop 冷停，不在这层做假睡）。
#[test]
fn sleep_mode_default_follows_accel() {
    assert_eq!(
        SleepMode::default_for_accel(QemuAccel::Kvm),
        Some(SleepMode::Snapshot)
    );
    assert_eq!(SleepMode::default_for_accel(QemuAccel::Whpx), None);
}

/// 成功路径产物（形态锚定）：Snapshot 只填 tag；MigrateFile 只填状态路径。
#[test]
fn sleep_artifact_shape() {
    let snap = SleepArtifact {
        snapshot_tag: Some(SLEEP_SNAPSHOT_TAG.to_string()),
        state_path: None,
    };
    let mig = SleepArtifact {
        snapshot_tag: None,
        state_path: Some(std::path::PathBuf::from("/x/vm.state")),
    };
    assert!(snap.snapshot_tag.is_some() && snap.state_path.is_none());
    assert!(mig.snapshot_tag.is_none() && mig.state_path.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleep_snapshot_success_saves_and_quits() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let tmp = tempfile::tempdir().expect("tempdir");
    let artifact = sleep_via_qmp(
        handle.addr(),
        SleepMode::Snapshot,
        tmp.path(),
        Duration::from_secs(10),
    )
    .await
    .expect("snapshot sleep ok");
    assert_eq!(
        artifact,
        SleepArtifact {
            snapshot_tag: Some(SLEEP_SNAPSHOT_TAG.to_string()),
            state_path: None
        }
    );
    let cmds = handle.commands();
    // savevm 经 HMP 桥 + 成功后 quit（VM 收敛退出）。
    assert!(
        cmds.contains(&"human-monitor-command".to_string()),
        "{cmds:?}"
    );
    assert!(cmds.contains(&"quit".to_string()), "{cmds:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleep_snapshot_quit_teardown_reset_still_ok() {
    // quit 后 QEMU 停机拆除竞速（quit 回包被 RST/断连取代——2026-10-18 远程
    // KVM savevm 误报失败根因）：savevm 已成功、状态载体在手，sleep 必须
    // 仍 Ok。成功判据是 QEMU 退出（实例层 child.wait 核验），不是 quit
    // 回包到达。
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior("quit", fake_qmp::Behavior::Reset);
    let tmp = tempfile::tempdir().expect("tempdir");
    let artifact = sleep_via_qmp(
        handle.addr(),
        SleepMode::Snapshot,
        tmp.path(),
        Duration::from_secs(10),
    )
    .await
    .expect("savevm 已成功的 sleep 不因 quit 回包未达而失败");
    assert_eq!(
        artifact,
        SleepArtifact {
            snapshot_tag: Some(SLEEP_SNAPSHOT_TAG.to_string()),
            state_path: None
        }
    );
    let cmds = handle.commands();
    assert!(
        cmds.contains(&"human-monitor-command".to_string()),
        "{cmds:?}"
    );
    assert!(cmds.contains(&"quit".to_string()), "{cmds:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleep_snapshot_hmp_text_error_keeps_vm_running() {
    // HMP 失败形态（Error: 文本在 return 里）→ Err 且**不发 quit**——
    // 状态没保存成功就退出 = 丢状态，VM 保持运行由调用方裁决。
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior(
        "human-monitor-command",
        fake_qmp::Behavior::Return(serde_json::Value::String(
            "Error: Could not create VM snapshot\n".into(),
        )),
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let err = sleep_via_qmp(
        handle.addr(),
        SleepMode::Snapshot,
        tmp.path(),
        Duration::from_secs(10),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, VmError::Qmp(_)), "{err:?}");
    let cmds = handle.commands();
    assert!(
        cmds.contains(&"human-monitor-command".to_string()),
        "{cmds:?}"
    );
    assert!(
        !cmds.contains(&"quit".to_string()),
        "失败路径禁止 quit（VM 保持运行）: {cmds:?}"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleep_snapshot_hang_times_out_no_quit() {
    // savevm 挂死（WHPX 早验实锤形态的协议面）→ 预算内 Timeout + 无 quit。
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior("human-monitor-command", fake_qmp::Behavior::Hang);
    let tmp = tempfile::tempdir().expect("tempdir");
    let started = std::time::Instant::now();
    let err = sleep_via_qmp(
        handle.addr(),
        SleepMode::Snapshot,
        tmp.path(),
        Duration::from_millis(800),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, VmError::Qmp(super::super::qmp::QmpError::Timeout)),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "预算内收敛");
    let cmds = handle.commands();
    assert!(
        !cmds.contains(&"quit".to_string()),
        "挂死路径禁止 quit: {cmds:?}"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleep_migrate_success_roundtrip_artifact() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let tmp = tempfile::tempdir().expect("tempdir");
    let artifact = sleep_via_qmp(
        handle.addr(),
        SleepMode::MigrateFile,
        tmp.path(),
        Duration::from_secs(10),
    )
    .await
    .expect("migrate sleep ok");
    let expected = tmp.path().join("vm.state");
    assert_eq!(artifact.state_path.as_deref(), Some(expected.as_path()));
    assert!(artifact.snapshot_tag.is_none());
    let cmds = handle.commands();
    assert!(cmds.contains(&"migrate".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"query-migrate".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"quit".to_string()), "{cmds:?}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sleep_migrate_failed_status_no_quit() {
    // 迁移 failed 状态 → 诚实错误 + 无 quit（VM 保持运行）。
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior(
        "query-migrate",
        fake_qmp::Behavior::Return(serde_json::json!({"status": "failed"})),
    );
    let tmp = tempfile::tempdir().expect("tempdir");
    let err = sleep_via_qmp(
        handle.addr(),
        SleepMode::MigrateFile,
        tmp.path(),
        Duration::from_secs(10),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VmError::Launch(m) if m.contains("failed")),
        "{err:?}"
    );
    let cmds = handle.commands();
    assert!(
        !cmds.contains(&"quit".to_string()),
        "失败禁止 quit: {cmds:?}"
    );
    handle.shutdown().await;
}
