//! 系统集成注册面（nemesis-sysint）——「安装器搬文件、bot 做集成」分工里
//! 系统级设置的执行体（2026-11-10 用户拍板）。
//!
//! 职责边界：
//! - 安装器只保留文件落位（staging/原子提交/install.json 账本/卸载副本/
//!   延迟自删/包验签/磁盘预检/运行实例闸）；
//! - **全部系统集成住本 crate**：ARP 卸载面板条目、开机自启（Windows=HKCU
//!   Run / Linux=systemd user unit）、快捷方式（Windows=.lnk / Linux=
//!   symlink + XDG .desktop + PATH 哨兵块）、信任根证书库；
//! - 安装器对系统集成**零第二实现**：只 spawn `nemesisbot system
//!   register/unregister`，失败=警告不回滚（铁律：注册失败留白诚实，
//!   绝不回退到安装器内嵌实现）。
//!
//! 平台分派（同源 nemesis-sandbox/backend.rs 先例——不同系统不同执行方式）：
//! - Windows：HKCU 注册表（ARP/Run，零 UAC）+ WScript.Shell .lnk +
//!   Win32 证书库（HKCU Root，装库触发系统确认框=知情同意 UI）；
//! - Linux：XDG 用户级三件套（~/.local/bin symlink / .desktop / PATH
//!   哨兵块）+ systemd --user unit + ca-certificates（需 root，缺席诚实报错）；
//! - 其余平台：诚实无实现（返回不可用说明，绝不静默假成功）。
//!
//! 证书纪律（用户拍板「默认签名不安装」）：证书库操作**独立于 register
//! 流程**，只有显式 `nemesisbot system cert install` 才落库；register/
//! uninstall 永不触碰证书。
//!
//! 铁律：
//! - register 逐项 best-effort：单项失败继续其余项，失败明细进 warnings
//!   （调用方自行决定呈现）；全零成功 + 零警告 = 完整成功；
//! - unregister 幂等：条目缺席也回成功（卸载链先 unregister 后删文件，
//!   必须容忍「装的时候注册就失败了」的既往态）；
//! - 零 tokio/async、零 nemesis-* 依赖（只 anyhow/sha2/hex/base64 +
//!   平台 crate）——不依赖 gateway 装配，CLI 轻路径毫秒级返回。

pub mod certdata;
pub mod content;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

use std::path::Path;
use std::path::PathBuf;

/// 注册请求（CLI 旗标面 == 安装器 spawn 参数面的共同落点）。
#[derive(Debug, Clone)]
pub struct RegisterRequest {
    /// 安装目录（含 nemesisbot[.exe] 与卸载副本）。
    pub install_dir: PathBuf,
    /// 产品版本（ARP DisplayVersion 来源；调用方=bot 自身 VERSION_INFO）。
    pub version: String,
    /// 同时配置开机自启。
    pub autostart: bool,
    /// 创建快捷方式（Windows=开始菜单；Linux=symlink+.desktop+PATH）。
    pub shortcuts: bool,
    /// Windows 同时建桌面快捷方式（Linux 无桌面概念，忽略）。
    pub desktop: bool,
}

/// 注册结果：写成功的项 + 非致命失败明细。
#[derive(Debug, Default, Clone)]
pub struct RegisterReport {
    pub registered: Vec<String>,
    pub warnings: Vec<String>,
}

impl RegisterReport {
    /// 整体成功判定：至少注册了一项，或压根没有警告（全空注册 = 全失败）。
    pub fn ok(&self) -> bool {
        !self.registered.is_empty() || self.warnings.is_empty()
    }
}

/// 注销结果（幂等：条目全缺席也回成功——removed 空不算失败）。
#[derive(Debug, Default, Clone)]
pub struct UnregisterReport {
    pub removed: Vec<String>,
    pub warnings: Vec<String>,
}

/// 状态读数：（条目，状态）人读对，CLI 直接打印。
pub type StatusReport = Vec<(String, String)>;

/// 注册系统集成（逐项 best-effort；不同平台不同执行方式）。
pub fn register(req: &RegisterRequest) -> RegisterReport {
    #[cfg(windows)]
    {
        windows::register(req)
    }
    #[cfg(target_os = "linux")]
    {
        linux::register(req)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = req;
        RegisterReport {
            registered: vec![],
            warnings: vec!["本平台暂无系统集成实现（当前仅 Windows/Linux）".into()],
        }
    }
}

/// 注销系统集成（幂等；Linux PATH 哨兵块按设计保留，见 linux::unregister）。
pub fn unregister(install_dir: &Path) -> UnregisterReport {
    #[cfg(windows)]
    {
        windows::unregister(install_dir)
    }
    #[cfg(target_os = "linux")]
    {
        linux::unregister(install_dir)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = install_dir;
        UnregisterReport {
            warnings: vec!["本平台暂无系统集成实现（当前仅 Windows/Linux）".into()],
        }
    }
}

/// 读取系统集成状态（各条目在场/缺席 + 指向）。
pub fn status(install_dir: &Path) -> anyhow::Result<StatusReport> {
    #[cfg(windows)]
    {
        windows::status(install_dir)
    }
    #[cfg(target_os = "linux")]
    {
        linux::status(install_dir)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = install_dir;
        anyhow::bail!("本平台暂无系统集成实现（当前仅 Windows/Linux）");
    }
}

/// 安装信任根证书到系统信任库（**默认永不调用**——只有显式 `system cert
/// install` 走这里）。返回指纹（sha256 hex，卸载/查询按同一指纹对账）。
pub fn cert_install(der: &[u8]) -> anyhow::Result<String> {
    let fp = certdata::fingerprint(der);
    #[cfg(windows)]
    {
        windows::cert_install(der)?;
    }
    #[cfg(target_os = "linux")]
    {
        linux::cert_install(der)?;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = der;
        anyhow::bail!("本平台暂无证书库实现（当前仅 Windows/Linux）");
    }
    Ok(fp)
}

/// 从系统信任库摘除证书（按指纹匹配；证书不在库 = Err 显式报）。
pub fn cert_uninstall(der: &[u8]) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows::cert_uninstall(der)?;
    }
    #[cfg(target_os = "linux")]
    {
        linux::cert_uninstall(der)?;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = der;
        anyhow::bail!("本平台暂无证书库实现（当前仅 Windows/Linux）");
    }
    Ok(())
}

/// 查询证书是否在系统信任库（按指纹匹配）。
pub fn cert_query(der: &[u8]) -> anyhow::Result<bool> {
    #[cfg(windows)]
    {
        windows::cert_query(der)
    }
    #[cfg(target_os = "linux")]
    {
        linux::cert_query(der)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = der;
        anyhow::bail!("本平台暂无证书库实现（当前仅 Windows/Linux）");
    }
}
