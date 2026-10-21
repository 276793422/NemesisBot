//! VM 冒烟示例（P5 第 2 步真机验证载体）：KVM/WHPX 宿主上跑最小真实闭环
//! ——probe → overlay → start → guest agent 首帧鉴权 → exec 回环 → 隔离
//! 断言 → kill 探针 → stop 收敛。
//!
//! 环境不满足（QEMU 缺席 / 无硬件加速）= **诚实 SKIP**（exit 0，打 SKIP
//! 标记行——自动化脚本按标记区分「跑过全绿」与「没跑」）。
//!
//! 前置：`--base` 指向已制备的 base 镜像（`scripts/build-vm-image.sh` 产
//! 出；guest 二进制经 cloud-init 已进镜像）。
//!
//! ```text
//! cargo run -p nemesis-sandbox --features vm --example vm_smoke -- \
//!     --base /vm/images/base.qcow2 \
//!     [--run-dir DIR] [--qemu-dir DIR] [--shared DIR] [--deadline-secs N]
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use nemesis_sandbox::vm::assembly::resolve_qemu_binary;
use nemesis_sandbox::vm::guest_arch::{GuestArch, QemuAccel};
use nemesis_sandbox::vm::lifecycle::{
    VmInstance, VmStartParams, adjudicate_accel_real, auth_handshake, generate_token, guest_call,
};
use nemesis_sandbox::vm::overlay;

struct Args {
    base: Option<PathBuf>,
    run_dir: PathBuf,
    qemu_dir: Option<PathBuf>,
    shared: Option<PathBuf>,
    deadline_secs: u64,
}

fn parse_args() -> Args {
    let mut a = Args {
        base: None,
        run_dir: PathBuf::from("target/vm-smoke"),
        qemu_dir: None,
        shared: None,
        deadline_secs: 300,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = |name: &str| it.next().unwrap_or_else(|| panic!("{name} 缺值"));
        match k.as_str() {
            "--base" => a.base = Some(PathBuf::from(val("--base"))),
            "--run-dir" => a.run_dir = PathBuf::from(val("--run-dir")),
            "--qemu-dir" => a.qemu_dir = Some(PathBuf::from(val("--qemu-dir"))),
            "--shared" => a.shared = Some(PathBuf::from(val("--shared"))),
            "--deadline-secs" => a.deadline_secs = val("--deadline-secs").parse().expect("u64"),
            other => panic!("未知参数 {other}"),
        }
    }
    a
}

fn skip(reason: &str) -> ! {
    println!("VM_SMOKE: SKIP — {reason}");
    std::process::exit(0);
}

fn step(n: u8, what: &str) {
    println!("[vm-smoke {n}/8] {what}");
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args = parse_args();
    let Some(base) = args.base.clone() else {
        skip("缺 --base（先用 scripts/build-vm-image.sh 制备镜像）");
    };
    if !base.exists() {
        skip(&format!("base 镜像不存在: {}", base.display()));
    }

    // [1] accel 裁决（探针缺席 = 诚实 SKIP——本机无硬件虚拟化不硬跑）。
    step(1, "accel 探针裁决");
    let arch = GuestArch::X86_64;
    let accel = match adjudicate_accel_real(false, None, arch) {
        Ok(a) => {
            println!("    accel = {}", a.as_str());
            a
        }
        Err(e) => skip(&format!("无可用硬件加速（{e}）")),
    };
    let qemu_bin = resolve_qemu_binary(arch, args.qemu_dir.as_deref());
    match nemesis_sandbox::vm::probe::qemu_version_ok(&qemu_bin).await {
        Ok(v) => println!("    qemu = {v}"),
        Err(e) => skip(&format!("QEMU 不可用（{e}）")),
    }
    if accel == QemuAccel::Kvm && !Path::new("/dev/kvm").exists() {
        skip("accel 裁决选了 kvm 但 /dev/kvm 缺席（真启动前诚实护栏）");
    }

    // [2] overlay 创建（base 作 backing）。
    step(2, "overlay 创建");
    let run_dir = std::fs::canonicalize(&args.run_dir).unwrap_or_else(|_| {
        std::fs::create_dir_all(&args.run_dir).expect("run_dir");
        std::fs::canonicalize(&args.run_dir).expect("run_dir canonicalize")
    });
    let overlay_path = run_dir.join("smoke-overlay.qcow2");
    let _ = std::fs::remove_file(&overlay_path);
    // qemu-img 与 qemu-system 同目录（--qemu-dir 给了就一起用；缺省走 PATH）。
    // Windows 产物带 .exe 后缀——解析形态与 assembly::resolve_qemu_binary 同源。
    let qemu_img = match &args.qemu_dir {
        Some(dir) => {
            #[cfg(target_os = "windows")]
            let p = dir.join("qemu-img.exe");
            #[cfg(not(target_os = "windows"))]
            let p = dir.join("qemu-img");
            if p.exists() {
                p
            } else {
                PathBuf::from("qemu-img")
            }
        }
        None => PathBuf::from("qemu-img"),
    };
    overlay::create_overlay(&qemu_img, &overlay_path, &base, 20)
        .await
        .expect("overlay create");
    println!("    overlay = {}", overlay_path.display());

    // [3] VM 启动（QMP 就绪裁决）。
    step(3, "VM 启动（QMP 就绪）");
    let token = generate_token();
    let shared = args
        .shared
        .as_ref()
        .map(|dir| std::fs::canonicalize(dir).expect("shared dir canonicalize"));
    let mut vm = VmInstance::start(VmStartParams {
        id: format!("smoke-{}", chrono::Utc::now().timestamp_millis()),
        run_dir: run_dir.clone(),
        qemu_bin: qemu_bin.clone(),
        accel,
        arch,
        vcpu: 2,
        ram_mb: 2048,
        overlay_path: overlay_path.clone(),
        token: token.clone(),
        shared_host_path: shared,
        loadvm_tag: None,
        incoming_file: None,
        display_vnc_port: None,
    })
    .await
    .expect("VM start");
    println!(
        "    pid={:?} qmp={} agent={} stderr={}",
        vm.pid(),
        vm.qmp_addr,
        vm.agent_addr,
        vm.stderr_log_path().display()
    );

    // [4] guest agent 就绪等待（首启含 cloud-init 装配，可能数分钟）。
    step(4, "guest agent 就绪等待（首帧鉴权）");
    let deadline_secs = args.deadline_secs;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(deadline_secs);
    let mut stream = loop {
        match auth_handshake(vm.agent_addr, &token, Duration::from_secs(3)).await {
            Ok(s) => break s,
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    let _ = vm.kill().await;
                    panic!("guest agent {deadline_secs}s 内未就绪（最后错误: {e}）");
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    };
    println!("    鉴权通过");

    // [5] exec 回环。
    step(5, "exec 回环");
    let ctx = serde_json::json!({"session_id": "vm-smoke"});
    let resp = guest_call(
        &mut stream,
        "exec",
        r#"{"command":"echo p5-vm-loop"}"#,
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .expect("exec call");
    assert!(resp.ok, "exec 回环失败: {resp:?}");
    assert!(resp.result.contains("p5-vm-loop"), "{resp:?}");
    println!("    echo 回环 OK（call_id={}）", resp.call_id);

    // [6] 隔离断言（两层）：越界写被 guest 边界拒；VM 内写不落宿主盘。
    step(6, "隔离断言");
    let resp = guest_call(
        &mut stream,
        "write_file",
        r#"{"path":"/etc/p5-escape-probe.txt","content":"x"}"#,
        &ctx,
        Duration::from_secs(15),
    )
    .await
    .expect("boundary call");
    assert!(
        !resp.ok && resp.error.contains("outside the workspace"),
        "越界写应被拒: {resp:?}"
    );
    println!("    越界写被 guest 工作区边界拒绝 OK");

    let probe_id = format!("p5vmsmoke-{}", std::process::id());
    let cmd = format!(
        "mkdir -p /tmp/{probe_id} && echo vm-data > /tmp/{probe_id}/probe.txt && cat /tmp/{probe_id}/probe.txt"
    );
    let resp = guest_call(
        &mut stream,
        "exec",
        &serde_json::json!({"command": cmd}).to_string(),
        &ctx,
        Duration::from_secs(30),
    )
    .await
    .expect("isolation exec");
    assert!(resp.ok && resp.result.contains("vm-data"), "{resp:?}");
    #[cfg(target_os = "linux")]
    {
        let host_side = PathBuf::from(format!("/tmp/{probe_id}/probe.txt"));
        assert!(
            !host_side.exists(),
            "隔离破坏：guest 写穿透到了宿主盘 {}",
            host_side.display()
        );
        println!("    VM 内写未落宿主盘（{} 不存在）OK", host_side.display());
    }
    #[cfg(not(target_os = "linux"))]
    {
        println!("    宿主侧落盘核验仅 Linux 宿主做（本宿主跳过——guest 内 /tmp 映射不同）");
    }

    // [7] kill 探针：VM 意外死亡的诚实错误面（不挂死）。
    step(7, "kill 探针（VM 死亡诚实面）");
    vm.kill().await.expect("kill");
    let status_after = vm.status(Duration::from_secs(2)).await;
    assert!(
        status_after.is_err(),
        "VM 已死还报状态 = 不诚实: {status_after:?}"
    );
    println!(
        "    kill 后 status 诚实报错 OK（{:?}）",
        status_after.unwrap_err()
    );

    // [8] stop 收敛（kill 后再 stop 应幂等收敛）。
    step(8, "stop 收敛");
    vm.stop(Duration::from_secs(5)).await.expect("stop");
    println!("VM_SMOKE: PASS（8/8 全绿）");
}
