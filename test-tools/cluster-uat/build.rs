//! cluster-uat 构建脚本。
//!
//! Windows 主线程栈预留 16MB（默认 1MB；类 Unix 默认 8MB）：runner 是
//! `#[tokio::main]`，main 异步函数把全部套件（T1-T37 + T-XFER + T-MRG +
//! U 系 = 59 套，workspace 最大）`.await` 编进同一个巨型状态机，debug
//! 构建（无内联、全展开帧）对 1MB 贴线。integration-test 已于 2026-10-13
//! 实证溢出（0xc00000fd，`tool/list_dir` 套件尾部），与 nemesisbot
//! 2026-09-29 的 clap derive 巨帧溢出同一平台病理——同类预防性同修。
//! 16MB 仅是地址空间预留，commit 按需，release 无害。

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
