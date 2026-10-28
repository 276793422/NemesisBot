//! guest agent 入口（双栖形态，P5 第 1 步）。
//!
//! - 无参数：stdio 协议循环（宿主直跑——跨平台冒烟 / 集成测试用）；
//! - `--listen ADDR:PORT`：VM 内 TCP 服务（第 2 步 NAT-TCP 通道的 guest
//!   端），启动后向 stderr 打一行 `listening <local_addr>`（诊断走
//!   stderr——stdout 在 stdio 形态是协议通道，TCP 形态保持干净）。
//!
//! 工作区根：`NEMESISBOT_GUEST_WORKSPACE`（缺省 `/workspace`）。
//! 访问闸 token：`NEMESISBOT_GUEST_TOKEN`（缺省不设 = 鉴权闸关；VM 形态
//! 由 launcher 从 SMBIOS DMI serial 读出后注入——首帧 token 挑战鉴权）。
//! 日志：一律 stderr（协议字节不上日志）。

use nemesis_exec_protocol::{BoundedLine, read_line_bounded};
use nemesis_guest::GuestAgent;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let workspace = nemesis_guest::default_workspace();
    let agent_base = match std::env::var("NEMESISBOT_GUEST_TOKEN") {
        Ok(t) if !t.is_empty() => GuestAgent::new(workspace).with_token(t),
        _ => GuestAgent::new(workspace),
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build guest runtime");

    match args.first().map(|s| s.as_str()) {
        None => {
            runtime.block_on(run_stdio(agent_base));
        }
        Some("--listen") => {
            let addr = args
                .get(1)
                .expect("usage: nemesis-guest --listen ADDR:PORT");
            runtime.block_on(run_listen(agent_base, addr));
        }
        Some("--help") | Some("-h") => {
            eprintln!("usage: nemesis-guest [--listen ADDR:PORT]");
            eprintln!("  (no args)      stdio protocol loop (host dual-habitat / tests)");
            eprintln!("  --listen A:P   TCP server loop (guest side of NAT-TCP channel)");
            eprintln!("env: NEMESISBOT_GUEST_WORKSPACE (default /workspace)");
            eprintln!("env: NEMESISBOT_GUEST_TOKEN (first-frame auth; unset = gate off)");
        }
        Some(other) => {
            eprintln!("unknown argument: {other}");
            eprintln!("usage: nemesis-guest [--listen ADDR:PORT]");
            std::process::exit(2);
        }
    }
}

async fn run_stdio(agent: GuestAgent) {
    use tokio::io::AsyncWriteExt;
    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    loop {
        // 巨帧闸与 serve_stream 同源（read_line_bounded）：超限回一行
        // 诚实错误后收尾（帧失步，stdio 对端按 EOF 收到诚实失败）。
        let line = match read_line_bounded(&mut reader).await {
            Ok(BoundedLine::Line(l)) => l,
            Ok(BoundedLine::Eof) => return,
            Ok(BoundedLine::Overflow) => {
                let resp = nemesis_guest::frame_error_line(
                    "frame exceeds protocol MAX_FRAME_BYTES; line desynced, closing",
                );
                if stdout.write_all(resp.as_bytes()).await.is_err() {
                    return;
                }
                let _ = stdout.flush().await;
                return;
            }
            Err(_) => return,
        };
        if line.is_empty() {
            continue;
        }
        let resp = agent.serve_line(&line).await;
        if stdout.write_all(resp.as_bytes()).await.is_err() {
            return; // stdout closed → gateway gone, exit cleanly
        }
        let _ = stdout.flush().await;
    }
}
async fn run_listen(agent: GuestAgent, addr: &str) {
    // 鉴权闸未设 = TCP 形态全开（闸关不上的设计语义——stdio 裸跑不该因此
    // 不可用）。裸跑合理，但 TCP 服务静默无鉴权是配置事故（launcher 忘注
    // token），诚实 warn 一行诊断面，不拦启动。
    if std::env::var("NEMESISBOT_GUEST_TOKEN")
        .map(|t| t.is_empty())
        .unwrap_or(true)
    {
        eprintln!(
            "[guest] WARNING: --listen without NEMESISBOT_GUEST_TOKEN: TCP interface accepts unauthenticated tool calls"
        );
    }
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[guest] bind {addr} failed: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "listening {}",
        listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string())
    );
    if let Err(e) = agent.serve_tcp(listener).await {
        eprintln!("[guest] accept loop error: {e}");
        std::process::exit(1);
    }
}
