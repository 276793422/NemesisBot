//! fake QMP 独立运行入口（手工调试用；CI 走库 API `spawn()`）。
//!
//! ```text
//! fake-qmp --bind 127.0.0.1:7879
//! ```
//!
//! 启动后向 stderr 打一行 `listening <addr>`，常驻到进程被杀。

fn main() {
    let bind = std::env::args()
        .nth(1)
        .and_then(|a| a.strip_prefix("--bind ").map(String::from))
        .or_else(|| std::env::args().nth(2))
        .unwrap_or_else(|| "127.0.0.1:7879".to_string());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build fake-qmp runtime");
    if let Err(e) = runtime.block_on(fake_qmp::serve_forever(&bind)) {
        eprintln!("[fake-qmp] serve failed: {e}");
        std::process::exit(1);
    }
}
