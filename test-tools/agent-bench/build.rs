//! agent-bench 构建脚本。
//!
//! Windows 主线程栈预留 16MB（默认 1MB；类 Unix 默认 8MB）：runner 是
//! `#[tokio::main]`，main 异步函数把全部场景 `.await` 编进同一个状态机，
//! debug 构建对 1MB 贴线。integration-test 已于 2026-10-13 实证溢出
//!（0xc00000fd），与 nemesisbot 2026-09-29 的 clap derive 巨帧溢出同一
//! 平台病理——同类预防性同修。16MB 仅是地址空间预留，commit 按需，
//! release 无害。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let stack_arg = match std::env::var("CARGO_CFG_TARGET_ENV")
            .unwrap_or_default()
            .as_str()
        {
            "gnu" => "-Wl,--stack,16777216",
            _ => "/STACK:16777216", // msvc（默认工具链）
        };
        println!("cargo:rustc-link-arg-bins={stack_arg}");
    }
}
