//! xtask——商业装配流水线（太微子系统）。
//!
//! 用法：`cargo xtask assemble [--endpoint URL] [--pubkey HEX] [--token TOKEN]
//! [--profile release|iotsmall] [--clean]`。
//!
//! 设计不变量：**公开工作区与私有实现仓（submodule 挂载点）零写入**——复制、
//! 换指、构建全部发生在 `assembly/` 复制体里（git 忽略）；复制体递增同步
//! （mtime 保持）+ 共享 `assembly/target`，重复装配走增量编译。装配前后
//! `git status` 恒干净。

mod assemble;
mod cli;
mod copy_tree;
mod transform;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args)? {
        cli::Command::Assemble(opts) => assemble::run(&opts),
    }
}
