//! 系统集成 CLI（`nemesisbot system register/unregister/status/cert`）。
//!
//! 分工契约（2026-11-10 用户拍板「安装器搬文件、bot 做集成」）：
//! - 安装器只负责文件落位（staging/原子提交/卸载副本/自删），系统集成
//!   **零第二实现**——安装器 spawn 本命令组（失败=警告不回滚）；
//! - 本模块是 [`nemesis_sysint`] facade 的薄 CLI 壳：解析旗标 → 构造
//!   RegisterRequest（version 取 bot 自身 VERSION_INFO——二进制是自己的
//!   真相源）→ 打印人读报告；
//! - 轻路径：无 gateway/无通道装配，毫秒级返回；
//! - 证书组（cert install/uninstall/status）独立于 register 流程，只有
//!   显式调用才落库（用户拍板「默认签名不安装」）。Windows 装 HKCU Root
//!   会触发系统确认框 = 设计内知情同意 UI。

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;

use crate::common;
use nemesis_sysint::{RegisterRequest, certdata};

#[derive(Debug, Clone, Subcommand)]
pub enum SystemCommand {
    /// 注册系统集成（ARP 卸载面板 / 开机自启 / 快捷方式；逐项 best-effort）
    Register {
        /// 安装目录（含 nemesisbot 可执行文件与卸载副本）
        #[arg(long)]
        dir: PathBuf,
        /// 同时配置开机自启（Windows=HKCU Run；Linux=systemd --user unit）
        #[arg(long, default_value_t = false)]
        autostart: bool,
        /// 跳过快捷方式（Windows=开始菜单 .lnk；Linux=symlink+.desktop+PATH）
        #[arg(long, default_value_t = false)]
        no_shortcuts: bool,
        /// Windows 同时创建桌面快捷方式
        #[arg(long, default_value_t = false)]
        desktop: bool,
    },
    /// 注销系统集成（幂等：条目缺席也回成功；Linux PATH 哨兵按设计保留）
    Unregister {
        /// 安装目录
        #[arg(long)]
        dir: PathBuf,
    },
    /// 查看系统集成状态（ARP / 自启 / 快捷方式各条目在场情况）
    Status {
        /// 安装目录（仅用于展示指向）
        #[arg(long)]
        dir: PathBuf,
    },
    /// 信任根证书库管理（独立操作，默认永不安装）
    Cert {
        #[command(subcommand)]
        action: CertAction,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum CertAction {
    /// 安装根证书到系统信任库（Windows=HKCU Root，触发系统确认框；
    /// Linux=ca-certificates，需 root）
    Install {
        /// 证书文件（PEM 或 DER）
        cert: PathBuf,
    },
    /// 从系统信任库摘除根证书（按指纹匹配）
    Uninstall {
        /// 证书文件（PEM 或 DER；按同一指纹对账）
        cert: PathBuf,
    },
    /// 查询证书是否在系统信任库
    Status {
        /// 证书文件（PEM 或 DER）
        cert: PathBuf,
    },
}

/// 入口（main.rs dispatch；Err = 退码 1）。
pub fn run(action: SystemCommand) -> Result<()> {
    match action {
        SystemCommand::Register {
            dir,
            autostart,
            no_shortcuts,
            desktop,
        } => {
            let dir = normalize_dir(dir)?;
            let req = RegisterRequest {
                install_dir: dir,
                version: common::VERSION_INFO.version.to_string(),
                autostart,
                shortcuts: !no_shortcuts,
                desktop,
            };
            let rep = nemesis_sysint::register(&req);
            for item in &rep.registered {
                println!("✔ {item}");
            }
            for w in &rep.warnings {
                println!("⚠ {w}");
            }
            if rep.ok() {
                println!(
                    "注册完成（{} 项成功，{} 项警告）",
                    rep.registered.len(),
                    rep.warnings.len()
                );
                Ok(())
            } else {
                Err(anyhow!("注册全部失败:\n{}", rep.warnings.join("\n")))
            }
        }
        SystemCommand::Unregister { dir } => {
            let rep = nemesis_sysint::unregister(&normalize_dir(dir)?);
            for item in &rep.removed {
                println!("✔ {item}");
            }
            for w in &rep.warnings {
                println!("⚠ {w}");
            }
            if rep.warnings.is_empty() {
                println!("注销完成（幂等；{} 项摘除）", rep.removed.len());
                Ok(())
            } else {
                Err(anyhow!("注销存在失败项:\n{}", rep.warnings.join("\n")))
            }
        }
        SystemCommand::Status { dir } => {
            let rows = nemesis_sysint::status(&normalize_dir(dir)?)?;
            let width = rows
                .iter()
                .map(|(k, _)| k.chars().count())
                .max()
                .unwrap_or(0);
            for (k, v) in rows {
                println!("{:<width$}  {}", k, v, width = width);
            }
            Ok(())
        }
        SystemCommand::Cert { action } => run_cert(action),
    }
}

fn run_cert(action: CertAction) -> Result<()> {
    match action {
        CertAction::Install { cert } => {
            let der = certdata::cert_der_from_file(&cert)
                .with_context(|| format!("读取证书失败: {}", cert.display()))?;
            println!(
                "安装中：{}（指纹 {}）",
                cert.display(),
                certdata::fingerprint(&der)
            );
            #[cfg(windows)]
            println!("（系统可能弹出证书确认框，请在确认框中确认）");
            let fp = nemesis_sysint::cert_install(&der)?;
            println!("✔ 已安装（指纹 {fp}）");
            Ok(())
        }
        CertAction::Uninstall { cert } => {
            let der = certdata::cert_der_from_file(&cert)
                .with_context(|| format!("读取证书失败: {}", cert.display()))?;
            nemesis_sysint::cert_uninstall(&der)?;
            println!("✔ 已摘除（指纹 {}）", certdata::fingerprint(&der));
            Ok(())
        }
        CertAction::Status { cert } => {
            let der = certdata::cert_der_from_file(&cert)
                .with_context(|| format!("读取证书失败: {}", cert.display()))?;
            let fp = certdata::fingerprint(&der);
            let installed = nemesis_sysint::cert_query(&der)?;
            println!(
                "指纹 {}  {}",
                fp,
                if installed {
                    "在系统信任库"
                } else {
                    "不在系统信任库"
                }
            );
            Ok(())
        }
    }
}

/// 安装目录归一（相对路径 → 绝对路径；不存在诚实报错——集成指向空目录
/// 是死指针，比报错更糟）。canonicalize 产出的 `\\?\` 扩展前缀剥除——
/// 注册表/Run/快捷方式消费的是 Explorer 可读的常规形态。
fn normalize_dir(dir: PathBuf) -> Result<PathBuf> {
    let abs = std::fs::canonicalize(&dir)
        .with_context(|| format!("安装目录不可用: {}", dir.display()))?;
    let s = abs.to_string_lossy();
    Ok(PathBuf::from(
        s.strip_prefix(r"\\?\")
            .map(str::to_string)
            .unwrap_or(s.into()),
    ))
}
