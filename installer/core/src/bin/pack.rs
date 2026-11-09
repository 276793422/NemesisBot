//! setup-core CLI：keygen / pack / inspect / sign 四子命令（计划 §7a
//! ⑥⑦ 步的工具化入口；构建脚本与人工验证共用）。

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use setup_core::pack;
use setup_core::pkgfile;

#[derive(Parser)]
#[command(
    name = "pack",
    about = "安装包打包/签名/检视工具（NBSETUP1 格式）",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 生成签名密钥体系（keys.json），打印根锚 hex。
    Keygen {
        /// 输出 keys.json 路径。
        #[arg(long)]
        out: PathBuf,
    },
    /// 打包发行目录为安装包。
    Pack {
        /// 发行产物目录（bin/bin_windows 或 bin/bin_linux）。
        #[arg(long)]
        payload: PathBuf,
        /// 骨架文件（未签名原始安装器 exe）。
        #[arg(long)]
        skeleton: PathBuf,
        /// 输出安装包路径。
        #[arg(long)]
        out: PathBuf,
        /// bot 版本号（manifest.bot_version）。
        #[arg(long)]
        version: String,
        /// 平台（windows | linux）。
        #[arg(long)]
        platform: String,
        /// 签名密钥体系（keys.json）；缺省 = 不签（黄色提示形态）。
        #[arg(long)]
        sign: Option<PathBuf>,
        /// zstd 压缩级别（1-22，缺省 16）。
        #[arg(long, default_value_t = 16)]
        level: i32,
    },
    /// 检视安装包（结构 + 信任分级；不释放）。
    Inspect {
        /// 安装包路径。
        file: PathBuf,
        /// JSON 输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 对成品安装包补签名（PE→证书表 / ELF/raw→v4 footer）。
    Sign {
        /// 安装包路径（原地原子写回）。
        file: PathBuf,
        /// 签名密钥体系（keys.json）。
        #[arg(long)]
        keys: PathBuf,
    },
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("[pack] 错误: {e:#}");
        let mut src = e.source();
        while let Some(s) = src {
            eprintln!("  ↳ {s}");
            src = s.source();
        }
        std::process::exit(1);
    }
}

fn real_main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Keygen { out } => {
            let anchor = pack::keygen(&out)?;
            println!("keys.json 已生成: {}", out.display());
            println!("root_anchor: {anchor}");
            println!("（编译期注入 NEMESIS_BUILD_ROOT_ANCHOR 或运行时 NEMESIS_ROOT_ANCHOR）");
        }
        Cmd::Pack {
            payload,
            skeleton,
            out,
            version,
            platform,
            sign,
            level,
        } => {
            let stats = pack::run(&pack::PackOptions {
                payload_dir: payload.clone(),
                skeleton,
                out: out.clone(),
                bot_version: version,
                platform,
                sign_keys: sign,
                zstd_level: level,
            })?;
            println!(
                "打包完成: {}（{} 文件，payload {} → 压后 {:.1}MB，总长 {:.1}MB，压缩率 {:.0}%）",
                out.display(),
                stats.files,
                fmt_mb(stats.uncompressed_total),
                fmt_mb(stats.payload_len),
                fmt_mb(stats.out_len),
                if stats.uncompressed_total == 0 {
                    100.0
                } else {
                    100.0 * stats.payload_len as f64 / stats.uncompressed_total as f64
                }
            );
            println!(
                "签名: {}",
                if stats.signed {
                    "已签（绿级信任链）"
                } else {
                    "未签（黄色提示形态）"
                }
            );
        }
        Cmd::Inspect { file, json } => {
            let info = pkgfile::inspect(&file)?;
            let m = &info.manifest;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "file": file.display().to_string(),
                        "file_len": info.file_len,
                        "format_version": m.format_version,
                        "bot_version": m.bot_version,
                        "platform": m.platform,
                        "created_at": m.created_at,
                        "files": m.files.len(),
                        "total_size": m.total_size,
                        "payload_len": m.payload_len,
                        "manifest_signed": info.manifest_signed,
                        "signature": { "grade": format!("{:?}", info.sig.grade), "text": info.sig.text },
                    })
                );
                return Ok(());
            }
            println!(
                "文件:       {}（{}）",
                file.display(),
                fmt_mb(info.file_len)
            );
            println!("格式:       NBSETUP1 v{}", m.format_version);
            println!("版本:       {}", m.bot_version);
            println!("平台:       {}", m.platform);
            println!(
                "文件数:     {}（解压后 {}）",
                m.files.len(),
                fmt_mb(m.total_size)
            );
            // 布局几何以尾注字段为准（外壳签名会把证书表/v4 footer 追加到
            // 尾注之后——manifest 末端 ≠ file_len - 56，印 EOF 推导值会把
            // 外壳签名段误并进 manifest）。
            let payload_end = m.payload_offset + m.payload_len;
            let manifest_end = info.footer.manifest_offset + info.footer.manifest_len;
            let footer_end = manifest_end + setup_core::format::FOOTER_SIZE as u64;
            let mut layout = format!(
                "骨架 [0,{po}) payload [{po},{pe}) manifest [{pe},{me})",
                po = m.payload_offset,
                pe = payload_end,
                me = manifest_end
            );
            if info.file_len > footer_end {
                layout.push_str(&format!(" 外壳签名 [{footer_end},{})", info.file_len));
            }
            println!("布局:       {layout}");
            println!(
                "manifest:   {}",
                if info.manifest_signed {
                    "已签名"
                } else {
                    "未签名"
                }
            );
            println!("信任分级:   {:?} — {}", info.sig.grade, info.sig.text);
        }
        Cmd::Sign { file, keys } => {
            pack::sign_file(&file, &keys)?;
            println!("签名完成: {}", file.display());
            println!("（注意：PE 形态已签过名再追签会被诚实拒绝——nemesis-verify 同款语义）");
        }
    }
    Ok(())
}

fn fmt_mb(bytes: u64) -> String {
    format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
}
