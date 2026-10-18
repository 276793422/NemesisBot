//! VM 生命周期实机验证（P5 第 3 步 E 线 / G3 电池真腿，KVM 宿主）：在
//! `/dev/kvm` 在场的 Linux 宿主上跑完整「冷启 → marker → **真 savevm 睡眠**
//! → **真 loadvm 唤醒** → 冷重启持久重挂」闭环——CI 里只有 fake-qmp 状态机
//! 覆盖，本示例是唯一穿透真实 QEMU 睡眠线的验收载体。
//!
//! 双 marker 设计（区分「真唤醒」与「假唤醒=冷重启」的判据面）：
//! - **RAM marker**（`/dev/shm/p5-ram-marker.txt`，tmpfs）：只活在内存里。
//!   loadvm 恢复 RAM → 在场 = 真唤醒；任何冷重启 → tmpfs 清空 = 缺席。
//! - **磁盘 marker**（`/var/tmp/p5-vm-persist.txt`，磁盘持久、9p 工作区外）：
//!   overlay 重挂（wake 与冷重启都在）→ 在场 = 持久性成立。
//!
//! 环境不满足（QEMU 缺席 / 无 KVM / 镜像缺席）= **诚实 SKIP**（exit 0，
//! 打 SKIP 标记行——自动化脚本按标记区分「跑过全绿」与「没跑」）。
//!
//! ```text
//! cargo run -p nemesis-sandbox --features vm --example vm_lifecycle_kvm -- \
//!     --base /vm/images/base.qcow2 --seed /vm/images/seed.iso \
//!     [--run-dir DIR] [--qemu-dir DIR] [--sleep-budget-secs N]
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use nemesis_exec_protocol::ExecutorResponseWire;
use nemesis_sandbox::vm::assembly::resolve_qemu_binary;
use nemesis_sandbox::vm::guest_arch::{GuestArch, QemuAccel};
use nemesis_sandbox::vm::lifecycle::{
    SleepMode, VmInstance, VmStartParams, adjudicate_accel_real, auth_handshake, generate_token,
    guest_call,
};
use nemesis_sandbox::vm::overlay;

const RAM_MARKER: &str = "/dev/shm/p5-ram-marker.txt";
const DISK_MARKER: &str = "/var/tmp/p5-vm-persist.txt";

struct Args {
    base: Option<PathBuf>,
    seed: Option<PathBuf>,
    run_dir: PathBuf,
    qemu_dir: Option<PathBuf>,
    sleep_budget_secs: u64,
}

fn parse_args() -> Args {
    let mut a = Args {
        base: None,
        seed: None,
        run_dir: PathBuf::from("target/vm-lifecycle"),
        qemu_dir: None,
        sleep_budget_secs: 600,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut val = |name: &str| it.next().unwrap_or_else(|| panic!("{name} 缺值"));
        match k.as_str() {
            "--base" => a.base = Some(PathBuf::from(val("--base"))),
            "--seed" => a.seed = Some(PathBuf::from(val("--seed"))),
            "--run-dir" => a.run_dir = PathBuf::from(val("--run-dir")),
            "--qemu-dir" => a.qemu_dir = Some(PathBuf::from(val("--qemu-dir"))),
            "--sleep-budget-secs" => {
                a.sleep_budget_secs = val("--sleep-budget-secs").parse().expect("u64")
            }
            other => panic!("未知参数 {other}"),
        }
    }
    a
}

fn skip(reason: &str) -> ! {
    println!("VM_LIFECYCLE: SKIP — {reason}");
    std::process::exit(0);
}

fn step(n: u8, what: &str) {
    println!("[vm-lifecycle {n}/8] {what}");
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
    let seed = match args.seed.clone() {
        Some(s) if s.exists() => s,
        Some(s) => skip(&format!("seed ISO 不存在: {}", s.display())),
        None => skip("缺 --seed（scripts/build-vm-image.sh 产出的 cloud-init nocloud seed）"),
    };

    // [1] accel 裁决：本示例验证的是睡眠线真腿——当前只有 KVM 有
    //    （Snapshot；WHPX 双线挂死已定谳 = None）。非 KVM 宿主诚实 SKIP。
    step(1, "accel 探针裁决（KVM 专属——睡眠线真腿载体）");
    let arch = GuestArch::X86_64;
    let accel = match adjudicate_accel_real(false, None, arch) {
        Ok(a) => a,
        Err(e) => skip(&format!("无可用硬件加速（{e}）")),
    };
    if accel != QemuAccel::Kvm {
        skip(&format!(
            "本宿主 accel = {}——睡眠线真腿当前 KVM 专属（SleepMode::default_for_accel = {:?}）",
            accel.as_str(),
            SleepMode::default_for_accel(accel)
        ));
    }
    if !Path::new("/dev/kvm").exists() {
        skip("accel 裁决选了 kvm 但 /dev/kvm 缺席（真启动前诚实护栏）");
    }
    let Some(sleep_mode) = SleepMode::default_for_accel(accel) else {
        skip("KVM 宿主但 SleepMode::default_for_accel 回 None——睡眠线定义与探针失配，诚实拒绝");
    };
    let qemu_bin = resolve_qemu_binary(arch, args.qemu_dir.as_deref());
    match nemesis_sandbox::vm::probe::qemu_version_ok(&qemu_bin).await {
        Ok(v) => println!("    qemu = {v}"),
        Err(e) => skip(&format!("QEMU 不可用（{e}）")),
    }

    // [2] overlay 创建（base 作 backing；旧 overlay 清掉——本示例验证的是
    //    「一次装配 + 睡眠/唤醒/重启」，不是跨 run 复用）。
    step(2, "overlay 创建");
    let run_dir = std::fs::canonicalize(&args.run_dir).unwrap_or_else(|_| {
        std::fs::create_dir_all(&args.run_dir).expect("run_dir");
        std::fs::canonicalize(&args.run_dir).expect("run_dir canonicalize")
    });
    let overlay_path = run_dir.join("lifecycle-overlay.qcow2");
    let _ = std::fs::remove_file(&overlay_path);
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
    if let Err(e) = overlay::create_overlay(&qemu_img, &overlay_path, &base, 20).await {
        println!("VM_LIFECYCLE: FAIL — overlay 创建失败: {e}");
        std::process::exit(1);
    }
    println!("    overlay = {}", overlay_path.display());

    // 手头实例登记：任一步失败（Err 返回）由收尾统一 kill，不留孤儿烧宿主。
    let mut live: Option<VmInstance> = None;
    let token = generate_token();
    let id = format!("lifecycle-{}", chrono::Utc::now().timestamp_millis());
    let result = run_lifecycle(
        &args,
        &base,
        &seed,
        &run_dir,
        &overlay_path,
        &qemu_bin,
        &token,
        &id,
        sleep_mode,
        &mut live,
    )
    .await;
    if let (Err(reason), Some(mut vm)) = (&result, live.take()) {
        let _ = vm.kill().await;
        println!("VM_LIFECYCLE: FAIL — {reason}（残留实例已 kill 收敛）");
        std::process::exit(1);
    }
    println!("VM_LIFECYCLE: PASS（8/8 全绿）");
}

#[allow(clippy::too_many_arguments)]
async fn run_lifecycle(
    args: &Args,
    base: &Path,
    seed: &Path,
    run_dir: &Path,
    overlay_path: &Path,
    qemu_bin: &Path,
    token: &str,
    id: &str,
    sleep_mode: SleepMode,
    live: &mut Option<VmInstance>,
) -> Result<(), String> {
    let _ = base; // base 只在 overlay backing 里消费（参数面留档）。
    let sleep_budget = Duration::from_secs(args.sleep_budget_secs);
    let start_params =
        |id: String, seed_iso: Option<PathBuf>, loadvm: Option<String>| VmStartParams {
            id,
            run_dir: run_dir.to_path_buf(),
            qemu_bin: qemu_bin.to_path_buf(),
            accel: QemuAccel::Kvm,
            arch: GuestArch::X86_64,
            vcpu: 2,
            ram_mb: 2048,
            overlay_path: overlay_path.to_path_buf(),
            seed_iso,
            token: token.to_string(),
            shared_host_path: None,
            loadvm_tag: loadvm,
            incoming_file: None,
        };

    // [3] 冷启 + guest agent 就绪（首启含 cloud-init 装配，可能数分钟）。
    step(3, "冷启（QMP 就绪）+ guest agent 就绪");
    let vm = VmInstance::start(start_params(id.to_string(), Some(seed.to_path_buf()), None))
        .await
        .map_err(|e| format!("VM 冷启失败: {e}"))?;
    println!(
        "    pid={:?} qmp={} agent={} stderr={}",
        vm.pid(),
        vm.qmp_addr,
        vm.agent_addr,
        vm.stderr_log_path().display()
    );
    *live = Some(vm);
    let vm = live.as_mut().expect("live");
    let mut stream = wait_agent(vm, 420).await?;

    // [4] 双 marker 落位：RAM marker（tmpfs，唤醒判据）+ 磁盘 marker
    //    （持久判据）。当场回读。
    step(4, "双 marker 落位（RAM tmpfs + 磁盘持久）");
    let r = exec(
        &mut stream,
        &format!("echo RAM_OK > {RAM_MARKER} && cat {RAM_MARKER}"),
        Duration::from_secs(30),
    )
    .await?;
    if !r.result.contains("RAM_OK") {
        return Err(format!("RAM marker 回读异常: {r:?}"));
    }
    let r = exec(
        &mut stream,
        &format!("echo PERSIST_OK > {DISK_MARKER} && cat {DISK_MARKER}"),
        Duration::from_secs(30),
    )
    .await?;
    if !r.result.contains("PERSIST_OK") {
        return Err(format!("磁盘 marker 回读异常: {r:?}"));
    }

    // [5] 真睡眠（savevm → quit）：Ok = QEMU 已退出且快照在 qcow2 内。
    //    savevm 大内存需分钟级——预算 600s 起步。
    // P5_PRE_SLEEP_SECS：诊断辅助（远程 savevm 死因归因实验）——在落 marker
    // 与发 savevm 之间插入纯等待，分离「spawn+15s 定时器」与「savevm 触发」
    // 两个重叠锚点。缺省 0 = 行为不变。
    let pre_sleep: u64 = std::env::var("P5_PRE_SLEEP_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if pre_sleep > 0 {
        println!(
            "    [diag] P5_PRE_SLEEP_SECS={pre_sleep}：savevm 前纯等待（此窗口内死亡=定时器锚，savevm 后死亡=savevm 锚）"
        );
        tokio::time::sleep(Duration::from_secs(pre_sleep)).await;
        println!("    [diag] 等待结束，准备发 savevm（QEMU 仍活着 = 非定时器锚）");
    }
    step(5, "真 savevm 睡眠（Snapshot 线）");
    // fs 落盘闸（2026-10-18 远程取证定谳的冷启 0-byte 文件根因）：savevm 后
    // quit 对 guest 是断电——进程 kill 语义。ext4 默认 5s commit 间隔内未提交
    // 的写（inode 已提交、数据在 journal/页缓存）随 RAM 蒸发，冷启看到的正是
    // 「0 字节文件」：agent 二进制、launcher、systemd unit（systemd 把空 unit
    // 文件按 masked 处理）、网络配置全部空心化。savevm 前必须让 guest 把
    // 页缓存/journal 全部落盘（syncfs 语义），这是睡眠线的 fs 静默契约——
    // 编排层持有 agent 连接，sync 在这里做（lifecycle 的 sleep 是 QMP 层，
    // 不持有 agent；运行时接线处同契约，见实施计划 §9）。
    exec(&mut stream, "sync", Duration::from_secs(60))
        .await
        .map_err(|e| format!("savevm 前 guest sync 失败（fs 静默契约）: {e}"))?;
    println!("    guest sync 完成（页缓存/journal 已落盘）");
    let artifact = vm
        .sleep(sleep_mode, sleep_budget)
        .await
        .map_err(|e| format!("savevm 睡眠失败: {e}"))?;
    if artifact.snapshot_tag.is_none() {
        return Err(format!("睡眠产物缺 snapshot tag: {artifact:?}"));
    }
    println!(
        "    savevm 完成（tag={}），QEMU 已退出",
        artifact.snapshot_tag.as_deref().unwrap_or("?")
    );

    // [6] 真唤醒（loadvm 重挂）：RAM marker 在场 = RAM 状态真恢复（冷重启
    //    会清 tmpfs——这是「真唤醒 vs 假唤醒」的判据面）；磁盘 marker 同样
    //    在场（overlay 一致）。
    step(6, "真 loadvm 唤醒 + RAM marker 判据");
    let vm_wake = VmInstance::start(start_params(
        format!("{id}-wake"),
        None, // 快照恢复不吃 seed（cloud-init 早已装配完）
        artifact.snapshot_tag.clone(),
    ))
    .await
    .map_err(|e| format!("loadvm 重挂失败: {e}"))?;
    println!(
        "    wake pid={:?} qmp={} agent={}",
        vm_wake.pid(),
        vm_wake.qmp_addr,
        vm_wake.agent_addr
    );
    *live = Some(vm_wake);
    let vm_wake = live.as_mut().expect("live");
    let mut stream2 = wait_agent(vm_wake, 120).await?;
    let r = exec(
        &mut stream2,
        &format!("cat {RAM_MARKER}"),
        Duration::from_secs(30),
    )
    .await?;
    if !r.result.contains("RAM_OK") {
        return Err(format!(
            "RAM marker 缺席——loadvm 未恢复 RAM（假唤醒=冷重启？）: {r:?}"
        ));
    }
    let r = exec(
        &mut stream2,
        &format!("cat {DISK_MARKER}"),
        Duration::from_secs(30),
    )
    .await?;
    if !r.result.contains("PERSIST_OK") {
        return Err(format!(
            "磁盘 marker 缺席——唤醒后 overlay 状态不一致: {r:?}"
        ));
    }
    println!("    RAM marker 在场（真唤醒）+ 磁盘 marker 在场 OK");
    // 唤醒窗 fs 落盘闸（与 step 5 同一契约的另一半）：boot A 尾部未提交的
    // 写以 RAM 形态随 savevm/loadvm 走到唤醒窗（marker 读页缓存全通过——
    // 读不穿帮），但唤醒 QEMU 的 stop 同样是断电语义，不落盘就二次蒸发。
    // 唤醒窗本示例只读不写，要落盘的正是 boot A 遗留的那批。
    exec(&mut stream2, "sync", Duration::from_secs(60))
        .await
        .map_err(|e| format!("唤醒 stop 前 guest sync 失败（fs 静默契约）: {e}"))?;
    println!("    唤醒窗 guest sync 完成（boot A 遗留写已落盘）");
    vm_wake
        .stop(Duration::from_secs(15))
        .await
        .map_err(|e| format!("唤醒实例 stop 收敛失败: {e}"))?;

    // [7] 冷重启（同 overlay 重挂，无 loadvm）：磁盘 marker 仍在 = 持久
    //    重挂成立（G3 门 2 的 VM 内非共享子树路径）；RAM marker 缺席 =
    //    真冷启负面判据（tmpfs 随重启清空——证明不是意外 wake 形态串场）。
    step(7, "冷重启同 overlay 重挂（持久性）");
    let vm_cold = VmInstance::start(start_params(
        format!("{id}-cold"),
        Some(seed.to_path_buf()),
        None,
    ))
    .await
    .map_err(|e| format!("冷重启失败: {e}"))?;
    *live = Some(vm_cold);
    let vm_cold = live.as_mut().expect("live");
    let mut stream3 = wait_agent(vm_cold, 420).await?;
    let r = exec(
        &mut stream3,
        &format!("cat {DISK_MARKER}"),
        Duration::from_secs(30),
    )
    .await?;
    if !r.result.contains("PERSIST_OK") {
        return Err(format!(
            "磁盘 marker 缺席——overlay 冷重启持久性不成立: {r:?}"
        ));
    }
    match exec(
        &mut stream3,
        &format!("cat {RAM_MARKER}"),
        Duration::from_secs(30),
    )
    .await
    {
        // cat 失败（ok=false）= 缺席语义，正是想要的；ok=true 且含 RAM_OK
        // 才是坏事（tmpfs 没清 = 这不是冷启）。
        Ok(r) if r.result.contains("RAM_OK") => {
            return Err("RAM marker 在冷重启后仍在场——这不是冷启（tmpfs 未清？）".to_string());
        }
        _ => println!("    磁盘 marker 在场 + RAM marker 缺席（真冷启负面判据）OK"),
    }

    // [8] stop 收敛。
    step(8, "stop 收敛");
    vm_cold
        .stop(Duration::from_secs(15))
        .await
        .map_err(|e| format!("冷启实例 stop 收敛失败: {e}"))?;
    *live = None;
    Ok(())
}

/// guest exec 便捷封装（失败带响应现场成 Err，由调用方统一收敛）。
async fn exec(
    stream: &mut tokio::net::TcpStream,
    command: &str,
    timeout: Duration,
) -> Result<ExecutorResponseWire, String> {
    let ctx = serde_json::json!({"session_id": "vm-lifecycle"});
    let req = serde_json::json!({ "command": command }).to_string();
    match guest_call(stream, "exec", &req, &ctx, timeout).await {
        Ok(r) if r.ok => Ok(r),
        Ok(r) => Err(format!("exec 失败（{command}）: {r:?}")),
        Err(e) => Err(format!("exec 调用失败（{command}）: {e}")),
    }
}

/// guest agent 就绪等待：轮询首帧鉴权直至预算耗尽（现场带最后错误）。
async fn wait_agent(vm: &VmInstance, budget_secs: u64) -> Result<tokio::net::TcpStream, String> {
    let token = vm.token.clone();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(budget_secs);
    loop {
        match auth_handshake(vm.agent_addr, &token, Duration::from_secs(3)).await {
            Ok(s) => return Ok(s),
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(format!(
                        "guest agent {budget_secs}s 内未就绪（最后错误: {e}）——stderr: {}",
                        vm.stderr_log_path().display()
                    ));
                }
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    }
}
