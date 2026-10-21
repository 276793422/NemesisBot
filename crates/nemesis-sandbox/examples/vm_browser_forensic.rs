//! VM browser 取证示例（诊断用，不进常规测试面）：进 guest 复现 chromium
//! spawn 现场，拿 stderr / 内存 / OOM 证据。VM-BROWSER-1 三跑仍
//! 「CDP 9222 not ready」后的直接取证载体。
//!
//! ```text
//! cargo run -p nemesis-sandbox --features vm --example vm_browser_forensic -- \
//!     --base target/vm-image/base.qcow2
//! ```

use std::path::PathBuf;
use std::time::Duration;

use nemesis_sandbox::vm::assembly::resolve_qemu_binary;
use nemesis_sandbox::vm::guest_arch::GuestArch;
use nemesis_sandbox::vm::lifecycle::{
    VmInstance, VmStartParams, adjudicate_accel_real, auth_handshake, generate_token, guest_call,
};
use nemesis_sandbox::vm::overlay;

struct Args {
    base: Option<PathBuf>,
    /// 复用既有 overlay（UAT 遗留实例现场）——给了就不再新建 overlay，
    /// 直接用这张盘起 VM（cloud-init 不重播，agent 快速就绪）。
    overlay: Option<PathBuf>,
    run_dir: PathBuf,
    qemu_dir: Option<PathBuf>,
    deadline_secs: u64,
}

fn parse_args() -> Args {
    let mut a = Args {
        base: None,
        overlay: None,
        run_dir: PathBuf::from("target/vm-forensic"),
        qemu_dir: None,
        deadline_secs: 420,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = |name: &str| it.next().unwrap_or_else(|| panic!("{name} 缺值"));
        match k.as_str() {
            "--base" => a.base = Some(PathBuf::from(val("--base"))),
            "--overlay" => a.overlay = Some(PathBuf::from(val("--overlay"))),
            "--run-dir" => a.run_dir = PathBuf::from(val("--run-dir")),
            "--qemu-dir" => a.qemu_dir = Some(PathBuf::from(val("--qemu-dir"))),
            "--deadline-secs" => a.deadline_secs = val("--deadline-secs").parse().expect("u64"),
            other => panic!("未知参数 {other}"),
        }
    }
    a
}

fn skip(reason: &str) -> ! {
    println!("VM_FORENSIC: SKIP — {reason}");
    std::process::exit(0);
}

async fn probe(stream: &mut tokio::net::TcpStream, label: &str, cmd: &str) {
    // 300s call 预算：collect 步内含 sleep 150 + 观察哨 180s 窗，120s 会误杀。
    println!("\n=== {label} ===");
    println!("    $ {cmd}");
    let ctx = serde_json::json!({"session_id": "vm-forensic"});
    match guest_call(
        stream,
        "exec",
        &serde_json::json!({"command": cmd}).to_string(),
        &ctx,
        Duration::from_secs(300),
    )
    .await
    {
        Ok(resp) => {
            if resp.ok {
                println!("--- stdout ---\n{}", resp.result);
            } else {
                println!("--- ERROR ---\n{}", resp.error);
            }
        }
        Err(e) => println!("--- CALL_ERR ---\n{e}"),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args = parse_args();
    let overlay_path: PathBuf = match &args.overlay {
        Some(p) => {
            if !p.exists() {
                skip(&format!("overlay 不存在: {}", p.display()));
            }
            std::fs::canonicalize(p).expect("overlay canonicalize")
        }
        None => {
            let Some(base) = args.base.clone() else {
                skip("缺 --base（或 --overlay 复用既有实例盘）");
            };
            if !base.exists() {
                skip(&format!("base 镜像不存在: {}", base.display()));
            }
            let run_dir0 = std::fs::canonicalize(&args.run_dir).unwrap_or_else(|_| {
                std::fs::create_dir_all(&args.run_dir).expect("run_dir");
                std::fs::canonicalize(&args.run_dir).expect("run_dir canonicalize")
            });
            let op = run_dir0.join("forensic-overlay.qcow2");
            let _ = std::fs::remove_file(&op);
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
            overlay::create_overlay(&qemu_img, &op, &base, 20)
                .await
                .expect("overlay create");
            op
        }
    };
    println!("    overlay = {}", overlay_path.display());

    let arch = GuestArch::X86_64;
    let accel = match adjudicate_accel_real(false, None, arch) {
        Ok(a) => {
            println!("    accel = {}", a.as_str());
            a
        }
        Err(e) => skip(&format!("无可用硬件加速（{e}）")),
    };
    let qemu_bin = resolve_qemu_binary(arch, args.qemu_dir.as_deref());
    let run_dir = std::fs::canonicalize(&args.run_dir).unwrap_or_else(|_| {
        std::fs::create_dir_all(&args.run_dir).expect("run_dir");
        std::fs::canonicalize(&args.run_dir).expect("run_dir canonicalize")
    });

    let token = generate_token();
    let mut vm = VmInstance::start(VmStartParams {
        id: format!("forensic-{}", chrono::Utc::now().timestamp_millis()),
        run_dir: run_dir.clone(),
        qemu_bin: qemu_bin.clone(),
        accel,
        arch,
        vcpu: 2,
        ram_mb: 2048,
        overlay_path: overlay_path.clone(),
        token: token.clone(),
        shared_host_path: None,
        loadvm_tag: None,
        incoming_file: None,
        display_vnc_port: None,
    })
    .await
    .expect("VM start");
    println!("    pid={:?} agent={}", vm.pid(), vm.agent_addr);

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

    // [1] 首启态快照：agent 刚就绪时的系统状态。
    probe(
        &mut stream,
        "agent 刚就绪：cloud-init 状态 / 内存 / 磁盘",
        "cloud-init status; free -m; df -h /tmp / | tail -2; uptime",
    )
    .await;

    // [2] 装观察哨：每 3s 一拍（时间戳/chromium 进程数/9222 监听/curl 探测/
    // stderr 尾行）追加进 /tmp/watch.log，共 60 拍（180s），后台游离。
    let watcher = "\
        nohup sh -c 'for i in $(seq 1 60); do \
          echo \"--- tick $i $(date +%T)\"; \
          ps aux | grep -c \"[c]hromium\"; \
          ss -tln 2>/dev/null | grep 9222 || echo NO9222; \
          curl -s -m 2 -o /dev/null -w \"HTTP:%{http_code}\" http://127.0.0.1:9222/json/version || echo \" CURLFAIL\"; \
          echo; tail -2 /tmp/chr.log 2>/dev/null; \
          sleep 3; done' >/tmp/watch.log 2>&1 & echo WATCHER-UP";
    probe(&mut stream, "装观察哨（180s 窗）", watcher).await;

    // [3] **走 agent 真实 browser 模块**（ensure_chromium 路径——UAT 失败面），
    // 不再 exec 手工 spawn。guest 侧预算 120s（SPAWN_READY_SECS），host 侧
    // 300s 容纳往返。成败都继续收现场。
    println!("\n=== agent browser 模块直调（navigate about:blank）===");
    let ctx = serde_json::json!({"session_id": "vm-forensic", "source": "browser"});
    let browser_outcome = guest_call(
        &mut stream,
        "browser",
        r#"{"action":"navigate","url":"about:blank"}"#,
        &ctx,
        Duration::from_secs(300),
    )
    .await;
    match &browser_outcome {
        Ok(resp) if resp.ok => println!("--- OK ---\n{}", resp.result),
        Ok(resp) => println!("--- ERROR ---\n{}", resp.error),
        Err(e) => println!("--- CALL_ERR ---\n{e}"),
    }

    // [4] 现场捕捉（同 boot、browser 调用刚返回）：进程表全貌 + 9222 + agent
    // spawned chromium 的 stderr（/tmp/nemesis-chromium.log）+ profile 锁。
    probe(
        &mut stream,
        "现场：进程全貌 / 监听 / SingletonLock",
        "ps aux | grep -v grep | grep chromium | head -5; \
         ss -tlnp 2>/dev/null | grep 9222 || echo NO-LISTENER; \
         ls -la /workspace/.browser-profile/Singleton* 2>/dev/null; \
         readlink /workspace/.browser-profile/SingletonLock 2>/dev/null || echo NO-SINGLETON-LOCK",
    )
    .await;
    probe(
        &mut stream,
        "agent-spawned chromium stderr（/tmp/nemesis-chromium.log）",
        "ls -la /tmp/nemesis-chromium.log 2>/dev/null && tail -30 /tmp/nemesis-chromium.log || echo LOG-ABSENT",
    )
    .await;
    probe(
        &mut stream,
        "chromium 进程内核态（wchan/状态）+ 运行环境差异面",
        "for p in $(pgrep -f 'remote-debugging-port' 2>/dev/null | head -2); do \
           echo \"pid=$p state=$(awk '{print $3}' /proc/$p/stat 2>/dev/null) wchan=$(cat /proc/$p/wchan 2>/dev/null)\"; \
           echo \"cmdline: $(tr '\\0' ' ' </proc/$p/cmdline 2>/dev/null | head -c 300)\"; \
         done; \
         echo '--- env of first chromium (agent-inherited) ---'; \
         tr '\\0' '\\n' </proc/$(pgrep -f 'remote-debugging-port' | head -1)/environ 2>/dev/null | grep -E 'HOME|PATH|XDG|DBUS|DISPLAY' ",
    )
    .await;
    probe(&mut stream, "观察哨全程", "cat /tmp/watch.log").await;

    let _ = vm.kill().await;
    println!("\nVM_FORENSIC: DONE");
}
