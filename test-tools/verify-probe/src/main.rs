//! verify-probe（诚实边界收口 ③a，2026-10-19）：delegate / iroh 实机验收的
//! 预检探针 + 验收驱动。
//!
//! 3a 产出物（计划：`docs/PLAN/2026-10-19_诚实边界收口三步计划.md` §3a）：
//! - 预检探针：`delegate-probe`（CLI 在场/版本/auth 信息面）+ `iroh-self`
//!   /`iroh-ping`（端点绑定/连通 RTT）+ `nic`（网卡/网段自检）。
//! - 验收驱动：`delegate-roundtrip`（真实 CLI 往返 + stop 树杀无孤儿）+
//!   `delegate-estop`（半程急停 → stop_all 排空 → 无孤儿）。
//!
//! 退出码契约（脚本/手册消费，机器可判）：
//! - `0` = PASS（该腿全过）
//! - `1` = FAIL（失败信息自带现场：原始输出尾/pid 残留/错误链）
//! - `3` = SKIP-READY（环境不满足：CLI 不在场 / feature 未编译——原因写明）
//!
//! 诚实边界：依赖外部工具（codex CLI 等）时「二进制在场 ≠ 可用」——
//! presence 只判在场，可用性由 roundtrip 真实往返判定；auth 探测是
//! 信息面（子命令不存在/输出不认 = 打印原文，不猜结论）。iroh 腿在同一
//! 网段运行时 relay 参与发现但数据面可能直连——NAT 穿越覆盖与否由手册
//! 按两机网段比对判定，探针只如实报告 direct_addrs。

mod delegate;
#[cfg(feature = "iroh")]
mod iroh_leg;

use clap::Parser;

#[derive(Parser)]
#[command(name = "verify-probe", about = "delegate/iroh 实机验收探针（③a）")]
enum Cmd {
    /// delegate 腿预检：CLI 在场 + 版本 + auth 信息面（不烧 token）。
    DelegateProbe {
        /// 探测的 CLI（codex / claude）。
        #[arg(long, default_value = "codex")]
        cli: String,
    },
    /// delegate 腿验收：真实 CLI 启动 → prompt 往返（断言标记回声）→ stop
    /// → 断言无孤儿进程。
    DelegateRoundtrip {
        #[arg(long, default_value = "codex")]
        cli: String,
        /// 工作目录（缺省 = 系统临时目录下 verify-probe-delegate/）。
        #[arg(long)]
        cwd: Option<String>,
        /// 往返等待上限（秒）。
        #[arg(long, default_value_t = 120)]
        wait_secs: u64,
        /// 回声标记（assistant 文本须包含）。
        #[arg(long, default_value = "PROBE_OK")]
        marker: String,
    },
    /// delegate 腿验收：空闲会话启动 → 半程急停 → 断言 stop_all 排空 +
    /// 无孤儿进程（不烧 token：不发 prompt，只验生命周期）。
    DelegateEstop {
        #[arg(long, default_value = "codex")]
        cli: String,
        #[arg(long)]
        cwd: Option<String>,
        /// 急停后收尸等待上限（秒）。
        #[arg(long, default_value_t = 20)]
        wait_secs: u64,
    },
    /// iroh 腿：绑定端点 + echo serve，打印 node_id/直连地址（对端 ping
    /// 用）。feature=iroh 编译。
    #[cfg(feature = "iroh")]
    IrohSelf {
        /// 身份密钥目录（缺省 = 系统临时目录下 verify-probe-iroh/）。
        #[arg(long)]
        workspace: Option<String>,
        /// serve 保持秒数（超时自动退出；0 = 一直等到 Ctrl+C）。
        #[arg(long, default_value_t = 600)]
        serve_secs: u64,
        /// Minimal 预设（无中继纯直连——loopback/受限网测试形态）。
        #[arg(long, default_value_t = false)]
        direct: bool,
    },
    /// iroh 腿：ping 对端（生产地址语法 `iroh:<hex>[@<ip:port>]`——
    /// `iroh-self` 输出里有现成命令形态），打印 RTT。
    #[cfg(feature = "iroh")]
    IrohPing {
        /// 对端地址（`iroh:<hex>` 走发现 / `iroh:<hex>@<ip:port>` 钉直连）。
        addr: String,
        /// 身份密钥目录（缺省 = 系统临时目录下 verify-probe-iroh/）。
        #[arg(long)]
        workspace: Option<String>,
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
        /// Minimal 预设（与钉直连地址配套；缺省走 N0 发现）。
        #[arg(long, default_value_t = false)]
        direct: bool,
    },
    /// 网卡/网段自检：默认路由出口 IP（UDP connect 技巧，零发包）+ 平台
    /// 原生接口清单 dump——两机比对判定是否真跨网。
    Nic,
}

#[tokio::main]
async fn main() {
    let cmd = Cmd::parse();
    let code = match cmd {
        Cmd::DelegateProbe { cli } => delegate::probe(&cli).await,
        Cmd::DelegateRoundtrip {
            cli,
            cwd,
            wait_secs,
            marker,
        } => delegate::roundtrip(&cli, cwd.as_deref(), wait_secs, &marker).await,
        Cmd::DelegateEstop {
            cli,
            cwd,
            wait_secs,
        } => delegate::estop(&cli, cwd.as_deref(), wait_secs).await,
        #[cfg(feature = "iroh")]
        Cmd::IrohSelf {
            workspace,
            serve_secs,
            direct,
        } => iroh_leg::self_probe(workspace.as_deref(), serve_secs, direct).await,
        #[cfg(feature = "iroh")]
        Cmd::IrohPing {
            addr,
            workspace,
            timeout_secs,
            direct,
        } => iroh_leg::ping(&addr, workspace.as_deref(), timeout_secs, direct).await,
        Cmd::Nic => nic(),
    };
    std::process::exit(code);
}

/// PASS / FAIL / SKIP 退出码（模块头契约）。
pub const EXIT_PASS: i32 = 0;
pub const EXIT_FAIL: i32 = 1;
pub const EXIT_SKIP: i32 = 3;

/// 默认路由出口 IP（UDP connect 不发包：内核选路后 local_addr 即出口
/// 接口地址；8.8.8.8:53 只是选路锚点，无数据到达对端）。
fn default_route_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:53").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

fn nic() -> i32 {
    println!("== 默认路由出口 ==");
    match default_route_ip() {
        Some(ip) => println!("outbound_ip: {ip}"),
        None => println!("outbound_ip: <不可判定>"),
    }
    println!("== 接口清单（平台原生） ==");
    let (prog, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("ipconfig", vec!["/all"])
    } else {
        ("ip", vec!["-4", "addr"])
    };
    match std::process::Command::new(prog).args(&args).output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            // Windows ipconfig 输出是 OEM 代码页——中文环境乱码不影响网段
            // 判读（IPv4 行是 ASCII）；原样打印。
            print!("{text}");
        }
        Err(e) => {
            println!("<{prog} 执行失败: {e}>");
            return EXIT_FAIL;
        }
    }
    EXIT_PASS
}

/// pid 存活探针（树杀验收共用）：Windows tasklist 按 PID 过滤 / Unix
/// `kill -0`。返回 None = 探针本身失败（按残留处理并打印原因）。
pub(crate) fn pid_alive(pid: u32) -> Option<bool> {
    if cfg!(windows) {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // 无匹配时 tasklist 打印 "INFO: ..."（exit code 仍 0）——按文本判。
        Some(text.contains(&format!("\"{pid}\"")))
    } else {
        // kill -0：0 = 在，非 0（EPERM 除外）= 不在。EPERM（他人进程）极
        // 罕见——本探针只查自己启动的子进程树。
        Some(
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(true),
        )
    }
}

/// 轮询 pid 消失（树杀收尸验收）：超时仍存活 = FAIL 现场（打印残留 pid）。
pub(crate) async fn wait_pid_gone(pid: u32, wait_secs: u64) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(wait_secs);
    loop {
        match pid_alive(pid) {
            Some(false) => return Ok(()),
            Some(true) => {}
            None => {
                return Err(format!(
                    "进程存活探针失败（tasklist/kill 不可用），pid {pid} 状态未知"
                ));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "孤儿进程：pid {pid} 在收尸等待 {wait_secs}s 后仍存活（tasklist/ps 可见）"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}
