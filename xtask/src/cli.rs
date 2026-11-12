//! `cargo xtask assemble` 参数解析（手写——子命令只有一个，不引 clap 负担）。

use anyhow::{Result, anyhow, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    #[default]
    Release,
    IotSmall,
}

impl Profile {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "release" => Ok(Self::Release),
            "iotsmall" => Ok(Self::IotSmall),
            other => bail!("--profile 只认 release | iotsmall，收到 {other:?}"),
        }
    }

    /// cargo --profile 值 = target 输出子目录名（两者恒同形）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Release => "release",
            Self::IotSmall => "iotsmall",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AssembleOpts {
    /// 编译期上报端点覆盖（缺省 = ambient env > 实现内建默认）。
    pub endpoint: Option<String>,
    /// 编译期信封公钥覆盖（sec1 compressed 66 / uncompressed 130 hex）。
    pub pubkey: Option<String>,
    /// 编译期产品 token 覆盖（只进构建环境，永不回显）。
    pub token: Option<String>,
    pub profile: Profile,
    /// 装配前清空 assembly/（复制体 + 共享 target + dist 全清 → 全量重编）。
    pub clean: bool,
}

#[derive(Debug)]
pub enum Command {
    Assemble(AssembleOpts),
}

const USAGE: &str = "\
cargo xtask——商业装配流水线（太微子系统）

USAGE:
    cargo xtask assemble [OPTIONS]

OPTIONS:
    --endpoint <URL>    编译期上报端点覆盖（缺省 = env NEMESIS_TELEMETRY_ENDPOINT > 实现内建默认）
    --pubkey <HEX>      编译期信封公钥覆盖（sec1 compressed 66 / uncompressed 130 hex）
    --token <TOKEN>     编译期产品 token 覆盖（只进构建环境，不回显不落日志）
    --profile <P>       release（默认）| iotsmall
    --clean             装配前清空 assembly/（复制体+target+dist 全清，全量重编）
    -h, --help          本帮助

装配全程发生在 assembly/ 复制体里（git 忽略）：公开工作区与私有实现
（submodule 挂载点 slots/taiwei/impl）零写入。重复装配走递增同步 + 共享
target 增量编译。运行期开关（NEMESIS_TELEMETRY_KEY_ID 等）属运行时语义，
不经装配注入。";

pub fn parse(args: &[String]) -> Result<Command> {
    if args.is_empty() {
        bail!("缺少子命令。\n\n{USAGE}");
    }
    match args[0].as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        "assemble" => Ok(Command::Assemble(parse_assemble(&args[1..])?)),
        other => bail!("未知子命令 {other:?}（只支持 assemble）。\n\n{USAGE}"),
    }
}

fn parse_assemble(args: &[String]) -> Result<AssembleOpts> {
    let mut opts = AssembleOpts {
        profile: Profile::Release,
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "--endpoint" => opts.endpoint = Some(take_value(args, &mut i, a)?),
            "--pubkey" => opts.pubkey = Some(take_value(args, &mut i, a)?),
            "--token" => opts.token = Some(take_value(args, &mut i, a)?),
            "--profile" => opts.profile = Profile::parse(&take_value(args, &mut i, a)?)?,
            "--clean" => opts.clean = true,
            other => bail!("未知选项 {other:?}。\n\n{USAGE}"),
        }
        i += 1;
    }
    Ok(opts)
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| anyhow!("{flag} 缺值。\n\n{USAGE}"))
}

#[cfg(test)]
mod tests;
